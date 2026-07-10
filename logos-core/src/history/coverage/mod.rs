//! The coverage half of the evidence store ([CR-007], [ADR-23], [S-049]).
//!
//! Turns an external LCOV / Cobertura coverage report into anchored, deterministic
//! evidence in `.logos/history.db` — the store [S-046] established, rescoped by
//! [ADR-23] from "git-history store" to **evidence store**. The pipeline:
//!
//! 1. **read + detect + parse** ([parse]) — auto-detect the format from content
//!    (or honour `--format`) and parse the whole report *before* any store write,
//!    so a malformed report is rejected atomically with a byte-identical store
//!    ([FR-CV-01]).
//! 2. **map** ([pathmap]) — bind each report path to exactly one indexed file by
//!    longest-unique-suffix matching, never guessing ([FR-CV-03]).
//! 3. **anchor + persist** ([store]) — anchor each matched file to its content
//!    hash and HEAD SHA, then merge into the open same-HEAD snapshot or start a new
//!    one ([FR-CV-02], [FR-CV-04]).
//!
//! # Tier boundary ([BR-28])
//! Coverage is advisory evidence: it lands only in `history.db` (never `logos.db`,
//! never `ATTACH`-ed), survives a full `index`, and the gate never reads it. This
//! module is the **writer**; the read surfaces (`coverage status`, the
//! untested-hotspots join) are [S-051] in iteration 4.
//!
//! # Indexed-path source ([BR-28])
//! [`ingest`] takes the indexed-file set as a parameter rather than reading
//! `logos.db`, keeping the evidence store decoupled from the canonical graph. The
//! [S-051] surface supplies the paths from a graph read at the `api` layer.
//!
//! [CR-007]: ../../../../docs/requests/CR-007-coverage-ingestion.md
//! [ADR-23]: ../../../../docs/specs/architecture/decisions/ADR-23.md
//! [BR-28]: ../../../../docs/specs/software-spec.md#323-coverage-test-evidence
//! [FR-CV-01]: ../../../../docs/specs/requirements/FR-CV-01.md
//! [FR-CV-02]: ../../../../docs/specs/requirements/FR-CV-02.md
//! [FR-CV-03]: ../../../../docs/specs/requirements/FR-CV-03.md
//! [FR-CV-04]: ../../../../docs/specs/requirements/FR-CV-04.md
//! [S-046]: ../../../../docs/planning/journal.md#s-046-history-store-and-incremental-git-miner
//! [S-049]: ../../../../docs/planning/journal.md#s-049-coverage-store-parsers-and-ingest-pipeline
//! [S-051]: ../../../../docs/planning/journal.md#s-051-coverage-surfaces-and-untested-hotspots

mod artifact;
mod parse;
mod pathmap;
mod read;
mod store;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::config::{EffectiveCoverage, EffectiveCoverageIngest};

/// The basis-point scale (`/10000`) every coverage ratio rounds to -- the same
/// integer resolution the gated 0--10000 signal and the temporal `bp` metrics
/// use ([ADR-08]), so cross-target arithmetic is byte-identical ([NFR-RA-06]).
/// Shared by the file-level [`read`] aggregates.
///
/// [ADR-08]: ../../../../docs/specs/architecture/decisions/ADR-08.md
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
pub(super) const BP_SCALE: i64 = 10_000;

pub use parse::CoverageFormat;

pub(crate) use artifact::{discover as discover_artifact, ArtifactMatcher};
pub(crate) use read::read_latest;

use pathmap::PathMapper;
use store::MatchedFile;

/// The outcome of a coverage ingest — a `Serialize` read-model ([ADR-01]) so the
/// [S-051] CLI/MCP surfaces render the same payload for free ([FR-CV-01] "the
/// ingest summary reports files matched/unmatched, line totals, and freshness").
///
/// [ADR-01]: ../../../../docs/specs/architecture/decisions/ADR-01.md
/// [S-051]: ../../../../docs/planning/journal.md#s-051-coverage-surfaces-and-untested-hotspots
/// [FR-CV-01]: ../../../../docs/specs/requirements/FR-CV-01.md
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IngestSummary {
    /// The format used — detected from content or forced by `--format`.
    pub format: CoverageFormat,
    /// The ingest-time HEAD SHA every file in this ingest is anchored to.
    pub head_sha: String,
    /// The snapshot this ingest wrote into.
    pub snapshot_id: i64,
    /// The effective `[coverage]` config hash recorded on the snapshot ([FR-CV-09]).
    pub config_hash: String,
    /// blake3 of the report file bytes — the dedup key for idempotent re-ingest.
    pub report_hash: String,
    /// Files whose report path bound to an indexed file.
    pub matched_files: usize,
    /// Report paths that bound to no indexed file or to more than one (ambiguous);
    /// listed, never silently dropped ([FR-CV-03]).
    pub unmatched: Vec<String>,
    /// Total instrumented lines across matched files (covered + uncovered).
    pub instrumented_lines: usize,
    /// Instrumented lines with at least one hit.
    pub covered_lines: usize,
    /// `true` when a snapshot already existed at this HEAD (a merge), `false` when
    /// this ingest opened a new snapshot ([FR-CV-04]).
    pub merged_into_existing: bool,
    /// `true` when this exact report was already ingested into the snapshot —
    /// nothing was written ([UAT-CV-01] idempotency).
    pub already_ingested: bool,
    /// Matched files rejected during a merge because their anchored content hash
    /// no longer matched the current file ([FR-CV-04] mismatch notice).
    pub rejected_stale: Vec<String>,
}

/// The outcome of the opt-in `coverage refresh` ([FR-CV-10], [ADR-38]): the
/// command that was run, the artifact it produced and ingested, and the resulting
/// [`IngestSummary`]. A `Serialize` read-model so the CLI `coverage refresh` and
/// the MCP `coverage_refresh` twin render the same payload byte-for-byte
/// ([NFR-CC-01]).
///
/// [NFR-CC-01]: ../../../../docs/specs/requirements/NFR-CC-01.md
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CoverageRefreshSummary {
    /// The `refresh_cmd` that was run (via `sh -c`, cwd = project root). The lone
    /// explicit coverage subprocess, off the serve/watcher path ([ADR-38]).
    pub command: String,
    /// The root-relative artifact path the refresh produced and ingested.
    pub artifact: String,
    /// The ingest summary for the produced artifact ([FR-CV-01]).
    pub ingest: IngestSummary,
}

/// Ingest a coverage report at `report_path` into the evidence store under `root`,
/// mapping report paths against `indexed_paths` ([FR-CV-01]..[FR-CV-04]).
///
/// `format_override` forces a parser (the `--format` flag); `None` auto-detects
/// from content. `cfg` supplies `path_strip_prefixes`; `cfg` and `ingest_cfg`
/// together produce the config hash recorded on the snapshot — both the
/// `[coverage]` and `[coverage_ingest]` tables are folded into one provenance
/// hash ([FR-CV-09], [ADR-38]), so a change to either re-stamps the next ingest.
///
/// # Errors
/// Fails loud (the surface maps to a non-zero exit) when the report is unreadable,
/// its format is unrecognized (and not forced), it is malformed (atomic rejection,
/// no partial write), or `root` has no resolvable HEAD to anchor to. The expected
/// per-file outcomes (unmatched, stale-rejected, idempotent no-op) are carried in
/// the returned [`IngestSummary`], never errors.
pub fn ingest(
    root: &Path,
    report_path: &Path,
    format_override: Option<CoverageFormat>,
    cfg: &EffectiveCoverage,
    ingest_cfg: &EffectiveCoverageIngest,
    indexed_paths: &[String],
) -> Result<IngestSummary> {
    // 1. Read + detect + parse — all before any store write (atomic rejection).
    let text = std::fs::read_to_string(report_path)
        .with_context(|| format!("reading coverage report {}", report_path.display()))?;
    let format = match format_override {
        Some(f) => f,
        None => parse::detect_format(&text).ok_or_else(|| {
            anyhow!(
                "could not detect the coverage format of {} (expected LCOV `TN:`/`SF:` \
                 records or a Cobertura `<coverage>` XML root); pass --format",
                report_path.display()
            )
        })?,
    };
    let parsed = parse::parse(format, &text)
        .with_context(|| format!("parsing {} as {}", report_path.display(), format.as_str()))?;
    let report_hash = blake3::hash(text.as_bytes()).to_hex().to_string();

    // 2. Anchor to HEAD — without a HEAD there is nothing to anchor to ([FR-CV-02]).
    let head_sha = super::miner::head_sha(root).ok_or_else(|| {
        anyhow!(
            "cannot ingest coverage: {} is not a git repository with a resolvable HEAD \
             (coverage snapshots are anchored to the HEAD SHA)",
            root.display()
        )
    })?;
    let config_hash = combined_config_hash(cfg, ingest_cfg);

    // 3. Map report paths to indexed files; dedup each file's lines (max within a
    //    report — Cobertura emits a line at both method and class level), summing
    //    only happens across reports in the store ([FR-CV-04]).
    let mapper = PathMapper::new(indexed_paths, &cfg.path_strip_prefixes);
    let mut per_indexed: BTreeMap<String, BTreeMap<i64, i64>> = BTreeMap::new();
    let mut unmatched: BTreeSet<String> = BTreeSet::new();
    for file in parsed {
        match mapper.map(&file.path) {
            Some(indexed) => {
                let lines = per_indexed.entry(indexed.to_string()).or_default();
                for hit in file.lines {
                    let slot = lines.entry(hit.line_no).or_insert(hit.hits);
                    if hit.hits > *slot {
                        *slot = hit.hits;
                    }
                }
            }
            None => {
                unmatched.insert(file.path);
            }
        }
    }

    // 4. Anchor each matched file to its current content hash. A matched file that
    //    cannot be read on disk cannot be anchored — surfaced as unmatched rather
    //    than fabricated ([NFR-RA-05]).
    let mut matched_files = Vec::new();
    for (path, lines_map) in per_indexed {
        match content_hash(&root.join(&path)) {
            Some(content_hash) => matched_files.push(MatchedFile {
                path,
                content_hash,
                lines: lines_map.into_iter().collect(),
            }),
            None => {
                unmatched.insert(path);
            }
        }
    }

    let instrumented_lines: usize = matched_files.iter().map(|f| f.lines.len()).sum();
    let covered_lines: usize = matched_files
        .iter()
        .map(|f| f.lines.iter().filter(|(_, hits)| *hits > 0).count())
        .sum();

    // 5. Persist (opening the store is the first write — atomic rejection above
    //    guarantees a corrupt report never reaches this point).
    let mut conn = super::open(root)?;
    let outcome = store::persist_ingest(
        &mut conn,
        &head_sha,
        &config_hash,
        &report_hash,
        format,
        &matched_files,
    )?;

    Ok(IngestSummary {
        format,
        head_sha,
        snapshot_id: outcome.snapshot_id,
        config_hash,
        report_hash,
        matched_files: matched_files.len(),
        unmatched: unmatched.into_iter().collect(),
        instrumented_lines,
        covered_lines,
        merged_into_existing: outcome.merged_into_existing,
        already_ingested: outcome.already_ingested,
        rejected_stale: outcome.rejected_stale,
    })
}

/// blake3 of a file's bytes — the per-file freshness anchor ([FR-CV-02]). `None`
/// if the file cannot be read (the caller treats it as unmatched, never fabricates
/// an anchor).
fn content_hash(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
}

/// The provenance hash recorded on a coverage snapshot ([FR-CV-09], [ADR-21]
/// pattern): a blake3 over the two advisory-tier config-table hashes — `[coverage]`
/// (`rules.toml`, path mapping) and `[coverage_ingest]` (`config.toml`, automatic
/// ingest, [ADR-38]). Folding both into one digest means a change to *either*
/// table re-stamps the next ingest, while a fixed config is byte-identical across
/// targets ([NFR-RA-06]). The sub-hashes are fixed-length hex with a labeled
/// separator, so no concatenation can collide.
fn combined_config_hash(cov: &EffectiveCoverage, ingest_cfg: &EffectiveCoverageIngest) -> String {
    let canonical = format!("coverage={};coverage_ingest={}", cov.hash(), ingest_cfg.hash());
    blake3::hash(canonical.as_bytes()).to_hex().to_string()
}

// ── Read surface ([FR-CV-05], [FR-CV-06], [S-051]) ──────────────────────────

/// A covered file is **fresh**: its current content matches the ingest anchor, so
/// its derived coverage value is rendered ([FR-CV-05]).
pub const FRESHNESS_FRESH: &str = "fresh";
/// A covered file is **stale**: its content moved since ingest — the label is
/// shown, the (shifted) line data never is ([FR-CV-05], [NFR-RA-05]).
pub const FRESHNESS_STALE: &str = "stale";
/// A file the snapshot never covered — `n/a` on a read surface ([FR-CV-05]).
pub const FRESHNESS_NA: &str = "n/a";

/// The one-line notice both surfaces render when no coverage has been ingested
/// ([FR-CV-06]): `n/a`, exit 0, never an error.
pub const NO_COVERAGE_NOTICE: &str =
    "no coverage ingested — run `logos coverage ingest <report>` to populate the evidence tier";

/// The `coverage status` read-model ([FR-CV-06]) — a `Serialize` payload shared
/// byte-for-byte by the CLI and the MCP `coverage_status` twin ([NFR-CC-01]).
///
/// Carries snapshot provenance, the per-file freshness rows ([FR-CV-05]), the
/// overall freshness fraction, and the overall line-coverage aggregate
/// ([FR-CV-06], [CR-021]). Raw numbers only — no thresholds, no grading
/// ([BR-28], [NFR-CC-04]).
///
/// [NFR-CC-01]: ../../../../docs/specs/requirements/NFR-CC-01.md
/// [NFR-CC-04]: ../../../../docs/specs/requirements/NFR-CC-04.md
/// [BR-28]: ../../../../docs/specs/software-spec.md#323-coverage-test-evidence
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CoverageStatus {
    /// The ingest-time HEAD SHA the snapshot is anchored to; `None` when no
    /// coverage exists.
    pub head_sha: Option<String>,
    /// The effective `[coverage]` config hash recorded at ingest ([FR-CV-09]).
    pub config_hash: Option<String>,
    /// Distinct report formats merged into the snapshot, sorted.
    pub formats: Vec<String>,
    /// Number of reports merged into the snapshot ([FR-CV-06]).
    pub report_count: usize,
    /// Covered files in the snapshot (fresh + stale).
    pub total_files: usize,
    /// Covered files whose content still matches the anchor ([FR-CV-05]).
    pub fresh_files: usize,
    /// Covered files whose content moved since ingest ([FR-CV-05]).
    pub stale_files: usize,
    /// Fraction of covered files that are fresh, in basis points (0–10000);
    /// `None` when the snapshot covers no files.
    pub freshness_bp: Option<i64>,
    /// The overall line-coverage aggregate ([FR-CV-06], [CR-021]): covered ÷
    /// instrumented lines summed over the **fresh** files, in basis points
    /// (0–10000); `None` (`n/a`) when no fresh covered lines exist. Raw and
    /// ungraded so the Dashboard can headline one project-wide % without
    /// computing it in the view ([BR-28], [FR-UI-03]).
    pub overall_coverage_bp: Option<i64>,
    /// One row per covered file, ordered by `path`.
    pub files: Vec<CoverageFileStatus>,
    /// `n/a` notice when no coverage has been ingested; `None` otherwise.
    pub notice: Option<String>,
    /// The working tree's current HEAD SHA at read time, or `None` when `root` is
    /// not a git work tree with a resolvable HEAD (so no artifact-vs-HEAD
    /// comparison is possible). Reused from the ingest anchoring path ([FR-CV-02]).
    pub current_head: Option<String>,
    /// `true` when coverage exists but was ingested at a HEAD other than the
    /// current one — the artifact **lags HEAD** and may no longer reflect the tree
    /// ([FR-CV-06], [ADR-38]). `false` when fresh, when no coverage exists, or when
    /// HEAD is unresolvable (never guessed, [NFR-RA-05]).
    pub head_stale: bool,
    /// A one-line refresh prompt when [`head_stale`](Self::head_stale); `None`
    /// otherwise. Advisory wording only — it never grades or moves the gate
    /// ([BR-28]).
    pub staleness_prompt: Option<String>,
}

/// One covered file's freshness-resolved coverage on the `status` surface
/// ([FR-CV-05]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CoverageFileStatus {
    /// Repo-relative indexed path ([FR-CV-03]).
    pub path: String,
    /// `"fresh"` or `"stale"` ([FR-CV-05]).
    pub freshness: &'static str,
    /// Line coverage in basis points (0–10000); `Some` only when fresh — stale
    /// coverage is a label, never a (shifted) number ([FR-CV-05]).
    pub coverage_bp: Option<i64>,
    /// Instrumented line count; `0` (not rendered) when stale.
    pub instrumented_lines: i64,
    /// Covered line count; `0` when stale.
    pub covered_lines: i64,
}

/// Read the latest coverage snapshot under `root` into the [`CoverageStatus`]
/// read-model, resolving each file's freshness against the current tree
/// ([FR-CV-05], [FR-CV-06]).
///
/// With no coverage ingested, returns an `n/a` status carrying [`NO_COVERAGE_NOTICE`]
/// — never an error, so the surface exits 0 ([FR-CV-06]).
///
/// # Errors
/// Returns an error only on an unexpected store failure.
pub fn status(root: &Path) -> Result<CoverageStatus> {
    // The working tree's current HEAD — the artifact-vs-HEAD staleness reference
    // ([FR-CV-06], [ADR-38]). `None` for a non-git/unborn/shallow repo, in which
    // case no comparison is made (never fabricated, [NFR-RA-05]).
    let current_head = super::miner::head_sha(root);

    let Some(view) = read::read_latest(root)? else {
        return Ok(CoverageStatus {
            head_sha: None,
            config_hash: None,
            formats: Vec::new(),
            report_count: 0,
            total_files: 0,
            fresh_files: 0,
            stale_files: 0,
            freshness_bp: None,
            overall_coverage_bp: None,
            files: Vec::new(),
            notice: Some(NO_COVERAGE_NOTICE.to_string()),
            current_head,
            head_stale: false,
            staleness_prompt: None,
        });
    };

    let total_files = view.files.len();
    let fresh_files = view.files.iter().filter(|f| f.fresh).count();
    let stale_files = total_files - fresh_files;
    let freshness_bp = (total_files > 0)
        .then(|| (fresh_files as i64 * 10_000 + total_files as i64 / 2) / total_files as i64);
    let overall_coverage_bp = read::overall_coverage_bp(&view.files);

    let files = view
        .files
        .iter()
        .map(|f| CoverageFileStatus {
            path: f.path.clone(),
            freshness: if f.fresh {
                FRESHNESS_FRESH
            } else {
                FRESHNESS_STALE
            },
            coverage_bp: f.coverage_bp(),
            instrumented_lines: f.instrumented_lines,
            covered_lines: f.covered_lines,
        })
        .collect();

    // Artifact-vs-HEAD staleness ([FR-CV-06], [ADR-38]): the snapshot is anchored
    // to the HEAD the coverage was ingested at; if the tree has since moved to a
    // different HEAD the artifact lags and a refresh is advised. Only compared
    // when the current HEAD resolves — an unresolvable HEAD is never treated as
    // stale (never guessed, [NFR-RA-05]). This is coarser than, and independent
    // of, the per-file content freshness above: files can be content-fresh while
    // the artifact still predates the current commit.
    let head_stale = current_head
        .as_deref()
        .is_some_and(|head| head != view.head_sha);
    let staleness_prompt = head_stale.then(|| {
        format!(
            "coverage artifact lags HEAD (ingested at {}, HEAD is {}) — \
             re-ingest with `logos coverage ingest <report>` or `logos coverage refresh`",
            short_sha(&view.head_sha),
            current_head.as_deref().map(short_sha).unwrap_or_default(),
        )
    });

    Ok(CoverageStatus {
        head_sha: Some(view.head_sha),
        config_hash: Some(view.config_hash),
        formats: view.formats,
        report_count: view.report_count,
        total_files,
        fresh_files,
        stale_files,
        freshness_bp,
        overall_coverage_bp,
        files,
        notice: None,
        current_head,
        head_stale,
        staleness_prompt,
    })
}

/// The conventional 7-char short form of a commit SHA for human-facing prompts.
/// Shorter strings pass through unchanged (a test fixture or unusual ref).
fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}
