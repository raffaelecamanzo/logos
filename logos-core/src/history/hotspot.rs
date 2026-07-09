//! Hotspot ranking — the temporal tier's headline surface ([FR-GH-06]).
//!
//! A hotspot is a file that is **both** changing a lot (churn, [FR-GH-03]) and
//! structurally complex (per-function cyclomatic complexity, aggregated per
//! file). The score is the product of the file's *churn rank* and its
//! *complexity rank* — a **Rust-side join across the two stores**
//! (`history.db` churn × `logos.db` complexity), never an SQL `ATTACH`
//! ([ADR-22], [BR-26]).
//!
//! ## Why ranks, not raw magnitudes
//! Ranking compresses each axis onto the same ordinal scale, so the *join*
//! drives the top of the board rather than one runaway axis (a giant generated
//! file with enormous raw churn, say). Competition ranking is used: a file's
//! rank on an axis is `1 + (number of candidate files with a strictly lower
//! value on that axis)`, so ties share the lower rank and the highest value
//! earns the highest rank. The score sorts descending; ties break by path
//! ascending — byte-identical across runs and the four CI targets
//! ([NFR-RA-06]).
//!
//! ## Honest handling of missing inputs ([NFR-RA-05])
//! Only files present in **both** inputs are ranked. A file with no in-window
//! history (absent from the temporal report) or no parsed functions (no
//! aggregated complexity) is **excluded** — never scored with a fabricated
//! zero. When the whole tier is degraded (non-git / `git` absent / shallow),
//! the report is empty and carries the degraded notice ([FR-GH-08]).
//!
//! This module is pure (no I/O): the [`Engine`](crate::Engine) reads the two
//! stores and hands the results to [`aggregate_complexity`] + [`rank`].
//!
//! [FR-GH-03]: ../../../docs/specs/requirements/FR-GH-03.md
//! [FR-GH-05]: ../../../docs/specs/requirements/FR-GH-05.md
//! [FR-GH-06]: ../../../docs/specs/requirements/FR-GH-06.md
//! [FR-GH-08]: ../../../docs/specs/requirements/FR-GH-08.md
//! [ADR-22]: ../../../docs/specs/architecture/decisions/ADR-22.md
//! [BR-26]: ../../../docs/specs/software-spec.md#322-git-history-analytics
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::Serialize;

use super::miner::DegradedReason;
use super::temporal::TemporalReport;
use crate::graph_store::{FunctionMetricRow, NodeRow};
use crate::model::NodeId;

/// The non-gated tier label — makes the two-tier boundary explicit on every
/// surface that renders temporal data ([NFR-CC-04], [BR-26]).
pub const TIER_LABEL: &str = "temporal (non-gated, advisory)";

/// The mandatory label on the defect-history column: it is a commit-hygiene
/// **heuristic**, never a defect measure ([FR-GH-05], [NFR-CC-04]).
pub const DEFECT_LABEL: &str = "heuristic";

/// The one-line notice rendered when an evaluation populated a previously-empty
/// history store ([FR-GH-02]) — driven by [`TemporalReport::first_mine`].
pub const FIRST_MINE_NOTICE: &str =
    "history mined for the first time — temporal metrics are now available";

/// The coverage column basis when a coverage snapshot exists: the column carries
/// real execution-coverage values ([FR-CV-07]).
pub const COVERAGE_BASIS: &str = "coverage";
/// The coverage column basis when no coverage has been ingested: the
/// untested ranking falls back to the static-reachability signal, explicitly
/// labeled so the two signals are never blended silently ([FR-CV-07], [FR-GV-08],
/// [NFR-CC-04]).
pub const STATIC_BASIS: &str = "static-reachability";
/// The mandatory label on the static-reachability fallback — the canonical
/// `test_gaps` caveat ([FR-GV-08], [BR-16]): reachability is not execution
/// coverage. Rendered whenever `--untested` runs with no coverage ingested.
pub const STATIC_FALLBACK_LABEL: &str = "static reachability, not execution coverage";

/// The per-file coverage cell on a hotspot row ([FR-CV-05], [FR-CV-07]):
/// `fresh` (with a value), `stale` (label only), or `n/a` (never covered).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageCell {
    /// One of [`coverage::FRESHNESS_FRESH`](crate::history::coverage::FRESHNESS_FRESH),
    /// `FRESHNESS_STALE`, or `FRESHNESS_NA`.
    pub state: &'static str,
    /// Line coverage in basis points (0–10000); `Some` only when `state` is
    /// fresh — stale/`n/a` coverage is never a number ([FR-CV-05]).
    pub coverage_bp: Option<i64>,
}

/// A covered file's freshness-resolved coverage, as the rank join consumes it —
/// the minimal projection of [`coverage::CoverageView`](crate::history::coverage)
/// the [`rank`] needs (the richer per-line counts stay on the `status` surface).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileCoverage {
    /// `true` when the file's content still matches the ingest anchor ([FR-CV-05]).
    pub fresh: bool,
    /// Line coverage in basis points (0–10000); meaningful only when `fresh`.
    pub coverage_bp: i64,
}

/// The hotspot ranking read-model ([FR-GH-06]) — a `Serialize` payload shared
/// byte-for-byte by the CLI and MCP surfaces ([NFR-CC-01]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HotspotReport {
    /// Tier label ([NFR-CC-04]): the temporal tier is advisory and never moves
    /// the gate ([BR-26]).
    pub tier: &'static str,
    /// The mandatory heuristic label on every file's `defect_commits`
    /// ([FR-GH-05]).
    pub defect_label: &'static str,
    /// The HEAD the ranking was computed at; `None` when degraded / empty.
    pub head_sha: Option<String>,
    /// The effective `[history]` config hash ([FR-GH-09]).
    pub config_hash: String,
    /// The `--limit N` applied to `files`, if any.
    pub limit: Option<usize>,
    /// Total files that satisfied **both** inputs and were ranked, before
    /// `--limit` truncation — so a caller knows how many hotspots exist.
    pub ranked_files: usize,
    /// The ranked hotspots, highest score first, capped at `limit`.
    pub files: Vec<Hotspot>,
    /// `Some` when the tier degraded ([FR-GH-08]); `files` is then empty.
    pub degraded: Option<DegradedReason>,
    /// A one-line notice (first-mine and/or degraded), or `None`.
    pub notice: Option<String>,
    /// Whether the `--untested` filter was applied ([FR-CV-07]): when `true`,
    /// `files` is restricted to untested hotspots (no fresh positive coverage).
    pub untested: bool,
    /// Whether the optional production-scope filter was applied ([FR-GH-06],
    /// [CR-076]): when `true`, whole test files are dropped from the candidate
    /// set before ranking. `false` (the default) is byte-identical to the
    /// whole-repo board.
    ///
    /// [CR-076]: ../../../docs/requests/CR-076-hotspots-production-scope-filter.md
    pub production_scope: bool,
    /// The coverage column / untested-filter basis: [`COVERAGE_BASIS`] when a
    /// coverage snapshot exists, else [`STATIC_BASIS`] (the labeled fallback).
    pub coverage_basis: &'static str,
    /// The static-reachability caveat ([`STATIC_FALLBACK_LABEL`]), present only
    /// when `coverage_basis` is the fallback — the two signals are never blended
    /// without a label ([FR-CV-07], [NFR-CC-04]).
    pub coverage_label: Option<&'static str>,
}

/// One ranked hotspot file ([FR-GH-06]). Flat and scannable; the defect column
/// is labeled a heuristic at the report level ([FR-GH-05]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hotspot {
    /// Repo-relative path (the post-rename path recorded in `file_changes`).
    pub path: String,
    /// `churn_rank × complexity_rank` — higher is hotter.
    pub score: i64,
    /// Competition rank of the file's churn among the ranked set (higher =
    /// more churn).
    pub churn_rank: i64,
    /// In-window commit count — the churn axis ([FR-GH-03]).
    pub churn_commits: i64,
    /// Competition rank of the file's aggregated complexity (higher = more
    /// complex).
    pub complexity_rank: i64,
    /// Σ per-function `cyclomatic_complexity` over the file ([FR-EX-03]).
    pub complexity: i64,
    /// Co-change scatter ([FR-GH-04]) — context for the hotspot.
    pub co_change_count: i64,
    /// Defect-history **heuristic** count ([FR-GH-05]); see `defect_label`.
    pub defect_commits: i64,
    /// Per-file coverage column ([FR-CV-05], [FR-CV-07]): fresh value / stale
    /// label / `n/a`. `n/a` for every file when no coverage is ingested.
    pub coverage: CoverageCell,
}

/// Aggregate per-function cyclomatic complexity into a per-file total
/// (`Σ cyclomatic_complexity`), keyed by repo-relative path in canonical order.
///
/// A function whose complexity was never computed (`NULL`) contributes nothing;
/// a file all of whose functions are `NULL` (or that has no functions) is
/// **absent** from the map — the "no parsed functions" exclusion ([FR-GH-06],
/// [NFR-RA-05]). A node with no `file_path` is skipped.
pub fn aggregate_complexity(
    nodes: &[NodeRow],
    functions: &[FunctionMetricRow],
) -> BTreeMap<String, i64> {
    // NodeId → its file path (only nodes bound to a file participate).
    let file_of: BTreeMap<_, &str> = nodes
        .iter()
        .filter_map(|n| n.file_path.as_deref().map(|p| (n.id, p)))
        .collect();

    let mut by_file: BTreeMap<String, i64> = BTreeMap::new();
    for f in functions {
        let (Some(cc), Some(path)) = (f.cyclomatic_complexity, file_of.get(&f.id)) else {
            continue;
        };
        *by_file.entry((*path).to_string()).or_insert(0) += cc;
    }
    by_file
}

/// The optional production-scope candidate-set filter ([FR-GH-06], [CR-076]):
/// the set of file paths that are **whole test files** — every
/// complexity-contributing function in the file (the same functions
/// [`aggregate_complexity`] sums) is `is_test` ([FR-AN-05]). A production file
/// carrying an in-file `#[cfg(test)] mod tests` keeps at least one non-test
/// contributing function, so it is never a member here — only its test
/// functions are `is_test`, and the file's production functions keep it on the
/// board ([CR-076] AC). A file with no complexity-contributing functions at
/// all is never a member either (it is already absent from
/// [`aggregate_complexity`]'s map, so [`rank`] excludes it regardless).
///
/// [CR-076]: ../../../docs/requests/CR-076-hotspots-production-scope-filter.md
/// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
pub fn test_only_files(
    nodes: &[NodeRow],
    functions: &[FunctionMetricRow],
    test_ids: &HashSet<NodeId>,
) -> BTreeSet<String> {
    let file_of: BTreeMap<_, &str> = nodes
        .iter()
        .filter_map(|n| n.file_path.as_deref().map(|p| (n.id, p)))
        .collect();

    // path → has this file seen a non-test complexity-contributing function?
    let mut has_production_function: BTreeMap<String, bool> = BTreeMap::new();
    for f in functions {
        let (Some(_), Some(path)) = (f.cyclomatic_complexity, file_of.get(&f.id)) else {
            continue;
        };
        let entry = has_production_function
            .entry((*path).to_string())
            .or_insert(false);
        *entry |= !test_ids.contains(&f.id);
    }

    has_production_function
        .into_iter()
        .filter(|(_, has_production)| !has_production)
        .map(|(path, _)| path)
        .collect()
}

/// Rank the files present in **both** the temporal report and the per-file
/// complexity map, joining the coverage column and producing the
/// [`HotspotReport`] ([FR-GH-06], [FR-CV-07]).
///
/// `coverage` is the latest snapshot's freshness-resolved per-file coverage, or
/// `None` when no coverage has been ingested — in which case the coverage column
/// is `n/a` for every file and the report carries the static-reachability
/// fallback label ([FR-CV-07], [FR-GV-08]). `untested = true` applies the
/// untested filter ([FR-CV-07]): files with **no fresh positive coverage** (n/a,
/// stale, or fresh-0%) are retained — stale is treated as absent for ranking so
/// shifted line data never drives the board ([NFR-RA-05]); the order is the same
/// hotspot score (`churn_rank × complexity_rank`), tie-broken by path ascending.
///
/// Pure and deterministic over its inputs ([NFR-RA-06]). A degraded temporal
/// tier yields an empty, `n/a` report carrying the degraded notice
/// ([FR-GH-08]); a first-mine evaluation carries the first-mine notice
/// ([FR-GH-02]).
///
/// `production_scope = true` drops every path in `test_only_files` ([CR-076])
/// from the candidate set **before** the churn/complexity competition ranks
/// are computed, so the ranks reflect the narrowed production-only set, not
/// the whole-repo set with test rows subtracted after the fact. `false` (the
/// default) ignores `test_only_files` entirely and is byte-identical to the
/// pre-[CR-076] whole-repo board.
///
/// [CR-076]: ../../../docs/requests/CR-076-hotspots-production-scope-filter.md
#[allow(clippy::too_many_arguments)]
pub fn rank(
    temporal: TemporalReport,
    complexity_by_path: &BTreeMap<String, i64>,
    coverage: Option<&BTreeMap<String, FileCoverage>>,
    limit: Option<usize>,
    untested: bool,
    production_scope: bool,
    test_only_files: &BTreeSet<String>,
) -> HotspotReport {
    // The coverage column basis is independent of the temporal tier's health: it
    // reflects only whether a coverage snapshot exists ([FR-CV-07]).
    let coverage_basis = if coverage.is_some() {
        COVERAGE_BASIS
    } else {
        STATIC_BASIS
    };
    let coverage_label = (coverage_basis == STATIC_BASIS).then_some(STATIC_FALLBACK_LABEL);

    // Degraded: the whole tier is n/a — never a partial or fabricated board.
    if let Some(reason) = temporal.degraded {
        return HotspotReport {
            tier: TIER_LABEL,
            defect_label: DEFECT_LABEL,
            head_sha: None,
            config_hash: temporal.config_hash,
            limit,
            ranked_files: 0,
            files: Vec::new(),
            degraded: Some(reason),
            notice: Some(reason.message().to_string()),
            untested,
            production_scope,
            coverage_basis,
            coverage_label,
        };
    }

    // The coverage cell for one file ([FR-CV-05]): fresh value / stale / n/a.
    let coverage_cell = |path: &str| -> CoverageCell {
        match coverage.and_then(|c| c.get(path)) {
            Some(fc) if fc.fresh => CoverageCell {
                state: crate::history::coverage::FRESHNESS_FRESH,
                coverage_bp: Some(fc.coverage_bp),
            },
            Some(_) => CoverageCell {
                state: crate::history::coverage::FRESHNESS_STALE,
                coverage_bp: None,
            },
            None => CoverageCell {
                state: crate::history::coverage::FRESHNESS_NA,
                coverage_bp: None,
            },
        }
    };

    // The join set: files with BOTH churn (in-window history) and complexity
    // (parsed functions). Missing either input is an honest exclusion.
    struct Candidate<'a> {
        path: &'a str,
        churn: i64,
        complexity: i64,
        co_change_count: i64,
        defect_commits: i64,
    }
    let candidates: Vec<Candidate> = temporal
        .files
        .iter()
        .filter_map(|f| {
            // Production-scope exclusion happens BEFORE the competition ranks
            // below are computed, so an enabled filter narrows the ranking
            // population itself, not just the rendered rows (CR-076).
            if production_scope && test_only_files.contains(&f.path) {
                return None;
            }
            complexity_by_path
                .get(&f.path)
                .map(|&complexity| Candidate {
                    path: &f.path,
                    churn: f.commit_count,
                    complexity,
                    co_change_count: f.co_change_count,
                    defect_commits: f.defect_commits,
                })
        })
        .collect();

    // Competition rank on each axis: 1 + #{strictly lower}. Highest value wins
    // the highest rank; ties share. Computed against the candidate set.
    let churns: Vec<i64> = candidates.iter().map(|c| c.churn).collect();
    let complexities: Vec<i64> = candidates.iter().map(|c| c.complexity).collect();
    let competition_rank =
        |value: i64, all: &[i64]| -> i64 { 1 + all.iter().filter(|&&v| v < value).count() as i64 };

    let mut files: Vec<Hotspot> = candidates
        .iter()
        .map(|c| {
            let churn_rank = competition_rank(c.churn, &churns);
            let complexity_rank = competition_rank(c.complexity, &complexities);
            Hotspot {
                path: c.path.to_string(),
                score: churn_rank * complexity_rank,
                churn_rank,
                churn_commits: c.churn,
                complexity_rank,
                complexity: c.complexity,
                co_change_count: c.co_change_count,
                defect_commits: c.defect_commits,
                coverage: coverage_cell(c.path),
            }
        })
        .collect();

    // `--untested`: retain only files with no FRESH POSITIVE coverage ([FR-CV-07]).
    // Stale is treated as absent (its line data is never trusted, [NFR-RA-05]);
    // with no coverage ingested every file is n/a, so the board is unchanged and
    // the static-reachability fallback label carries the provenance.
    if untested {
        files.retain(|h| {
            h.coverage.state != crate::history::coverage::FRESHNESS_FRESH
                || h.coverage.coverage_bp == Some(0)
        });
    }

    // Deterministic order: score desc, then path asc (the documented
    // tie-break, [NFR-RA-06]).
    files.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));

    let ranked_files = files.len();
    if let Some(n) = limit {
        files.truncate(n);
    }

    let notice = temporal.first_mine.then(|| FIRST_MINE_NOTICE.to_string());

    HotspotReport {
        tier: TIER_LABEL,
        defect_label: DEFECT_LABEL,
        head_sha: temporal.head_sha,
        config_hash: temporal.config_hash,
        limit,
        ranked_files,
        files,
        degraded: None,
        notice,
        untested,
        production_scope,
        coverage_basis,
        coverage_label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::FileTemporal;
    use crate::model::{LogosSymbol, NodeId, NodeKind};

    /// A `FileTemporal` with only the fields the hotspot join reads set; the
    /// rest are zeroed (irrelevant to ranking).
    fn ft(path: &str, churn: i64, co_change: i64, defect: i64) -> FileTemporal {
        FileTemporal {
            path: path.to_string(),
            commit_count: churn,
            lines_added: 0,
            lines_deleted: 0,
            last_change_age_days: 0,
            age_dispersion_days: 0,
            ownership_dispersion_bp: 0,
            change_entropy_bp: 0,
            co_change_count: co_change,
            defect_commits: defect,
        }
    }

    fn report(files: Vec<FileTemporal>) -> TemporalReport {
        TemporalReport {
            head_sha: Some("deadbeef".into()),
            mined_through: Some("deadbeef".into()),
            git_version: Some("git version 2.50".into()),
            config_hash: "cafef00d".into(),
            window_months: 12,
            head_committed_at: Some(1_700_000_000),
            files,
            degraded: None,
            first_mine: false,
        }
    }

    fn node(id: i64, path: &str) -> NodeRow {
        NodeRow {
            id: NodeId(id),
            symbol: LogosSymbol::parse(&format!("local {id}")).expect("local symbol parses"),
            kind: NodeKind::Function,
            name: format!("f{id}"),
            file_path: Some(path.to_string()),
            start_line: None,
            end_line: None,
        }
    }

    fn func(id: i64, cc: Option<i64>) -> FunctionMetricRow {
        FunctionMetricRow {
            id: NodeId(id),
            cyclomatic_complexity: cc,
            is_dead: None,
            is_duplicate: None,
            line_count: None,
            max_nesting_depth: None,
            clone_group: None,
        }
    }

    #[test]
    fn complexity_sums_per_file_and_excludes_null_and_unbound() {
        let nodes = vec![node(1, "a.rs"), node(2, "a.rs"), node(3, "b.rs")];
        let funcs = vec![func(1, Some(3)), func(2, Some(5)), func(3, None)];
        let agg = aggregate_complexity(&nodes, &funcs);
        assert_eq!(agg.get("a.rs"), Some(&8), "Σ CC over a.rs");
        // b.rs has only a NULL-CC function → absent (no parsed complexity).
        assert!(!agg.contains_key("b.rs"), "NULL-CC file is excluded");
    }

    #[test]
    fn engineered_high_churn_high_complexity_file_ranks_first() {
        // hot.rs is top on both axes → must rank #1 (UAT-GH-03).
        let temporal = report(vec![
            ft("hot.rs", 10, 4, 2),
            ft("churny.rs", 9, 1, 0),
            ft("complex.rs", 1, 0, 0),
        ]);
        let complexity = BTreeMap::from([
            ("hot.rs".to_string(), 40),
            ("churny.rs".to_string(), 2),
            ("complex.rs".to_string(), 38),
        ]);
        let r = rank(temporal, &complexity, None, None, false, false, &BTreeSet::new());
        assert_eq!(r.files[0].path, "hot.rs");
        assert_eq!(r.files[0].churn_rank, 3);
        assert_eq!(r.files[0].complexity_rank, 3);
        assert_eq!(r.files[0].score, 9);
        assert_eq!(r.ranked_files, 3);
        // No coverage ingested → every column is n/a and the report carries the
        // static-reachability fallback basis (FR-CV-07).
        assert_eq!(r.coverage_basis, STATIC_BASIS);
        assert_eq!(r.coverage_label, Some(STATIC_FALLBACK_LABEL));
        assert!(r.files.iter().all(|h| h.coverage.state == "n/a"));
    }

    #[test]
    fn missing_either_input_is_excluded_never_fabricated() {
        let temporal = report(vec![
            ft("both.rs", 5, 0, 0),
            ft("no_complexity.rs", 7, 0, 0), // history but no parsed functions
        ]);
        // "complexity_only.rs" has complexity but no history.
        let complexity = BTreeMap::from([
            ("both.rs".to_string(), 10),
            ("complexity_only.rs".to_string(), 99),
        ]);
        let r = rank(temporal, &complexity, None, None, false, false, &BTreeSet::new());
        assert_eq!(r.ranked_files, 1, "only the intersection is ranked");
        assert_eq!(r.files[0].path, "both.rs");
    }

    #[test]
    fn ties_break_by_path_and_limit_truncates() {
        // Two files identical on both axes → identical score; path breaks the tie.
        let temporal = report(vec![ft("z.rs", 3, 0, 0), ft("a.rs", 3, 0, 0)]);
        let complexity = BTreeMap::from([("z.rs".to_string(), 5), ("a.rs".to_string(), 5)]);
        let r = rank(temporal, &complexity, None, Some(1), false, false, &BTreeSet::new());
        assert_eq!(r.ranked_files, 2);
        assert_eq!(r.files.len(), 1, "--limit truncates");
        assert_eq!(r.files[0].path, "a.rs", "tie breaks by path asc");
    }

    #[test]
    fn every_degraded_mode_is_na_with_notice_and_no_files() {
        // All three FR-GH-08 / UAT-GH-04 modes flow through the same n/a path:
        // non-git, `git` absent, and shallow clone (the real-fixture variants
        // are covered by tests/hotspots.rs; this pins the surface handling for
        // each reason, including `git`-absent which is awkward to fixture).
        for reason in [
            DegradedReason::NotGit,
            DegradedReason::GitAbsent,
            DegradedReason::Shallow,
        ] {
            let mut temporal = report(vec![ft("x.rs", 1, 0, 0)]);
            temporal.degraded = Some(reason);
            let r = rank(
                temporal,
                &BTreeMap::new(),
                None,
                None,
                false,
                false,
                &BTreeSet::new(),
            );
            assert!(r.files.is_empty(), "{reason:?}: no fabricated board");
            assert_eq!(r.degraded, Some(reason));
            assert_eq!(r.notice.as_deref(), Some(reason.message()));
            assert_eq!(r.defect_label, DEFECT_LABEL);
        }
    }

    #[test]
    fn first_mine_carries_the_notice() {
        let mut temporal = report(vec![ft("a.rs", 1, 0, 0)]);
        temporal.first_mine = true;
        let complexity = BTreeMap::from([("a.rs".to_string(), 3)]);
        let r = rank(temporal, &complexity, None, None, false, false, &BTreeSet::new());
        assert_eq!(r.notice.as_deref(), Some(FIRST_MINE_NOTICE));
    }

    /// A fresh-covered file reports its value; a stale one shows the label with no
    /// number; an absent one is `n/a`. With a coverage view present the basis is
    /// "coverage" and no fallback label rides ([FR-CV-05], [FR-CV-07]).
    #[test]
    fn coverage_column_renders_fresh_stale_and_na() {
        let temporal = report(vec![
            ft("fresh.rs", 3, 0, 0),
            ft("stale.rs", 2, 0, 0),
            ft("absent.rs", 1, 0, 0),
        ]);
        let complexity = BTreeMap::from([
            ("fresh.rs".to_string(), 5),
            ("stale.rs".to_string(), 5),
            ("absent.rs".to_string(), 5),
        ]);
        let coverage = BTreeMap::from([
            (
                "fresh.rs".to_string(),
                FileCoverage {
                    fresh: true,
                    coverage_bp: 7500,
                },
            ),
            (
                "stale.rs".to_string(),
                FileCoverage {
                    fresh: false,
                    coverage_bp: 0,
                },
            ),
        ]);
        let r = rank(
            temporal,
            &complexity,
            Some(&coverage),
            None,
            false,
            false,
            &BTreeSet::new(),
        );
        assert_eq!(r.coverage_basis, COVERAGE_BASIS);
        assert_eq!(
            r.coverage_label, None,
            "no fallback label when coverage exists"
        );

        let cell = |path: &str| {
            r.files
                .iter()
                .find(|h| h.path == path)
                .unwrap()
                .coverage
                .clone()
        };
        assert_eq!(cell("fresh.rs").state, "fresh");
        assert_eq!(cell("fresh.rs").coverage_bp, Some(7500));
        assert_eq!(cell("stale.rs").state, "stale");
        assert_eq!(
            cell("stale.rs").coverage_bp,
            None,
            "stale carries no number"
        );
        assert_eq!(cell("absent.rs").state, "n/a");
        assert_eq!(cell("absent.rs").coverage_bp, None);
    }

    /// `--untested` keeps only files with no fresh positive coverage (n/a, stale,
    /// or fresh-0%), preserving the hotspot score order ([FR-CV-07], [NFR-RA-05]).
    #[test]
    fn untested_filter_keeps_only_uncovered_and_stale_files() {
        // covered.rs is the hottest on both axes, but it is fresh-covered → it
        // must be filtered out under --untested even though it would rank #1.
        let temporal = report(vec![
            ft("covered.rs", 10, 0, 0),
            ft("zero.rs", 8, 0, 0),
            ft("stale.rs", 6, 0, 0),
            ft("absent.rs", 4, 0, 0),
        ]);
        let complexity = BTreeMap::from([
            ("covered.rs".to_string(), 40),
            ("zero.rs".to_string(), 30),
            ("stale.rs".to_string(), 20),
            ("absent.rs".to_string(), 10),
        ]);
        let coverage = BTreeMap::from([
            (
                "covered.rs".to_string(),
                FileCoverage {
                    fresh: true,
                    coverage_bp: 9000,
                },
            ),
            (
                "zero.rs".to_string(),
                FileCoverage {
                    fresh: true,
                    coverage_bp: 0,
                },
            ),
            (
                "stale.rs".to_string(),
                FileCoverage {
                    fresh: false,
                    coverage_bp: 0,
                },
            ),
        ]);
        let r = rank(
            temporal,
            &complexity,
            Some(&coverage),
            None,
            true,
            false,
            &BTreeSet::new(),
        );
        let paths: Vec<&str> = r.files.iter().map(|h| h.path.as_str()).collect();
        assert!(
            !paths.contains(&"covered.rs"),
            "fresh-covered file is excluded under --untested: {paths:?}"
        );
        assert_eq!(
            paths,
            ["zero.rs", "stale.rs", "absent.rs"],
            "fresh-0%, stale, and absent files remain, in score order"
        );
        assert!(r.untested);
        assert_eq!(r.ranked_files, 3, "ranked_files counts the filtered board");
    }

    /// With no coverage ingested, `--untested` falls back to the
    /// static-reachability basis and labels it — the two signals are never
    /// blended silently ([FR-CV-07], [FR-GV-08]).
    #[test]
    fn untested_without_coverage_uses_the_labeled_static_fallback() {
        let temporal = report(vec![ft("a.rs", 3, 0, 0), ft("b.rs", 2, 0, 0)]);
        let complexity = BTreeMap::from([("a.rs".to_string(), 5), ("b.rs".to_string(), 3)]);
        let r = rank(temporal, &complexity, None, None, true, false, &BTreeSet::new());
        assert_eq!(r.coverage_basis, STATIC_BASIS);
        assert_eq!(r.coverage_label, Some(STATIC_FALLBACK_LABEL));
        assert_eq!(
            r.files.len(),
            2,
            "every file is untested (n/a) under fallback"
        );
        assert!(r.files.iter().all(|h| h.coverage.state == "n/a"));
    }

    // ── CR-076: optional production-scope filter ─────────────────────────────

    #[test]
    fn test_only_files_marks_whole_test_files_and_spares_mixed_files() {
        // a.rs: every complexity-contributing function is is_test → a whole
        // test file. b.rs: mixed (one test fn, one production fn) — mirrors a
        // production file carrying an in-file `#[cfg(test)] mod tests`, which
        // must NOT be excluded ([CR-076] AC). c.rs: no is_test functions.
        let nodes = vec![
            node(1, "a.rs"),
            node(2, "a.rs"),
            node(3, "b.rs"),
            node(4, "b.rs"),
            node(5, "c.rs"),
        ];
        let functions = vec![
            func(1, Some(3)),
            func(2, Some(2)),
            func(3, Some(4)),
            func(4, Some(1)),
            func(5, Some(6)),
        ];
        let test_ids: HashSet<NodeId> = [NodeId(1), NodeId(2), NodeId(3)].into_iter().collect();
        let excluded = test_only_files(&nodes, &functions, &test_ids);
        assert!(excluded.contains("a.rs"), "a.rs: all contributing fns are is_test");
        assert!(
            !excluded.contains("b.rs"),
            "b.rs keeps a non-test contributing function (fn 4)"
        );
        assert!(!excluded.contains("c.rs"), "c.rs has no is_test functions at all");
    }

    #[test]
    fn test_only_files_never_marks_a_file_with_no_contributing_function() {
        // d.rs has a function but its complexity is NULL → absent from
        // aggregate_complexity's own map; the filter must not invent a verdict
        // for a file `rank` would already exclude for a different reason.
        let nodes = vec![node(1, "d.rs")];
        let functions = vec![func(1, None)];
        let test_ids: HashSet<NodeId> = [NodeId(1)].into_iter().collect();
        let excluded = test_only_files(&nodes, &functions, &test_ids);
        assert!(!excluded.contains("d.rs"));
    }

    /// `production_scope = true` drops a whole test file from the candidate set
    /// BEFORE the competition ranks are computed, so the remaining files' ranks
    /// reflect the narrowed production-only set — not the whole-repo set with
    /// one row subtracted after the fact. `false` stays byte-identical to the
    /// whole-repo board ([CR-076] AC).
    #[test]
    fn production_scope_drops_whole_test_files_before_ranking() {
        // tests.rs is hottest on both axes but is a whole test file.
        let temporal = report(vec![
            ft("tests.rs", 10, 0, 0),
            ft("prod.rs", 5, 0, 0),
            ft("calm.rs", 1, 0, 0),
        ]);
        let complexity = BTreeMap::from([
            ("tests.rs".to_string(), 50),
            ("prod.rs".to_string(), 20),
            ("calm.rs".to_string(), 5),
        ]);
        let test_only = BTreeSet::from(["tests.rs".to_string()]);

        // Disabled: byte-identical to today's whole-repo board.
        let off = rank(
            temporal.clone(),
            &complexity,
            None,
            None,
            false,
            false,
            &test_only,
        );
        assert_eq!(
            off.files[0].path, "tests.rs",
            "filter off: whole-repo board unchanged"
        );
        assert_eq!(off.ranked_files, 3);
        assert!(!off.production_scope);

        // Enabled: tests.rs is gone, and prod.rs's ranks are relative to the
        // narrowed 2-file production set (rank 2 of 2 on both axes), not the
        // original 3-file set.
        let on = rank(temporal, &complexity, None, None, false, true, &test_only);
        let paths: Vec<&str> = on.files.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(
            paths,
            ["prod.rs", "calm.rs"],
            "tests.rs excluded from the candidate set: {paths:?}"
        );
        assert_eq!(on.ranked_files, 2, "ranked_files reflects the narrowed set");
        assert_eq!(
            on.files[0].churn_rank, 2,
            "rank is computed over the 2-file production set, not the whole repo"
        );
        assert!(on.production_scope);
    }

    /// `production_scope` composes with `--untested`: the two filters combine
    /// (test files dropped first, then the untested retention rule applied).
    #[test]
    fn production_scope_composes_with_untested() {
        let temporal = report(vec![
            ft("tests.rs", 10, 0, 0),
            ft("covered.rs", 8, 0, 0),
            ft("prod.rs", 5, 0, 0),
        ]);
        let complexity = BTreeMap::from([
            ("tests.rs".to_string(), 50),
            ("covered.rs".to_string(), 30),
            ("prod.rs".to_string(), 20),
        ]);
        let coverage = BTreeMap::from([(
            "covered.rs".to_string(),
            FileCoverage {
                fresh: true,
                coverage_bp: 9000,
            },
        )]);
        let test_only = BTreeSet::from(["tests.rs".to_string()]);

        let r = rank(
            temporal,
            &complexity,
            Some(&coverage),
            None,
            true,
            true,
            &test_only,
        );
        let paths: Vec<&str> = r.files.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(
            paths,
            ["prod.rs"],
            "tests.rs dropped by production scope, covered.rs dropped by --untested: {paths:?}"
        );
        assert!(r.untested);
        assert!(r.production_scope);
    }
}
