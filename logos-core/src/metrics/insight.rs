//! The **"where to start"** ranking and the per-member **outlier labels**
//! ([FR-QM-17], [CR-164], S-504) — pure, no I/O.
//!
//! # The ranking
//!
//! A *fix unit* is what one fix addresses: one class-like container (Cohesion,
//! Focus), one function (Nesting, Conciseness), one exact-duplicate group
//! (Redundancy) or one near-clone group (Uniqueness). Each offending unit is
//! stated in **recoverable points**: the aggregate signal recomputed with that
//! unit's contribution removed, minus the current signal — an estimate under a
//! single-unit counterfactual ([`WhereToStart::BASIS`]). Removing a unit's
//! contribution means, per kind:
//!
//! - **function** — it stops counting as deeply nested and as a brain method
//!   (a re-run `compute` sees it with no recorded nesting depth);
//! - **class** — its `1/LCOM4` becomes 1, and it stops counting as god;
//! - **exact-duplicate group of k** — the redundant count drops by `k − 1`: one
//!   copy survives and the others lose their duplicate verdict. A dead member is
//!   redundant whatever the fix, so the survivor is a dead member when there is
//!   one (and every other dead member stays redundant); otherwise the lowest-id
//!   member. Which copy survives therefore never depends on id order;
//! - **near-clone group** — its members leave the near-cloned count.
//!
//! The counterfactual is not a second implementation of the signal: it swaps
//! the changed dimension values into a copy of the computed snapshot and runs
//! the very reductions [`compute`](super::compute) runs —
//! [`from_ratio`](super::extended::from_ratio),
//! [`cohesion_of`](super::extended::cohesion_of),
//! [`redundancy_of`](super::redundancy_of) and [`signal_of`](super::signal_of)
//! — over the same counts in the same order, so each figure equals a re-run
//! `compute` on the changed inputs, bit for bit. A function, Focus or
//! near-clone unit is O(1) in the counts, a duplicate group O(k) in its
//! members, and a Cohesion unit re-reduces the class mean.
//!
//! Units are ordered by points descending, then the canonical order of their
//! first dimension, then node id ([NFR-RA-06]), and capped at
//! [`WHERE_TO_START_CAP`]. A unit whose fix recovers no whole point is not
//! ranked: the signal is an integer, and ranking zeros would present the
//! tie-break order as a priority. Equality and Depth have no unique
//! counterfactual — splitting a function or a module has no single result — so
//! they never enter the ranking; Acyclicity and Modularity neither.
//!
//! # The outlier labels
//!
//! Each per-unit measure ([`OutlierMeasure::MEASURES`]) is calibrated on **this
//! member's own** production distribution and never pooled across members
//! ([BR-56]): a unit is an outlier when its value is above the Tukey upper
//! fence `Q3 + 1.5 × IQR` **and** at or above the measure's detection
//! threshold, and no label is emitted below [`OUTLIER_MIN_UNITS`] units. Both
//! constants are fixed semantics, not `rules.toml` keys ([CR-164] CRA-02).
//! Quartiles interpolate linearly between order statistics (the common
//! "type 7" definition); for integer values `4·Q` is an integer, so the fence
//! comparison runs in exact integer arithmetic and only the reported fence is
//! a division — by 8, exact in `f64`.
//!
//! Nothing here feeds the signal, the gate or `check` ([BR-61]): the inputs are
//! read, the computed [`MetricSnapshot`] is read, and the result is persisted
//! beside it.
//!
//! [FR-QM-17]: ../../../docs/specs/requirements/FR-QM-17.md
//! [CR-164]: ../../../docs/requests/CR-164-insight-layer-ranks-what-to-fix-first.md
//! [BR-56]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [BR-61]: ../../../docs/specs/software-spec.md#311-quality-metrics
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::{HashMap, HashSet};

use crate::graph_store::{EdgeRow, FunctionMetricRow, NodeRow};
use crate::model::NodeId;
use crate::models::quality::{
    FixUnit, FixUnitKind, MetricSnapshot, OutlierLabel, OutlierMeasure, WhereToStart,
};

use super::extended::{self, ContainerFacts, ContainerIndex, Thresholds, LOW_COHESION_LCOM4};
use super::{
    clone_groups_by_mass, is_redundant, redundancy_of, rounded_mean, signal_of, CloneGroupMass,
    WORST_OFFENDER_CAP,
};

/// The most fix units the ranking keeps ([FR-QM-17]).
///
/// [FR-QM-17]: ../../../docs/specs/requirements/FR-QM-17.md
pub const WHERE_TO_START_CAP: usize = 10;

/// The fewest units a measure's distribution needs before it is calibrated at
/// all; below it no outlier label is emitted ([FR-QM-17]). A fixed constant of
/// the semantics, not a `rules.toml` key.
///
/// [FR-QM-17]: ../../../docs/specs/requirements/FR-QM-17.md
pub const OUTLIER_MIN_UNITS: usize = 8;

/// Rank the fix units and label the outliers of one member's snapshot inputs
/// — pure, no I/O ([FR-QM-17]).
///
/// `nodes`, `edges`, `functions`, `test_ids`, `thresholds` and `member_access`
/// are the inputs [`compute`](super::compute) scored, and `current` is what it computed from
/// them; `duplicate_groups` are the store's exact-duplicate groups
/// ([`GraphStore::duplicate_groups`](crate::graph_store::GraphStore::duplicate_groups)),
/// test members included — they are scoped here. Always
/// [`recorded`](WhereToStart::recorded). An empty production graph (no signal)
/// ranks no unit; its labels are still calibrated.
///
/// [FR-QM-17]: ../../../docs/specs/requirements/FR-QM-17.md
// The inputs `compute` scored, one by one, so the ranking can never read a
// different set than the metrics did; bundling them would only rename that list.
#[allow(clippy::too_many_arguments)]
pub fn where_to_start(
    nodes: &[NodeRow],
    edges: &[EdgeRow],
    functions: &[FunctionMetricRow],
    test_ids: &HashSet<NodeId>,
    duplicate_groups: &[Vec<NodeId>],
    thresholds: Thresholds,
    member_access: &HashSet<String>,
    current: &MetricSnapshot,
) -> WhereToStart {
    let inputs = Inputs::new(nodes, edges, functions, test_ids, duplicate_groups, member_access);
    let outliers = outlier_measures(&inputs, &thresholds);
    let labelled: HashSet<(&str, NodeId)> = outliers
        .iter()
        .flat_map(|m| m.labelled.iter().map(move |&(id, _)| (m.measure, id)))
        .collect();
    let mut units = match current.aggregate_signal {
        Some(signal) => rank(&inputs, &thresholds, current, i64::from(signal), &labelled),
        None => Vec::new(),
    };
    units.truncate(WHERE_TO_START_CAP);
    WhereToStart {
        units,
        outliers: outliers
            .into_iter()
            .map(|m: Calibrated| m.into_measure(&inputs))
            .collect(),
        ..WhereToStart::recorded()
    }
}

/// The inputs both halves read, scoped to production once.
struct Inputs<'a> {
    by_id: HashMap<NodeId, &'a NodeRow>,
    /// Production functions, id ascending (the `function_metrics` order).
    production: Vec<&'a FunctionMetricRow>,
    /// Every class-like container's facts, in the order Cohesion reduces in.
    containers: Vec<ContainerFacts>,
    /// Exact-duplicate groups' production members whose scored row carries the
    /// duplicate verdict (id ascending), groups with none dropped, ordered by
    /// lowest member.
    duplicates: Vec<Vec<&'a FunctionMetricRow>>,
}

impl<'a> Inputs<'a> {
    fn new(
        nodes: &'a [NodeRow],
        edges: &[EdgeRow],
        functions: &'a [FunctionMetricRow],
        test_ids: &HashSet<NodeId>,
        duplicate_groups: &[Vec<NodeId>],
        member_access: &HashSet<String>,
    ) -> Self {
        let production: Vec<&FunctionMetricRow> = functions
            .iter()
            .filter(|f| !test_ids.contains(&f.id))
            .collect();
        let row_of: HashMap<NodeId, &FunctionMetricRow> =
            production.iter().map(|f| (f.id, *f)).collect();
        let mut duplicates: Vec<Vec<&FunctionMetricRow>> = duplicate_groups
            .iter()
            .map(|group| {
                // Only a member the scored rows count as duplicate: the groups
                // and the rows are separate statements of one read, so an
                // annotation committed between them could otherwise hand the
                // counterfactual more copies to clear than Redundancy counted.
                let mut members: Vec<&FunctionMetricRow> = group
                    .iter()
                    .filter_map(|id| row_of.get(id).copied())
                    .filter(|f| f.is_duplicate == Some(true))
                    .collect();
                members.sort_by_key(|f| f.id);
                members
            })
            .filter(|members| !members.is_empty())
            .collect();
        duplicates.sort_by_key(|members| members[0].id);
        Self {
            by_id: nodes.iter().map(|n| (n.id, n)).collect(),
            containers: ContainerIndex::build(nodes, edges, functions, test_ids).facts(member_access),
            production,
            duplicates,
        }
    }

    /// The name/file/line a unit or label reports for `id`, or `None` for an
    /// id absent from the node set (never expected — dropped, not fabricated).
    fn named(&self, id: NodeId) -> Option<(String, String, Option<i64>)> {
        self.by_id.get(&id).map(|n| {
            (
                n.name.clone(),
                n.file_path.clone().unwrap_or_default(),
                n.start_line,
            )
        })
    }
}

// ── The ranking ──────────────────────────────────────────────────────────────

/// One offending unit before it is named: what it is, what removing it changes
/// in the snapshot, and how it reads.
struct Candidate {
    kind: FixUnitKind,
    id: NodeId,
    dimensions: Vec<&'static str>,
    detail: String,
    /// The measures this unit is looked up under for its outlier labels.
    measures: &'static [&'static str],
    /// The snapshot with this unit's contribution removed.
    counterfactual: MetricSnapshot,
}

/// Every offending unit stated in points, in rank order — uncapped; the caller
/// keeps the head.
fn rank(
    inputs: &Inputs<'_>,
    t: &Thresholds,
    current: &MetricSnapshot,
    signal: i64,
    labelled: &HashSet<(&str, NodeId)>,
) -> Vec<FixUnit> {
    let mut candidates = Vec::new();
    function_units(inputs, t, current, &mut candidates);
    class_units(inputs, t, current, &mut candidates);
    duplicate_group_units(inputs, current, &mut candidates);
    clone_group_units(inputs, current, &mut candidates);

    let mut ranked: Vec<(i64, Candidate)> = candidates
        .into_iter()
        .filter_map(|c| {
            let points = i64::from(signal_of(&c.counterfactual)?) - signal;
            (points >= 1).then_some((points, c))
        })
        .collect();
    let first_dimension = |c: &Candidate| {
        c.dimensions
            .first()
            .and_then(|d| FixUnit::DIMENSIONS.iter().position(|x| x == d))
            .unwrap_or(FixUnit::DIMENSIONS.len())
    };
    ranked.sort_by(|(ap, a), (bp, b)| {
        bp.cmp(ap)
            .then(first_dimension(a).cmp(&first_dimension(b)))
            .then(a.id.cmp(&b.id))
            .then(a.kind.as_str().cmp(b.kind.as_str()))
    });
    ranked
        .into_iter()
        .filter_map(|(points, c)| {
            let (name, file, line) = inputs.named(c.id)?;
            let outliers = OutlierMeasure::MEASURES
                .iter()
                .copied()
                .filter(|m| c.measures.contains(m) && labelled.contains(&(*m, c.id)))
                .collect();
            Some(FixUnit {
                kind: c.kind,
                node_id: c.id.0,
                name,
                file,
                line,
                dimensions: c.dimensions,
                points,
                detail: c.detail,
                outliers,
            })
        })
        .collect()
}

/// Function units: each deeply-nested function or brain method, which stops
/// counting as either.
fn function_units(
    inputs: &Inputs<'_>,
    t: &Thresholds,
    current: &MetricSnapshot,
    out: &mut Vec<Candidate>,
) {
    let total = inputs.production.len();
    let deep = inputs.production.iter().filter(|f| extended::is_deep(f, t)).count();
    let brain = inputs.production.iter().filter(|f| extended::is_brain(f, t)).count();
    for f in &inputs.production {
        let (is_deep, is_brain) = (extended::is_deep(f, t), extended::is_brain(f, t));
        if !is_deep && !is_brain {
            continue;
        }
        let mut counterfactual = current.clone();
        let mut dimensions = Vec::new();
        if is_deep {
            counterfactual.nesting = extended::from_ratio(deep - 1, total);
            dimensions.push("nesting");
        }
        if is_brain {
            counterfactual.conciseness = extended::from_ratio(brain - 1, total);
            dimensions.push("conciseness");
        }
        // The Worst-offender descriptors, so a unit reads like its list row.
        let detail = match (f.cyclomatic_complexity, f.line_count, f.max_nesting_depth) {
            (Some(cc), Some(loc), Some(nest)) if is_brain => {
                format!("CC {cc} · LOC {loc} · nesting {nest}")
            }
            (_, _, Some(nest)) => format!("nesting depth {nest}"),
            _ => String::new(),
        };
        out.push(Candidate {
            kind: FixUnitKind::Function,
            id: f.id,
            dimensions,
            detail,
            measures: &["cc", "nesting"],
            counterfactual,
        });
    }
}

/// Class units: each low-cohesion class (its `1/LCOM4` becomes 1) or god
/// container (it stops counting as god).
fn class_units(
    inputs: &Inputs<'_>,
    t: &Thresholds,
    current: &MetricSnapshot,
    out: &mut Vec<Candidate>,
) {
    let containers = &inputs.containers;
    let god = containers
        .iter()
        .filter(|c| extended::is_god(c.methods, c.span, t))
        .count();
    for c in containers {
        let low_cohesion = c.lcom4.is_some_and(|l| l >= LOW_COHESION_LCOM4);
        let is_god = extended::is_god(c.methods, c.span, t);
        if !low_cohesion && !is_god {
            continue;
        }
        let mut counterfactual = current.clone();
        let mut dimensions = Vec::new();
        let mut detail = Vec::new();
        if let (true, Some(lcom4)) = (low_cohesion, c.lcom4) {
            // The same reduction Cohesion runs, in the same container order,
            // with this class's term set to 1/1.
            counterfactual.cohesion = extended::cohesion_of(
                containers
                    .iter()
                    .filter_map(|o| o.lcom4.map(|l| if o.id == c.id { 1 } else { l })),
            );
            dimensions.push("cohesion");
            detail.push(format!("LCOM4 {lcom4}"));
        }
        if is_god {
            counterfactual.focus = Some(extended::from_ratio(god - 1, containers.len()));
            dimensions.push("focus");
            detail.push(format!("{} methods · span {}", c.methods, c.span));
        }
        out.push(Candidate {
            kind: FixUnitKind::Class,
            id: c.id,
            dimensions,
            detail: detail.join(" · "),
            measures: &["lcom4", "methods", "span"],
            counterfactual,
        });
    }
}

/// Exact-duplicate group units: a group of k lowers the redundant count by
/// `k − 1` — every copy but one loses its duplicate verdict. The survivor is a
/// dead member when there is one (it stays redundant either way), so the
/// figure never depends on which member has the lowest id; a dead member that
/// does not survive stays redundant too.
fn duplicate_group_units(inputs: &Inputs<'_>, current: &MetricSnapshot, out: &mut Vec<Candidate>) {
    let total = inputs.production.len();
    let redundant = inputs.production.iter().filter(|f| is_redundant(f)).count();
    for members in &inputs.duplicates {
        // The live members leave the redundant count — all of them when a dead
        // member is the survivor, all but the survivor when none is dead.
        let live = members.iter().filter(|f| f.is_dead != Some(true)).count();
        let cleared = if live == members.len() { live - 1 } else { live };
        let mut counterfactual = current.clone();
        counterfactual.redundancy = redundancy_of(redundant - cleared, total);
        out.push(Candidate {
            kind: FixUnitKind::DuplicateGroup,
            id: members[0].id,
            dimensions: vec!["redundancy"],
            detail: group_detail("duplicate group", members),
            measures: &["duplicate_mass"],
            counterfactual,
        });
    }
}

/// Near-clone group units: the group's production members leave the
/// near-cloned count.
fn clone_group_units(inputs: &Inputs<'_>, current: &MetricSnapshot, out: &mut Vec<Candidate>) {
    let total = inputs.production.len();
    let cloned = inputs
        .production
        .iter()
        .filter(|f| f.clone_group.is_some())
        .count();
    for g in clone_groups_by_mass(&inputs.production) {
        let mut counterfactual = current.clone();
        counterfactual.uniqueness = extended::from_ratio(cloned - g.members.len(), total);
        out.push(Candidate {
            kind: FixUnitKind::CloneGroup,
            id: g.members[0],
            dimensions: vec!["uniqueness"],
            detail: g.detail(),
            measures: &["clone_mass"],
            counterfactual,
        });
    }
}

/// `"<what> · N members × L lines"`, `L` the mean recorded line count rounded
/// half up — the clone-group descriptor's shape; the members alone when no line
/// count is recorded, never a fabricated `0 lines`.
fn group_detail(what: &str, members: &[&FunctionMetricRow]) -> String {
    let n = members.len();
    match mean_lines(members) {
        Some(mean) => format!("{what} · {n} members × {mean} lines"),
        None => format!("{what} · {n} members"),
    }
}

/// The members' mean recorded line count, rounded half up
/// ([`rounded_mean`](super::rounded_mean)); `None` when none is recorded.
fn mean_lines(members: &[&FunctionMetricRow]) -> Option<i64> {
    let lines: Vec<i64> = members.iter().filter_map(|f| f.line_count).collect();
    rounded_mean(lines.iter().sum(), lines.len() as i64)
}

// ── The outlier labels ───────────────────────────────────────────────────────

/// One measure's calibration over the member's distribution, before naming.
struct Calibrated {
    measure: &'static str,
    units: usize,
    fence_eighths: Option<i128>,
    threshold: Option<i64>,
    /// Every labelled unit's `(id, value)`, value descending then id ascending.
    labelled: Vec<(NodeId, i64)>,
}

impl Calibrated {
    fn into_measure(self, inputs: &Inputs<'_>) -> OutlierMeasure {
        let labels = self
            .labelled
            .iter()
            .filter_map(|&(id, value)| {
                let (name, file, line) = inputs.named(id)?;
                Some(OutlierLabel {
                    node_id: id.0,
                    name,
                    file,
                    line,
                    value,
                })
            })
            .take(WORST_OFFENDER_CAP)
            .collect();
        OutlierMeasure {
            measure: self.measure,
            units: self.units as u64,
            // A multiple of 1/8 well inside f64's exact integer range.
            fence: self.fence_eighths.map(|f| f as f64 / 8.0),
            threshold: self.threshold,
            labelled: self.labelled.len() as u64,
            labels,
        }
    }
}

/// One measure's distribution before calibration: its name, every unit's
/// `(id, value)`, and its detection threshold (`None` for a group mass).
type Distribution = (&'static str, Vec<(NodeId, i64)>, Option<i64>);

/// Calibrate every measure of [`OutlierMeasure::MEASURES`], in that order.
fn outlier_measures(inputs: &Inputs<'_>, t: &Thresholds) -> Vec<Calibrated> {
    let containers = &inputs.containers;
    let production = &inputs.production;
    let duplicate_masses: Vec<(NodeId, i64)> = inputs
        .duplicates
        .iter()
        .filter_map(|members| Some((members[0].id, members.len() as i64 * mean_lines(members)?)))
        .collect();
    let clone_masses: Vec<(NodeId, i64)> = clone_groups_by_mass(production)
        .into_iter()
        .filter_map(|g: CloneGroupMass| {
            Some((g.members[0], g.members.len() as i64 * g.mean_lines()?))
        })
        .collect();

    let distributions: [Distribution; 7] = [
        (
            "lcom4",
            containers
                .iter()
                .filter_map(|c| Some((c.id, c.lcom4? as i64)))
                .collect(),
            Some(LOW_COHESION_LCOM4 as i64),
        ),
        (
            "methods",
            containers.iter().map(|c| (c.id, c.methods as i64)).collect(),
            Some(t.god_methods),
        ),
        (
            "span",
            // A span of 0 is "unrecorded", never a measured zero.
            containers
                .iter()
                .filter(|c| c.span > 0)
                .map(|c| (c.id, c.span))
                .collect(),
            Some(t.god_span),
        ),
        (
            "cc",
            production
                .iter()
                .filter_map(|f| Some((f.id, f.cyclomatic_complexity?)))
                .collect(),
            Some(t.brain_cc),
        ),
        (
            "nesting",
            production
                .iter()
                .filter_map(|f| Some((f.id, f.max_nesting_depth?)))
                .collect(),
            Some(t.nest),
        ),
        ("duplicate_mass", duplicate_masses, None),
        ("clone_mass", clone_masses, None),
    ];
    debug_assert!(
        distributions
            .iter()
            .map(|d| d.0)
            .eq(OutlierMeasure::MEASURES),
        "measures calibrate in their pinned order"
    );
    distributions
        .into_iter()
        .map(|(measure, values, threshold)| calibrate(measure, values, threshold))
        .collect()
}

/// Calibrate one measure on its distribution: the Tukey fence, then every
/// unit above it and at or above `threshold` (no threshold for a group mass).
fn calibrate(
    measure: &'static str,
    values: Vec<(NodeId, i64)>,
    threshold: Option<i64>,
) -> Calibrated {
    let mut sorted: Vec<i64> = values.iter().map(|&(_, v)| v).collect();
    sorted.sort_unstable();
    let fence_eighths = upper_fence_eighths(&sorted);
    let mut labelled: Vec<(NodeId, i64)> = match fence_eighths {
        Some(fence) => values
            .iter()
            .copied()
            .filter(|&(_, v)| 8 * i128::from(v) > fence && threshold.is_none_or(|t| v >= t))
            .collect(),
        None => Vec::new(),
    };
    labelled.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Calibrated {
        measure,
        units: sorted.len(),
        fence_eighths,
        threshold,
        labelled,
    }
}

/// `8 × (Q3 + 1.5 × IQR)` over an ascending distribution, exactly, or `None`
/// below [`OUTLIER_MIN_UNITS`] values.
///
/// With linear interpolation `Q(p) = x[⌊h⌋] + (h − ⌊h⌋)(x[⌊h⌋+1] − x[⌊h⌋])`,
/// `h = (n − 1)p`, each of `4·Q1` and `4·Q3` is an integer, and
/// `8·(Q3 + 3/2·(Q3 − Q1)) = 5·(4·Q3) − 3·(4·Q1)`.
fn upper_fence_eighths(sorted: &[i64]) -> Option<i128> {
    if sorted.len() < OUTLIER_MIN_UNITS {
        return None;
    }
    let q1 = quartile_quarters(sorted, 1);
    let q3 = quartile_quarters(sorted, 3);
    Some(5 * q3 - 3 * q1)
}

/// `4 × Q(k/4)` of an ascending, non-empty distribution, exactly.
fn quartile_quarters(sorted: &[i64], k: usize) -> i128 {
    let h4 = k * (sorted.len() - 1); // the position h, in quarters
    let (i, r) = (h4 / 4, (h4 % 4) as i128);
    let low = i128::from(sorted[i]);
    if r == 0 {
        4 * low
    } else {
        4 * low + r * (i128::from(sorted[i + 1]) - low)
    }
}

#[cfg(test)]
#[path = "insight_tests.rs"]
mod tests;
