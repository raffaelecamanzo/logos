//! Per-file temporal metrics over the mined `history.db` facts ([FR-GH-03],
//! [FR-GH-04], [FR-GH-05]) and the append-only snapshot series ([FR-GH-09]).
//!
//! Every value is a **documented heuristic, never a calibrated measure**
//! ([NFR-CC-04]); the defect count in particular must be rendered with an
//! explicit "heuristic" label by every surface ([FR-GH-05]).
//!
//! # The determinism contract ([BR-27], [NFR-RA-06], [ADR-08])
//! The whole computation is a **pure function of (stored facts, the HEAD
//! committer timestamp, the effective config)** — the wall clock never enters
//! any formula. Three disciplines make it byte-identical across runs and the
//! four CI targets:
//!
//! - **HEAD-anchored "now"** — every age is `head_committed_at − committed_at`,
//!   read from the mined `commits` rows ([`super::db::committed_at_of`]); no
//!   `git`/`Utc::now` call at compute time.
//! - **Re-filter at compute time** — the store is a *superset* (a prior, larger
//!   window may have mined more). The metrics re-apply the *current* window
//!   cutoff ([`super::miner::window_cutoff`]) and the *current* mega-commit cap,
//!   so a shrunk window or a lowered cap changes the output deterministically
//!   without any stale per-file table.
//! - **Canonical order + integer rounding** ([ADR-08]) — facts load in a fixed
//!   `(committed_at, sha)` / `(path, sha)` order; ratio metrics round to an
//!   integer `bp` (`/10000`, the gated-signal scale) so cross-target `ln`/`sqrt`
//!   residue is absorbed before serialization. The golden ([UAT-GH-01]) pins the
//!   rounded values on every target.
//!
//! Files with **no in-window commit** never appear here — a lookup miss is the
//! `n/a` the surface renders, never a fabricated zero ([FR-GH-03], [NFR-RA-05]).
//!
//! [FR-GH-03]: ../../../docs/specs/requirements/FR-GH-03.md
//! [FR-GH-04]: ../../../docs/specs/requirements/FR-GH-04.md
//! [FR-GH-05]: ../../../docs/specs/requirements/FR-GH-05.md
//! [FR-GH-09]: ../../../docs/specs/requirements/FR-GH-09.md
//! [BR-27]: ../../../docs/specs/software-spec.md#322-git-history-analytics
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
//! [UAT-GH-01]: ../../../docs/specs/requirements/UAT-GH-01.md

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use regex::RegexSet;
use serde::Serialize;

use crate::config::EffectiveHistory;

use super::db::{TemporalSnapshotRow, WindowChange, WindowCommit};
use super::miner::{window_cutoff, DegradedReason, MineOutcome};

/// Co-change support floor: a partner file must co-occur with the subject in at
/// least this many qualifying (non-mega) commits to count toward its scatter
/// ([FR-GH-04] "above a documented support threshold"). Fixed at 1 — every
/// genuine co-occurrence counts; the mega-commit cap, not this floor, is what
/// keeps a mass refactor from poisoning the score. A named constant per the
/// [CR-006] CRA-05 "fix the constant under golden tests" posture.
///
/// [CR-006]: ../../../docs/requests/CR-006-git-history-analytics.md
const CO_CHANGE_MIN_SUPPORT: u32 = 1;

/// The basis-point scale (`/10000`) ratio metrics round to — the same integer
/// resolution the gated 0–10000 signal uses ([ADR-08]).
const BP_SCALE: f64 = 10_000.0;

/// Seconds in a day — the unit ages are reported in (calendar-free; a pure
/// difference of committer timestamps).
const SECS_PER_DAY: f64 = 86_400.0;

/// The per-file temporal payload ([FR-GH-03]..[FR-GH-05]) plus the evaluation
/// provenance ([FR-GH-09]). The read-model the [api-facade] temporal surfaces
/// ([S-048]) consume; serialisable per the [ADR-01] Engine convention.
///
/// `files` is in canonical path order; a path absent from it has no in-window
/// history and renders `n/a` ([NFR-RA-05]). When `degraded` is `Some`, the tier
/// could not be computed (non-git / `git` absent / shallow) and `files` is empty.
///
/// [api-facade]: ../../../docs/specs/architecture/components/api-facade.md
/// [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
/// [S-048]: ../../../docs/planning/journal.md#s-048-hotspot-ranking-and-temporal-reporting-surfaces
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TemporalReport {
    /// The evaluated HEAD commit, or `None` when degraded / an empty repo.
    pub head_sha: Option<String>,
    /// The newest commit incorporated into the store at this evaluation.
    pub mined_through: Option<String>,
    /// The local `git --version` the facts were mined with ([ADR-22]).
    pub git_version: Option<String>,
    /// The effective `[history]` config hash recorded with this evaluation
    /// ([BR-27], [FR-GH-09]) — changes whenever any `[history]` key changes.
    pub config_hash: String,
    /// The resolved HEAD-anchored window in calendar months.
    pub window_months: u32,
    /// The HEAD committer timestamp — the determinism "now" ([BR-27]); `None`
    /// when degraded / empty.
    pub head_committed_at: Option<i64>,
    /// Per-file temporal metrics, canonical path order. Absent path = `n/a`.
    pub files: Vec<FileTemporal>,
    /// `Some` when the tier degraded instead of computing ([FR-GH-08]).
    pub degraded: Option<DegradedReason>,
    /// `true` when this evaluation populated a previously-empty store — the
    /// surface renders the [FR-GH-02] one-line first-mine notice from this flag
    /// (carried up from [`MineOutcome::first_mine`](super::miner::MineOutcome)).
    pub first_mine: bool,
}

/// One file's temporal metrics over the HEAD-anchored window ([FR-GH-03]..05).
/// All fields are integers ([ADR-08] rounded-persistence discipline).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileTemporal {
    /// Repo-relative path (the post-rename path as recorded in `file_changes`).
    pub path: String,
    /// **Churn** — number of in-window commits that touched the file ([FR-GH-03]).
    pub commit_count: i64,
    /// **Churn** — lines added across those commits (binary changes count 0).
    pub lines_added: i64,
    /// **Churn** — lines deleted across those commits.
    pub lines_deleted: i64,
    /// **Code-age volatility (recency)** — whole days from the file's most recent
    /// in-window change to HEAD ([FR-GH-03]); `0` when HEAD itself touched it.
    pub last_change_age_days: i64,
    /// **Code-age volatility (dispersion)** — population std-dev of the file's
    /// change ages in whole days ([FR-GH-03]); `0` for a single change.
    pub age_dispersion_days: i64,
    /// **Ownership dispersion** — `(1 − dominant-author share) × 10000`
    /// ([FR-GH-03]); `0` for a single author, approaching `10000` as authorship
    /// fragments.
    pub ownership_dispersion_bp: i64,
    /// **Change entropy** — Shannon entropy of the per-author change distribution,
    /// normalised by `log2(author count)` and scaled `× 10000` ([FR-GH-03]); `0`
    /// for a single author, `10000` for a perfectly even split.
    pub change_entropy_bp: i64,
    /// **Co-change scatter** — distinct files that co-changed with this one in a
    /// non-mega commit, at or above the support floor ([FR-GH-04]).
    pub co_change_count: i64,
    /// **Defect-history heuristic** — count of in-window commits touching the file
    /// whose subject matched `[history] defect_patterns` ([FR-GH-05]). A
    /// commit-hygiene heuristic, **not** a defect measure: every surface MUST
    /// render it with an explicit "heuristic" label ([NFR-CC-04]).
    pub defect_commits: i64,
}

/// Compute the per-file temporal report from a mine outcome + the store, and —
/// for a real (non-degraded, headed) evaluation — the [`TemporalSnapshotRow`] to
/// append ([FR-GH-09]).
///
/// Degraded / empty outcomes short-circuit to an empty, `n/a`-everywhere report
/// with **no** snapshot (there are no facts to record — never fabricate,
/// [NFR-RA-05]). Otherwise it loads the in-window facts once and computes both
/// the report and its aggregates deterministically (module docs).
///
/// # Errors
/// Returns an error on a store-read failure or if `defect_patterns` does not
/// compile (a `[history]` misconfiguration, surfaced at read time exactly like
/// an invalid `rules.toml`).
pub(crate) fn compute_temporal(
    conn: &rusqlite::Connection,
    cfg: &EffectiveHistory,
    outcome: &MineOutcome,
    git_version: &str,
) -> Result<(TemporalReport, Option<TemporalSnapshotRow>)> {
    let config_hash = cfg.hash();

    // Degraded or headless (empty repo): no facts, no snapshot, no fabrication.
    let Some(head_sha) = outcome.head_sha.clone() else {
        return Ok((
            empty_report(cfg, config_hash, outcome.degraded, outcome.first_mine),
            None,
        ));
    };
    if outcome.degraded.is_some() {
        return Ok((
            empty_report(cfg, config_hash, outcome.degraded, outcome.first_mine),
            None,
        ));
    }

    // The determinism "now": HEAD's committer timestamp, read from a mined fact.
    let Some(head_committed_at) = super::db::committed_at_of(conn, &head_sha)? else {
        // HEAD is not in the store (an empty/headless mine) — n/a everywhere.
        return Ok((
            empty_report(cfg, config_hash, None, outcome.first_mine),
            None,
        ));
    };

    // Re-filter the (possibly larger) store to the *current* window.
    let cutoff = window_cutoff(head_committed_at, cfg.window_months);
    let commits = super::db::load_window_commits(conn, cutoff)?;
    let changes = super::db::load_window_changes(conn, cutoff)?;

    let defect_set =
        RegexSet::new(&cfg.defect_patterns).context("compiling [history] defect_patterns")?;

    let files = compute_files(&commits, &changes, head_committed_at, cfg, &defect_set);
    let mined_through = outcome
        .mined_through
        .clone()
        .unwrap_or_else(|| head_sha.clone());

    let row = aggregate(&files, &commits, &changes, &defect_set).into_row(
        head_sha.clone(),
        mined_through.clone(),
        git_version.to_string(),
        config_hash.clone(),
        cfg.window_months,
        head_committed_at,
    );

    let report = TemporalReport {
        head_sha: Some(head_sha),
        mined_through: Some(mined_through),
        git_version: Some(git_version.to_string()),
        config_hash,
        window_months: cfg.window_months,
        head_committed_at: Some(head_committed_at),
        files,
        degraded: None,
        first_mine: outcome.first_mine,
    };
    Ok((report, Some(row)))
}

/// An empty report (degraded, headless, or empty repo) — every file is `n/a`.
fn empty_report(
    cfg: &EffectiveHistory,
    config_hash: String,
    degraded: Option<DegradedReason>,
    first_mine: bool,
) -> TemporalReport {
    TemporalReport {
        head_sha: None,
        mined_through: None,
        git_version: None,
        config_hash,
        window_months: cfg.window_months,
        head_committed_at: None,
        files: Vec::new(),
        degraded,
        first_mine,
    }
}

/// The pure metric core — deterministic over its inputs (the unit-test seam).
///
/// `commits` and `changes` are the in-window facts in canonical order; the result
/// is in canonical path order (a `BTreeMap` drives both the accumulation and the
/// co-change pairing).
fn compute_files(
    commits: &[WindowCommit],
    changes: &[WindowChange],
    head_committed_at: i64,
    cfg: &EffectiveHistory,
    defect_set: &RegexSet,
) -> Vec<FileTemporal> {
    // sha → its commit fact, for per-change lookups.
    let by_sha: BTreeMap<&str, &WindowCommit> =
        commits.iter().map(|c| (c.sha.as_str(), c)).collect();

    // Per-file accumulator, keyed in canonical path order.
    let mut acc: BTreeMap<&str, FileAccum> = BTreeMap::new();
    for ch in changes {
        let Some(commit) = by_sha.get(ch.commit_sha.as_str()) else {
            continue; // a change whose commit fell outside the window (defensive)
        };
        let entry = acc.entry(ch.path.as_str()).or_default();
        entry.commit_count += 1;
        entry.lines_added += ch.added.unwrap_or(0);
        entry.lines_deleted += ch.deleted.unwrap_or(0);
        entry
            .ages_secs
            .push((head_committed_at - commit.committed_at).max(0));
        *entry
            .author_commits
            .entry(commit.author_email.as_str())
            .or_insert(0) += 1;
        if defect_set.is_match(&commit.subject) {
            entry.defect_commits += 1;
        }
    }

    let co_change = co_change_counts(commits, changes, cfg.co_change_max_commit_files);

    acc.into_iter()
        .map(|(path, a)| FileTemporal {
            path: path.to_string(),
            commit_count: a.commit_count,
            lines_added: a.lines_added,
            lines_deleted: a.lines_deleted,
            last_change_age_days: a.last_change_age_days(),
            age_dispersion_days: a.age_dispersion_days(),
            ownership_dispersion_bp: a.ownership_dispersion_bp(),
            change_entropy_bp: a.change_entropy_bp(),
            co_change_count: *co_change.get(path).unwrap_or(&0),
            defect_commits: a.defect_commits,
        })
        .collect()
}

/// Per-file co-change scatter ([FR-GH-04]): the count of distinct partner files
/// reaching [`CO_CHANGE_MIN_SUPPORT`] co-occurrences in **non-mega** commits.
///
/// A commit touching more than `cap` files is skipped **for pairing only** — its
/// changes still counted toward churn above — so one mass refactor cannot inflate
/// every file's scatter ([FR-GH-04] mega-commit cap).
fn co_change_counts<'a>(
    commits: &[WindowCommit],
    changes: &'a [WindowChange],
    cap: u32,
) -> BTreeMap<&'a str, i64> {
    // Files touched by each qualifying (non-mega) commit, canonical order.
    let file_count_by_sha: BTreeMap<&str, i64> = commits
        .iter()
        .map(|c| (c.sha.as_str(), c.file_count))
        .collect();
    let mut files_by_commit: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for ch in changes {
        let over_cap = file_count_by_sha
            .get(ch.commit_sha.as_str())
            .is_some_and(|&n| n > cap as i64);
        if over_cap {
            continue; // mega-commit: excluded from pairing
        }
        files_by_commit
            .entry(ch.commit_sha.as_str())
            .or_default()
            .push(ch.path.as_str());
    }

    // partner support: file → (partner → co-occurrence count).
    let mut support: BTreeMap<&str, BTreeMap<&str, u32>> = BTreeMap::new();
    for files in files_by_commit.values() {
        for &f in files {
            for &g in files {
                if f != g {
                    *support.entry(f).or_default().entry(g).or_insert(0) += 1;
                }
            }
        }
    }

    support
        .into_iter()
        .map(|(f, partners)| {
            let n = partners
                .values()
                .filter(|&&s| s >= CO_CHANGE_MIN_SUPPORT)
                .count() as i64;
            (f, n)
        })
        .collect()
}

/// Project-level aggregates over the per-file report — the `temporal_snapshots`
/// row body ([FR-GH-09]).
pub(crate) fn aggregate(
    files: &[FileTemporal],
    commits: &[WindowCommit],
    changes: &[WindowChange],
    defect_set: &RegexSet,
) -> Aggregates {
    let total_added: i64 = changes.iter().map(|c| c.added.unwrap_or(0)).sum();
    let total_deleted: i64 = changes.iter().map(|c| c.deleted.unwrap_or(0)).sum();
    let defect_commits = commits
        .iter()
        .filter(|c| defect_set.is_match(&c.subject))
        .count() as i64;
    let max_churn_commits = files.iter().map(|f| f.commit_count).max().unwrap_or(0);
    let n = files.len() as i64;
    // Round-to-nearest, mirroring the per-file `bp` discipline ([ADR-08]) rather
    // than truncating toward zero — the means are persisted snapshot aggregates.
    let mean = |sum: i64| {
        if n == 0 {
            0
        } else {
            (sum as f64 / n as f64).round() as i64
        }
    };
    Aggregates {
        file_count: n,
        total_commits: commits.len() as i64,
        total_added,
        total_deleted,
        defect_commits,
        max_churn_commits,
        mean_ownership_dispersion_bp: mean(files.iter().map(|f| f.ownership_dispersion_bp).sum()),
        mean_change_entropy_bp: mean(files.iter().map(|f| f.change_entropy_bp).sum()),
    }
}

/// Project-level temporal aggregates ([FR-GH-09]).
pub(crate) struct Aggregates {
    pub(crate) file_count: i64,
    pub(crate) total_commits: i64,
    pub(crate) total_added: i64,
    pub(crate) total_deleted: i64,
    /// Distinct in-window commits whose subject matched — **not** a sum of the
    /// per-file `defect_commits` (a fix-commit touching N files is one here, N
    /// across the per-file rows).
    pub(crate) defect_commits: i64,
    pub(crate) max_churn_commits: i64,
    pub(crate) mean_ownership_dispersion_bp: i64,
    pub(crate) mean_change_entropy_bp: i64,
}

impl Aggregates {
    /// Assemble the `temporal_snapshots` row to append, pinning the provenance
    /// every snapshot carries ([FR-GH-09], [BR-27]).
    pub(crate) fn into_row(
        self,
        head_sha: String,
        mined_through: String,
        git_version: String,
        config_hash: String,
        window_months: u32,
        head_committed_at: i64,
    ) -> TemporalSnapshotRow {
        TemporalSnapshotRow {
            head_sha,
            mined_through,
            git_version,
            config_hash,
            window_months,
            head_committed_at,
            file_count: self.file_count,
            total_commits: self.total_commits,
            total_added: self.total_added,
            total_deleted: self.total_deleted,
            defect_commits: self.defect_commits,
            max_churn_commits: self.max_churn_commits,
            mean_ownership_dispersion_bp: self.mean_ownership_dispersion_bp,
            mean_change_entropy_bp: self.mean_change_entropy_bp,
        }
    }
}

/// Mutable per-file accumulator used while folding the change facts.
#[derive(Default)]
struct FileAccum<'a> {
    commit_count: i64,
    lines_added: i64,
    lines_deleted: i64,
    /// Age (HEAD − commit) in seconds, one per touching commit.
    ages_secs: Vec<i64>,
    /// author email → commits touching the file.
    author_commits: BTreeMap<&'a str, i64>,
    defect_commits: i64,
}

impl FileAccum<'_> {
    /// Whole days from the most recent change to HEAD (smallest age).
    fn last_change_age_days(&self) -> i64 {
        self.ages_secs
            .iter()
            .min()
            .map(|&s| (s as f64 / SECS_PER_DAY).floor() as i64)
            .unwrap_or(0)
    }

    /// Population std-dev of the change ages in whole days ([ADR-08] rounded).
    fn age_dispersion_days(&self) -> i64 {
        let n = self.ages_secs.len();
        if n < 2 {
            return 0;
        }
        let days: Vec<f64> = self
            .ages_secs
            .iter()
            .map(|&s| s as f64 / SECS_PER_DAY)
            .collect();
        let mean = days.iter().sum::<f64>() / n as f64;
        let var = days.iter().map(|d| (d - mean) * (d - mean)).sum::<f64>() / n as f64;
        var.sqrt().round() as i64
    }

    /// `(1 − dominant share) × 10000`, rounded and clamped to `0..=10000`.
    fn ownership_dispersion_bp(&self) -> i64 {
        let total: i64 = self.author_commits.values().sum();
        if total == 0 {
            return 0;
        }
        let dominant = self.author_commits.values().copied().max().unwrap_or(0);
        let ratio = 1.0 - (dominant as f64 / total as f64);
        round_bp(ratio)
    }

    /// Normalised Shannon entropy `× 10000`, rounded ([ADR-08]).
    fn change_entropy_bp(&self) -> i64 {
        let total: i64 = self.author_commits.values().sum();
        let k = self.author_commits.len();
        if total == 0 || k < 2 {
            return 0;
        }
        // Canonical order: `author_commits` is a BTreeMap (sorted by email).
        let total = total as f64;
        let entropy: f64 = self
            .author_commits
            .values()
            .map(|&c| {
                let p = c as f64 / total;
                -p * p.log2()
            })
            .sum();
        round_bp(entropy / (k as f64).log2())
    }
}

/// Round a `[0, 1]` ratio to an integer basis point, clamped to `0..=10000`.
fn round_bp(ratio: f64) -> i64 {
    (ratio * BP_SCALE).round().clamp(0.0, BP_SCALE) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> EffectiveHistory {
        EffectiveHistory {
            window_months: 12,
            co_change_max_commit_files: 50,
            defect_patterns: EffectiveHistory::default_defect_patterns(),
        }
    }

    fn commit(sha: &str, at: i64, email: &str, subject: &str, file_count: i64) -> WindowCommit {
        WindowCommit {
            sha: sha.to_string(),
            committed_at: at,
            author_email: email.to_string(),
            subject: subject.to_string(),
            file_count,
        }
    }

    fn change(sha: &str, path: &str, added: i64, deleted: i64) -> WindowChange {
        WindowChange {
            commit_sha: sha.to_string(),
            path: path.to_string(),
            added: Some(added),
            deleted: Some(deleted),
        }
    }

    /// Churn, ownership, entropy, recency, dispersion and the defect heuristic on
    /// a hand-verifiable two-author fixture. HEAD = day 4; A touched on days 1,2,3.
    #[test]
    fn metrics_on_a_two_author_file() {
        let day = 86_400;
        let head = 4 * day;
        let defect = RegexSet::new(&cfg().defect_patterns).unwrap();
        let commits = vec![
            commit("c1", day, "alice@x", "initial A", 1),
            commit("c2", 2 * day, "bob@x", "fix: bug in A", 1),
            commit("c3", 3 * day, "alice@x", "tweak A", 1),
        ];
        let changes = vec![
            change("c1", "a.rs", 1, 0),
            change("c2", "a.rs", 1, 0),
            change("c3", "a.rs", 1, 0),
        ];
        let files = compute_files(&commits, &changes, head, &cfg(), &defect);
        assert_eq!(files.len(), 1);
        let a = &files[0];
        assert_eq!(a.commit_count, 3);
        assert_eq!(a.lines_added, 3);
        assert_eq!(a.lines_deleted, 0);
        // Most recent change is c3 (day 3) → 1 day old; ages [3,2,1] → std-dev ≈ 0.82 → 1.
        assert_eq!(a.last_change_age_days, 1);
        assert_eq!(a.age_dispersion_days, 1);
        // Alice 2 / 3 dominant → dispersion 1 − 2/3 = 0.3333 → 3333 bp.
        assert_eq!(a.ownership_dispersion_bp, 3333);
        // H(2/3,1/3)/log2(2) ≈ 0.9183 → 9183 bp.
        assert_eq!(a.change_entropy_bp, 9183);
        // "fix: bug in A" matches the default patterns once.
        assert_eq!(a.defect_commits, 1);
    }

    /// A single-author single-change file reports zero dispersion/ownership/
    /// entropy — never a fabricated nonzero (the n/a-adjacent honesty floor).
    #[test]
    fn single_author_file_has_zero_dispersion() {
        let defect = RegexSet::new(&cfg().defect_patterns).unwrap();
        let commits = vec![commit("c1", 100, "solo@x", "add", 1)];
        let changes = vec![change("c1", "solo.rs", 5, 0)];
        let files = compute_files(&commits, &changes, 100, &cfg(), &defect);
        let f = &files[0];
        assert_eq!(f.commit_count, 1);
        assert_eq!(f.ownership_dispersion_bp, 0);
        assert_eq!(f.change_entropy_bp, 0);
        assert_eq!(f.age_dispersion_days, 0);
        assert_eq!(f.last_change_age_days, 0, "HEAD itself touched it");
        assert_eq!(f.defect_commits, 0);
    }

    /// A perfectly even two-author split is maximum entropy (10000 bp).
    #[test]
    fn even_split_is_max_entropy() {
        let defect = RegexSet::new(&cfg().defect_patterns).unwrap();
        let commits = vec![
            commit("c1", 10, "alice@x", "a", 1),
            commit("c2", 20, "bob@x", "b", 1),
        ];
        let changes = vec![change("c1", "f.rs", 1, 0), change("c2", "f.rs", 1, 0)];
        let files = compute_files(&commits, &changes, 20, &cfg(), &defect);
        assert_eq!(files[0].change_entropy_bp, 10000);
        assert_eq!(files[0].ownership_dispersion_bp, 5000, "1 − 1/2");
    }

    /// The mega-commit cap excludes a mass refactor from pairing while its churn
    /// still lands ([FR-GH-04]).
    #[test]
    fn mega_commit_excluded_from_co_change_but_not_churn() {
        let defect = RegexSet::new(&cfg().defect_patterns).unwrap();
        let cap_cfg = EffectiveHistory {
            co_change_max_commit_files: 3,
            ..cfg()
        };
        // A normal 2-file commit pairs a.rs↔b.rs; a 4-file mega commit (> cap 3)
        // touches a.rs, x, y, z but must NOT pair them.
        let commits = vec![
            commit("normal", 10, "dev@x", "pair", 2),
            commit("mega", 20, "dev@x", "mass refactor", 4),
        ];
        let changes = vec![
            change("normal", "a.rs", 1, 0),
            change("normal", "b.rs", 1, 0),
            change("mega", "a.rs", 1, 0),
            change("mega", "x.rs", 1, 0),
            change("mega", "y.rs", 1, 0),
            change("mega", "z.rs", 1, 0),
        ];
        let files = compute_files(&commits, &changes, 20, &cap_cfg, &defect);
        let a = files.iter().find(|f| f.path == "a.rs").unwrap();
        // Churn reflects BOTH commits.
        assert_eq!(
            a.commit_count, 2,
            "the mega commit still counts toward churn"
        );
        // Co-change sees only b.rs (from the normal commit) — never x/y/z.
        assert_eq!(a.co_change_count, 1, "only the non-mega partner pairs");
    }

    /// A binary file change (`added`/`deleted` = `None`) contributes **zero**
    /// lines toward churn — never a panic, never a fabricated nonzero.
    #[test]
    fn binary_file_change_counts_zero_lines() {
        let defect = RegexSet::new(&cfg().defect_patterns).unwrap();
        let commits = vec![commit("c1", 100, "dev@x", "add asset", 1)];
        let changes = vec![WindowChange {
            commit_sha: "c1".to_string(),
            path: "logo.png".to_string(),
            added: None,
            deleted: None,
        }];
        let files = compute_files(&commits, &changes, 100, &cfg(), &defect);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].lines_added, 0, "binary → 0 added, not fabricated");
        assert_eq!(files[0].lines_deleted, 0);
        assert_eq!(files[0].commit_count, 1, "the change still counts as churn");
    }

    /// A file whose only commit predates the cutoff is simply absent — the
    /// `n/a` the surface renders, never a fabricated zero ([NFR-RA-05]). Here the
    /// caller has already filtered facts to the window, so the file's empty
    /// change set yields no entry at all.
    #[test]
    fn file_with_no_in_window_change_is_absent() {
        let defect = RegexSet::new(&cfg().defect_patterns).unwrap();
        let commits = vec![commit("c1", 100, "a@x", "in window", 1)];
        let changes = vec![change("c1", "present.rs", 1, 0)];
        let files = compute_files(&commits, &changes, 100, &cfg(), &defect);
        assert!(files.iter().all(|f| f.path == "present.rs"));
        assert!(
            !files.iter().any(|f| f.path == "absent.rs"),
            "a file with no in-window change never appears (n/a, never fabricated)"
        );
    }
}
