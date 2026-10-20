//! The **branch delta** ([FR-GV-24], [ADR-82], S-677): every offender
//! ([FR-QM-19]) and every `check` violation of the working tree, classified
//! per symbol against the merge-base of `HEAD` and a base ref.
//!
//! # The two measurements
//!
//! - **After** is the graph as it stands. Nothing is reconciled first: a
//!   review does not sync, so an edit made since the last sync is not in the
//!   graph — and every entry in such a file says so (`stale`).
//! - **Before** is the merge-base commit, and only that commit ([ADR-82]):
//!   1. the snapshot recorded at it, when that snapshot holds everything the
//!      review compares — its uncapped offender list under the current
//!      thresholds and metric semantics, and a contract that evaluates no
//!      rule (a snapshot keeps no violation set, so a contract with rules
//!      needs the tree itself) — and never under a `[review]` policy
//!      ([`branch_delta`]);
//!   2. otherwise a shadow index of the merge-base tree, materialised from
//!      the object database by `git read-tree`/`checkout-index` into a private
//!      temp index and directory, indexed by [`shadow_reindex`], measured, and
//!      cached by commit under `.logos/review-cache/` (gitignored). A cached
//!      entry is reused only under the same contract, thresholds, admission,
//!      semantics and logos version; the cache keeps the newest
//!      [`CACHE_KEEP`] entries.
//!
//!   No other commit is ever read. When neither is possible the review says
//!   why and classifies nothing.
//!
//! Both sides are evaluated under the **current** contract and thresholds, so
//! the delta measures the code, not a contract edit: each side's derived
//! annotations are re-materialised under the live `rules.toml` before it is
//! measured, as `check` re-materialises the live graph ([`rematerialise`]).
//!
//! # Matching
//!
//! An offender is matched by dimension, measure and identity — its
//! [`Offender::node_id`], a Depth chain's head name, or an Acyclicity cycle's
//! directories (a cycle is named by its lowest-storage-id member, which a
//! re-index can change). A violation is matched by rule and subject, and
//! carries a value wherever the budget names one per subject: the review
//! evaluates the per-subject budgets with their offender marks switched on,
//! which changes no violation, only what it records. A violation with no
//! value is matched by its message too.
//!
//! # The security section
//!
//! [`security_delta`] classifies each scanner's latest stored findings against
//! the merge-base ([FR-SC-18], S-686), over evidence the engine reads and hands
//! in ([`SecurityEvidence`]) — this module never opens `history.db`, and the
//! merge-base tree is never scanned:
//!
//! - a scanner whose report was **recorded at the merge-base** is compared by
//!   fingerprint: absent there is `introduced`; present there and inside a
//!   hunk the branch changed is `touched`. A region-keyed fingerprint moves
//!   with its line, so a new fingerprint outside every hunk that has a
//!   vanished merge-base twin (same scanner, rule and file) is that finding,
//!   moved: `pre-existing`, and the twin is not resolved;
//! - any other scanner by **hunk**: inside a changed hunk is `introduced`,
//!   unless the branch deleted the file;
//! - otherwise `pre-existing`;
//! - a merge-base fingerprint the latest report no longer carries is
//!   `resolved` only when that report saw its file as it stands — the file
//!   still anchors a fresh finding of that report, or was last modified before
//!   it was ingested — and `stale` otherwise. A deleted file's finding is
//!   resolved: no scan can report it.
//!
//! A suppressed finding is listed apart with its reason and expiry.
//!
//! [FR-GV-24]: ../../../docs/specs/requirements/FR-GV-24.md
//! [FR-SC-18]: ../../../docs/specs/requirements/FR-SC-18.md
//! [FR-QM-19]: ../../../docs/specs/requirements/FR-QM-19.md
//! [ADR-82]: ../../../docs/specs/architecture/decisions/ADR-82.md
//! [`shadow_reindex`]: crate::pipeline::shadow_reindex

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{
    Budget, Constraints, DriftBudget, GodContainersBudget, MaxDead, RatchetBudget, RatchetFlag, ReviewRules,
    Rules,
};
use crate::engine::Engine;
use crate::history::{SecurityFinding, FRESHNESS_FRESH, FRESHNESS_NA, FRESHNESS_STALE};
use crate::hydrate::{build_view, Granularity};
use crate::metrics::SignalTerms;
use crate::models::quality::{
    BeforeStateCache, BranchReview, BranchShare, BranchesProjection, CloneGroupMember, DriftReference, DriverMember,
    ExceededLimit, GateResult, IntroducedCloneGroup, MetricSnapshot, Offender, ProjectedBranch,
    ProjectedDrift, ReviewPolicy, ReviewStatus, ReviewedOffender, ReviewedViolation, SecurityDelta,
    SecurityDeltaEntry, SecurityDeltaScanner, SecurityDeltaStatus, SignalAttribution, SignalDrift,
    SuppressedDeltaEntry, UnprojectedLimit, Violation, WorstOffenders,
};
use crate::navigate::branch::{changed_hunks, git_command, git_line, rev_parse, FileRanges, WHOLE_FILE};
use crate::graph_store::MetricSnapshotRow;
use crate::runtime::Runtime;
use crate::security::context::FindingContext;

use super::attribution::{attribute, Drivers};
use super::thin_graph::{absence_tag, ThinGraph};
use super::{
    append_detail, effective_thresholds, evolution_point, load_rules_cached, member_access_extensions,
    metric_snapshot_from_row, quality_runtime, rematerialise, CompiledRules, SecurityEvidence, StoreFacts,
    METRIC_NAMES, METRIC_NOISE, SCOPE_PROJECT,
};

/// The cache directory, relative to the project root. Ignored by the managed
/// `.logos/.gitignore` block.
const CACHE_DIR_REL: &str = ".logos/review-cache";

/// How many cached before-states of each kind are kept; older ones are
/// pruned, oldest first ([ADR-82]: bounded by distinct merge-bases). A review's
/// entries and a projection's (`<commit>.projection.json`, [FR-GV-28]) are
/// pruned apart, so neither evicts the other, and a projection never prunes
/// an entry its own run used: it may need more commits than this.
///
/// [FR-GV-28]: ../../../docs/specs/requirements/FR-GV-28.md
/// [ADR-82]: ../../../docs/specs/architecture/decisions/ADR-82.md
const CACHE_KEEP: usize = 16;

/// The cache entry layout. Bumped when [`Measured`] changes shape, so an old
/// entry is rebuilt rather than misread (3: the signal terms, S-755).
const CACHE_FORMAT: u32 = 3;

/// One offender of one measurement, as the cache stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Point {
    dimension: String,
    node_id: Option<String>,
    name: String,
    file: String,
    line: Option<i64>,
    detail: String,
    measure: Option<String>,
    value: Option<i64>,
    threshold: Option<i64>,
}

/// One violation of one measurement, as the cache stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Breach {
    rule: String,
    rule_type: String,
    subject: String,
    file: String,
    value: Option<u64>,
    message: String,
}

/// Everything one side of the review compares.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Measured {
    offenders: Vec<Point>,
    violations: Vec<Breach>,
    /// The terms this side's signal is the geometric mean of, which the
    /// attribution decomposes ([FR-GV-26]); `None` is an empty production graph.
    ///
    /// [FR-GV-26]: ../../../docs/specs/requirements/FR-GV-26.md
    terms: Option<SignalTerms>,
    /// The graph dimensions drop out of this side's signal: too few
    /// cross-module edges to measure module structure ([FR-QM-18]).
    ///
    /// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
    graph_not_applicable: bool,
    /// The dependency-cycle count `max_cycles` reads, recorded by a
    /// projection measurement only ([`Contract::projection`]); elided otherwise,
    /// so a review's cache entry is unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cycles: Option<u64>,
}

/// The dimensions whose offenders explain the module graph, outside the signal
/// when it is too thin to measure ([FR-QM-18]). Modularity lists no offenders.
///
/// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
const GRAPH_DIMENSIONS: [&str; 2] = ["acyclicity", "depth"];

/// Drop the graph dimensions' offenders from both sides when either side's
/// graph is too thin to measure: the gate counts no movement of a dimension
/// that is not applicable on either side (`metric_regressions`), so a longer
/// chain on a thin graph is no regression here either. The note that says so,
/// when it applies.
fn without_inapplicable_graph_points(before: &mut Measured, after: &mut Measured) -> Option<String> {
    if !(before.graph_not_applicable || after.graph_not_applicable) {
        return None;
    }
    for side in [before, after] {
        side.offenders.retain(|p| !GRAPH_DIMENSIONS.contains(&p.dimension.as_str()));
    }
    Some(
        "acyclicity and depth offenders are not classified: the module graph has too few \
         cross-module edges to measure on the merge-base or the branch"
            .to_string(),
    )
}

/// A cached before-state, keyed by commit and by everything it was measured
/// under.
#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    format: u32,
    commit: String,
    key: String,
    before: Measured,
}

/// What both sides are measured under.
struct Contract {
    /// The contract as declared — what the shadow tree's policy graph is
    /// materialised under.
    declared: std::sync::Arc<CompiledRules>,
    /// The same contract with every per-subject budget's offender marks on.
    marked: CompiledRules,
    thresholds: crate::metrics::Thresholds,
    member_access: HashSet<String>,
    /// `marked` is the [`projected`] contract ([FR-GV-28]): every subject's
    /// value is recorded, offenders are not listed and violation messages are
    /// dropped. Cached under its own key.
    ///
    /// [FR-GV-28]: ../../../docs/specs/requirements/FR-GV-28.md
    projection: bool,
}

/// The base ref as stated, else the repository's default branch, with where
/// it came from.
fn resolve_base(root: &Path, base: Option<&str>) -> (Option<String>, &'static str) {
    match base.map(str::trim).filter(|b| !b.is_empty()) {
        Some(base) => (Some(base.to_string()), BranchReview::STATED),
        None => (default_branch(root), BranchReview::DEFAULT_BRANCH),
    }
}

/// Why nothing is compared when no base was given and none resolves.
const NO_BASE: &str = "no base was given and no default branch resolves (no origin/HEAD, main or master); \
                       pass --base <ref>";

/// `review --base` ([FR-GV-24]): see the module docs.
///
/// # Errors
/// Returns an error on a transient engine, a store fault, an invalid
/// `rules.toml`, or a failed shadow index. A base that does not resolve, or a
/// merge-base that cannot be measured, is a `measured: false` answer, not an
/// error.
///
/// [FR-GV-24]: ../../../docs/specs/requirements/FR-GV-24.md
pub(crate) fn branch_review(
    engine: &Engine,
    base: Option<&str>,
    security: impl FnOnce(&str) -> Result<SecurityEvidence>,
) -> Result<BranchReview> {
    let mut review = branch_delta(engine, base)?;
    if let (true, Some(head), Some(merge_base)) = (review.measured, review.head.clone(), review.merge_base.clone()) {
        review.sections.security = Some(security_section(engine.root(), &head, &merge_base, security));
    }
    // FR-GV-27: the trend reads the recorded series, measured or not. The
    // contract is read fail-soft: a measured review has already failed loud
    // on an invalid one, and an unmeasured one never read it before.
    if let Some(policy) = load_rules_cached(engine, None).ok().and_then(|c| c.rules.review.clone()) {
        review.warnings = trend_warnings(quality_runtime(engine)?, &policy)?;
    }
    Ok(review)
}

/// The quality half of [`branch_review`] — every section but security — and
/// what `gate` judges under a `rules.toml [review]` policy ([FR-GV-25]).
/// Under the policy each listed offender carries whether it gates
/// ([`mark_gating`]), and a snapshot recorded at the merge-base is never read:
/// every gate records one at `HEAD` over the working tree, so a snapshot "at"
/// the merge-base may hold uncommitted work made there — and once that work is
/// committed, a failing entry would read `unchanged` against it. A snapshot
/// does not record whether its tree was clean, so the merge-base tree is
/// measured instead (shadow-indexed once per merge-base, then cached).
///
/// # Errors
/// As [`branch_review`].
///
/// [FR-GV-25]: ../../../docs/specs/requirements/FR-GV-25.md
pub(crate) fn branch_delta(engine: &Engine, base: Option<&str>) -> Result<BranchReview> {
    let root = engine.root();
    let runtime = quality_runtime(engine)?;
    let (base, base_origin) = resolve_base(root, base);
    let mut review = BranchReview {
        base: base.clone().unwrap_or_default(),
        base_origin,
        head: rev_parse(root, "HEAD"),
        ..BranchReview::default()
    };
    let Some(base) = base else {
        return Ok(not_measured(review, NO_BASE));
    };
    let Some(head) = review.head.clone() else {
        return Ok(not_measured(review, "HEAD does not resolve to a commit"));
    };
    let Some(base_commit) = rev_parse(root, &base) else {
        return Ok(not_measured(review, &format!("{base} does not resolve to a commit")));
    };
    let Some(merge_base) = git_line(root, &["merge-base", &head, &base_commit]) else {
        return Ok(not_measured(
            review,
            &format!("HEAD and {base} have no common ancestor"),
        ));
    };
    review.merge_base = Some(merge_base.clone());

    let declared = load_rules_cached(engine, None)?;
    let contract = Contract {
        marked: CompiledRules::compile(marked(&declared.rules), declared.hash.clone())?,
        thresholds: effective_thresholds(&declared.rules),
        member_access: member_access_extensions(engine),
        declared,
        projection: false,
    };
    // The live graph was annotated under the contract of its last sync, which
    // a `rules.toml` edit since may have changed; `check` re-materialises for
    // the same reason, and so does the shadow below, so both sides are
    // measured under one contract.
    let (_, registry, config) = engine.pipeline_ctx()?;
    rematerialise(runtime, registry, &config, &contract.declared.rules)?;
    engine.advance_sync_stamp();
    let (mut after, after_checked) = measure(runtime, &contract)?;
    let policy = contract.declared.rules.review.clone();
    let recorded = if policy.is_some() {
        None
    } else {
        recorded_before(runtime, &merge_base, &contract, after_checked)?
    };
    let mut before = match recorded {
        Some(before) => {
            review.before_state = Some(BranchReview::RECORDED_SNAPSHOT);
            before
        }
        None => match shadow_before(engine, &merge_base, &contract) {
            Ok((before, cache)) => {
                review.before_state = Some(BranchReview::SHADOW_INDEX);
                review.before_state_cache = cache;
                before
            }
            // ADR-82 §3: no comparable before-state is an answer that says so
            // and classifies nothing, never a substitute and never a fault.
            Err(err) => {
                return Ok(not_measured(
                    review,
                    &format!("no comparable before-state: the merge-base tree could not be measured ({err:#})"),
                ))
            }
        },
    };

    let trust = Trust::read(engine, runtime, &before, &after)?;
    review.stale_files = trust.stale.iter().cloned().collect();
    review.notes.extend(without_inapplicable_graph_points(&mut before, &mut after));
    // Every entry is classified and counted; an `unchanged` one is counted and
    // not listed. On this repository it is 1,614 of 1,694 offenders, and
    // listing them took the answer past 500 KB — more than an agent host
    // accepts from one tool call, for entries no consumer acts on.
    review.offenders = classify_offenders(&before.offenders, &after.offenders, &trust);
    review.offenders.retain(|entry| {
        review.offender_counts.add(entry.status);
        entry.status != ReviewStatus::Unchanged
    });
    review.violations = classify_violations(&before.violations, &after.violations, &contract, &trust);
    review.violations.retain(|entry| {
        review.violation_counts.add(entry.status);
        entry.status != ReviewStatus::Unchanged
    });
    review.introduced_clone_groups =
        introduced_clone_groups(&before.offenders, &after.offenders, &trust);
    review.attribution = Some(attribute(
        before.terms.as_ref(),
        after.terms.as_ref(),
        &Drivers {
            offenders: &review.offenders,
            introduced_groups: group_members(&after.offenders, &before.offenders),
            removed_groups: group_members(&before.offenders, &after.offenders),
        },
    ));
    review.policy = policy.map(|policy| mark_gating(&mut review.offenders, &policy));
    review.measured = true;
    review.notes.push(
        "after is the graph as last synced, not reconciled: an entry in a file edited since \
         reads stale, and a file never synced is not in the answer"
            .to_string(),
    );
    review.notes.push(
        "an unchanged entry is counted in offender_counts or violation_counts and not listed"
            .to_string(),
    );
    Ok(review)
}

/// Mark each listed offender the `[review]` policy gates on ([FR-GV-25]):
/// `new` or `worsened`, in a `gate_dimensions` dimension, and judged on a file
/// the graph resolves (an entry in a stale file, or in a language bound within
/// files only, is listed and never gates — the rule the Stop hook has always
/// blocked by). Every listed entry already meets its dimension's threshold:
/// that is what makes it an offender.
///
/// [FR-GV-25]: ../../../docs/specs/requirements/FR-GV-25.md
fn mark_gating(offenders: &mut [ReviewedOffender], policy: &ReviewRules) -> ReviewPolicy {
    let mut gating = 0;
    for entry in offenders {
        entry.gating = matches!(entry.status, ReviewStatus::New | ReviewStatus::Worsened)
            && policy.gate_dimensions.contains(&entry.dimension)
            && entry.trust == BranchReview::TRUST_RESOLVED;
        gating += u64::from(entry.gating);
    }
    ReviewPolicy {
        gate_dimensions: policy.gate_dimensions.clone(),
        gating,
    }
}

/// How many gating entries the gate's message names; `gating_entries` carries
/// them all.
const GATE_MESSAGE_ENTRIES: usize = 5;

/// Fold the branch `delta` into a gate verdict under a `[review]` policy
/// ([FR-GV-25]): each gating entry fails the gate and is named with its value,
/// threshold and trust. A delta that was not measured judges nothing and says
/// why, as a warning; it never fails the gate by itself, as an unmeasured Stop
/// hook leg never blocks.
///
/// [FR-GV-25]: ../../../docs/specs/requirements/FR-GV-25.md
pub(super) fn fold_into_gate(result: &mut GateResult, delta: BranchReview) {
    if !delta.measured {
        result.warnings.push(format!(
            "[review]: the branch delta was not measured, so no per-symbol entry was judged: {}",
            delta.not_measured_because.as_deref().unwrap_or("no reason was recorded")
        ));
        return;
    }
    result.gating_entries = delta.offenders.into_iter().filter(|e| e.gating).collect();
    let count = result.gating_entries.len();
    if count == 0 {
        return;
    }
    result.passed = false;
    let mut named: Vec<String> = result
        .gating_entries
        .iter()
        .take(GATE_MESSAGE_ENTRIES)
        .map(describe_gating)
        .collect();
    if count > GATE_MESSAGE_ENTRIES {
        named.push(format!("and {} more", count - GATE_MESSAGE_ENTRIES));
    }
    let merge_base = delta.merge_base.as_deref().unwrap_or_default();
    append_detail(
        &mut result.message,
        format!(
            "{count} gating branch-delta {} against the merge-base {}: {}",
            if count == 1 { "entry" } else { "entries" },
            merge_base.get(..12).unwrap_or(merge_base),
            named.join("; ")
        ),
    );
}

/// One gating entry as the gate names it: dimension, symbol, place, the value
/// it moved to, its threshold, its class and its trust.
fn describe_gating(e: &ReviewedOffender) -> String {
    let place = match e.line {
        Some(line) => format!("{}:{line}", e.file),
        None => e.file.clone(),
    };
    let number = |v: Option<i64>| v.map_or_else(|| "?".to_string(), |v| v.to_string());
    let value = match e.before {
        Some(before) => format!("{before} -> {}", number(e.after)),
        None => number(e.after),
    };
    let threshold = e.threshold.map_or_else(|| "no threshold".to_string(), |t| format!("threshold {t}"));
    let status = match e.status {
        ReviewStatus::New => "new",
        ReviewStatus::Worsened => "worsened",
        ReviewStatus::Improved => "improved",
        ReviewStatus::Resolved => "resolved",
        ReviewStatus::Unchanged => "unchanged",
    };
    format!(
        "{} `{}` ({place}) {} {value}, {threshold} [{status}, trust {}]",
        e.dimension,
        e.name,
        e.measure.as_deref().unwrap_or("value"),
        e.trust
    )
}

/// The aggregate's movement from `baseline` to `current`, judged by `budget`
/// ([FR-GV-25]): a numeric budget is exceeded when the signal fell by more
/// than it; `"report"` never is.
///
/// [FR-GV-25]: ../../../docs/specs/requirements/FR-GV-25.md
pub(super) fn signal_drift(baseline: u32, current: u32, budget: DriftBudget) -> SignalDrift {
    let drop = i64::from(baseline) - i64::from(current);
    let budget = match budget {
        DriftBudget::Points(points) => Some(points),
        DriftBudget::Report(_) => None,
    };
    SignalDrift {
        baseline,
        current,
        drop,
        budget,
        exceeded: budget.is_some_and(|points| drop as f64 > points),
    }
}

/// The gate's message for a [`SignalDrift`].
pub(super) fn drift_message(d: &SignalDrift) -> String {
    let (current, baseline) = (d.current, d.baseline);
    let movement = if d.drop > 0 {
        format!("signal {current} drifted {} below the baseline {baseline}", d.drop)
    } else {
        format!("signal {current} holds the baseline {baseline}")
    };
    match d.budget {
        Some(points) if d.exceeded => format!("{movement}, past drift_budget {points}"),
        Some(points) => format!("{movement}, within drift_budget {points}"),
        None => format!("{movement} (drift_budget \"report\": reported, never failing)"),
    }
}

/// The trend warnings `scan`, `gate` and `review` carry under a `[review]`
/// `policy` ([FR-GV-27]): [`declining_dimensions`] over every recorded
/// snapshot. Advisory — no caller moves a verdict or an exit code by it.
///
/// [FR-GV-27]: ../../../docs/specs/requirements/FR-GV-27.md
pub(super) fn trend_warnings(runtime: &Runtime, policy: &ReviewRules) -> Result<Vec<String>> {
    let rows = runtime.submit_read(|store| store.metric_snapshots())?;
    Ok(declining_dimensions(&rows, policy.trend_window))
}

/// One warning per distributional dimension ([BR-95]) that declined in each
/// of the last `window` steps of the series that moved it, in canonical metric
/// order, naming the dimension, its values oldest first (exact, not rounded)
/// and the window.
///
/// A step is read off the series' own [`evolution_point`], so it is moved,
/// flat or not applicable exactly as `evolution` reports it. A flat step — a
/// `gate` re-measuring the tree a `scan` just measured — neither breaks the
/// run nor extends it. A rise ends it, and so does a step the dimension does
/// not apply on, an empty graph, or a change of metric semantics or
/// thresholds: across those the two values do not measure the same thing.
///
/// [BR-95]: ../../../docs/specs/software-spec.md#312-governance--rules-gate-evolution-dsm-test-gaps
pub(super) fn declining_dimensions(rows: &[MetricSnapshotRow], window: u32) -> Vec<String> {
    METRIC_NAMES
        .iter()
        .enumerate()
        .filter(|(_, name)| ReviewRules::DISTRIBUTIONAL_DIMENSIONS.contains(name))
        .filter_map(|(metric, name)| {
            let values = decline_run(rows, metric, window as usize)?;
            // Each value as its shortest exact spelling: a decline just past
            // the noise floor still prints as one, and the value reads back.
            let chain: Vec<String> = values.iter().map(f64::to_string).collect();
            Some(format!(
                "{name} declined at each of the last {window} recorded snapshot{} that moved it: {} \
                 (review.trend_window = {window}); a trend warning, never a failure",
                if window == 1 { "" } else { "s" },
                chain.join(" → "),
            ))
        })
        .collect()
}

/// The `window + 1` values, oldest first, of the latest run of `window`
/// declines of `METRIC_NAMES[metric]`, or `None` when the latest run is
/// shorter.
fn decline_run(rows: &[MetricSnapshotRow], metric: usize, window: usize) -> Option<Vec<f64>> {
    let value = |row: &MetricSnapshotRow| evolution_point(row, None).metric_deltas[metric].normalized;
    let mut values = vec![value(rows.last()?)];
    for pair in rows.windows(2).rev() {
        if values.len() > window {
            break;
        }
        let (prev, row) = (&pair[0], &pair[1]);
        let comparable = !prev.empty
            && !row.empty
            && prev.metric_version == row.metric_version
            && prev.thresholds_hash == row.thresholds_hash;
        match evolution_point(row, Some(prev)).metric_deltas[metric].delta {
            Some(delta) if comparable && delta.abs() <= METRIC_NOISE => {}
            Some(delta) if comparable && delta < 0.0 => values.push(value(prev)),
            _ => return None,
        }
    }
    (values.len() > window).then(|| {
        values.reverse();
        values
    })
}

/// `review --base <ref> --branches <b1> <b2> …` ([FR-GV-28]): each branch's
/// delta against `base`, summed and projected onto the shared limits.
///
/// Every side is a committed tree — the base commit, and each branch's
/// merge-base and tip — measured by the review's one path ([`shadow_before`]:
/// shadow-indexed once per commit, then cached) under the [`projected`]
/// contract, which records every subject's value instead of only those past a
/// ceiling. A branch's share of a (limit, subject) is its tip's value minus
/// its merge-base's, and the projection is the base's value plus every share
/// ([`project`]). A limit is listed when the shares raise it past its ceiling;
/// one the base already exceeds and no branch raises is `check`'s to report.
/// A branch that does not resolve, shares no history with the base, repeats a
/// summed branch's commit or cannot be indexed is named and left out of the
/// sum. Neither `HEAD` nor the live graph is measured; the live store is read
/// only for each listed subject's trust label.
///
/// # Errors
/// Returns an error on a transient engine, a store fault or an invalid
/// `rules.toml`. A base that does not resolve, or one that cannot be measured,
/// is a `measured: false` answer.
///
/// [FR-GV-28]: ../../../docs/specs/requirements/FR-GV-28.md
pub(crate) fn branches_review(engine: &Engine, base: Option<&str>, branches: &[String]) -> Result<BranchesProjection> {
    let root = engine.root();
    let (base, base_origin) = resolve_base(root, base);
    let mut out = BranchesProjection {
        base: base.clone().unwrap_or_default(),
        base_origin,
        ..BranchesProjection::default()
    };
    let Some(base) = base else {
        return Ok(not_projected(out, NO_BASE));
    };
    let Some(base_commit) = rev_parse(root, &base) else {
        return Ok(not_projected(out, &format!("{base} does not resolve to a commit")));
    };
    out.base_commit = Some(base_commit.clone());
    let declared = load_rules_cached(engine, None)?;
    let (limits, not_projected_limits) = projected_limits(&declared.rules);
    out.not_projected = not_projected_limits;
    let drift_budget = declared.rules.review.as_ref().map(|policy| policy.drift_budget);
    let drift_points = match drift_budget {
        Some(DriftBudget::Points(points)) => Some(points),
        Some(DriftBudget::Report(_)) => {
            out.not_projected.push(UnprojectedLimit {
                rule: "drift_budget".to_string(),
                reason: "drift_budget = \"report\" reports the drift and never fails".to_string(),
            });
            None
        }
        None => None,
    };
    if limits.is_empty() && drift_points.is_none() {
        return Ok(not_projected(
            out,
            "rules.toml declares no projectable [constraints] budget and no numeric [review] drift_budget: \
             there is no limit to project",
        ));
    }
    let ratchet = !declared.rules.constraints.ratchet_rules().is_empty();
    let contract = Contract {
        marked: CompiledRules::compile(projected(&declared.rules), declared.hash.clone())?,
        thresholds: effective_thresholds(&declared.rules),
        member_access: member_access_extensions(engine),
        declared,
        projection: true,
    };
    let base_side = match shadow_before(engine, &base_commit, &contract) {
        Ok((side, _)) => side,
        Err(err) => {
            return Ok(not_projected(out, &format!("the base commit {base_commit} could not be measured ({err:#})")))
        }
    };
    let mut sides = Vec::new();
    let mut used = BTreeSet::from([base_commit.clone()]);
    for name in branches {
        let mut branch = ProjectedBranch {
            name: name.clone(),
            ..ProjectedBranch::default()
        };
        match measure_branch(engine, &mut branch, &base_commit, &base_side, &contract, &out.branches, &mut used) {
            Ok(pair) => {
                branch.measured = true;
                sides.push((name.clone(), pair));
            }
            Err(why) => branch.not_measured_because = Some(why),
        }
        out.branches.push(branch);
    }
    prune_projection_cache(engine, &used);
    if sides.is_empty() {
        return Ok(not_projected(out, "no branch could be measured: each is named with why"));
    }

    let tallies: Vec<(String, Tally, Tally)> = sides
        .iter()
        .map(|(name, (before, after))| (name.clone(), tally(before, &limits), tally(after, &limits)))
        .collect();
    let (mut exceeded, subjects) = project(&limits, &tally(&base_side, &limits), &tallies);
    let files: BTreeSet<String> = exceeded.iter().map(|e| e.file.clone()).filter(|f| !f.is_empty()).collect();
    let labels = Trust::labels(engine, quality_runtime(engine)?, files, &BTreeSet::new())?;
    for entry in &mut exceeded {
        entry.trust = labels
            .get(&entry.file)
            .cloned()
            .unwrap_or_else(|| BranchReview::TRUST_RESOLVED.to_string());
    }
    out.limits = limits.iter().map(|l| l.rule.to_string()).collect();
    if let Some(points) = drift_points {
        let reference = drift_reference(engine, &contract.thresholds.hash())?;
        match project_drift(points, &base_side, &sides, reference) {
            Ok((drift, note)) => {
                out.limits.push("drift_budget".to_string());
                out.drift = drift;
                out.notes.push(note);
            }
            Err(reason) => out.not_projected.push(UnprojectedLimit {
                rule: "drift_budget".to_string(),
                reason,
            }),
        }
    }
    if out.limits.is_empty() {
        return Ok(not_projected(
            out,
            "no declared limit could be projected: each is named under not_projected with why",
        ));
    }
    out.exceeded = exceeded;
    out.subjects = subjects;
    out.measured = true;
    out.notes.extend(projection_notes(ratchet));
    Ok(out)
}

/// How every measured projection was produced.
fn projection_notes(ratchet: bool) -> Vec<String> {
    let mut notes = vec![
        "each side is a full index of a committed tree — the base commit, each branch's merge-base and its \
         tip: uncommitted work is not in the answer"
            .to_string(),
        "a share is a branch tip's value minus its merge-base's, and the projection is the base's value plus \
         every share: a change two branches both make is counted twice, as the merge would count it once"
            .to_string(),
        "a limit the base already exceeds is listed only when the shares raise it; check reports the rest"
            .to_string(),
    ];
    if ratchet {
        notes.push("a ratchet budget is projected against its max: the blessed set is not consulted".to_string());
    }
    notes
}

/// The `measured: false` projection: the stated refs, the reason, nothing projected.
fn not_projected(mut out: BranchesProjection, why: &str) -> BranchesProjection {
    out.not_measured_because = Some(why.to_string());
    out
}

/// Measure `branch`'s before-state and its tip, recording its commit,
/// merge-base and the summed branch it is stacked on as they resolve; the
/// reason it is left out of the sum otherwise. `summed` is every branch before
/// it. The before-state is the merge-base with the base, or — for a tip built
/// on a summed branch's tip — that tip, so no commit is counted twice. Each
/// commit it measures is added to `used`, which the run's pruning keeps.
fn measure_branch(
    engine: &Engine,
    branch: &mut ProjectedBranch,
    base_commit: &str,
    base_side: &Measured,
    contract: &Contract,
    summed: &[ProjectedBranch],
    used: &mut BTreeSet<String>,
) -> std::result::Result<(Measured, Measured), String> {
    let root = engine.root();
    let name = branch.name.trim().to_string();
    // A ref never starts with `-`; refusing one keeps it from reading as a
    // git option.
    if name.is_empty() || name.starts_with('-') {
        return Err(format!("`{}` is not a ref name", branch.name));
    }
    let commit = rev_parse(root, &name).ok_or_else(|| format!("{name} does not resolve to a commit"))?;
    branch.commit = Some(commit.clone());
    let merge_base = git_line(root, &["merge-base", &commit, base_commit])
        .ok_or_else(|| format!("{name} and the base have no common ancestor"))?;
    branch.merge_base = Some(merge_base.clone());
    // A summed branch's commits are already in the sum: a tip among them adds
    // nothing, and a tip built on one is measured from that tip, so the shared
    // commits count once.
    let summed: Vec<(&str, &str)> = summed
        .iter()
        .filter(|b| b.measured)
        .filter_map(|b| Some((b.name.as_str(), b.commit.as_deref()?)))
        .collect();
    if let Some((within, _)) = summed.iter().find(|(_, tip)| is_ancestor(root, &commit, tip)) {
        return Err(format!("its commits are all in {within}'s share, which is already in the sum"));
    }
    let under: Vec<(&str, &str)> = summed.iter().copied().filter(|(_, tip)| is_ancestor(root, tip, &commit)).collect();
    let before_commit = match under.iter().find(|(_, tip)| under.iter().all(|(_, other)| is_ancestor(root, other, tip))) {
        Some((on, tip)) => {
            branch.stacked_on = Some((*on).to_string());
            (*tip).to_string()
        }
        None if under.is_empty() => merge_base,
        None => {
            let names: Vec<&str> = under.iter().map(|(n, _)| *n).collect();
            return Err(format!(
                "it holds the commits of {}, already in the sum, on no one line: its own share cannot be separated",
                names.join(" and ")
            ));
        }
    };
    used.extend([before_commit.clone(), commit.clone()]);
    let measured = |at: &str| -> std::result::Result<Measured, String> {
        if at == base_commit {
            return Ok(base_side.clone());
        }
        shadow_before(engine, at, contract)
            .map(|(side, _)| side)
            .map_err(|err| format!("{at} could not be measured ({err:#})"))
    };
    Ok((measured(&before_commit)?, measured(&commit)?))
}

/// `true` when `ancestor` is `descendant` or one of its ancestors.
fn is_ancestor(root: &Path, ancestor: &str, descendant: &str) -> bool {
    git_command(root)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .output()
        .is_ok_and(|out| out.status.success())
}

/// The `rules.toml` contract a projection measures under ([FR-GV-28]): the
/// declared contract with every projectable `[constraints]` budget rewritten
/// to record **every** subject's value — a per-subject budget as `{ max = 0,
/// ratchet = true }`, so each module, function or file with a value above 0 is
/// one marked violation carrying it; a symbol-set budget likewise, one marked
/// violation per member — and with nothing else to evaluate. `max_cycles`
/// keeps its form: its count is recorded beside ([`Measured::cycles`]).
///
/// [FR-GV-28]: ../../../docs/specs/requirements/FR-GV-28.md
fn projected(rules: &Rules) -> Rules {
    let declared = &rules.constraints;
    let every = || RatchetBudget { max: 0, ratchet: true };
    let on = |budget: &Option<Budget>| budget.as_ref().map(|_| Budget::Ratchet(every()));
    let mut rules = rules.clone();
    rules.constraints = Constraints {
        max_cycles: declared.max_cycles.clone(),
        max_cc: on(&declared.max_cc),
        max_fn_lines: on(&declared.max_fn_lines),
        no_god_files: on(&declared.no_god_files),
        max_fan_in: on(&declared.max_fan_in),
        max_fan_out: on(&declared.max_fan_out),
        max_dead: declared.max_dead.as_ref().map(|_| MaxDead::Ratchet(every())),
        max_duplicates: on(&declared.max_duplicates),
        max_nesting_depth: on(&declared.max_nesting_depth),
        max_brain_methods: on(&declared.max_brain_methods),
        max_clone_ratio: None,
        no_god_containers: None,
    };
    rules.layers.clear();
    rules.boundaries.clear();
    rules.forbidden_imports.clear();
    rules.require_tested.clear();
    rules.require_documented.clear();
    rules
}

/// One limit a projection judges: its `rules.toml` key, the value it lets
/// through, and whether it holds each subject or one project-wide count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Limit {
    rule: &'static str,
    ceiling: u64,
    per_subject: bool,
}

/// The declared `[constraints]` limits a projection judges, in evaluation
/// order, and the declared ones it does not, each with why.
fn projected_limits(rules: &Rules) -> (Vec<Limit>, Vec<UnprojectedLimit>) {
    let c = &rules.constraints;
    let budget = |rule, b: &Option<Budget>, per_subject| {
        b.as_ref().map(|b| Limit {
            rule,
            ceiling: u64::from(b.max()),
            per_subject,
        })
    };
    let limits = [
        c.max_cycles.as_ref().map(|b| Limit {
            rule: "max_cycles",
            ceiling: b.ceiling(),
            per_subject: false,
        }),
        budget("max_cc", &c.max_cc, true),
        budget("max_fn_lines", &c.max_fn_lines, true),
        budget("no_god_files", &c.no_god_files, true),
        budget("max_fan_in", &c.max_fan_in, true),
        budget("max_fan_out", &c.max_fan_out, true),
        c.max_dead.as_ref().map(|b| Limit {
            rule: "max_dead",
            ceiling: b.ceiling(),
            per_subject: false,
        }),
        budget("max_duplicates", &c.max_duplicates, false),
        budget("max_nesting_depth", &c.max_nesting_depth, true),
        budget("max_brain_methods", &c.max_brain_methods, false),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut unprojected = Vec::new();
    if c.max_clone_ratio.is_some() {
        unprojected.push(UnprojectedLimit {
            rule: "max_clone_ratio".to_string(),
            reason: "a ratio whose numerator and denominator every branch moves: a sum of per-branch shares \
                     does not project it"
                .to_string(),
        });
    }
    if c.no_god_containers.as_ref().is_some_and(GodContainersBudget::enforced) {
        unprojected.push(UnprojectedLimit {
            rule: "no_god_containers".to_string(),
            reason: "a container is a god container by thresholds over two measures, not by a count against a \
                     ceiling: no share sums to it"
                .to_string(),
        });
    }
    (limits, unprojected)
}

/// Each (limit, subject)'s value on one side, with the subject's file. A
/// project-wide count's subject is `""`.
type Tally = BTreeMap<(&'static str, String), (u64, String)>;

/// One side's [`Tally`] over `limits`: a per-subject limit's value per marked
/// violation (a subject at 0 records none), and a project-wide count — a
/// symbol set's members, `max_cycles`'s cycles — always, at 0 too.
fn tally(side: &Measured, limits: &[Limit]) -> Tally {
    let mut out = Tally::new();
    for limit in limits {
        if limit.rule == "max_cycles" {
            out.insert((limit.rule, String::new()), (side.cycles.unwrap_or(0), String::new()));
            continue;
        }
        if !limit.per_subject {
            out.insert((limit.rule, String::new()), (0, String::new()));
        }
        for b in side.violations.iter().filter(|b| b.rule == limit.rule) {
            if limit.per_subject {
                let value = b.value.unwrap_or(0);
                out.entry((limit.rule, coordinate_less(&b.subject)))
                    .and_modify(|entry| entry.0 = entry.0.max(value))
                    .or_insert((value, b.file.clone()));
            } else {
                out.entry((limit.rule, String::new())).or_insert((0, String::new())).0 += 1;
            }
        }
    }
    out
}

/// `subject` without its repository coordinate: a module key
/// (`module:<symbol>`) names its module by the symbol's descriptor tail, as a
/// function's ratchet identity already does. A shadow-indexed tree takes its
/// coordinate from the export directory, which names nothing the reader has.
fn coordinate_less(subject: &str) -> String {
    let Some(symbol) = subject.strip_prefix("module:") else {
        return subject.to_string();
    };
    crate::model::LogosSymbol::parse(symbol)
        .ok()
        .and_then(|s| s.descriptor_tail().map(|tail| format!("module:{tail}")))
        .unwrap_or_else(|| subject.to_string())
}

/// Sum each branch's shares onto the `base` tally ([FR-GV-28]): every limit
/// the shares raise past its ceiling, in `limits` order then by subject, and
/// how many (limit, subject) pairs any side measured. A subject a side does
/// not measure is `0` there. A share of `0` is not listed.
///
/// [FR-GV-28]: ../../../docs/specs/requirements/FR-GV-28.md
fn project(limits: &[Limit], base: &Tally, branches: &[(String, Tally, Tally)]) -> (Vec<ExceededLimit>, u64) {
    let keys: BTreeSet<&(&'static str, String)> = base
        .keys()
        .chain(branches.iter().flat_map(|(_, before, after)| before.keys().chain(after.keys())))
        .collect();
    let value = |t: &Tally, key: &(&'static str, String)| t.get(key).map_or(0, |(v, _)| *v);
    let mut exceeded = Vec::new();
    for key in &keys {
        let Some(limit) = limits.iter().find(|l| l.rule == key.0) else {
            continue;
        };
        let at_base = value(base, key);
        let room = i64::try_from(limit.ceiling).unwrap_or(i64::MAX) - i64::try_from(at_base).unwrap_or(i64::MAX);
        let shares: Vec<BranchShare> = branches
            .iter()
            .map(|(name, before, after)| (name, value(after, key) as i64 - value(before, key) as i64))
            .filter(|(_, share)| *share != 0)
            .map(|(name, share)| BranchShare {
                branch: name.clone(),
                share,
                exceeds_alone: share > room,
            })
            .collect();
        let total: i64 = shares.iter().map(|s| s.share).sum();
        if total <= 0 || total <= room {
            continue;
        }
        let file = branches
            .iter()
            .rev()
            .find_map(|(_, before, after)| after.get(*key).or_else(|| before.get(*key)))
            .or_else(|| base.get(*key))
            .map(|(_, file)| file.clone())
            .unwrap_or_default();
        exceeded.push(ExceededLimit {
            rule: key.0.to_string(),
            subject: key.1.clone(),
            file,
            trust: String::new(),
            limit: limit.ceiling,
            base: at_base,
            projected: at_base as i64 + total,
            shares,
        });
    }
    let order = |rule: &str| limits.iter().position(|l| l.rule == rule);
    exceeded.sort_by(|a, b| order(&a.rule).cmp(&order(&b.rule)).then_with(|| a.subject.cmp(&b.subject)));
    (exceeded, keys.len() as u64)
}

/// The signal a projected drift is judged from ([FR-GV-28], [CR-237] §3.2):
/// this checkout's blessed gate baseline — its signal and snapshot id — when
/// it was scored under the current metric semantics and `thresholds_hash`, as
/// the gate compares one; otherwise why not, and the base commit's signal is
/// the reference.
///
/// [FR-GV-28]: ../../../docs/specs/requirements/FR-GV-28.md
/// [CR-237]: ../../../docs/requests/CR-237-branch-delta-gating-attribution-and-budgets.md
fn drift_reference(engine: &Engine, thresholds_hash: &str) -> Result<std::result::Result<(u32, i64), String>> {
    let baseline = quality_runtime(engine)?.submit_read(|store| store.baseline_snapshot(SCOPE_PROJECT))?;
    Ok(match baseline {
        None => Err("no gate baseline is saved in this checkout".to_string()),
        Some(b) if b.metric_version != crate::metrics::METRIC_SEMANTICS_VERSION => {
            Err("the gate baseline was saved under other metric semantics".to_string())
        }
        Some(b) if b.thresholds_hash.as_deref() != Some(thresholds_hash) => {
            Err("the gate baseline was saved under other thresholds".to_string())
        }
        Some(b) => match b.aggregate_signal.and_then(|s| u32::try_from(s).ok()) {
            Some(signal) => Ok((signal, b.id)),
            None => Err("the gate baseline has no signal: its production graph was empty".to_string()),
        },
    })
}

/// The summed signal drops against a numeric `drift_budget` of `points`
/// ([FR-GV-28]), judged from `reference` ([`drift_reference`]), with the note
/// that names the reference: the drift when the summed drops take it past the
/// budget, else `None`. Why it cannot be projected when a side has no signal.
///
/// [FR-GV-28]: ../../../docs/specs/requirements/FR-GV-28.md
fn project_drift(
    points: f64,
    base: &Measured,
    sides: &[(String, (Measured, Measured))],
    reference: std::result::Result<(u32, i64), String>,
) -> std::result::Result<(Option<ProjectedDrift>, String), String> {
    let signal = |side: &Measured| side.terms.as_ref().map(SignalTerms::signal);
    let empty = |what: &str| format!("{what} has no signal: its production graph is empty");
    let at_base = signal(base).ok_or_else(|| empty("the base commit"))?;
    let mut drops = Vec::new();
    for (name, (before, after)) in sides {
        let (Some(before), Some(after)) = (signal(before), signal(after)) else {
            return Err(empty(&format!("a side of {name}")));
        };
        drops.push((name.clone(), i64::from(before) - i64::from(after)));
    }
    let note = match &reference {
        Ok((_, id)) => format!(
            "drift_budget is projected from this checkout's blessed gate baseline (snapshot {id}), as the gate \
             after the merge would judge it: another checkout's baseline can give another answer"
        ),
        Err(why) => format!("drift_budget is projected from the base commit's signal: {why}"),
    };
    Ok((judge_drift(points, at_base, &drops, reference), note))
}

/// Judge the summed `drops` against `points` from `reference` ([FR-GV-28]):
/// the merge is projected to measure `at_base` less every drop, and its drift
/// is measured from the baseline's signal, or from `at_base` when `reference`
/// says why there is none. Listed only when the drops are a net fall that
/// takes the drift past the budget — a base already past it that no branch
/// lowers is the gate's to report. A share exceeds alone when, with the base's
/// own distance from the reference, it would by itself.
///
/// [FR-GV-28]: ../../../docs/specs/requirements/FR-GV-28.md
fn judge_drift(
    points: f64,
    at_base: u32,
    drops: &[(String, i64)],
    reference: std::result::Result<(u32, i64), String>,
) -> Option<ProjectedDrift> {
    let (from, reference) = match reference {
        Ok((signal, id)) => (
            signal,
            DriftReference {
                kind: DriftReference::BASELINE,
                snapshot_id: Some(id),
                reason: None,
            },
        ),
        Err(why) => (
            at_base,
            DriftReference {
                kind: DriftReference::BASE,
                snapshot_id: None,
                reason: Some(why),
            },
        ),
    };
    let offset = i64::from(from) - i64::from(at_base);
    let shares: Vec<BranchShare> = drops
        .iter()
        .filter(|(_, drop)| *drop != 0)
        .map(|(name, drop)| BranchShare {
            branch: name.clone(),
            share: *drop,
            exceeds_alone: (offset + drop) as f64 > points,
        })
        .collect();
    let total: i64 = shares.iter().map(|s| s.share).sum();
    let current = u32::try_from((i64::from(at_base) - total).clamp(0, i64::from(u32::MAX))).unwrap_or(0);
    let drift = signal_drift(from, current, DriftBudget::Points(points));
    (total > 0 && drift.exceeded).then_some(ProjectedDrift { drift, shares, reference })
}

/// The `measured: false` answer: the stated refs, the reason, nothing classified.
fn not_measured(mut review: BranchReview, why: &str) -> BranchReview {
    review.not_measured_because = Some(why.to_string());
    review
}

/// The repository's default branch: `origin/HEAD`'s target, else `main`, else
/// `master` — whichever resolves first.
fn default_branch(root: &Path) -> Option<String> {
    git_line(root, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"])
        .into_iter()
        .chain(["main".to_string(), "master".to_string()])
        .find(|name| rev_parse(root, name).is_some())
}

/// `rules` with every per-subject budget's offender marks on. Only the budgets
/// whose violations are one per subject whatever their form are switched —
/// switching changes what each violation records (its subject and value), not
/// which violations exist. `max_dead`, `max_duplicates`, `max_brain_methods`
/// and `max_cycles` are left as declared: their absolute forms are one
/// project-wide count, and their ratchet forms a different violation set.
fn marked(rules: &Rules) -> Rules {
    let mut rules = rules.clone();
    let constraints = &mut rules.constraints;
    for budget in [
        &mut constraints.max_cc,
        &mut constraints.max_fn_lines,
        &mut constraints.no_god_files,
        &mut constraints.max_fan_in,
        &mut constraints.max_fan_out,
        &mut constraints.max_nesting_depth,
    ] {
        if let Some(declared) = budget.as_ref() {
            *budget = Some(Budget::Ratchet(RatchetBudget {
                max: declared.max(),
                ratchet: true,
            }));
        }
    }
    if constraints
        .no_god_containers
        .as_ref()
        .is_some_and(GodContainersBudget::enforced)
    {
        constraints.no_god_containers = Some(GodContainersBudget::Ratchet(RatchetFlag { ratchet: true }));
    }
    rules
}

/// The ceiling `rule` holds each subject to, for the per-subject budgets.
fn budget_max(rules: &Rules, rule: &str) -> Option<u64> {
    let constraints = &rules.constraints;
    let budget = match rule {
        "max_cc" => constraints.max_cc.as_ref(),
        "max_fn_lines" => constraints.max_fn_lines.as_ref(),
        "no_god_files" => constraints.no_god_files.as_ref(),
        "max_fan_in" => constraints.max_fan_in.as_ref(),
        "max_fan_out" => constraints.max_fan_out.as_ref(),
        "max_nesting_depth" => constraints.max_nesting_depth.as_ref(),
        _ => None,
    };
    budget.map(|b| u64::from(b.max()))
}

/// Measure the store behind `runtime` — its uncapped offenders and its
/// violations under the marked contract — and the number of rules evaluated.
/// A projection measurement lists no offender, drops each violation's message
/// (it records one violation per measured subject) and records the cycle
/// count. A pure read.
fn measure(runtime: &Runtime, contract: &Contract) -> Result<(Measured, u32)> {
    let facts = StoreFacts::read(runtime)?;
    let view = build_view(Granularity::ExcludeContains, &facts.nodes, &facts.edges);
    let test_ids = facts.test_ids();
    let offenders = if contract.projection {
        Vec::new()
    } else {
        points(&crate::metrics::all_offenders(
            &view,
            &facts.nodes,
            &facts.edges,
            &facts.function_metrics,
            &test_ids,
            contract.thresholds,
            &contract.member_access,
        ))
    };
    let declares_cycles = contract.marked.rules.constraints.max_cycles.is_some();
    let cycles = if declares_cycles { facts.cycle_count(&view) } else { 0 };
    let (violations, checked) = facts.evaluate(&contract.marked, cycles);
    let identity: HashMap<i64, &str> = facts
        .nodes
        .iter()
        .map(|n| (n.id.get(), n.identity()))
        .collect();
    let violations = violations
        .iter()
        .map(|v| breach(v, &identity))
        .map(|mut b| {
            if contract.projection {
                b.message.clear();
            }
            b
        })
        .collect();
    let graph_not_applicable = crate::metrics::graph_dimensions_not_applicable(&view, &facts.nodes, &test_ids);
    let metrics = crate::metrics::compute(
        &view,
        &facts.nodes,
        &facts.edges,
        &facts.function_metrics,
        &test_ids,
        contract.thresholds,
        &contract.member_access,
    );
    Ok((
        Measured {
            offenders,
            violations,
            terms: crate::metrics::signal_terms(&metrics),
            graph_not_applicable,
            cycles: (contract.projection && declares_cycles).then_some(cycles),
        },
        checked,
    ))
}

/// Flatten the nine lists into points, in [`WorstOffenders::DIMENSIONS`] order.
fn points(offenders: &WorstOffenders) -> Vec<Point> {
    offenders
        .lists()
        .into_iter()
        .flat_map(|(dimension, list)| list.iter().map(move |o| point(dimension, o)))
        .collect()
}

fn point(dimension: &str, o: &Offender) -> Point {
    Point {
        dimension: dimension.to_string(),
        node_id: o.node_id.clone(),
        name: o.name.clone(),
        file: o.file.clone(),
        line: o.line,
        detail: o.detail.clone(),
        measure: o.measure.map(str::to_string),
        value: o.value,
        threshold: o.threshold,
    }
}

/// A violation with its subject: the ratchet mark's symbol, else the node's
/// identity, else the file — empty for a project-wide count.
fn breach(v: &Violation, identity: &HashMap<i64, &str>) -> Breach {
    let subject = match (&v.ratchet, v.node_id) {
        (Some(mark), _) => mark.symbol.clone(),
        (None, Some(id)) => identity
            .get(&id)
            .map_or_else(|| v.file.clone(), |s| (*s).to_string()),
        (None, None) => v.file.clone(),
    };
    Breach {
        rule: v.rule.clone(),
        rule_type: v.rule_type.clone(),
        subject,
        file: v.file.clone(),
        value: v.ratchet.as_ref().map(|mark| mark.value),
        message: v.message.clone(),
    }
}

/// The before-state from the snapshot recorded at `merge_base`, when one holds
/// everything the review compares (see the module docs); otherwise `None`.
fn recorded_before(
    runtime: &Runtime,
    merge_base: &str,
    contract: &Contract,
    checked_rules: u32,
) -> Result<Option<Measured>> {
    if checked_rules > 0 {
        return Ok(None);
    }
    let hash = contract.thresholds.hash();
    let snapshot = runtime
        .submit_read(|store| store.metric_snapshots())?
        .into_iter()
        .filter(|row| {
            row.commit_sha.as_deref() == Some(merge_base)
                && row.metric_version == crate::metrics::METRIC_SEMANTICS_VERSION
                && row.thresholds_hash.as_deref() == Some(hash.as_str())
        })
        .max_by_key(|row| row.id);
    let Some(snapshot) = snapshot else {
        return Ok(None);
    };
    let graph_not_applicable =
        snapshot.acyclicity_applicable == Some(false) || snapshot.depth_applicable == Some(false);
    let (offenders, row) = runtime.submit_read(move |store| {
        Ok((store.uncapped_offenders_of(snapshot.id)?, store.metric_snapshot(snapshot.id)?))
    })?;
    let (Some(offenders), Some(row)) = (offenders.filter(|o| o.values_recorded), row) else {
        return Ok(None);
    };
    Ok(Some(Measured {
        offenders: points(&offenders),
        violations: Vec::new(),
        terms: crate::metrics::signal_terms(&metric_snapshot_from_row(row)),
        graph_not_applicable,
        cycles: None,
    }))
}

/// The before-state from the cache, or from a shadow index of the merge-base
/// tree that is then cached.
fn shadow_before(
    engine: &Engine,
    merge_base: &str,
    contract: &Contract,
) -> Result<(Measured, BeforeStateCache)> {
    let (_, _, config) = engine.pipeline_ctx()?;
    let key = cache_key(contract, &config);
    let dir = engine.root().join(CACHE_DIR_REL);
    let path = cache_path(&dir, merge_base, contract.projection);
    if let Some(before) = read_cache(&path, merge_base, &key) {
        return Ok((before, BeforeStateCache::Reused));
    }
    let before = measure_tree(engine, merge_base, contract)?;
    let entry = CacheEntry {
        format: CACHE_FORMAT,
        commit: merge_base.to_string(),
        key,
        before,
    };
    // The cache is derived state (ADR-82): a before-state that was measured
    // but cannot be kept still answers, and the next run measures again. A
    // projection prunes once its run is over ([`prune_projection_cache`]).
    match write_cache(&dir, &path, &entry) {
        Ok(()) if !contract.projection => prune_cache(&dir, std::slice::from_ref(&path), false),
        Ok(()) => {}
        Err(err) => tracing::warn!("review: the merge-base before-state was measured but not cached: {err:#}"),
    }
    Ok((entry.before, BeforeStateCache::Built))
}

/// Where `commit`'s cached measurement lives under `dir`: a projection's entry
/// sits beside the review's entry for the same commit, under its own name.
fn cache_path(dir: &Path, commit: &str, projection: bool) -> PathBuf {
    let suffix = if projection { ".projection" } else { "" };
    dir.join(format!("{commit}{suffix}.json"))
}

/// Prune the projection entries once a projection run is over, never one of
/// `commits` — every side it measured, which may be more than [`CACHE_KEEP`].
fn prune_projection_cache(engine: &Engine, commits: &BTreeSet<String>) {
    let dir = engine.root().join(CACHE_DIR_REL);
    let keep: Vec<PathBuf> = commits.iter().map(|c| cache_path(&dir, c, true)).collect();
    prune_cache(&dir, &keep, true);
}

/// Write `entry` to `path` under `dir`, atomically.
fn write_cache(dir: &Path, path: &Path, entry: &CacheEntry) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating the review cache {}", dir.display()))?;
    let bytes = serde_json::to_vec(entry).context("serialising the review cache entry")?;
    crate::config::atomic::publish(path, &bytes, None)
        .with_context(|| format!("writing the review cache entry {}", path.display()))
}

/// Everything a cached before-state was measured under, hashed: a different
/// contract, threshold set, admission, semantics or logos build re-measures,
/// and a projection measurement never reads a review's entry or the reverse.
fn cache_key(contract: &Contract, config: &crate::config::Config) -> String {
    let semantics = serde_json::to_string(&config.semantics).unwrap_or_default();
    let canonical = format!(
        "format={CACHE_FORMAT};metrics={};thresholds={};rules={};admission={};semantics={semantics};logos={}{}",
        crate::metrics::METRIC_SEMANTICS_VERSION,
        contract.thresholds.hash(),
        contract.declared.hash,
        config.admission_fingerprint(),
        env!("CARGO_PKG_VERSION"),
        if contract.projection { ";projection" } else { "" },
    );
    blake3::hash(canonical.as_bytes()).to_hex().to_string()
}

/// The cached before-state at `path`, when it is for `commit` under `key`.
/// An unreadable or mismatched entry reads as absent and is rebuilt.
fn read_cache(path: &Path, commit: &str, key: &str) -> Option<Measured> {
    let bytes = std::fs::read(path).ok()?;
    let entry: CacheEntry = serde_json::from_slice(&bytes).ok()?;
    (entry.format == CACHE_FORMAT && entry.commit == commit && entry.key == key)
        .then_some(entry.before)
}

/// Keep the newest [`CACHE_KEEP`] entries of one kind in `dir` — projection
/// entries when `projection`, a review's otherwise — counting and never
/// removing `keep`. Best-effort: a cache that cannot be pruned still serves.
fn prune_cache(dir: &Path, keep: &[PathBuf], projection: bool) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let of_kind = |p: &Path| {
        p.extension().is_some_and(|ext| ext == "json")
            && p.to_string_lossy().ends_with(".projection.json") == projection
    };
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = read
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| of_kind(p) && !keep.contains(p))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    // Newest first, then by name, so ties prune deterministically.
    entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, stale) in entries.into_iter().skip(CACHE_KEEP.saturating_sub(keep.len())) {
        let _ = std::fs::remove_file(stale);
    }
}

/// Shadow-index the merge-base tree and measure it under the contract.
fn measure_tree(engine: &Engine, merge_base: &str, contract: &Contract) -> Result<Measured> {
    let (_, registry, config) = engine.pipeline_ctx()?;
    let tree = TreeExport::create(engine.root(), merge_base)?;
    let shadow = crate::pipeline::ShadowStore::create()?;
    let shadow_rt = crate::pipeline::shadow_reindex(registry, tree.path(), &config, &shadow)?;
    // The index annotated the export under the tree's own `rules.toml`, which
    // an untracked contract is not in: re-materialise under the live one.
    rematerialise(&shadow_rt, registry, &config, &contract.declared.rules)?;
    let (measured, _) = measure(&shadow_rt, contract)?;
    // The runtime releases the db before the guard removes it.
    drop(shadow_rt);
    Ok(measured)
}

/// Monotonic counter keeping each [`TreeExport`] directory unique in-process.
static EXPORT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A commit's tree written out under the OS temp dir from the object database
/// alone — a private temp index (`GIT_INDEX_FILE`), so the repository's own
/// index, worktree and refs are never touched — and removed on drop. Large
/// file filters are not run (`GIT_LFS_SKIP_SMUDGE`), so the export never
/// reaches a remote.
struct TreeExport {
    dir: PathBuf,
    tree: PathBuf,
}

impl TreeExport {
    fn create(root: &Path, commit: &str) -> Result<Self> {
        let dir = private_dir()?;
        // Owned from here, so the guard removes it on every error path below.
        let export = Self { tree: dir.join("tree"), dir };
        std::fs::create_dir(&export.tree)
            .with_context(|| format!("creating the merge-base export {}", export.tree.display()))?;
        let index = export.dir.join("index");
        let prefix = format!("--prefix={}/", export.tree.display());
        for args in [
            vec!["read-tree", commit],
            vec!["checkout-index", "--all", "--force", prefix.as_str()],
        ] {
            let out = git_command(root)
                .env("GIT_INDEX_FILE", &index)
                .env("GIT_LFS_SKIP_SMUDGE", "1")
                .args(&args)
                .output()
                .with_context(|| format!("running git {}", args[0]))?;
            anyhow::ensure!(
                out.status.success(),
                "exporting the merge-base tree {commit}: git {} failed: {}",
                args[0],
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(export)
    }

    fn path(&self) -> &Path {
        &self.tree
    }
}

/// A fresh directory under the OS temp dir that this process alone created:
/// `mkdir` is exclusive (it fails on any existing entry, a planted symlink
/// included), the name carries the clock's nanoseconds beside the pid and a
/// counter, and on Unix the mode is `0700`. A predictable path that is removed
/// and re-created could be swapped for a symlink in between by another user of
/// a shared temp dir, and `checkout-index --force` would write through it.
fn private_dir() -> Result<PathBuf> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    for _ in 0..16 {
        let seq = EXPORT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let dir = std::env::temp_dir().join(format!(
            "logos-review-{}-{seq}-{nanos:08x}",
            std::process::id()
        ));
        match builder.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(e).with_context(|| format!("creating the merge-base export {}", dir.display()))
            }
        }
    }
    anyhow::bail!("could not create a fresh merge-base export directory under the temp dir")
}

impl Drop for TreeExport {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The trust label of every file the review names ([FR-QM-18]). The default
/// reads every file `resolved`: a gate attribution names no trust.
///
/// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
#[derive(Default)]
struct Trust {
    /// Indexed files whose disk content no longer matches the graph.
    stale: BTreeSet<String>,
    /// Each named file's label.
    by_file: HashMap<String, String>,
}

impl Trust {
    fn read(engine: &Engine, runtime: &Runtime, before: &Measured, after: &Measured) -> Result<Self> {
        let named: BTreeSet<String> = [before, after]
            .into_iter()
            .flat_map(|m| {
                m.offenders
                    .iter()
                    .map(|p| p.file.clone())
                    .chain(m.violations.iter().map(|b| b.file.clone()))
            })
            .filter(|f| !f.is_empty())
            .collect();
        let indexed = runtime.submit_read(|store| store.indexed_files())?;
        let stale = stale_files(engine.root(), &indexed);
        let by_file = Self::labels(engine, runtime, named, &stale)?;
        Ok(Self { stale, by_file })
    }

    /// Each of `named`'s labels: `stale` when it is in `stale`, else the tag
    /// of a language the graph binds within files only, else `resolved`.
    fn labels(
        engine: &Engine,
        runtime: &Runtime,
        named: BTreeSet<String>,
        stale: &BTreeSet<String>,
    ) -> Result<HashMap<String, String>> {
        let paths: Vec<String> = named.iter().cloned().collect();
        let (thin, languages) =
            runtime.submit_read(move |store| Ok((ThinGraph::read(store)?, store.file_languages(&paths)?)))?;
        Ok(named
            .into_iter()
            .map(|file| {
                let label = if stale.contains(&file) {
                    BranchReview::TRUST_STALE.to_string()
                } else {
                    let language = languages
                        .get(&file)
                        .cloned()
                        .flatten()
                        .or_else(|| plugin_language(engine, &file));
                    language
                        .and_then(|l| thin.same_file_only(&l))
                        .map_or_else(|| BranchReview::TRUST_RESOLVED.to_string(), absence_tag)
                };
                (file, label)
            })
            .collect())
    }

    /// The label of `file`; an entry with no file (a Depth chain, a
    /// project-wide count) reads `stale` while any indexed file is.
    fn of(&self, file: &str) -> String {
        match self.by_file.get(file) {
            Some(label) => label.clone(),
            None if self.stale.is_empty() => BranchReview::TRUST_RESOLVED.to_string(),
            None => BranchReview::TRUST_STALE.to_string(),
        }
    }
}

/// The language a plugin claims `file` by — for a file the index no longer
/// holds (a resolved entry's deleted file).
fn plugin_language(engine: &Engine, file: &str) -> Option<String> {
    engine
        .registry()
        .and_then(|r| r.for_path(file))
        .map(|p| p.name().to_string())
}

/// The indexed files whose content on disk differs from the content hash the
/// graph recorded, or that are gone — the blake3 the pipeline's dirty
/// detection compares ([FR-SY-03]).
///
/// [FR-SY-03]: ../../../docs/specs/requirements/FR-SY-03.md
fn stale_files(root: &Path, indexed: &[crate::graph_store::FileRecord]) -> BTreeSet<String> {
    indexed
        .iter()
        .filter(|f| {
            let Some(recorded) = f.content_hash.as_deref() else {
                return false;
            };
            std::fs::read(root.join(&f.path))
                .map_or(true, |bytes| blake3::hash(&bytes).to_hex().as_str() != recorded)
        })
        .map(|f| f.path.clone())
        .collect()
}

/// The identity an offender is matched by within its dimension and measure.
fn offender_key(p: &Point) -> String {
    match p.dimension.as_str() {
        // A chain names directories, not a symbol: its head is its name.
        "depth" => p.name.clone(),
        // A cycle is named by its lowest-storage-id member, which a re-index
        // can change; the directories it spans are what stays.
        "acyclicity" => cycle_directories(&p.detail).to_string(),
        _ => p
            .node_id
            .clone()
            .unwrap_or_else(|| format!("{}:{}", p.file, p.name)),
    }
}

/// The directory list of an Acyclicity detail (`"N symbols across M
/// directories: a, b"`), without the size that changes as the cycle grows.
fn cycle_directories(detail: &str) -> &str {
    detail.split_once(" across ").map_or(detail, |(_, dirs)| dirs)
}

/// The status of an entry present before (`before`) and/or after (`after`).
fn status<T: Ord>(before: Option<Option<T>>, after: Option<Option<T>>) -> ReviewStatus {
    match (before, after) {
        (None, _) => ReviewStatus::New,
        (Some(_), None) => ReviewStatus::Resolved,
        (Some(Some(b)), Some(Some(a))) if a > b => ReviewStatus::Worsened,
        (Some(Some(b)), Some(Some(a))) if a < b => ReviewStatus::Improved,
        _ => ReviewStatus::Unchanged,
    }
}

/// Pair the entries of the two sides by `key`. Entries sharing a key on one
/// side — two cycles across the same directories, say — are paired in their
/// computed order rather than overwriting one another, and the surplus on
/// either side is one entry new or resolved each.
fn join<'a, T, K: Ord + Clone>(
    before: &'a [T],
    after: &'a [T],
    key: impl Fn(&T) -> K,
) -> Vec<(K, Option<&'a T>, Option<&'a T>)> {
    let mut sides: BTreeMap<K, (Vec<&'a T>, Vec<&'a T>)> = BTreeMap::new();
    for b in before {
        sides.entry(key(b)).or_default().0.push(b);
    }
    for a in after {
        sides.entry(key(a)).or_default().1.push(a);
    }
    sides
        .into_iter()
        .flat_map(|(k, (b, a))| {
            (0..b.len().max(a.len())).map(move |i| (k.clone(), b.get(i).copied(), a.get(i).copied()))
        })
        .collect()
}

/// Classify every offender of either side; see the module docs for matching.
fn classify_offenders(before: &[Point], after: &[Point], trust: &Trust) -> Vec<ReviewedOffender> {
    type Key = (usize, Option<String>, String);
    let key = |p: &Point| -> Key {
        let rank = WorstOffenders::DIMENSIONS
            .iter()
            .position(|d| *d == p.dimension)
            .unwrap_or(usize::MAX);
        (rank, p.measure.clone(), offender_key(p))
    };
    let mut entries: Vec<(usize, ReviewedOffender)> = join(before, after, key)
        .into_iter()
        .filter_map(|((rank, _, _), b, a)| {
            let current = a.or(b)?;
            Some((
                rank,
                ReviewedOffender {
                    dimension: current.dimension.clone(),
                    status: status(b.map(|p| p.value), a.map(|p| p.value)),
                    node_id: current.node_id.clone(),
                    name: current.name.clone(),
                    file: current.file.clone(),
                    line: current.line,
                    measure: current.measure.clone(),
                    before: b.and_then(|p| p.value),
                    after: a.and_then(|p| p.value),
                    threshold: current.threshold,
                    detail: current.detail.clone(),
                    trust: trust.of(&current.file),
                    gating: false,
                },
            ))
        })
        .collect();
    entries.sort_by(|(ra, a), (rb, b)| {
        ra.cmp(rb)
            .then(a.status.cmp(&b.status))
            .then_with(|| a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.measure.cmp(&b.measure))
            .then_with(|| a.detail.cmp(&b.detail))
    });
    entries.into_iter().map(|(_, e)| e).collect()
}

/// Classify every violation of either side: by rule and subject, and by
/// message too where the rule records no value.
fn classify_violations(
    before: &[Breach],
    after: &[Breach],
    contract: &Contract,
    trust: &Trust,
) -> Vec<ReviewedViolation> {
    type Key = (String, String, String);
    let key = |b: &Breach| -> Key {
        let message = if b.value.is_some() { String::new() } else { b.message.clone() };
        (b.rule.clone(), b.subject.clone(), message)
    };
    let mut entries: Vec<ReviewedViolation> = join(before, after, key)
        .into_iter()
        .filter_map(|(_, b, a)| {
            let current = a.or(b)?;
            Some(ReviewedViolation {
                rule: current.rule.clone(),
                rule_type: current.rule_type.clone(),
                status: status(b.map(|v| v.value), a.map(|v| v.value)),
                subject: current.subject.clone(),
                file: current.file.clone(),
                before: b.and_then(|v| v.value),
                after: a.and_then(|v| v.value),
                threshold: budget_max(&contract.declared.rules, &current.rule),
                message: current.message.clone(),
                trust: trust.of(&current.file),
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        a.rule
            .cmp(&b.rule)
            .then(a.status.cmp(&b.status))
            .then_with(|| a.subject.cmp(&b.subject))
            .then_with(|| a.message.cmp(&b.message))
    });
    entries
}

/// The near-clone groups of one side: Uniqueness rows come one group at a
/// time, each member carrying the group's size, so a group is the run of
/// consecutive rows that share a size and a detail, cut at that size.
fn clone_groups(points: &[Point]) -> Vec<Vec<&Point>> {
    let mut groups: Vec<Vec<&Point>> = Vec::new();
    for p in points.iter().filter(|p| p.dimension == "uniqueness") {
        let size = usize::try_from(p.value.unwrap_or(0)).unwrap_or(0);
        match groups.last_mut() {
            Some(group)
                if group.len() < size
                    && group[0].value == p.value
                    && group[0].detail == p.detail =>
            {
                group.push(p);
            }
            _ => groups.push(vec![p]),
        }
    }
    groups
}

/// The near-clone groups of `side` whose member set no group of `other` has.
fn groups_absent_from<'a>(side: &'a [Point], other: &[Point]) -> Vec<Vec<&'a Point>> {
    let existing: HashSet<BTreeSet<String>> = clone_groups(other)
        .iter()
        .map(|g| g.iter().map(|p| offender_key(p)).collect())
        .collect();
    clone_groups(side)
        .into_iter()
        .filter(|g| !existing.contains(&g.iter().map(|p| offender_key(p)).collect::<BTreeSet<_>>()))
        .collect()
}

/// [`groups_absent_from`] as the attribution names them: each group's members
/// by file, line and name.
fn group_members(side: &[Point], other: &[Point]) -> Vec<Vec<DriverMember>> {
    groups_absent_from(side, other)
        .into_iter()
        .map(|g| {
            let mut members: Vec<DriverMember> = g
                .iter()
                .map(|p| DriverMember {
                    node_id: p.node_id.clone(),
                    name: p.name.clone(),
                    file: p.file.clone(),
                    line: p.line,
                })
                .collect();
            members.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)).then_with(|| a.name.cmp(&b.name)));
            members
        })
        .collect()
}

/// The attribution of the change between two recorded measurements — the
/// gate's, from the baseline snapshot to the current one ([FR-GV-26]). The
/// drivers are the two snapshots' uncapped offenders, classified as the review
/// classifies its own; a side whose offender values were never recorded names
/// none, and says why.
///
/// [FR-GV-26]: ../../../docs/specs/requirements/FR-GV-26.md
pub(super) fn snapshot_attribution(
    before: &MetricSnapshot,
    after: &MetricSnapshot,
    offenders: Option<(WorstOffenders, WorstOffenders)>,
) -> SignalAttribution {
    let recorded = offenders.filter(|(b, a)| b.values_recorded && a.values_recorded);
    let (before_points, after_points) = recorded
        .as_ref()
        .map(|(b, a)| (points(b), points(a)))
        .unwrap_or_default();
    let trust = Trust::default();
    let mut classified = classify_offenders(&before_points, &after_points, &trust);
    classified.retain(|e| e.status != ReviewStatus::Unchanged);
    let mut attribution = attribute(
        crate::metrics::signal_terms(before).as_ref(),
        crate::metrics::signal_terms(after).as_ref(),
        &Drivers {
            offenders: &classified,
            introduced_groups: group_members(&after_points, &before_points),
            removed_groups: group_members(&before_points, &after_points),
        },
    );
    if recorded.is_none() && attribution.not_attributable_because.is_none() {
        attribution.drivers_not_named_because = Some(
            "a snapshot predates recorded offender values (FR-QM-19), so there is nothing to \
             classify the drivers against"
                .to_string(),
        );
    }
    attribution
}

/// The groups after whose member set no group before has.
fn introduced_clone_groups(before: &[Point], after: &[Point], trust: &Trust) -> Vec<IntroducedCloneGroup> {
    let duplicated_before: HashSet<String> = clone_groups(before)
        .iter()
        .flatten()
        .map(|p| offender_key(p))
        .collect();
    let mut introduced: Vec<IntroducedCloneGroup> = groups_absent_from(after, before)
        .into_iter()
        .map(|g| {
            let mut members: Vec<CloneGroupMember> = g
                .iter()
                .map(|p| CloneGroupMember {
                    node_id: p.node_id.clone(),
                    name: p.name.clone(),
                    file: p.file.clone(),
                    line: p.line,
                    newly_duplicated: !duplicated_before.contains(&offender_key(p)),
                    trust: trust.of(&p.file),
                })
                .collect();
            members.sort_by(|a, b| {
                a.file
                    .cmp(&b.file)
                    .then(a.line.cmp(&b.line))
                    .then_with(|| a.name.cmp(&b.name))
            });
            IntroducedCloneGroup {
                size: members.len() as u64,
                members,
            }
        })
        .collect();
    introduced.sort_by(|a, b| {
        let first = |g: &IntroducedCloneGroup| g.members.first().map(|m| (m.file.clone(), m.line));
        first(a).cmp(&first(b))
    });
    introduced
}

/// The security section: the evidence `read` returns for `merge_base`,
/// classified ([`security_delta`]). A read that fails, a diff that cannot be
/// read, or a project with no stored report answers `measured: false` with the
/// reason — the quality half of the review stands either way.
fn security_section(
    root: &Path,
    head: &str,
    merge_base: &str,
    read: impl FnOnce(&str) -> Result<SecurityEvidence>,
) -> SecurityDelta {
    let not_measured = |why: String| SecurityDelta {
        not_measured_because: Some(why),
        ..SecurityDelta::default()
    };
    let evidence = match read(merge_base) {
        Ok(evidence) => evidence,
        Err(err) => return not_measured(format!("the security findings could not be read ({err:#})")),
    };
    if evidence.current.scanned.is_empty() {
        return not_measured(
            evidence
                .current
                .notice
                .unwrap_or("no security report is stored")
                .to_string(),
        );
    }
    let hunks = match changed_hunks(root, merge_base) {
        Ok(hunks) => hunks,
        Err(err) => return not_measured(format!("the branch's changed hunks could not be read ({err:#})")),
    };
    let mut delta = security_delta(&evidence, root, &hunks, |path| modified_secs(&root.join(path)));
    if head == merge_base && !evidence.base.snapshots.is_empty() {
        // One report is kept per commit and scanner, so a scan on a branch
        // with no commit of its own replaces the merge-base record.
        delta.notes.push(
            "HEAD is the merge-base: a scan run now replaces the report recorded there, so the \
             fingerprint comparison is only as old as the last scan; commit before re-scanning"
                .to_string(),
        );
    }
    delta
}

/// A file's last modification, in whole Unix seconds; `None` when it is gone
/// or its time cannot be read.
fn modified_secs(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let secs = modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    i64::try_from(secs).ok()
}

/// Classify the evidence against the branch's `hunks` (see the module docs).
/// `modified` answers a file's modification time in Unix seconds, `None` when
/// the file is gone; `root` makes an absolute report path project-relative for
/// the ignore file's path entries. Pure apart from `modified`.
fn security_delta(
    evidence: &SecurityEvidence,
    root: &Path,
    hunks: &FileRanges,
    modified: impl Fn(&str) -> Option<i64>,
) -> SecurityDelta {
    let current = &evidence.current;
    // A merge-base report that `--path` re-scans rewrote since its full scan
    // (one per commit and scanner, so a scoped run at HEAD = merge-base, as the
    // Stop hook's, replaces it) holds what the working tree said then inside its
    // scope, not the commit: it is no before-state, and that scanner is
    // classified by hunk, as one never scanned there.
    let rewritten: HashSet<&str> = evidence
        .base
        .snapshots
        .iter()
        .filter(|s| !s.scope.is_empty())
        .map(|s| s.scanner_name.as_str())
        .collect();
    let recorded = |f: &SecurityFinding| !rewritten.contains(f.scanner_name.as_str());
    // A scanner is compared by fingerprint exactly when its report was
    // recorded at the merge-base.
    let mut before: HashMap<&str, HashSet<&str>> = evidence
        .base
        .snapshots
        .iter()
        .filter(|s| s.scope.is_empty())
        .map(|s| (s.scanner_name.as_str(), HashSet::new()))
        .collect();
    for f in evidence.base.findings.iter().filter(|f| recorded(f)) {
        before.entry(f.scanner_name.as_str()).or_default().insert(f.fingerprint.as_str());
    }
    let method = |scanner: &str| {
        if before.contains_key(scanner) {
            SecurityDelta::METHOD_FINGERPRINT
        } else {
            SecurityDelta::METHOD_HUNK
        }
    };
    // The entry that suppresses a finding, current or resolved: the first
    // unexpired one naming it, as the labeller picks.
    let unexpired = current.suppressions.iter().filter(|e| !e.expired).map(|e| &e.entry);
    let matcher = crate::history::security::EntryMatcher::new(unexpired, root);
    let suppressing = |f: &SecurityFinding| matcher.naming(f).next().map(|(_, entry)| entry);

    let mut delta = SecurityDelta {
        measured: true,
        scanners: current
            .scanned
            .iter()
            .map(|r| SecurityDeltaScanner {
                scanner: r.driver.clone(),
                method: method(&r.driver),
                latest_head: r.head_sha.clone(),
            })
            .collect(),
        ..SecurityDelta::default()
    };
    let mut entries: Vec<(SecurityDeltaEntry, Option<&crate::history::security::IgnoreEntry>)> = Vec::new();

    let mut now: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut freshness_now: HashMap<(&str, &str), &str> = HashMap::new();
    for f in current.findings.iter().map(|listed| &listed.finding) {
        now.entry(f.scanner_name.as_str()).or_default().insert(f.fingerprint.as_str());
        if let Some(path) = f.path.as_deref() {
            freshness_now.insert((f.scanner_name.as_str(), path), f.freshness);
        }
    }
    // A report with no stable fingerprint keys a finding by its region, so a
    // line added above it moves its fingerprint. The merge-base findings the
    // latest report no longer carries, by scanner, rule and file, in store
    // order: a finding outside every hunk whose fingerprint is new is paired
    // with one of them — the same finding, moved — rather than read as one
    // introduced and another resolved.
    let mut vanished: HashMap<(&str, &str, Option<&str>), std::collections::VecDeque<&str>> = HashMap::new();
    for f in evidence.base.findings.iter().filter(|f| recorded(f)) {
        if !now.get(f.scanner_name.as_str()).is_some_and(|fps| fps.contains(f.fingerprint.as_str())) {
            vanished
                .entry((f.scanner_name.as_str(), f.rule_id.as_str(), f.path.as_deref()))
                .or_default()
                .push_back(f.fingerprint.as_str());
        }
    }
    let mut moved: HashSet<(&str, &str)> = HashSet::new();
    for listed in &current.findings {
        let f = &listed.finding;
        let method = method(&f.scanner_name);
        let changed = in_hunk(hunks, f);
        // A file the branch deleted is a whole-file hunk, and deleting a file
        // introduces nothing.
        let gone = f.location().is_some_and(|p| modified(&p).is_none());
        let status = match before.get(f.scanner_name.as_str()) {
            Some(fingerprints) if !fingerprints.contains(f.fingerprint.as_str()) => SecurityDeltaStatus::Introduced,
            Some(_) if changed => SecurityDeltaStatus::Touched,
            None if changed && !gone => SecurityDeltaStatus::Introduced,
            _ => SecurityDeltaStatus::PreExisting,
        };
        let status = match status {
            SecurityDeltaStatus::Introduced if method == SecurityDelta::METHOD_FINGERPRINT && !changed => {
                let twin = vanished
                    .get_mut(&(f.scanner_name.as_str(), f.rule_id.as_str(), f.path.as_deref()))
                    .and_then(std::collections::VecDeque::pop_front);
                match twin {
                    Some(fingerprint) => {
                        moved.insert((f.scanner_name.as_str(), fingerprint));
                        SecurityDeltaStatus::PreExisting
                    }
                    None => status,
                }
            }
            status => status,
        };
        let entry = delta_entry(status, method, f, &listed.owasp, &listed.context, f.freshness);
        entries.push((entry, suppressing(f)));
    }

    // A merge-base fingerprint the latest report no longer carries.
    let ingested: HashMap<&str, i64> = evidence
        .latest
        .iter()
        .map(|s| (s.scanner_name.as_str(), s.created_at))
        .collect();
    for ((f, owasp), context) in evidence
        .base
        .findings
        .iter()
        .zip(&evidence.base_owasp)
        .zip(&evidence.base_contexts)
    {
        let scanner = f.scanner_name.as_str();
        if !recorded(f)
            || now.get(scanner).is_some_and(|fps| fps.contains(f.fingerprint.as_str()))
            || moved.contains(&(scanner, f.fingerprint.as_str()))
        {
            continue;
        }
        let freshness = match f.path.as_deref() {
            // No file to have moved: the re-scan simply no longer reports it.
            None => FRESHNESS_NA,
            Some(path) => match freshness_now.get(&(scanner, path)) {
                // The latest report still anchors the file.
                Some(freshness) => freshness,
                None => match (modified(path), ingested.get(scanner)) {
                    // Gone: no scan can report it.
                    (None, _) => FRESHNESS_NA,
                    // Strictly before, so an edit in the ingest's own second
                    // is never taken as scanned.
                    (Some(at), Some(&ingested)) if at < ingested => FRESHNESS_FRESH,
                    _ => FRESHNESS_STALE,
                },
            },
        };
        let status = if freshness == FRESHNESS_STALE {
            SecurityDeltaStatus::Stale
        } else {
            SecurityDeltaStatus::Resolved
        };
        let entry = delta_entry(status, SecurityDelta::METHOD_FINGERPRINT, f, owasp, context, freshness);
        entries.push((entry, suppressing(f)));
    }

    entries.sort_by(|(a, _), (b, _)| {
        a.status
            .cmp(&b.status)
            .then_with(|| a.path.cmp(&b.path))
            .then(a.start_line.cmp(&b.start_line))
            .then_with(|| a.scanner.cmp(&b.scanner))
            .then_with(|| a.rule_id.cmp(&b.rule_id))
            .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });
    for (entry, ignore) in entries {
        match ignore {
            Some(ignore) => {
                delta.counts.suppressed += 1;
                delta.suppressed.push(SuppressedDeltaEntry {
                    entry,
                    reason: ignore.reason.clone(),
                    expires: ignore.expires.map(|d| d.to_string()),
                });
            }
            None => {
                delta.counts.add(entry.status);
                delta.findings.push(entry);
            }
        }
    }
    let not_rescanned: Vec<&str> = evidence
        .base
        .snapshots
        .iter()
        .filter(|b| evidence.latest.iter().any(|l| l.snapshot_id == b.snapshot_id))
        .map(|b| b.scanner_name.as_str())
        .collect();
    if !not_rescanned.is_empty() {
        delta.notes.push(format!(
            "{} has not re-scanned since the merge-base: a finding in a file edited since reads stale, \
             and nothing it found can be resolved until it does",
            not_rescanned.join(", ")
        ));
    }
    if !rewritten.is_empty() {
        let mut names: Vec<&str> = rewritten.iter().copied().collect();
        names.sort_unstable();
        delta.notes.push(format!(
            "{}: the merge-base report was rewritten by a --path re-scan at that commit, so it holds \
             the working tree of that run, not the commit; its findings are classified by the \
             branch's changed hunks",
            names.join(", ")
        ));
    }
    if !moved.is_empty() {
        delta.notes.push(format!(
            "{} finding(s) outside every changed hunk carry a fingerprint that moved with their line \
             (a report with no stable fingerprint); each is matched to its merge-base finding by \
             scanner, rule and file and reads pre-existing",
            moved.len()
        ));
    }
    delta.notes.push(
        "a resolved finding is one the latest re-scan no longer reports on a file it saw as it \
         stands; on a file edited since, it reads stale instead"
            .to_string(),
    );
    delta
}

/// Whether a finding lies inside a hunk the branch changed: its line range
/// overlaps one, or — with no line to place it — its whole file is new or gone.
/// The file is the finding's [location](SecurityFinding::location), so a
/// finding in a file the index did not hold when it was ingested (a new file
/// not yet synced: `unmatched`) is placed by the path its report named.
fn in_hunk(hunks: &FileRanges, f: &SecurityFinding) -> bool {
    let Some(ranges) = f.location().and_then(|p| hunks.get(&p)) else {
        return false;
    };
    let line = |n: i64| u32::try_from(n.max(1)).unwrap_or(u32::MAX);
    match f.start_line {
        Some(start) => {
            let (lo, hi) = (line(start), line(f.end_line.unwrap_or(start).max(start)));
            ranges.iter().any(|&(a, b)| a <= hi && b >= lo)
        }
        None => ranges.contains(&WHOLE_FILE),
    }
}

/// One delta entry. Every string is the stored one: a secret-class finding's
/// message is already the redacted text.
fn delta_entry(
    status: SecurityDeltaStatus,
    method: &'static str,
    f: &SecurityFinding,
    owasp: &[&'static str],
    context: &FindingContext,
    freshness: &str,
) -> SecurityDeltaEntry {
    SecurityDeltaEntry {
        status,
        method,
        scanner: f.scanner_name.clone(),
        fingerprint: f.fingerprint.clone(),
        rule_id: f.rule_id.clone(),
        severity: f.severity,
        severity_reason: f.severity.reason(),
        owasp: owasp.iter().map(|c| (*c).to_string()).collect(),
        message: f.message.clone(),
        redacted: f.redacted,
        path: f.path.clone(),
        start_line: f.start_line,
        end_line: f.end_line,
        freshness: freshness.to_string(),
        reachability: wire(&context.reachability.verdict),
        confidence: wire(&context.reachability.confidence),
    }
}

/// A unit enum's wire token, read from its own serialisation rather than spelled.
fn wire(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::quality::ReviewCounts;

    fn point(dimension: &str, id: &str, value: i64, detail: &str) -> Point {
        Point {
            dimension: dimension.to_string(),
            node_id: Some(id.to_string()),
            name: id.to_string(),
            file: format!("src/{id}.rs"),
            line: Some(1),
            detail: detail.to_string(),
            measure: Some("nesting_depth".to_string()),
            value: Some(value),
            threshold: Some(4),
        }
    }

    fn no_trust() -> Trust {
        Trust {
            stale: BTreeSet::new(),
            by_file: HashMap::new(),
        }
    }

    #[test]
    fn each_class_follows_presence_and_value() {
        let before = [
            point("nesting", "worse", 5, ""),
            point("nesting", "better", 7, ""),
            point("nesting", "gone", 6, ""),
            point("nesting", "same", 5, ""),
        ];
        let after = [
            point("nesting", "worse", 6, ""),
            point("nesting", "better", 5, ""),
            point("nesting", "same", 5, ""),
            point("nesting", "fresh", 6, ""),
        ];
        let got: BTreeMap<String, (ReviewStatus, Option<i64>, Option<i64>)> =
            classify_offenders(&before, &after, &no_trust())
                .into_iter()
                .map(|e| (e.name, (e.status, e.before, e.after)))
                .collect();
        assert_eq!(got["worse"], (ReviewStatus::Worsened, Some(5), Some(6)));
        assert_eq!(got["better"], (ReviewStatus::Improved, Some(7), Some(5)));
        assert_eq!(got["gone"], (ReviewStatus::Resolved, Some(6), None));
        assert_eq!(got["same"], (ReviewStatus::Unchanged, Some(5), Some(5)));
        assert_eq!(got["fresh"], (ReviewStatus::New, None, Some(6)));
    }

    #[test]
    fn each_class_is_counted_in_its_own_slot() {
        let mut counts = ReviewCounts::default();
        for (status, times) in [
            (ReviewStatus::New, 1),
            (ReviewStatus::Worsened, 2),
            (ReviewStatus::Improved, 3),
            (ReviewStatus::Resolved, 4),
            (ReviewStatus::Unchanged, 5),
        ] {
            (0..times).for_each(|_| counts.add(status));
        }
        assert_eq!(
            counts,
            ReviewCounts { new: 1, worsened: 2, improved: 3, resolved: 4, unchanged: 5 }
        );
    }

    #[test]
    fn entries_of_one_class_are_ordered_by_file_line_and_subject() {
        let mut later = point("nesting", "later", 5, "");
        later.file = "src/b.rs".to_string();
        let mut sooner = point("nesting", "sooner", 5, "");
        sooner.file = "src/a.rs".to_string();
        sooner.line = Some(9);
        let mut first = point("nesting", "first", 5, "");
        first.file = "src/a.rs".to_string();
        let names: Vec<String> = classify_offenders(&[], &[later, sooner, first], &no_trust())
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["first", "sooner", "later"]);

        let breach = |subject: &str| Breach {
            rule: "max_cc".to_string(),
            rule_type: "constraint".to_string(),
            subject: subject.to_string(),
            file: String::new(),
            value: Some(20),
            message: format!("{subject} is complex"),
        };
        let rules = Rules::default();
        let contract = Contract {
            marked: CompiledRules::compile(rules.clone(), String::new()).unwrap(),
            declared: std::sync::Arc::new(CompiledRules::compile(rules, String::new()).unwrap()),
            thresholds: crate::metrics::Thresholds::default(),
            member_access: HashSet::new(),
            projection: false,
        };
        let subjects: Vec<String> =
            classify_violations(&[], &[breach("z()."), breach("a().")], &contract, &no_trust())
                .into_iter()
                .map(|v| v.subject)
                .collect();
        assert_eq!(subjects, ["a().", "z()."]);
    }

    #[test]
    fn a_measure_switch_is_one_entry_resolved_and_another_new() {
        let mut methods = point("focus", "Big", 25, "");
        methods.measure = Some("methods".to_string());
        let mut span = point("focus", "Big", 600, "");
        span.measure = Some("span".to_string());
        let got: Vec<(Option<String>, ReviewStatus)> =
            classify_offenders(&[methods], &[span], &no_trust())
                .into_iter()
                .map(|e| (e.measure, e.status))
                .collect();
        assert_eq!(
            got,
            vec![
                (Some("span".to_string()), ReviewStatus::New),
                (Some("methods".to_string()), ReviewStatus::Resolved),
            ]
        );
    }

    #[test]
    fn a_cycle_is_matched_by_its_directories_not_its_naming_member() {
        let before = point("acyclicity", "a", 3, "3 symbols across 2 directories: src/a, src/b");
        let after = point("acyclicity", "b", 4, "4 symbols across 2 directories: src/a, src/b");
        let got = classify_offenders(&[before], &[after], &no_trust());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].status, ReviewStatus::Worsened);
        let elsewhere = point("acyclicity", "a", 3, "3 symbols across 2 directories: src/a, src/c");
        let moved = classify_offenders(&[elsewhere], &[point("acyclicity", "a", 3, "3 symbols across 2 directories: src/a, src/b")], &no_trust());
        assert_eq!(moved.len(), 2, "a cycle over other directories is another cycle");
    }

    #[test]
    fn two_cycles_over_the_same_directories_never_overwrite_each_other() {
        let dirs = "2 symbols across 2 directories: src/a, src/b";
        let before = [point("acyclicity", "f1", 2, dirs), point("acyclicity", "f2", 2, dirs)];
        let after = [point("acyclicity", "f1", 2, dirs)];
        let got: Vec<ReviewStatus> = classify_offenders(&before, &after, &no_trust())
            .into_iter()
            .map(|e| e.status)
            .collect();
        assert_eq!(got, vec![ReviewStatus::Resolved, ReviewStatus::Unchanged], "the broken cycle is reported");
    }

    #[test]
    fn clone_groups_are_cut_at_their_size_even_when_two_groups_read_alike() {
        let rows = [
            point("uniqueness", "a1", 2, "2 functions · 10 lines"),
            point("uniqueness", "a2", 2, "2 functions · 10 lines"),
            point("uniqueness", "b1", 2, "2 functions · 10 lines"),
            point("uniqueness", "b2", 2, "2 functions · 10 lines"),
            point("uniqueness", "c1", 3, "3 functions · 10 lines"),
        ];
        let sizes: Vec<usize> = clone_groups(&rows).iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![2, 2, 1]);
    }

    #[test]
    fn only_a_group_with_a_new_member_set_is_introduced() {
        let kept = [
            point("uniqueness", "a1", 2, "x"),
            point("uniqueness", "a2", 2, "x"),
        ];
        let mut after = kept.to_vec();
        after.extend([point("uniqueness", "b1", 2, "y"), point("uniqueness", "b2", 2, "y")]);
        let before = [kept.to_vec(), vec![point("uniqueness", "b1", 2, "y"), point("uniqueness", "z", 2, "y")]].concat();
        let groups = introduced_clone_groups(&before, &after, &no_trust());
        assert_eq!(groups.len(), 1);
        let fresh: Vec<(&str, bool)> = groups[0]
            .members
            .iter()
            .map(|m| (m.name.as_str(), m.newly_duplicated))
            .collect();
        assert_eq!(fresh, vec![("b1", false), ("b2", true)]);
    }

    #[test]
    fn a_valued_violation_compares_its_value_and_an_unvalued_one_its_message() {
        let contract_rules = Rules::default();
        let contract = Contract {
            marked: CompiledRules::compile(contract_rules.clone(), String::new()).unwrap(),
            declared: std::sync::Arc::new(CompiledRules::compile(contract_rules, String::new()).unwrap()),
            thresholds: crate::metrics::Thresholds::default(),
            member_access: HashSet::new(),
            projection: false,
        };
        let breach = |rule: &str, subject: &str, value: Option<u64>, message: &str| Breach {
            rule: rule.to_string(),
            rule_type: "constraint".to_string(),
            subject: subject.to_string(),
            file: String::new(),
            value,
            message: message.to_string(),
        };
        let before = [
            breach("max_cc", "f().", Some(12), "fn `f` has cyclomatic complexity 12"),
            breach("layer-ordering", "src/a.rs", None, "a -> b"),
        ];
        let after = [
            breach("max_cc", "f().", Some(14), "fn `f` has cyclomatic complexity 14"),
            breach("layer-ordering", "src/a.rs", None, "a -> c"),
        ];
        let got: Vec<(String, ReviewStatus)> = classify_violations(&before, &after, &contract, &no_trust())
            .into_iter()
            .map(|v| (v.message, v.status))
            .collect();
        assert_eq!(
            got,
            vec![
                ("a -> c".to_string(), ReviewStatus::New),
                ("a -> b".to_string(), ReviewStatus::Resolved),
                ("fn `f` has cyclomatic complexity 14".to_string(), ReviewStatus::Worsened),
            ]
        );
    }

    #[test]
    fn marking_switches_only_the_per_subject_budgets() {
        let rules = crate::config::parse_rules(
            "[constraints]\nmax_cc = 10\nmax_dead = 3\nno_god_containers = false\nmax_nesting_depth = 4\n",
            Path::new("rules.toml"),
        )
        .unwrap();
        let marked = marked(&rules);
        let c = &marked.constraints;
        assert!(c.max_cc.as_ref().is_some_and(Budget::is_ratchet));
        assert_eq!(c.max_cc.as_ref().map(Budget::max), Some(10));
        assert!(c.max_nesting_depth.as_ref().is_some_and(Budget::is_ratchet));
        assert_eq!(c.max_dead, rules.constraints.max_dead, "a set budget keeps its declared form");
        assert_eq!(c.no_god_containers, rules.constraints.no_god_containers, "an unenforced flag stays off");
        assert_eq!(budget_max(&rules, "max_nesting_depth"), Some(4));
        assert_eq!(budget_max(&rules, "max_dead"), None);
    }

    #[test]
    fn the_cache_keeps_the_newest_entries_and_never_the_one_just_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = |i: usize| dir.path().join(format!("{i:02}.json"));
        let base = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        for i in 0..CACHE_KEEP + 4 {
            std::fs::write(path(i), b"{}").unwrap();
            let when = base + std::time::Duration::from_secs(i as u64);
            std::fs::File::options().write(true).open(path(i)).unwrap().set_modified(when).unwrap();
        }
        std::fs::write(dir.path().join("notes.txt"), b"kept: not a cache entry").unwrap();
        // The entry just written is the oldest by mtime here, and still survives.
        prune_cache(dir.path(), &[path(0)], false);
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        let mut expected: Vec<String> = std::iter::once(0)
            .chain(CACHE_KEEP + 4 - (CACHE_KEEP - 1)..CACHE_KEEP + 4)
            .map(|i| format!("{i:02}.json"))
            .collect();
        expected.push("notes.txt".to_string());
        assert_eq!(left, expected);
    }

    /// Each kind is pruned apart, and a projection run keeps every entry it
    /// used, past [`CACHE_KEEP`].
    #[test]
    fn a_projection_prunes_only_its_own_kind_and_keeps_every_entry_its_run_used() {
        let dir = tempfile::tempdir().unwrap();
        let base = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        let entry = |name: String, i: usize| {
            let path = dir.path().join(name);
            std::fs::write(&path, b"{}").unwrap();
            let when = base + std::time::Duration::from_secs(i as u64);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
            path
        };
        let reviews: Vec<PathBuf> = (0..4).map(|i| entry(format!("r{i:02}.json"), i)).collect();
        let used: Vec<PathBuf> = (0..CACHE_KEEP + 3).map(|i| entry(format!("u{i:02}.projection.json"), 10 + i)).collect();
        let older = entry("old.projection.json".to_string(), 5);

        prune_cache(dir.path(), &used, true);
        assert!(used.iter().chain(&reviews).all(|p| p.exists()), "the run's entries and every review entry stay");
        assert!(!older.exists(), "past the bound, an entry the run did not use goes");

        prune_cache(dir.path(), &reviews[..1], false);
        assert!(used.iter().all(|p| p.exists()), "a review never prunes a projection entry");
    }

    #[cfg(unix)]
    #[test]
    fn the_export_directory_is_fresh_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let (a, b) = (private_dir().unwrap(), private_dir().unwrap());
        assert_ne!(a, b, "every export gets its own directory");
        let mode = std::fs::metadata(&a).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{mode:o}");
        for dir in [a, b] {
            std::fs::remove_dir(dir).unwrap();
        }
    }

    fn finding(scanner: &str, fingerprint: &str, path: Option<&str>, lines: (Option<i64>, Option<i64>)) -> SecurityFinding {
        SecurityFinding {
            scanner_name: scanner.to_string(),
            scanner_version: "1".to_string(),
            head_sha: "0".repeat(40),
            fingerprint: fingerprint.to_string(),
            rule_id: format!("rule-{fingerprint}"),
            severity: crate::model::Severity::High,
            cwes: Vec::new(),
            message: "m".to_string(),
            redacted: false,
            binding: crate::history::FindingBinding::Bound,
            report_path: path.map(str::to_string),
            path: path.map(str::to_string),
            start_line: lines.0,
            start_column: None,
            end_line: lines.1,
            end_column: None,
            commits: Vec::new(),
            legacy_fingerprints: Vec::new(),
            anchor: None,
            freshness: FRESHNESS_FRESH,
        }
    }

    fn snapshot(id: i64, scanner: &str, created_at: i64) -> crate::history::ScannerSnapshot {
        crate::history::ScannerSnapshot {
            snapshot_id: id,
            scanner_name: scanner.to_string(),
            scanner_version: "1".to_string(),
            head_sha: "0".repeat(40),
            report_hash: String::new(),
            finding_count: 0,
            created_at,
            // A full scan: its own HEAD stands behind every finding.
            full_head_sha: Some("0".repeat(40)),
            scope: Vec::new(),
        }
    }

    /// Evidence with `current` findings in the latest report (snapshot 2,
    /// ingested at second 1000) and `base` ones recorded at the merge-base
    /// (snapshot 1), all from scanner `s`.
    fn evidence(current: Vec<SecurityFinding>, base: Vec<SecurityFinding>) -> SecurityEvidence {
        let listed = current
            .into_iter()
            .map(|finding| crate::security::ListedFinding {
                context: FindingContext::unbound(finding.start_line, crate::security::context::UnboundBecause::NoSymbol),
                severity_reason: finding.severity.reason(),
                finding,
                owasp: Vec::new(),
                unclassified: None,
                suppression: None,
            })
            .collect::<Vec<_>>();
        let n = base.len();
        SecurityEvidence {
            current: crate::security::SecurityFindings {
                edition: crate::history::security::OwaspEdition::default(),
                as_of: "2026-01-01".to_string(),
                scanned: vec![crate::security::ScannedReport {
                    driver: "s".to_string(),
                    version: "1".to_string(),
                    head_sha: "0".repeat(40),
                    findings: listed.len(),
                }],
                never_run: Vec::new(),
                notice: None,
                total: listed.len(),
                findings: listed,
                categories: Vec::new(),
                suppressions: Vec::new(),
            },
            latest: vec![snapshot(2, "s", 1000)],
            base: crate::history::SecurityView { snapshots: vec![snapshot(1, "s", 10)], findings: base },
            base_owasp: vec![Vec::new(); n],
            base_contexts: (0..n)
                .map(|_| FindingContext::unbound(None, crate::security::context::UnboundBecause::NoSymbol))
                .collect(),
        }
    }

    #[test]
    fn a_finding_is_in_a_hunk_only_when_its_lines_overlap_one() {
        let hunks: FileRanges = [
            ("a.py".to_string(), vec![(10, 12)]),
            ("new.py".to_string(), vec![WHOLE_FILE]),
        ]
        .into_iter()
        .collect();
        let at = |path: &str, start: Option<i64>, end: Option<i64>| in_hunk(&hunks, &finding("s", "f", Some(path), (start, end)));
        assert!(at("a.py", Some(10), None), "first line of the hunk");
        assert!(at("a.py", Some(12), Some(12)), "last line of the hunk");
        assert!(at("a.py", Some(8), Some(10)), "a region reaching into it");
        assert!(!at("a.py", Some(9), Some(9)), "the line before");
        assert!(!at("a.py", Some(13), None), "the line after");
        assert!(!at("a.py", None, None), "no line on a partly changed file");
        assert!(at("new.py", None, None), "no line on a new file");
        assert!(!at("b.py", Some(10), None), "another file");
        assert!(!in_hunk(&hunks, &finding("s", "f", None, (Some(10), None))), "no file at all");
    }

    #[test]
    fn a_deleted_file_introduces_nothing_by_hunk_but_a_new_one_does() {
        let mut evidence = evidence(
            vec![
                finding("s", "deleted", Some("deleted.py"), (Some(2), None)),
                finding("s", "added", Some("added.py"), (Some(2), None)),
            ],
            Vec::new(),
        );
        evidence.base.snapshots.clear();
        let hunks: FileRanges = [
            ("deleted.py".to_string(), vec![WHOLE_FILE]),
            ("added.py".to_string(), vec![WHOLE_FILE]),
        ]
        .into_iter()
        .collect();
        let delta = security_delta(&evidence, Path::new("/repo"), &hunks, |path| (path == "added.py").then_some(1));
        let got: BTreeMap<&str, (SecurityDeltaStatus, &str)> =
            delta.findings.iter().map(|e| (e.fingerprint.as_str(), (e.status, e.method))).collect();
        assert_eq!(got["deleted"], (SecurityDeltaStatus::PreExisting, SecurityDelta::METHOD_HUNK));
        assert_eq!(got["added"], (SecurityDeltaStatus::Introduced, SecurityDelta::METHOD_HUNK));
    }

    /// An `unrated` entry of the delta carries its reason, as every other
    /// listing does (S-768); a rated one carries none, on the wire too.
    #[test]
    fn an_unrated_delta_entry_states_why_and_a_rated_one_does_not() {
        let unrated = SecurityFinding { severity: crate::model::Severity::Unrated, ..finding("s", "unrated", Some("a.py"), (Some(2), None)) };
        let mut evidence = evidence(vec![unrated, finding("s", "rated", Some("a.py"), (Some(3), None))], Vec::new());
        evidence.base.snapshots.clear();
        let hunks: FileRanges = [("a.py".to_string(), vec![WHOLE_FILE])].into_iter().collect();
        let delta = security_delta(&evidence, Path::new("/repo"), &hunks, |_| Some(1));
        let reasons: BTreeMap<&str, Option<&str>> =
            delta.findings.iter().map(|e| (e.fingerprint.as_str(), e.severity_reason)).collect();
        assert_eq!(reasons["unrated"], Some("the scanner gave none"));
        assert_eq!(reasons["rated"], None);
        let rated = delta.findings.iter().find(|e| e.fingerprint == "rated").unwrap();
        assert!(serde_json::to_value(rated).unwrap().get("severity_reason").is_none(), "absent, not null");
    }

    #[test]
    fn a_secret_in_a_file_not_yet_indexed_is_introduced_by_its_report_path() {
        // A scan of a new file nothing synced yet: the report names the file,
        // the index does not hold it, so the finding is `unmatched` with no
        // bound path. Its hunk is still the branch's.
        let unmatched = |fingerprint: &str, report: &str| SecurityFinding {
            binding: crate::history::FindingBinding::Unmatched,
            report_path: Some(report.to_string()),
            path: None,
            commits: Vec::new(),
            legacy_fingerprints: Vec::new(),
            anchor: None,
            freshness: FRESHNESS_NA,
            ..finding("s", fingerprint, None, (Some(1), Some(1)))
        };
        let mut evidence = evidence(
            vec![unmatched("new", "./app/new.py"), unmatched("elsewhere", "/ci/checkout/app/new.py")],
            Vec::new(),
        );
        evidence.base.snapshots.clear();
        let hunks: FileRanges = [("app/new.py".to_string(), vec![WHOLE_FILE])].into_iter().collect();
        let delta = security_delta(&evidence, Path::new("/repo"), &hunks, |_| Some(1));
        let got: BTreeMap<&str, SecurityDeltaStatus> =
            delta.findings.iter().map(|e| (e.fingerprint.as_str(), e.status)).collect();
        assert_eq!(got["new"], SecurityDeltaStatus::Introduced, "the report path, `./` dropped, is the hunk's file");
        assert_eq!(got["elsewhere"], SecurityDeltaStatus::PreExisting, "a path spelled for another root is no match");
    }

    #[test]
    fn a_finding_that_only_moved_lines_is_paired_with_its_merge_base_twin() {
        let moved = |line: i64, fingerprint: &str| SecurityFinding {
            rule_id: "weak-hash".to_string(),
            ..finding("s", fingerprint, Some("a.py"), (Some(line), None))
        };
        // Moved below an added line, outside every hunk: one pre-existing entry.
        let delta = security_delta(
            &evidence(vec![moved(3, "at-3")], vec![moved(2, "at-2")]),
            Path::new("/repo"),
            &[("a.py".to_string(), vec![(1, 1)])].into_iter().collect(),
            |_| Some(1),
        );
        let got: Vec<(&str, SecurityDeltaStatus)> =
            delta.findings.iter().map(|e| (e.fingerprint.as_str(), e.status)).collect();
        assert_eq!(got, vec![("at-3", SecurityDeltaStatus::PreExisting)]);
        assert!(delta.notes.iter().any(|n| n.contains("moved with their line")), "{:?}", delta.notes);

        // Inside a hunk the new fingerprint is the branch's.
        let delta = security_delta(
            &evidence(vec![moved(3, "at-3")], vec![moved(2, "at-2")]),
            Path::new("/repo"),
            &[("a.py".to_string(), vec![(3, 3)])].into_iter().collect(),
            |_| Some(1),
        );
        let got: Vec<SecurityDeltaStatus> = delta.findings.iter().map(|e| e.status).collect();
        assert_eq!(got, vec![SecurityDeltaStatus::Introduced, SecurityDeltaStatus::Resolved]);

        // Another rule is another finding.
        let mut other = moved(3, "at-3");
        other.rule_id = "another-rule".to_string();
        let delta = security_delta(&evidence(vec![other], vec![moved(2, "at-2")]), Path::new("/repo"), &FileRanges::new(), |_| Some(1));
        let got: Vec<SecurityDeltaStatus> = delta.findings.iter().map(|e| e.status).collect();
        assert_eq!(got, vec![SecurityDeltaStatus::Introduced, SecurityDeltaStatus::Resolved]);
    }

    /// `evidence` over two scanners, `a` and `b`, both recorded at the
    /// merge-base and both re-scanned since.
    fn two_scanners(current: Vec<SecurityFinding>, base: Vec<SecurityFinding>) -> SecurityEvidence {
        let mut evidence = evidence(current, base);
        let report = evidence.current.scanned[0].clone();
        evidence.current.scanned = ["a", "b"]
            .map(|driver| crate::security::ScannedReport { driver: driver.to_string(), ..report.clone() })
            .to_vec();
        evidence.latest = vec![snapshot(3, "a", 1000), snapshot(4, "b", 1000)];
        evidence.base.snapshots = vec![snapshot(1, "a", 10), snapshot(2, "b", 10)];
        evidence
    }

    #[test]
    fn a_shared_fingerprint_is_judged_per_scanner() {
        let at = |scanner: &str| finding(scanner, "x", Some("a.py"), (Some(1), None));
        let class = |delta: &SecurityDelta| -> Vec<(String, SecurityDeltaStatus)> {
            delta.findings.iter().map(|e| (e.scanner.clone(), e.status)).collect()
        };
        // Both reported `x` at the merge-base; only `a` still does.
        let delta = security_delta(&two_scanners(vec![at("a")], vec![at("a"), at("b")]), Path::new("/repo"), &FileRanges::new(), |_| Some(1));
        assert_eq!(
            class(&delta),
            vec![("b".to_string(), SecurityDeltaStatus::Resolved), ("a".to_string(), SecurityDeltaStatus::PreExisting)]
        );
        // Only `a` reported `x` at the merge-base; now `b` does.
        let delta = security_delta(&two_scanners(vec![at("b")], vec![at("a")]), Path::new("/repo"), &FileRanges::new(), |_| Some(1));
        assert_eq!(
            class(&delta),
            vec![("b".to_string(), SecurityDeltaStatus::Introduced), ("a".to_string(), SecurityDeltaStatus::Resolved)]
        );
    }

    #[test]
    fn a_section_that_cannot_be_read_says_why_and_classifies_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let commit = "0".repeat(40);
        let failed = security_section(dir.path(), &commit, &commit, |_| Err(anyhow::anyhow!("ignore file is malformed")));
        assert!(!failed.measured);
        let why = failed.not_measured_because.unwrap_or_default();
        assert!(why.contains("findings could not be read") && why.contains("ignore file is malformed"), "{why}");

        // Not a repository: the branch's hunks cannot be read.
        let undiffable = security_section(dir.path(), &commit, &commit, |_| Ok(evidence(Vec::new(), Vec::new())));
        assert!(!undiffable.measured && undiffable.findings.is_empty());
        let why = undiffable.not_measured_because.unwrap_or_default();
        assert!(why.contains("changed hunks could not be read"), "{why}");
    }

    #[test]
    fn a_vanished_fingerprint_is_resolved_only_on_a_file_older_than_the_rescan() {
        let base = vec![
            finding("s", "older", Some("older.py"), (Some(1), None)),
            finding("s", "same-second", Some("same.py"), (Some(1), None)),
            finding("s", "gone", Some("gone.py"), (Some(1), None)),
            finding("s", "kept", Some("kept.py"), (Some(1), None)),
        ];
        let current = vec![finding("s", "kept", Some("kept.py"), (Some(1), None))];
        let delta = security_delta(&evidence(current, base), Path::new("/repo"), &FileRanges::new(), |path| match path {
            "older.py" => Some(999),
            "same.py" => Some(1000),
            _ => None,
        });
        let got: BTreeMap<&str, (SecurityDeltaStatus, &str)> = delta
            .findings
            .iter()
            .map(|e| (e.fingerprint.as_str(), (e.status, e.freshness.as_str())))
            .collect();
        assert_eq!(got["older"], (SecurityDeltaStatus::Resolved, FRESHNESS_FRESH));
        assert_eq!(got["same-second"], (SecurityDeltaStatus::Stale, FRESHNESS_STALE), "an edit in the ingest's second is not taken as scanned");
        assert_eq!(got["gone"], (SecurityDeltaStatus::Resolved, FRESHNESS_NA), "a deleted file's finding cannot be reported");
        assert_eq!(got["kept"].0, SecurityDeltaStatus::PreExisting);
        assert_eq!((delta.counts.resolved, delta.counts.stale, delta.counts.pre_existing), (2, 1, 1));
    }

    #[test]
    fn only_an_unexpired_entry_suppresses() {
        let mut evidence = evidence(
            vec![finding("s", "live", Some("a.py"), (Some(1), None)), finding("s", "lapsed", Some("a.py"), (Some(2), None))],
            Vec::new(),
        );
        let entry = |fingerprint: &str, expired: bool| crate::history::security::EntryStatus {
            entry: crate::history::security::IgnoreEntry {
                selector: fingerprint.into(),
                reason: format!("reason {fingerprint}"),
                expires: chrono::NaiveDate::from_ymd_opt(2026, 1, 1),
            },
            expired,
            orphaned: false,
            suppressed: 1,
            should_name: None,
        };
        evidence.current.suppressions = vec![entry("live", false), entry("lapsed", true)];
        let delta = security_delta(&evidence, Path::new("/repo"), &FileRanges::new(), |_| None);
        assert_eq!(
            delta.suppressed.iter().map(|s| (s.entry.fingerprint.as_str(), s.reason.as_str(), s.expires.as_deref())).collect::<Vec<_>>(),
            vec![("live", "reason live", Some("2026-01-01"))]
        );
        assert_eq!(delta.findings.iter().map(|e| e.fingerprint.as_str()).collect::<Vec<_>>(), vec!["lapsed"]);
        assert_eq!(delta.counts.suppressed, 1);
    }

    /// A path entry (S-770) holds out what is under it — a finding the latest
    /// report carries and one it no longer does — and nothing beside it.
    #[test]
    fn a_path_entry_suppresses_current_and_resolved_findings_under_it() {
        // A lockfile the index does not hold, reported by its absolute path
        // under the root: the root is what makes it `fixtures/Cargo.lock`.
        let mut lockfile = finding("s", "lockfile", None, (None, None));
        lockfile.report_path = Some("/repo/fixtures/Cargo.lock".to_string());
        lockfile.binding = crate::history::FindingBinding::Unmatched;
        let mut evidence = evidence(
            vec![
                finding("s", "under", Some("fixtures/a.py"), (Some(1), None)),
                finding("s", "beside", Some("fixtures-old/a.py"), (Some(1), None)),
                lockfile,
            ],
            vec![finding("s", "gone", Some("fixtures/b.py"), (Some(1), None))],
        );
        evidence.current.suppressions = vec![crate::history::security::EntryStatus {
            entry: crate::history::security::IgnoreEntry {
                selector: crate::history::security::Selector::Path { path: "fixtures".into(), rule: None, scanner: None },
                reason: "fixtures".to_string(),
                expires: None,
            },
            expired: false,
            orphaned: false,
            suppressed: 1,
            should_name: None,
        }];
        let delta = security_delta(&evidence, Path::new("/repo"), &FileRanges::new(), |_| Some(1));
        let held: BTreeSet<&str> = delta.suppressed.iter().map(|s| s.entry.fingerprint.as_str()).collect();
        assert_eq!(held, BTreeSet::from(["gone", "lockfile", "under"]));
        assert_eq!(delta.findings.iter().map(|e| e.fingerprint.as_str()).collect::<Vec<_>>(), vec!["beside"]);
    }

    #[test]
    fn a_file_less_entry_reads_stale_while_any_file_is() {
        let mut trust = no_trust();
        assert_eq!(trust.of(""), BranchReview::TRUST_RESOLVED);
        trust.stale.insert("src/x.rs".to_string());
        assert_eq!(trust.of(""), BranchReview::TRUST_STALE);
    }

    // ── FR-GV-25 (S-754): the `[review]` policy ───────────────────────────────

    /// A policy over `dimensions`, reporting its drift.
    fn policy(dimensions: &[&str]) -> ReviewRules {
        ReviewRules {
            gate_dimensions: dimensions.iter().map(|d| (*d).to_string()).collect(),
            ..ReviewRules::default()
        }
    }

    /// The classified entries of a branch that adds `deep` (6), worsens
    /// `worse` (5 → 7), improves `better` (8 → 5), resolves `gone`, and adds an
    /// Equality entry `eq` — `stale` names a file whose entries read stale.
    fn delta(stale: &[&str]) -> Vec<ReviewedOffender> {
        let before = [
            point("nesting", "worse", 5, "nesting depth 5"),
            point("nesting", "better", 8, "nesting depth 8"),
            point("nesting", "gone", 6, "nesting depth 6"),
        ];
        let after = [
            point("nesting", "deep", 6, "nesting depth 6"),
            point("nesting", "worse", 7, "nesting depth 7"),
            point("nesting", "better", 5, "nesting depth 5"),
            point("equality", "eq", 12, "cyclomatic complexity 12"),
        ];
        let label = |file: &str| {
            if stale.contains(&file) { BranchReview::TRUST_STALE } else { BranchReview::TRUST_RESOLVED }
        };
        let trust = Trust {
            stale: stale.iter().map(|f| (*f).to_string()).collect(),
            by_file: before
                .iter()
                .chain(after.iter())
                .map(|p| (p.file.clone(), label(&p.file).to_string()))
                .collect(),
        };
        classify_offenders(&before, &after, &trust)
    }

    fn gating_names(entries: &[ReviewedOffender]) -> Vec<&str> {
        entries.iter().filter(|e| e.gating).map(|e| e.name.as_str()).collect()
    }

    /// Only a `new` or `worsened` entry in a gating dimension on a resolved
    /// file gates: never an improved or resolved one, never a dimension the
    /// policy does not name, never a stale file's entry.
    #[test]
    fn only_new_and_worsened_entries_of_a_gating_dimension_on_a_resolved_file_gate() {
        let mut entries = delta(&[]);
        let readout = mark_gating(&mut entries, &policy(&["nesting"]));
        assert_eq!(gating_names(&entries), ["deep", "worse"], "{entries:#?}");
        assert_eq!(readout, ReviewPolicy { gate_dimensions: vec!["nesting".to_string()], gating: 2 });

        // Equality is listed `new` but is not a gating dimension.
        let eq = entries.iter().find(|e| e.name == "eq").unwrap();
        assert_eq!((eq.status, eq.gating), (ReviewStatus::New, false));

        let mut entries = delta(&["src/deep.rs"]);
        mark_gating(&mut entries, &policy(&["nesting"]));
        assert_eq!(gating_names(&entries), ["worse"], "a stale entry is listed and never gates");

        let mut entries = delta(&[]);
        let readout = mark_gating(&mut entries, &policy(&["cohesion"]));
        assert!(gating_names(&entries).is_empty());
        assert_eq!(readout.gating, 0);
    }

    /// A numeric budget fails a drop past it and passes one at it; `"report"`
    /// never fails; a rise is a negative drop.
    #[test]
    fn the_drift_budget_fails_only_a_drop_past_it() {
        let points = |p: f64| DriftBudget::Points(p);
        let report = DriftBudget::default();
        assert!(signal_drift(8300, 8297, points(2.0)).exceeded);
        assert!(!signal_drift(8300, 8298, points(2.0)).exceeded, "a drop of exactly the budget passes");
        assert!(!signal_drift(8300, 8297, points(3.5)).exceeded);
        assert!(signal_drift(8300, 8299, points(0.0)).exceeded);
        assert!(!signal_drift(8300, 8000, report).exceeded, "report never fails");
        let rise = signal_drift(8300, 8310, points(0.0));
        assert_eq!((rise.drop, rise.exceeded), (-10, false));
        let d = signal_drift(8300, 8297, report);
        assert_eq!((d.baseline, d.current, d.drop, d.budget), (8300, 8297, 3, None));
    }

    #[test]
    fn the_drift_message_names_the_signal_the_baseline_and_the_budget() {
        let message = |b, c, budget| drift_message(&signal_drift(b, c, budget));
        assert_eq!(
            message(8300, 8297, DriftBudget::Points(2.0)),
            "signal 8297 drifted 3 below the baseline 8300, past drift_budget 2"
        );
        assert_eq!(
            message(8300, 8298, DriftBudget::Points(2.0)),
            "signal 8298 drifted 2 below the baseline 8300, within drift_budget 2"
        );
        assert_eq!(
            message(8300, 8297, DriftBudget::default()),
            "signal 8297 drifted 3 below the baseline 8300 (drift_budget \"report\": reported, never failing)"
        );
        assert_eq!(
            message(8300, 8301, DriftBudget::default()),
            "signal 8301 holds the baseline 8300 (drift_budget \"report\": reported, never failing)"
        );
    }

    /// The gate takes the gating entries, fails, and names each with value,
    /// threshold and trust; a delta with none leaves the verdict as it was; an
    /// unmeasured one warns and never fails.
    #[test]
    fn the_gate_fails_on_and_names_each_gating_entry() {
        let mut entries = delta(&[]);
        mark_gating(&mut entries, &policy(&["nesting"]));
        let review = BranchReview {
            measured: true,
            merge_base: Some("0123456789abcdef0123".to_string()),
            offenders: entries,
            ..BranchReview::default()
        };
        let mut result = GateResult {
            passed: true,
            message: "signal 8300 holds the baseline 8300".to_string(),
            ..GateResult::default()
        };
        fold_into_gate(&mut result, review.clone());
        assert!(!result.passed);
        assert_eq!(gating_names(&result.gating_entries), ["deep", "worse"]);
        assert_eq!(
            result.message,
            "signal 8300 holds the baseline 8300; 2 gating branch-delta entries against the merge-base \
             0123456789ab: nesting `deep` (src/deep.rs:1) nesting_depth 6, threshold 4 [new, trust resolved]; \
             nesting `worse` (src/worse.rs:1) nesting_depth 5 -> 7, threshold 4 [worsened, trust resolved]"
        );

        let mut quiet = review.clone();
        quiet.offenders.retain(|e| !e.gating);
        let mut result = GateResult { passed: true, ..GateResult::default() };
        fold_into_gate(&mut result, quiet);
        assert!(result.passed && result.gating_entries.is_empty() && result.message.is_empty());

        let unmeasured = BranchReview {
            not_measured_because: Some("HEAD and main have no common ancestor".to_string()),
            ..BranchReview::default()
        };
        let mut result = GateResult { passed: true, ..GateResult::default() };
        fold_into_gate(&mut result, unmeasured);
        assert!(result.passed);
        assert_eq!(
            result.warnings,
            ["[review]: the branch delta was not measured, so no per-symbol entry was judged: \
              HEAD and main have no common ancestor"]
        );
    }

    fn limit(rule: &'static str, ceiling: u64) -> Limit {
        Limit {
            rule,
            ceiling,
            per_subject: true,
        }
    }

    fn tally_of(entries: &[(&'static str, &str, u64)]) -> Tally {
        entries
            .iter()
            .map(|(rule, subject, value)| ((*rule, subject.to_string()), (*value, format!("{subject}.rs"))))
            .collect()
    }

    fn shares(exceeded: &ExceededLimit) -> Vec<(&str, i64, bool)> {
        exceeded.shares.iter().map(|s| (s.branch.as_str(), s.share, s.exceeds_alone)).collect()
    }

    /// FR-GV-28's case: two shares of 1 on a subject at `ceiling − 1` exceed
    /// it together and neither does alone; a subject they do not take past
    /// its ceiling is counted in the denominator and not listed.
    #[test]
    fn summed_shares_exceed_a_ceiling_that_neither_share_reaches_alone() {
        let limits = [limit("max_fan_out", 3)];
        let base = tally_of(&[("max_fan_out", "m", 2), ("max_fan_out", "n", 1)]);
        let b1 = ("b1".to_string(), base.clone(), tally_of(&[("max_fan_out", "m", 3), ("max_fan_out", "n", 1)]));
        let b2 = ("b2".to_string(), base.clone(), tally_of(&[("max_fan_out", "m", 3), ("max_fan_out", "n", 2)]));

        let (alone, subjects) = project(&limits, &base, std::slice::from_ref(&b1));
        assert!(alone.is_empty(), "{alone:?}");
        assert_eq!(subjects, 2);

        let (together, _) = project(&limits, &base, &[b1, b2]);
        assert_eq!(together.len(), 1, "{together:?}");
        let m = &together[0];
        assert_eq!((m.rule.as_str(), m.subject.as_str(), m.file.as_str()), ("max_fan_out", "m", "m.rs"));
        assert_eq!((m.base, m.limit, m.projected), (2, 3, 4));
        assert_eq!(shares(m), [("b1", 1, false), ("b2", 1, false)]);
    }

    /// A share past the ceiling by itself says so; a lowering share offsets
    /// the others, and a share of 0 is not listed.
    #[test]
    fn a_lowering_share_offsets_the_others_and_a_share_alone_past_the_ceiling_says_so() {
        let limits = [limit("max_cc", 3)];
        let base = tally_of(&[("max_cc", "m", 2)]);
        let up = ("up".to_string(), base.clone(), tally_of(&[("max_cc", "m", 5)]));
        let flat = ("flat".to_string(), base.clone(), base.clone());
        let down = |to| ("down".to_string(), base.clone(), tally_of(&[("max_cc", "m", to)]));

        let (offset, _) = project(&limits, &base, &[up.clone(), down(0)]);
        assert!(offset.is_empty(), "3 − 2 leaves m at 3: {offset:?}");
        let (raised, _) = project(&limits, &base, &[up, flat, down(1)]);
        assert_eq!(raised.len(), 1, "{raised:?}");
        assert_eq!(raised[0].projected, 4);
        assert_eq!(shares(&raised[0]), [("up", 3, true), ("down", -1, false)]);
    }

    /// A limit the base already exceeds is `check`'s to report until a share
    /// raises it; a subject absent on the base starts at 0.
    #[test]
    fn a_limit_the_base_already_exceeds_is_listed_only_once_a_branch_raises_it() {
        let limits = [limit("max_fan_out", 3)];
        let base = tally_of(&[("max_fan_out", "m", 5)]);
        let other = ("b1".to_string(), base.clone(), tally_of(&[("max_fan_out", "m", 5), ("max_fan_out", "new", 2)]));
        let (untouched, subjects) = project(&limits, &base, std::slice::from_ref(&other));
        assert!(untouched.is_empty(), "{untouched:?}");
        assert_eq!(subjects, 2);

        let raises = ("b2".to_string(), base.clone(), tally_of(&[("max_fan_out", "m", 6)]));
        let (listed, _) = project(&limits, &base, &[other, raises]);
        let subjects: Vec<&str> = listed.iter().map(|e| e.subject.as_str()).collect();
        assert_eq!(subjects, ["m"], "`new` at 2 fits the ceiling");
        assert_eq!((listed[0].base, listed[0].projected), (5, 6));
        assert_eq!(shares(&listed[0]), [("b2", 1, true)]);
    }

    /// Entries follow the limits' evaluation order, then subject — not the
    /// rules' names: `no_god_files` is evaluated before `max_fan_out`.
    #[test]
    fn exceeded_limits_follow_the_limits_order_then_the_subject() {
        let limits = [limit("no_god_files", 1), limit("max_fan_out", 1)];
        let base = Tally::new();
        let after = tally_of(&[("max_fan_out", "a", 2), ("no_god_files", "z", 2), ("no_god_files", "b", 2)]);
        let (exceeded, _) = project(&limits, &base, &[("b1".to_string(), Tally::new(), after)]);
        let keys: Vec<(&str, &str)> = exceeded.iter().map(|e| (e.rule.as_str(), e.subject.as_str())).collect();
        assert_eq!(keys, [("no_god_files", "b"), ("no_god_files", "z"), ("max_fan_out", "a")]);
    }

    /// A side's tally: a per-subject limit's value by coordinate-less
    /// subject, a symbol set's member count, the cycle count; a rule no limit
    /// names is not tallied.
    #[test]
    fn a_side_tallies_each_value_each_set_member_and_the_cycle_count() {
        let breach = |rule: &str, subject: &str, value| Breach {
            rule: rule.to_string(),
            rule_type: "constraint".to_string(),
            subject: subject.to_string(),
            file: "src/e.rs".to_string(),
            value: Some(value),
            message: String::new(),
        };
        let side = Measured {
            violations: vec![
                breach("max_fan_out", "module:logos . tree . src/`e.rs`/", 4),
                breach("max_dead", "a().", 1),
                breach("max_dead", "b().", 1),
                breach("max_cc", "f().", 9),
            ],
            cycles: Some(2),
            ..Measured::default()
        };
        let count = |rule| Limit {
            rule,
            ceiling: 0,
            per_subject: false,
        };
        let limits = [count("max_cycles"), limit("max_fan_out", 3), count("max_dead"), count("max_duplicates")];
        let tally = tally(&side, &limits);
        let entries: Vec<(&str, &str, u64)> = tally.iter().map(|((r, s), (v, _))| (*r, s.as_str(), *v)).collect();
        assert_eq!(
            entries,
            [
                ("max_cycles", "", 2),
                ("max_dead", "", 2),
                ("max_duplicates", "", 0),
                ("max_fan_out", "module:src/`e.rs`/", 4)
            ],
            "a project-wide count is in the denominator at 0 too"
        );
    }

    #[test]
    fn a_subject_loses_its_coordinate_and_nothing_else() {
        assert_eq!(coordinate_less("module:logos . tree . src/`e.rs`/"), "module:src/`e.rs`/");
        assert_eq!(coordinate_less("src/`e.rs`/f()."), "src/`e.rs`/f().");
        assert_eq!(coordinate_less("module:not a symbol"), "module:not a symbol");
        assert_eq!(coordinate_less("file:src/e.rs"), "file:src/e.rs");
    }

    /// Every declared projectable budget records every subject; the
    /// unprojectable ones and every policy section are dropped; the
    /// thresholds stay.
    #[test]
    fn the_projected_contract_records_every_subject_of_each_declared_budget() {
        let rules = crate::config::parse_rules(
            "[constraints]\nmax_cycles = 7\nmax_cc = 10\nmax_fan_out = { max = 30, ratchet = true }\n\
             max_dead = { baseline = 4, delta = 1 }\nmax_brain_methods = 2\nmax_clone_ratio = 0.1\n\
             no_god_containers = true\nmax_duplicates = 3\nmax_nesting_depth = 4\nmax_fn_lines = 50\n\
             no_god_files = 20\nmax_fan_in = 9\n[metric_thresholds]\nbrain_nesting = 5\n\
             [[layers]]\nname = \"core\"\npaths = [\"src/**\"]\norder = 0\n",
            Path::new("rules.toml"),
        )
        .unwrap();
        let every = Some(Budget::Ratchet(RatchetBudget { max: 0, ratchet: true }));
        let projected = projected(&rules);
        let c = &projected.constraints;
        for (rule, budget) in [
            ("max_cc", &c.max_cc),
            ("max_fn_lines", &c.max_fn_lines),
            ("no_god_files", &c.no_god_files),
            ("max_fan_in", &c.max_fan_in),
            ("max_fan_out", &c.max_fan_out),
            ("max_duplicates", &c.max_duplicates),
            ("max_nesting_depth", &c.max_nesting_depth),
            ("max_brain_methods", &c.max_brain_methods),
        ] {
            assert_eq!(budget, &every, "{rule} records every subject");
        }
        assert_eq!(c.max_dead, Some(MaxDead::Ratchet(RatchetBudget { max: 0, ratchet: true })));
        assert_eq!(c.max_cycles, rules.constraints.max_cycles, "the cycle count is recorded beside");
        let sparse = crate::config::parse_rules("[constraints]\nmax_cc = 10\n", Path::new("rules.toml")).unwrap();
        let sparse = super::projected(&sparse).constraints;
        assert_eq!((&sparse.max_fn_lines, &sparse.no_god_files, &sparse.max_fan_in), (&None, &None, &None), "undeclared stay off");
        assert_eq!((c.max_clone_ratio, c.no_god_containers), (None, None));
        assert!(projected.layers.is_empty() && !rules.layers.is_empty());
        assert_eq!(projected.metric_thresholds, rules.metric_thresholds);

        let (limits, unprojected) = projected_limits(&rules);
        let ceilings: Vec<(&str, u64, bool)> = limits.iter().map(|l| (l.rule, l.ceiling, l.per_subject)).collect();
        assert_eq!(
            ceilings,
            [
                ("max_cycles", 7, false),
                ("max_cc", 10, true),
                ("max_fn_lines", 50, true),
                ("no_god_files", 20, true),
                ("max_fan_in", 9, true),
                ("max_fan_out", 30, true),
                ("max_dead", 5, false),
                ("max_duplicates", 3, false),
                ("max_nesting_depth", 4, true),
                ("max_brain_methods", 2, false),
            ]
        );
        let names: Vec<&str> = unprojected.iter().map(|l| l.rule.as_str()).collect();
        assert_eq!(names, ["max_clone_ratio", "no_god_containers"]);

        let off = crate::config::parse_rules("[constraints]\nno_god_containers = false\n", Path::new("rules.toml")).unwrap();
        assert_eq!(projected_limits(&off), (Vec::new(), Vec::new()), "an unenforced flag is not a limit");
    }

    fn signal_side(equality: f64) -> Measured {
        Measured {
            terms: Some(SignalTerms {
                original: vec![(std::borrow::Cow::Borrowed("equality"), equality)],
                extended: Vec::new(),
            }),
            ..Measured::default()
        }
    }

    /// Two drops that each fit a numeric `drift_budget` fail it together; a
    /// side with no signal is a reason, never a guess.
    #[test]
    fn summed_drops_fail_a_drift_budget_that_neither_drop_fails_alone() {
        let base = signal_side(0.90);
        let signal = |m: &Measured| i64::from(m.terms.as_ref().unwrap().signal());
        let (one, two) = (signal_side(0.89), signal_side(0.88));
        let (d1, d2) = (signal(&base) - signal(&one), signal(&base) - signal(&two));
        assert!(d1 > 0 && d2 > d1, "the fixture drops: {d1}, {d2}");
        let budget = d2 as f64;
        let sides = vec![
            ("b1".to_string(), (base.clone(), one)),
            ("b2".to_string(), (base.clone(), two)),
        ];

        let none = || Err("no gate baseline is saved in this checkout".to_string());
        let (alone, note) = project_drift(budget, &base, &sides[..1], none()).unwrap();
        assert_eq!(alone, None);
        assert_eq!(note, "drift_budget is projected from the base commit's signal: no gate baseline is saved in this checkout");
        let drift = project_drift(budget, &base, &sides, none()).unwrap().0.expect("the sum exceeds the budget");
        assert_eq!(drift.reference.kind, DriftReference::BASE);
        assert_eq!((drift.drift.drop, drift.drift.budget, drift.drift.exceeded), (d1 + d2, Some(budget), true));
        assert_eq!(i64::from(drift.drift.baseline) - i64::from(drift.drift.current), d1 + d2);
        let named: Vec<(&str, i64, bool)> = drift.shares.iter().map(|s| (s.branch.as_str(), s.share, s.exceeds_alone)).collect();
        assert_eq!(named, [("b1", d1, false), ("b2", d2, false)]);
        assert_eq!(project_drift((d1 + d2) as f64, &base, &sides, none()).unwrap().0, None, "a drop of exactly the budget passes");

        let empty = vec![("b3".to_string(), (base.clone(), Measured::default()))];
        let why = project_drift(budget, &base, &empty, none()).unwrap_err();
        assert!(why.contains("b3") && why.contains("has no signal"), "{why}");
    }

    fn drops(entries: &[(&str, i64)]) -> Vec<(String, i64)> {
        entries.iter().map(|(n, d)| (n.to_string(), *d)).collect()
    }

    /// The review's example ([CR-237] §3.2): a blessed baseline of 7000, a base
    /// at 6990, `drift_budget = 15` and two drops of 3. From the baseline the
    /// merge falls 16 and the gate after it fails, so the projection lists it;
    /// from the base it would read a fall of 6.
    ///
    /// [CR-237]: ../../../docs/requests/CR-237-branch-delta-gating-attribution-and-budgets.md
    #[test]
    fn a_comparable_baseline_is_the_drift_reference_the_merged_gate_judges_by() {
        let two = drops(&[("b1", 3), ("b2", 3)]);
        let drift = judge_drift(15.0, 6990, &two, Ok((7000, 42))).expect("16 below the baseline");
        assert_eq!(
            (drift.drift.baseline, drift.drift.current, drift.drift.drop, drift.drift.exceeded),
            (7000, 6984, 16, true)
        );
        assert_eq!(
            drift.reference,
            DriftReference { kind: DriftReference::BASELINE, snapshot_id: Some(42), reason: None }
        );
        let alone: Vec<bool> = drift.shares.iter().map(|s| s.exceeds_alone).collect();
        assert_eq!(alone, [false, false], "10 + 3 fits 15");

        let base_only = Err("the gate baseline was saved under other thresholds".to_string());
        assert_eq!(judge_drift(15.0, 6990, &two, base_only), None, "from the base it is a fall of 6");

        let one = judge_drift(15.0, 6990, &drops(&[("b1", 6)]), Ok((7000, 42))).expect("10 + 6 > 15");
        assert!(one.shares[0].exceeds_alone, "the base's own distance counts toward a share alone");
    }

    /// A base already past the budget is the gate's to report until a branch
    /// lowers the signal further; a net rise lists nothing.
    #[test]
    fn a_drift_the_branches_do_not_deepen_is_not_listed() {
        let reference = || Ok((7000, 42));
        assert_eq!(judge_drift(15.0, 6980, &drops(&[("b1", -2)]), reference()), None, "a rise");
        assert_eq!(judge_drift(15.0, 6980, &drops(&[("b1", 0)]), reference()), None, "no change");
        let deeper = judge_drift(15.0, 6980, &drops(&[("b1", 1)]), reference()).expect("21 below");
        assert_eq!(deeper.drift.drop, 21);
        assert!(deeper.shares[0].exceeds_alone);
    }

    /// Past five entries the message counts the rest; `gating_entries` keeps all.
    #[test]
    fn the_gate_message_names_five_entries_and_counts_the_rest() {
        let after: Vec<Point> = (0..7).map(|i| point("nesting", &format!("f{i}"), 6, "nesting depth 6")).collect();
        let mut entries = classify_offenders(&[], &after, &no_trust());
        mark_gating(&mut entries, &policy(&["nesting"]));
        let review = BranchReview { measured: true, offenders: entries, ..BranchReview::default() };
        let mut result = GateResult::default();
        fold_into_gate(&mut result, review);
        assert_eq!(result.gating_entries.len(), 7);
        assert!(result.message.starts_with("7 gating branch-delta entries"), "{}", result.message);
        assert!(result.message.ends_with("; and 2 more"), "{}", result.message);
        assert_eq!(result.message.matches("[new, trust resolved]").count(), 5);
    }
}
