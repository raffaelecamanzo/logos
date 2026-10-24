//! The security half of the evidence store ([CR-217], [ADR-79], [S-663]).
//!
//! Turns a scanner's SARIF 2.1.0 report into anchored, redacted evidence in
//! `.logos/history.db`, beside coverage ([S-049]) and on its pattern:
//!
//! 1. **read + detect + parse + redact** ([parse], [redact]) — the whole report is
//!    validated before any store write, so a malformed report is rejected
//!    atomically and names its defect ([FR-SC-01]); a secret-class finding's
//!    message is replaced at parse time, and no snippet is ever read
//!    ([FR-SC-03]).
//! 2. **bind** — each report path binds to one indexed file through the binder
//!    coverage uses ([`PathMapper`]); an ambiguous path is unmatched with its
//!    candidates count, never guessed ([FR-SC-02]). A history-mode Gitleaks
//!    result is decided first, against its own path at its commit made
//!    project-relative: it is bound, to that very file, only where today's
//!    line there still holds its secret; otherwise it is `historical`,
//!    anchored to its commit (S-767, S-780).
//! 3. **fingerprint + anchor + persist** ([store]) — each finding is keyed by its
//!    fingerprint ([`fingerprint`]; a Gitleaks one by its secret,
//!    [`secret_fingerprint`], so the per-commit repeats of one secret are one
//!    finding listing its commits — a history-mode one by its own path, never
//!    by the file it binds to today); each bound file is anchored by content hash;
//!    the snapshot is anchored to HEAD and the scanner ([FR-SC-02]).
//!
//! [`read_latest`] is the reader the later stories build on. There is no CLI, MCP
//! or watcher surface here; [`crate::Engine::security_ingest`] is the one entry.
//!
//! # Indexed-path source
//! [`ingest`] takes the indexed-file set as a parameter, exactly as coverage's
//! does: `history.db` is never `ATTACH`-ed to the canonical graph, and the engine
//! supplies the set from a graph read.
//!
//! [CR-217]: ../../../../docs/requests/CR-217-security-findings-in-graph-context.md
//! [ADR-79]: ../../../../docs/specs/architecture/decisions/ADR-79.md
//! [FR-SC-01]: ../../../../docs/specs/requirements/FR-SC-01.md
//! [FR-SC-02]: ../../../../docs/specs/requirements/FR-SC-02.md
//! [FR-SC-03]: ../../../../docs/specs/requirements/FR-SC-03.md
//! [FR-SC-17]: ../../../../docs/specs/requirements/FR-SC-17.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [S-049]: ../../../../docs/planning/journal.md#s-049-coverage-store-parsers-and-ingest-pipeline
//! [S-663]: ../../../../docs/planning/journal.md#s-663-sarif-findings-are-ingested-into-the-evidence-store-anchored-and-redacted

pub mod mask;
mod overlay;
pub mod owasp;
mod parse;
mod read;
mod redact;
mod secret;
mod store;
mod suppress;

#[cfg(test)]
mod anchor_tests;
#[cfg(test)]
mod identity_tests;
#[cfg(test)]
mod path_suppress_tests;
#[cfg(test)]
mod suppress_tests;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use super::coverage::{content_hash, Binding, PathMapper};
use store::{FindingRow, ScannerBatch};

pub use overlay::{path_findings, PathFindings};
pub use owasp::{classify, OwaspEdition, OwaspReport};
pub use parse::{ParsedFinding, ParsedRun, Region, SecurityFormat};
pub use read::{latest_snapshots, read_at, read_baseline, read_latest};
pub use suppress::{
    canonical_path, counts, label, parse_expires, project_location, suppress, unsuppress, EntryMatcher, EntryStatus,
    IgnoreEntry, IgnoreFile, IgnoreWrite, Labelled, LabelledFinding, Selector, Suppression, IGNORE_FILE,
};
pub use suppress::{left_out, CodeScope, LeftOut};

/// Detect, parse and validate a security report's text — the parser on its own,
/// with no store and no repository: what [`ingest`] runs before it writes, and
/// what a scanner run checks its output with ([FR-SC-01]). A secret-class
/// finding's message is already redacted in the result, and no snippet is in it.
///
/// # Errors
/// Fails naming the defect when the text is not a SARIF 2.1.0 report or is
/// malformed.
pub fn parse_report(text: &str) -> Result<(SecurityFormat, Vec<ParsedRun>)> {
    let format = parse::detect_format(text).ok_or_else(|| {
        anyhow!("not a SARIF 2.1.0 report (expected a JSON object with `version` \"2.1.0\" and a `runs` array)")
    })?;
    Ok((format, parse::parse(format, text)?))
}

/// The finding severity — shared vocabulary in [`crate::model`], so `config`
/// can name a threshold without depending on this store ([S-668]). The two
/// parsers below are the store's own: how a stored token and a SARIF result map
/// onto it.
///
/// [S-668]: ../../../../docs/planning/journal.md#s-668-a-security-rule-table-gates-on-new-findings-and-caps-and-a-missing-scan-exits-5
pub use crate::model::Severity;

impl Severity {
    /// The severity a stored token names, or `None` for a token Logos never writes.
    pub(crate) fn from_str_stored(token: &str) -> Option<Self> {
        Some(match token {
            "info" => Severity::Info,
            "low" => Severity::Low,
            "unrated" => Severity::Unrated,
            "medium" => Severity::Medium,
            "high" => Severity::High,
            "critical" => Severity::Critical,
            _ => return None,
        })
    }

    /// The severity a SARIF result states: its `security-severity` CVSS score
    /// when one is given (the bands GitHub code scanning uses), else its level
    /// (`error` high, `warning` medium, `note` low, `none` info). `level` is
    /// `None` when the result's level is no rating: SARIF's implicit default
    /// (no result level and no rule `defaultConfiguration.level`, which does
    /// rate), or the level its scanner writes on every result ([`parse`]
    /// decides). It reads `unrated`, never `medium` ([S-768]).
    ///
    /// [S-768]: ../../../../docs/planning/journal.md#s-768-osv-scanner-severities-are-never-made-up
    pub(crate) fn from_sarif(cvss: Option<f64>, level: Option<&str>) -> Self {
        if let Some(score) = cvss {
            return if score >= 9.0 {
                Severity::Critical
            } else if score >= 7.0 {
                Severity::High
            } else if score >= 4.0 {
                Severity::Medium
            } else if score > 0.0 {
                Severity::Low
            } else {
                Severity::Info
            };
        }
        match level {
            Some("error") => Severity::High,
            Some("warning") => Severity::Medium,
            Some("note") => Severity::Low,
            Some("none") => Severity::Info,
            _ => Severity::Unrated,
        }
    }
}

/// How a finding's report path bound to the indexed tree ([FR-SC-01],
/// [FR-SC-02]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingBinding {
    /// Bound to exactly one indexed file.
    Bound,
    /// No indexed file carries the report path, or the one candidate could not
    /// be read to anchor it.
    Unmatched,
    /// Several indexed files tie on the report path; never guessed between.
    Ambiguous,
    /// The result has no physical location. Kept and listed, never dropped.
    Unbound,
    /// A history-mode result whose secret is not on its line in the current
    /// tree — its file is gone, or the line holds other text now — or could not
    /// be read to tell: anchored to its commit ([`SecurityFinding::anchor`])
    /// instead, never to today's file and line ([S-767], history.db
    /// migration 8). It still counts.
    ///
    /// [S-767]: ../../../../docs/planning/journal.md#s-767-a-secret-found-only-in-history-is-anchored-to-its-commit
    Historical,
}

impl FindingBinding {
    /// The lowercase wire token (the stored `binding`).
    pub fn as_str(self) -> &'static str {
        match self {
            FindingBinding::Bound => "bound",
            FindingBinding::Unmatched => "unmatched",
            FindingBinding::Ambiguous => "ambiguous",
            FindingBinding::Unbound => "unbound",
            FindingBinding::Historical => "historical",
        }
    }

    /// The binding a stored token names, or `None` for a token Logos never writes.
    pub(crate) fn from_str_stored(token: &str) -> Option<Self> {
        Some(match token {
            "bound" => FindingBinding::Bound,
            "unmatched" => FindingBinding::Unmatched,
            "ambiguous" => FindingBinding::Ambiguous,
            "unbound" => FindingBinding::Unbound,
            "historical" => FindingBinding::Historical,
            _ => return None,
        })
    }
}

/// One stored finding as the reader returns it — the finding model every later
/// security story consumes. It has no snippet field: none is stored
/// ([FR-SC-03]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SecurityFinding {
    /// The scanner that reported it (`tool.driver.name`).
    pub scanner_name: String,
    /// The scanner version recorded on its snapshot.
    pub scanner_version: String,
    /// The ingest-time HEAD its snapshot is anchored to.
    pub head_sha: String,
    /// The finding's identity within its snapshot ([`fingerprint`]).
    pub fingerprint: String,
    /// The rule the finding reports.
    pub rule_id: String,
    /// The normalised severity.
    pub severity: Severity,
    /// Sorted `CWE-<n>` identifiers; empty when the scanner gave none.
    pub cwes: Vec<String>,
    /// The rule message, or the fixed redacted text when [`redacted`](Self::redacted).
    pub message: String,
    /// `true` for a secret-class finding whose message was redacted at ingest.
    pub redacted: bool,
    /// How the report path bound.
    pub binding: FindingBinding,
    /// The path as the report named it; `None` when the result had no location.
    pub report_path: Option<String>,
    /// The repo-relative indexed path; `Some` only when [`binding`](Self::binding)
    /// is `Bound`.
    pub path: Option<String>,
    /// 1-based start line, as the report gave it.
    pub start_line: Option<i64>,
    /// 1-based start column, as the report gave it.
    pub start_column: Option<i64>,
    /// 1-based end line, as the report gave it.
    pub end_line: Option<i64>,
    /// 1-based end column, as the report gave it.
    pub end_column: Option<i64>,
    /// The commits the secret was found in, newest first — one finding per
    /// secret however many commits hold it ([FR-SC-02]); empty when the report
    /// named none (a scoped Gitleaks run, every other scanner).
    pub commits: Vec<String>,
    /// The fingerprints this finding's results had before a re-key, sorted:
    /// before a Gitleaks finding was keyed by its secret (history.db migration
    /// 7), and before an OSV-Scanner finding, or any unbound absolute path
    /// under the root, was keyed by its project-relative location (migration
    /// 10), and before a history-mode Gitleaks finding was keyed by its own
    /// path rather than the file it bound to (migration 11). Every surface
    /// still knows the finding by them — a fingerprint
    /// suppression, the ratchet's baseline and the branch delta — and
    /// [`suppress::label`] names the fingerprint an ignore entry written under
    /// one of them should name instead. Not serialised.
    #[serde(skip)]
    pub legacy_fingerprints: Vec<String>,
    /// The commit a history-mode finding is anchored to — the one whose place
    /// its region and report path are — and its path there (history.db
    /// migration 8); `None` when the report named no commit. Every surface
    /// shows its [`short_commit`](CommitAnchor::short_commit).
    pub anchor: Option<CommitAnchor>,
    /// `"fresh"`, `"stale"` (the file moved since ingest) or `"n/a"` (no bound
    /// file), decided at read time.
    pub freshness: &'static str,
}

impl SecurityFinding {
    /// Where the finding is: its bound path, else the path its report named,
    /// separator-normalised — the spelling its [`fingerprint`] and a scoped
    /// run's scope match use. `None` when the result had no location.
    pub fn location(&self) -> Option<String> {
        self.path
            .clone()
            .or_else(|| self.report_path.as_deref().map(normalise_report_path))
    }
}

/// The commit a history-mode finding is anchored to ([S-767]). No author,
/// email or commit message is in it, nor stored anywhere ([FR-SC-03]).
///
/// [S-767]: ../../../../docs/planning/journal.md#s-767-a-secret-found-only-in-history-is-anchored-to-its-commit
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CommitAnchor {
    /// The full commit id.
    pub commit: String,
    /// Its first [`SHORT_COMMIT`] digits: the one spelling the CLI, MCP and
    /// the Security page show.
    pub short_commit: String,
    /// The path the report named at that commit, separator-normalised.
    pub path: String,
}

/// How many digits of a commit id every surface shows.
pub const SHORT_COMMIT: usize = 7;

impl CommitAnchor {
    /// The anchor at `commit`, where the report named `path`.
    pub(crate) fn new(commit: String, path: &str) -> Self {
        let short_commit = commit.chars().take(SHORT_COMMIT).collect();
        Self { commit, short_commit, path: normalise_report_path(path) }
    }
}

/// One scanner's latest snapshot.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ScannerSnapshot {
    /// The snapshot row id.
    pub snapshot_id: i64,
    /// `tool.driver.name`.
    pub scanner_name: String,
    /// The recorded scanner version.
    pub scanner_version: String,
    /// The ingest-time HEAD it is anchored to.
    pub head_sha: String,
    /// blake3 of the report it was ingested from (and, for a scoped one, its
    /// scope).
    pub report_hash: String,
    /// Findings stored under it — `0` is *fresh, 0 findings*, not *never run*.
    pub finding_count: usize,
    /// When it was ingested, in Unix seconds. Not serialised: a surface that
    /// prints a snapshot stays byte-identical across re-reads, and the branch
    /// security delta reads it only to tell whether a file was edited after
    /// the scan ([FR-SC-18]).
    ///
    /// [FR-SC-18]: ../../../../docs/specs/requirements/FR-SC-18.md
    #[serde(skip)]
    pub created_at: i64,
    /// The HEAD of the full scan behind its findings outside [`scope`](Self::scope):
    /// its own `head_sha` for a full scan, an earlier one when a scoped run at a
    /// later HEAD carried them over, `None` when no full scan stands behind it
    /// ([FR-SC-17], migration 6).
    ///
    /// [FR-SC-17]: ../../../../docs/specs/requirements/FR-SC-17.md
    pub full_head_sha: Option<String>,
    /// The paths scoped runs re-scanned on top of that full scan, sorted; empty
    /// for a full scan.
    pub scope: Vec<String>,
}

/// Every scanner's latest snapshot and its findings, as [`read_latest`] returns.
/// A scanner absent from `snapshots` has never run here.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SecurityView {
    /// One row per scanner, ordered by name.
    pub snapshots: Vec<ScannerSnapshot>,
    /// Their findings, ordered by scanner, then path, line and rule.
    pub findings: Vec<SecurityFinding>,
}

/// The security ratchet's baseline as [`read_baseline`] returns it: the
/// fingerprints `gate --save` recorded ([FR-SC-10], migration 5).
///
/// [FR-SC-10]: ../../../../docs/specs/requirements/FR-SC-10.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityBaseline {
    /// HEAD when it was saved; `None` outside a repository.
    pub head_sha: Option<String>,
    /// The `[security]` table and ignore file it was saved under.
    pub config_hash: String,
    /// Every recorded fingerprint, sorted.
    pub fingerprints: BTreeSet<String>,
}

/// Record `findings` — `(scanner, fingerprint)` pairs — as the security
/// ratchet's baseline under `root`, replacing the previous one ([FR-SC-10]).
/// Returns how many fingerprints were recorded.
///
/// [FR-SC-10]: ../../../../docs/specs/requirements/FR-SC-10.md
///
/// # Errors
/// Returns an error only on a store failure.
pub fn save_baseline(
    root: &Path,
    head_sha: Option<&str>,
    config_hash: &str,
    findings: &[(String, String)],
) -> Result<usize> {
    let mut conn = super::open(root)?;
    store::persist_baseline(&mut conn, head_sha, config_hash, findings)
}

/// A report path that bound to no single indexed file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct UnmatchedPath {
    /// The path as the report named it.
    pub path: String,
    /// Indexed files it could have meant: `0` none, `1` one that could not be
    /// read to anchor, `2+` an ambiguous tie ([FR-SC-02]).
    pub candidates: usize,
}

/// One scanner's outcome within an ingest.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ScannerIngest {
    /// `tool.driver.name`.
    pub name: String,
    /// The recorded version.
    pub version: String,
    /// The snapshot its findings are stored under.
    pub snapshot_id: i64,
    /// Findings in this report for this scanner.
    pub findings: usize,
    /// `true` when an earlier snapshot of this scanner at this HEAD was replaced.
    pub replaced_existing: bool,
}

/// The outcome of a security ingest — a `Serialize` read-model the surfaces
/// render as-is.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SecurityIngestSummary {
    /// The detected format.
    pub format: SecurityFormat,
    /// The ingest-time HEAD every snapshot of this ingest is anchored to.
    pub head_sha: String,
    /// blake3 of the report bytes — the byte-identical re-ingest key; for a
    /// scoped report, of its bytes and its scope ([`ingest_scoped`]).
    pub report_hash: String,
    /// One row per scanner in the report, ordered by name.
    pub scanners: Vec<ScannerIngest>,
    /// All findings in the report.
    pub findings: usize,
    /// Findings bound to an indexed file.
    pub bound: usize,
    /// Findings with no physical location — kept, listed as unbound.
    pub unbound: usize,
    /// History-mode findings anchored to their commit, not to the current tree
    /// ([`FindingBinding::Historical`]).
    pub historical: usize,
    /// Secret-class findings whose message was redacted.
    pub redacted: usize,
    /// Report paths that bound to no single indexed file, with candidate counts.
    pub unmatched: Vec<UnmatchedPath>,
    /// `true` when this exact report was already stored at this HEAD: nothing
    /// was written ([FR-SC-01]).
    pub already_ingested: bool,
}

/// Ingest the security report at `report_path` into the evidence store under
/// `root`, binding report paths against `indexed_paths` ([FR-SC-01]..[FR-SC-03]).
///
/// # Errors
/// Fails loud (the surface maps it to a non-zero exit) when the report is
/// unreadable, not a SARIF report, malformed (the defect is named and nothing is
/// written), or `root` has no resolvable HEAD to anchor to. Per-finding outcomes
/// — unmatched, ambiguous, unbound, redacted, an idempotent no-op — ride in the
/// returned summary, never errors.
pub fn ingest(root: &Path, report_path: &Path, indexed_paths: &[String]) -> Result<SecurityIngestSummary> {
    ingest_scoped(root, report_path, indexed_paths, None)
}

/// A scoped run's scope, as its ingest needs it ([FR-SC-17]).
///
/// [FR-SC-17]: ../../../../docs/specs/requirements/FR-SC-17.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngestScope<'a> {
    /// The project-relative paths the run scanned.
    pub paths: &'a [String],
    /// `true` when the scoped run reads other evidence than a full scan does —
    /// Gitleaks reads the files as they are, its full scan their git history —
    /// so the full scan's findings in these paths are kept rather than
    /// replaced ([`store`]).
    pub keeps_full_findings: bool,
}

impl<'a> IngestScope<'a> {
    /// A scope whose run replaces every finding in `paths`.
    pub fn replacing(paths: &'a [String]) -> Self {
        Self { paths, keeps_full_findings: false }
    }
}

/// [`ingest`] a scoped run's report ([FR-SC-17]). `scope` names its
/// project-relative paths: the report's findings outside them are dropped, and
/// each scanner's findings and file anchors outside them are carried over
/// unchanged from its latest snapshot ([`store`]). `None` is [`ingest`].
///
/// # Errors
/// As [`ingest`].
pub fn ingest_scoped(
    root: &Path,
    report_path: &Path,
    indexed_paths: &[String],
    scope: Option<IngestScope<'_>>,
) -> Result<SecurityIngestSummary> {
    // 1. Read + detect + parse + redact — all before any store write.
    let bytes = std::fs::read(report_path)
        .with_context(|| format!("reading security report {}", report_path.display()))?;
    let text = std::str::from_utf8(&bytes).map_err(|err| {
        anyhow!("{} is not UTF-8 text ({err}); a SARIF report is JSON", report_path.display())
    })?;
    let (format, runs) = parse_report(text)
        .with_context(|| format!("rejecting {}: nothing was written", report_path.display()))?;
    let report_hash = report_key(&bytes, scope);

    // 2. Anchor to HEAD — without one there is nothing to anchor to.
    let head_sha = super::miner::head_sha(root).ok_or_else(|| {
        anyhow!(
            "cannot ingest security findings: {} is not a git repository with a resolvable HEAD \
             (security snapshots are anchored to the HEAD SHA)",
            root.display()
        )
    })?;

    // 3. Group runs by scanner (one tool may span several runs), bind, anchor and
    //    fingerprint.
    let grouped = group_by_scanner(runs);
    let mapper = PathMapper::new(indexed_paths, &[]);
    let mut anchors: HashMap<String, Option<String>> = HashMap::new();
    let mut unmatched: BTreeSet<UnmatchedPath> = BTreeSet::new();
    let (mut total, mut bound, mut unbound, mut historical, mut redacted) = (0, 0, 0, 0, 0);
    let mut batches = Vec::with_capacity(grouped.len());
    // Opened once, for a report that holds Gitleaks results, with the
    // project's prefix below the git root its history-mode paths carry.
    let has_gitleaks = grouped.keys().any(|name| redact::is_gitleaks(name));
    let mut digests = has_gitleaks.then(|| secret::SecretDigests::open(root)).flatten();
    let prefix = has_gitleaks.then(|| secret::project_prefix(root)).flatten().unwrap_or_default();
    for (name, (version, parsed)) in grouped {
        let mut files: BTreeMap<String, String> = BTreeMap::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut seen_pre10: HashMap<String, usize> = HashMap::new();
        let mut seen_pre11: HashMap<String, usize> = HashMap::new();
        let mut keyed: Vec<KeyedResult> = Vec::with_capacity(parsed.len());
        let gitleaks = redact::is_gitleaks(&name);
        for f in parsed {
            // A history-mode result is known by its own path at its commit,
            // made project-relative — never by the file it binds to today, which
            // an unrelated file sharing its suffix could change (history.db
            // migration 11, CR-241). Whether its secret is still on its line is
            // decided against that path; only then is it bound, for display.
            let own = f.commit.as_ref().and(f.report_path.as_deref()).map(|report| project_relative(root, &prefix, report));
            let on_its_line = f.commit.is_some() && still_on_its_line(digests.as_mut(), &f, own.as_deref());
            // Bind without side effects first: a scoped report's finding outside
            // its scope leaves no anchor and no unmatched entry behind.
            let ReportBinding { binding, path, anchor, miss } = match (f.report_path.as_deref(), own.as_deref()) {
                _ if f.commit.is_some() && !on_its_line => ReportBinding::historical(),
                (None, _) => ReportBinding::unbound(),
                (Some(report), Some(own)) => bind_own_path(root, &mapper, &mut anchors, report, own),
                (Some(report), None) => bind_report_path(root, &mapper, &mut anchors, report),
            };
            if let Some(scope) = scope {
                let location = path.clone().or_else(|| f.report_path.as_deref().map(normalise_report_path));
                if !in_scope(location.as_deref(), scope.paths) {
                    continue;
                }
            }
            if let Some(miss) = miss {
                unmatched.insert(miss);
            }

            // A history-mode result is keyed by its own path, so it keeps its
            // key when it turns historical and whatever today's files are; any
            // other by the path as it bound. Before migration 11 a history-mode
            // result was keyed by the file its report path bound to by suffix,
            // historical or not: those keys, and the ones before migration 10,
            // are its old keys ([`collapse`]).
            let key_path = if f.commit.is_some() { own.clone() } else { path.clone() };
            let pre11_bound = match (&f.commit, f.report_path.as_deref()) {
                (Some(_), Some(report)) => bind_report_path(root, &mapper, &mut anchors, report).path,
                _ => path.clone(),
            };
            let report_key = match f.commit {
                Some(_) => occurrence(&mut seen, history_report_key(fingerprint(&f, &name, root, key_path.as_deref()))),
                None => occurrence(&mut seen, fingerprint(&f, &name, root, key_path.as_deref())),
            };
            let mut old_keys: Vec<String> = pre10_fingerprints(&f, &name, pre11_bound.as_deref())
                .into_iter()
                .map(|base| occurrence(&mut seen_pre10, base))
                .collect();
            if f.commit.is_some() {
                old_keys.push(occurrence(&mut seen_pre11, fingerprint(&f, &name, root, pre11_bound.as_deref())));
            }
            let mut secret_key = None;
            if let Some(digests) = gitleaks.then_some(digests.as_mut()).flatten() {
                secret_key = secret_fingerprint(digests, &f, key_path.as_deref());
                if f.commit.is_some() {
                    old_keys.extend(secret_fingerprint(digests, &f, pre11_bound.as_deref()));
                }
            }
            if let Some((indexed, hash)) = anchor {
                files.insert(indexed, hash);
            }
            keyed.push(KeyedResult {
                secret_key,
                report_key,
                old_keys,
                committed_at: f.committed_at,
                row: FindingRow {
                    fingerprint: String::new(),
                    rule_id: f.rule_id,
                    severity: f.severity,
                    cwes: f.cwes,
                    message: f.message,
                    redacted: f.redacted,
                    binding,
                    report_path: f.report_path,
                    path,
                    region: f.region,
                    scoped: scope.is_some(),
                    anchor_commit: f.commit.clone(),
                    commits: f.commit.into_iter().collect(),
                    legacy_fingerprints: Vec::new(),
                    places: Vec::new(),
                },
            });
        }
        let findings = collapse(keyed);
        for f in &findings {
            total += 1;
            bound += usize::from(f.binding == FindingBinding::Bound);
            unbound += usize::from(f.binding == FindingBinding::Unbound);
            historical += usize::from(f.binding == FindingBinding::Historical);
            redacted += usize::from(f.redacted);
        }
        batches.push(ScannerBatch {
            name,
            version,
            files: files.into_iter().collect(),
            findings,
        });
    }

    // 4. Persist — opening the store is the first write.
    let mut conn = super::open(root)?;
    let outcome = store::persist_ingest(&mut conn, &head_sha, &report_hash, format, &batches, scope)?;

    let scanners = batches
        .iter()
        .zip(&outcome.snapshots)
        .map(|(b, &(snapshot_id, replaced_existing))| ScannerIngest {
            name: b.name.clone(),
            version: b.version.clone(),
            snapshot_id,
            findings: b.findings.len(),
            replaced_existing,
        })
        .collect();
    Ok(SecurityIngestSummary {
        format,
        head_sha,
        report_hash,
        scanners,
        findings: total,
        bound,
        unbound,
        historical,
        redacted,
        unmatched: unmatched.into_iter().collect(),
        already_ingested: outcome.already_ingested,
    })
}

/// The idempotency key of a report: its bytes' hash. A scoped report's key
/// folds in its scope: the same bytes over other paths — an empty report,
/// say — replace other findings.
fn report_key(bytes: &[u8], scope: Option<IngestScope<'_>>) -> String {
    let Some(scope) = scope else {
        return blake3::hash(bytes).to_hex().to_string();
    };
    let mut key = blake3::Hasher::new();
    key.update(bytes);
    for path in scope.paths.iter().collect::<BTreeSet<_>>() {
        key.update(b"\x1f");
        key.update(path.as_bytes());
    }
    key.finalize().to_hex().to_string()
}

/// The report's runs grouped by scanner — one tool may span several runs —
/// each with the version of its first run and the findings of all of them.
fn group_by_scanner(runs: Vec<ParsedRun>) -> BTreeMap<String, (String, Vec<ParsedFinding>)> {
    let mut grouped: BTreeMap<String, (String, Vec<ParsedFinding>)> = BTreeMap::new();
    for run in runs {
        let entry = grouped
            .entry(run.scanner_name)
            .or_insert_with(|| (run.scanner_version, Vec::new()));
        entry.1.extend(run.findings);
    }
    grouped
}

/// What binding one finding's report path gave, before its scope decides
/// whether the finding is kept: the binding, the indexed path, the file
/// anchor to record and the unmatched entry to report.
struct ReportBinding {
    binding: FindingBinding,
    path: Option<String>,
    anchor: Option<(String, String)>,
    miss: Option<UnmatchedPath>,
}

impl ReportBinding {
    /// A finding that names no path.
    fn unbound() -> Self {
        Self { binding: FindingBinding::Unbound, path: None, anchor: None, miss: None }
    }

    /// A history-mode result whose secret is no longer on its line: anchored
    /// to its commit, with no file of today's tree ([S-767]).
    ///
    /// [S-767]: ../../../../docs/planning/journal.md#s-767-a-secret-found-only-in-history-is-anchored-to-its-commit
    fn historical() -> Self {
        Self { binding: FindingBinding::Historical, path: None, anchor: None, miss: None }
    }

    /// A path that is not attached: `binding` says why, `candidates` how many
    /// indexed paths it could have been.
    fn missed(binding: FindingBinding, report: &str, candidates: usize) -> Self {
        let miss = UnmatchedPath { path: report.to_string(), candidates };
        Self { binding, path: None, anchor: None, miss: Some(miss) }
    }
}

/// Bind one report path against the indexed paths, hashing each bound file's
/// content once into `anchors`.
fn bind_report_path(
    root: &Path,
    mapper: &PathMapper<'_>,
    anchors: &mut HashMap<String, Option<String>>,
    report: &str,
) -> ReportBinding {
    match mapper.bind(report) {
        Binding::Bound(indexed) => {
            let hash = anchors
                .entry(indexed.to_string())
                .or_insert_with(|| content_hash(&root.join(indexed)));
            match hash {
                Some(hash) => ReportBinding {
                    binding: FindingBinding::Bound,
                    path: Some(indexed.to_string()),
                    anchor: Some((indexed.to_string(), hash.clone())),
                    miss: None,
                },
                // Bound but unreadable: it cannot be anchored, so it is not
                // attached ([NFR-RA-05]).
                None => ReportBinding::missed(FindingBinding::Unmatched, report, 1),
            }
        }
        Binding::Unmatched => ReportBinding::missed(FindingBinding::Unmatched, report, 0),
        Binding::Ambiguous { candidates } => ReportBinding::missed(FindingBinding::Ambiguous, report, candidates),
    }
}

/// Bind a history-mode result whose secret is still on its line at `own`, its
/// project-relative path, for display: to that very indexed file, never to
/// another the binder would take by suffix. `own` not in the index — a `.env`,
/// a file excluded from it — is `report`, unmatched.
fn bind_own_path(
    root: &Path,
    mapper: &PathMapper<'_>,
    anchors: &mut HashMap<String, Option<String>>,
    report: &str,
    own: &str,
) -> ReportBinding {
    let bound = bind_report_path(root, mapper, anchors, own);
    if bound.path.as_deref() == Some(own) {
        bound
    } else {
        ReportBinding::missed(FindingBinding::Unmatched, report, 0)
    }
}

/// A history-mode report path — relative to the git root, as Gitleaks' history
/// mode writes it — relative to the project instead: separator-normalised, with
/// the project's `prefix` below the git root ([`secret::project_prefix`]) taken
/// off. A path outside the project takes one `..` per segment of the prefix, so
/// it never spells a project file it is not. An absolute path under `root` is
/// taken relative to it, so the checkout's directory never enters a key
/// (history.db migration 10); one outside `root` keeps its spelling. The one
/// spelling a history-mode result's key, and its still-on-its-line read, use.
fn project_relative(root: &Path, prefix: &str, report: &str) -> String {
    let path = normalise_report_path(report);
    if path.starts_with('/') || Path::new(&path).is_absolute() {
        return project_location(root, &path).unwrap_or(path);
    }
    if prefix.is_empty() {
        return path;
    }
    match path.strip_prefix(prefix) {
        Some(rest) => rest.to_string(),
        None => format!("{}{path}", "../".repeat(prefix.split('/').filter(|s| !s.is_empty()).count())),
    }
}

/// A history-mode result's report key — used where its secret cannot be read —
/// from `base`, its [`fingerprint`] at its own path, in a namespace of its own
/// (history.db migration 11). No such key is one a result had before migration
/// 11, when its path was the file it bound to and two results of one commit
/// binding to one file told apart only by their occurrence suffix: an old key
/// could otherwise be another finding's new one, and the re-key would map one
/// finding's rows, baseline entry and suppressions onto the other.
fn history_report_key(base: String) -> String {
    blake3::hash(format!("logos-finding-v1\u{1f}history\u{1f}{base}").as_bytes()).to_hex().to_string()
}

/// `base`, or — for its second and later occurrence in one report — `base`
/// with an occurrence suffix, hashed. Two results with one identity in one
/// report are two findings, never one silently dropped: the later ones take
/// the suffix, in report order.
fn occurrence(seen: &mut HashMap<String, usize>, base: String) -> String {
    let n = seen.entry(base.clone()).or_insert(0);
    *n += 1;
    if *n == 1 {
        base
    } else {
        blake3::hash(format!("{base}#{n}").as_bytes()).to_hex().to_string()
    }
}

/// A finding's identity ([FR-SC-02], [ADR-79]): the report's own fingerprint
/// when it gives one (never OSV-Scanner's, below), otherwise its region — in
/// **both** cases hashed together with the rule id and the path, so the stored
/// key is a fixed-length blake3 hex that holds none of the report's fingerprint
/// values (Gitleaks' carry commit authors and messages). A Gitleaks finding
/// whose secret can be read is keyed by [`secret_fingerprint`] instead, and
/// keeps this key, with its [`pre10_fingerprints`], only as legacy
/// fingerprints: the keys it had before history.db migrations 7 and 10.
/// A history-mode Gitleaks result passes its own project-relative path as
/// `bound_path` ([`project_relative`]; migration 11), never the file it
/// binds to today.
///
/// - The **rule id** is folded in because a report fingerprint can be shared
///   across rules: CodeQL's line hash is, when two rules fire on one line.
/// - The **path** is folded in because a report fingerprint can be shared across
///   files: the same line hash in two files would otherwise be one key, so a
///   suppression or a fix of one would leak onto the other. The cost, accepted:
///   a renamed file's findings read as new.
/// - The path is the bound repo-relative path when the finding bound, so a CI
///   report's absolute path and a local scan's relative one key the same finding
///   alike. An unmatched, ambiguous or unbound finding uses the report's own
///   spelling, separator-normalised, with `root` taken off an absolute path
///   under it, so the checkout's directory never enters the key (history.db
///   migration 10): a clone at another path keys it alike. An absolute path
///   outside `root` keeps its spelling, and stays root-dependent. Empty when
///   there is no location.
/// - An **OSV-Scanner** finding is keyed by its location alone, never by the
///   report's identity: 2.x writes `partialFingerprints.primaryLocationLineHash`
///   where 1.x wrote none, so a scanner version change would re-key it
///   (migration 10, CR-241).
///
/// [`pre10_fingerprints`] are the keys a result could have had before migration
/// 10, which the ingest records as legacy fingerprints where they differ.
pub(crate) fn fingerprint(f: &ParsedFinding, scanner: &str, root: &Path, bound_path: Option<&str>) -> String {
    let path = match bound_path {
        Some(bound) => bound.to_string(),
        None => f
            .report_path
            .as_deref()
            .map(|report| project_location(root, report).unwrap_or_else(|| normalise_report_path(report)))
            .unwrap_or_default(),
    };
    let identity: &[String] = if parse::is_osv_scanner(scanner) { &[] } else { &f.identity };
    finding_key(&f.rule_id, &path, identity, &f.region)
}

/// The keys [`fingerprint`] could have given a result before history.db
/// migration 10: an unbound path in the report's own spelling, absolute or
/// not, and the report's identity for every scanner — computed exactly as it
/// was then, so the ingest can name each re-keyed row's old key ([`collapse`]);
/// never a stored row's new key. An OSV-Scanner result that carries an
/// identity (2.x's `partialFingerprints`) also has the key its 1.x report gave
/// it, which carried none: the first report after migration 10 may come from
/// either version, and the stored rows from the other (CR-241).
fn pre10_fingerprints(f: &ParsedFinding, scanner: &str, bound_path: Option<&str>) -> Vec<String> {
    let path = match bound_path {
        Some(bound) => bound.to_string(),
        None => f.report_path.as_deref().map(normalise_report_path).unwrap_or_default(),
    };
    let mut keys = vec![finding_key(&f.rule_id, &path, &f.identity, &f.region)];
    if parse::is_osv_scanner(scanner) && !f.identity.is_empty() {
        keys.push(finding_key(&f.rule_id, &path, &[], &f.region));
    }
    keys
}

/// The `logos-finding-v1` key of a result: `identity` when it is not empty,
/// else the region, hashed with the rule id and the path. The one spelling
/// [`fingerprint`] and [`pre10_fingerprints`] share, so they differ only in the
/// path and identity they pass.
fn finding_key(rule_id: &str, path: &str, identity: &[String], region: &Region) -> String {
    const SEP: char = '\u{1f}';
    let canonical = if identity.is_empty() {
        let pos = |v: Option<i64>| v.map(|n| n.to_string()).unwrap_or_default();
        format!(
            "logos-finding-v1{SEP}location{SEP}{rule_id}{SEP}{path}{SEP}{}:{}:{}:{}",
            pos(region.start_line),
            pos(region.start_column),
            pos(region.end_line),
            pos(region.end_column),
        )
    } else {
        format!("logos-finding-v1{SEP}report{SEP}{rule_id}{SEP}{path}{SEP}{}", identity.join(&SEP.to_string()))
    };
    blake3::hash(canonical.as_bytes()).to_hex().to_string()
}

/// A Gitleaks finding's identity ([FR-SC-02] as amended by CR-238 Amendment 1
/// and CR-241, [ADR-79]): the rule, the path and the keyed digest of the
/// secret ([`secret`]), hashed together. The report's own fingerprint differs
/// per commit and is left out, so every commit that holds one secret, and a
/// scoped run's reading of it, give one key; its position is left out, so a
/// moved line keeps it. `key_path` is the path: a history-mode result's own,
/// project-relative ([`project_relative`]; history.db migration 11), a scoped
/// one's bound file — the same file, so a secret still present keys alike in
/// both modes — else the path as reported; and, to give a history-mode
/// result's key before migration 11, the file its report path binds to by
/// suffix. `None` when the secret cannot be read: the result keeps its
/// report-keyed identity.
fn secret_fingerprint(digests: &mut secret::SecretDigests, f: &ParsedFinding, key_path: Option<&str>) -> Option<String> {
    const SEP: char = '\u{1f}';
    let report_path = f.report_path.as_deref().map(normalise_report_path)?;
    // A history-mode result is read where the commit had it, by the path the
    // report gives (git-root-relative), a scoped one where the scan read it:
    // the bound file, else the path as reported.
    let source = match (&f.commit, key_path) {
        (None, Some(bound)) => bound.to_string(),
        _ => report_path.clone(),
    };
    let digest = digests.digest(f.commit.as_deref(), &source, &f.region)?;
    let path = key_path.map_or(report_path, str::to_string);
    let canonical = format!(
        "logos-finding-v2{SEP}secret{SEP}{}{SEP}{path}{SEP}{}",
        f.rule_id,
        blake3::Hash::from(digest).to_hex()
    );
    Some(blake3::hash(canonical.as_bytes()).to_hex().to_string())
}

/// Whether a history-mode result's secret is still on its line in the
/// current tree ([S-767]): in the file at `own`, its own project-relative path
/// ([`project_relative`]) — never a file its report path would bind to by
/// suffix, which may hold the same text — indexed or not: a file outside the
/// index (a `.env`) that still holds it is no secret found only in history.
/// `false` when that file is gone or lies outside the project, its secret
/// cannot be read, or the line holds other text now — it is then
/// [`FindingBinding::Historical`].
///
/// [S-767]: ../../../../docs/planning/journal.md#s-767-a-secret-found-only-in-history-is-anchored-to-its-commit
fn still_on_its_line(digests: Option<&mut secret::SecretDigests>, f: &ParsedFinding, own: Option<&str>) -> bool {
    let (Some(digests), Some(commit), Some(report), Some(own)) =
        (digests, f.commit.as_deref(), f.report_path.as_deref(), own)
    else {
        return false;
    };
    digests.still_on_its_line(commit, &normalise_report_path(report), own, &f.region) == Some(true)
}

/// One result of a report, keyed both ways before [`collapse`].
struct KeyedResult {
    /// Its secret-keyed identity ([`secret_fingerprint`]); `None` when it has none.
    secret_key: Option<String>,
    /// Its report-keyed identity ([`fingerprint`] with its occurrence suffix).
    report_key: String,
    /// The identities it could have had before a re-key, each report-keyed one
    /// with its occurrence suffix: before history.db migration 10
    /// ([`pre10_fingerprints`]) — and, for a Gitleaks result, before migration
    /// 7 too — and, for a history-mode one, before migration 11, when it was
    /// keyed by the file its report path bound to today, report- and
    /// secret-keyed alike: what a row stored then is keyed by.
    old_keys: Vec<String>,
    /// Its commit's date, which orders a secret's commits.
    committed_at: Option<String>,
    /// The row, its fingerprint not yet set.
    row: FindingRow,
}

/// One finding per identity: the results that share a secret-keyed identity
/// are one finding listing their commits newest first, at the place the newest
/// commit holds the secret — the newest whose place still holds it today, when
/// one does, so the finding is bound wherever the secret is still on its line
/// ([S-767]) — with every report key among them, as keyed now and before
/// migrations 10 and 11, and its secret key before migration 11, recorded as a
/// legacy fingerprint. A result with no secret key is a finding of its own,
/// keyed by its report, as every other scanner's, with its keys before
/// migrations 10 and 11 recorded as legacy fingerprints where they differ:
/// what re-keys a stored row ([`store`]). A key some finding of the
/// report has as its own is never another's legacy fingerprint. In report
/// order of each finding's first result.
fn collapse(results: Vec<KeyedResult>) -> Vec<FindingRow> {
    let mut groups: Vec<Vec<KeyedResult>> = Vec::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    for result in results {
        match result.secret_key.clone() {
            None => groups.push(vec![result]),
            Some(key) => match by_key.get(&key) {
                Some(&at) => groups[at].push(result),
                None => {
                    by_key.insert(key, groups.len());
                    groups.push(vec![result]);
                }
            },
        }
    }
    let mut rows = groups
        .into_iter()
        .map(|mut group| {
            // Newest commit first (an ISO date sorts as text; none last), then
            // by commit and position, so the answer is the same in any report
            // order.
            group.sort_by(|a, b| {
                b.committed_at
                    .is_some()
                    .cmp(&a.committed_at.is_some())
                    .then_with(|| b.committed_at.cmp(&a.committed_at))
                    .then_with(|| a.row.commits.cmp(&b.row.commits))
                    .then_with(|| region_key(&a.row.region).cmp(&region_key(&b.row.region)))
            });
            let mut commits: Vec<String> = Vec::new();
            for commit in group.iter().flat_map(|r| r.row.commits.iter()) {
                if !commits.contains(commit) {
                    commits.push(commit.clone());
                }
            }
            let mut legacy: BTreeSet<String> =
                group.iter().flat_map(|r| std::iter::once(r.report_key.clone()).chain(r.old_keys.iter().cloned())).collect();
            let places = group.iter().filter_map(|r| r.row.anchor_commit.clone().map(|c| (c, r.row.region))).collect();
            let place = group.iter().position(|r| r.row.binding != FindingBinding::Historical).unwrap_or(0);
            let first = group.swap_remove(place);
            let mut row = first.row;
            row.fingerprint = first.secret_key.unwrap_or(first.report_key);
            legacy.remove(&row.fingerprint);
            row.legacy_fingerprints = legacy.into_iter().collect();
            row.commits = commits;
            row.places = places;
            row
        })
        .collect::<Vec<FindingRow>>();
    // A key some finding of this report has now is no old one: a report naming
    // one file by its absolute and by its relative path gives one result's new
    // key as another's old one, and the re-key would merge the two findings.
    let current: std::collections::HashSet<String> = rows.iter().map(|row| row.fingerprint.clone()).collect();
    for row in &mut rows {
        row.legacy_fingerprints.retain(|old| !current.contains(old));
    }
    rows
}

/// A region as an ordering key.
fn region_key(r: &Region) -> (Option<i64>, Option<i64>, Option<i64>, Option<i64>) {
    (r.start_line, r.start_column, r.end_line, r.end_column)
}

/// Whether `path` — project-relative, `/`-separated — is one of a scoped run's
/// `scope` paths or lies under one ([FR-SC-17]). No location is in no scope.
///
/// [FR-SC-17]: ../../../../docs/specs/requirements/FR-SC-17.md
pub(crate) fn in_scope(path: Option<&str>, scope: &[String]) -> bool {
    path.is_some_and(|p| scope.iter().any(|s| path_within(p, s)))
}

/// Whether `path` is `dir` or lies under it, by whole path segments — `src/a`
/// holds `src/a/b.py`, never `src/ab.py`.
pub(crate) fn path_within(path: &str, dir: &str) -> bool {
    path == dir || path.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/'))
}

/// A report path as an identity component: backslashes to `/` and a leading
/// `./` dropped, so one location spelled two ways keys alike.
fn normalise_report_path(path: &str) -> String {
    let slashed = path.replace('\\', "/");
    slashed
        .strip_prefix("./")
        .map(str::to_string)
        .unwrap_or(slashed)
}
