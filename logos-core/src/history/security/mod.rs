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
//!    candidates count, never guessed ([FR-SC-02]).
//! 3. **fingerprint + anchor + persist** ([store]) — each finding is keyed by its
//!    fingerprint ([`fingerprint`]); each bound file is anchored by content hash;
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
mod store;
mod suppress;

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
    label, parse_expires, suppress, unsuppress, EntryStatus, IgnoreEntry, IgnoreFile, IgnoreWrite, Labelled, LabelledFinding,
    Suppression, IGNORE_FILE,
};

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
            "medium" => Severity::Medium,
            "high" => Severity::High,
            "critical" => Severity::Critical,
            _ => return None,
        })
    }

    /// The severity a SARIF result states: its `security-severity` CVSS score
    /// when one is given (the bands GitHub code scanning uses), else its level
    /// (`error` high, `warning` medium, `note` low, `none` info), else SARIF's
    /// default level, `warning`.
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
            Some("note") => Severity::Low,
            Some("none") => Severity::Info,
            _ => Severity::Medium,
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
}

impl FindingBinding {
    /// The lowercase wire token (the stored `binding`).
    pub fn as_str(self) -> &'static str {
        match self {
            FindingBinding::Bound => "bound",
            FindingBinding::Unmatched => "unmatched",
            FindingBinding::Ambiguous => "ambiguous",
            FindingBinding::Unbound => "unbound",
        }
    }

    /// The binding a stored token names, or `None` for a token Logos never writes.
    pub(crate) fn from_str_stored(token: &str) -> Option<Self> {
        Some(match token {
            "bound" => FindingBinding::Bound,
            "unmatched" => FindingBinding::Unmatched,
            "ambiguous" => FindingBinding::Ambiguous,
            "unbound" => FindingBinding::Unbound,
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
    let (mut total, mut bound, mut unbound, mut redacted) = (0, 0, 0, 0);
    let mut batches = Vec::with_capacity(grouped.len());
    for (name, (version, parsed)) in grouped {
        let mut files: BTreeMap<String, String> = BTreeMap::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut findings = Vec::with_capacity(parsed.len());
        for f in parsed {
            // Bind without side effects first: a scoped report's finding outside
            // its scope leaves no anchor and no unmatched entry behind.
            let ReportBinding { binding, path, anchor, miss } = match f.report_path.as_deref() {
                None => ReportBinding::unbound(),
                Some(report) => bind_report_path(root, &mapper, &mut anchors, report),
            };
            if let Some(scope) = scope {
                let location = path.clone().or_else(|| f.report_path.as_deref().map(normalise_report_path));
                if !in_scope(location.as_deref(), scope.paths) {
                    continue;
                }
            }
            if let Some((indexed, hash)) = anchor {
                files.insert(indexed, hash);
            }
            if let Some(miss) = miss {
                unmatched.insert(miss);
            }
            total += 1;
            bound += usize::from(binding == FindingBinding::Bound);
            unbound += usize::from(binding == FindingBinding::Unbound);
            redacted += usize::from(f.redacted);

            let fingerprint = occurrence(&mut seen, fingerprint(&f, path.as_deref()));
            findings.push(FindingRow {
                fingerprint,
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
            });
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
/// when it gives one, otherwise its region — in **both** cases hashed together
/// with the rule id and the path, so the stored key is a fixed-length blake3 hex
/// that holds none of the report's fingerprint values (Gitleaks' carry commit
/// authors and messages).
///
/// - The **rule id** is folded in because a report fingerprint can be shared
///   across rules: CodeQL's line hash is, when two rules fire on one line.
/// - The **path** is folded in because a report fingerprint can be shared across
///   files: the same line hash in two files would otherwise be one key, so a
///   suppression or a fix of one would leak onto the other. The cost, accepted:
///   a renamed file's findings read as new.
/// - The path is the bound repo-relative path when the finding bound, so a CI
///   report's absolute path and a local scan's relative one key the same finding
///   alike; an unmatched, ambiguous or unbound finding uses the report's own
///   spelling, separator-normalised (empty when there is no location).
pub(crate) fn fingerprint(f: &ParsedFinding, bound_path: Option<&str>) -> String {
    const SEP: char = '\u{1f}';
    let path = match bound_path {
        Some(bound) => bound.to_string(),
        None => f
            .report_path
            .as_deref()
            .map(normalise_report_path)
            .unwrap_or_default(),
    };
    let canonical = if f.identity.is_empty() {
        let pos = |v: Option<i64>| v.map(|n| n.to_string()).unwrap_or_default();
        format!(
            "logos-finding-v1{SEP}location{SEP}{}{SEP}{path}{SEP}{}:{}:{}:{}",
            f.rule_id,
            pos(f.region.start_line),
            pos(f.region.start_column),
            pos(f.region.end_line),
            pos(f.region.end_column),
        )
    } else {
        format!(
            "logos-finding-v1{SEP}report{SEP}{}{SEP}{path}{SEP}{}",
            f.rule_id,
            f.identity.join(&SEP.to_string())
        )
    };
    blake3::hash(canonical.as_bytes()).to_hex().to_string()
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
