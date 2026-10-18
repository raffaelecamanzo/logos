//! Unit tests for the "where to start" ranking and the outlier labels (S-504,
//! [FR-QM-17]) — pure, over hand-built snapshots fed through the real
//! `ExcludeContains` view into [`compute`] and [`where_to_start`].
//!
//! The load-bearing proof is per unit kind: the points a unit states equal a
//! **re-run `compute`** over inputs changed so that unit no longer offends,
//! minus the current signal — not the counterfactual's own arithmetic checked
//! against itself.
//!
//! [FR-QM-17]: ../../../docs/specs/requirements/FR-QM-17.md

use std::collections::HashSet;

use crate::graph_store::{EdgeRow, FunctionMetricRow, NodeRow};
use crate::hydrate::{build_view, Granularity};
use crate::model::{EdgeKind, LogosSymbol, NodeId, NodeKind};
use crate::models::quality::{FixUnit, FixUnitKind, MetricSnapshot, WhereToStart};

use super::super::{compute, Thresholds};
use super::{
    rank, upper_fence_eighths, where_to_start, Inputs, OUTLIER_MIN_UNITS,
    WHERE_TO_START_CAP,
};

// ── fixture builders ─────────────────────────────────────────────────────────

fn node(id: i64, name: &str, kind: NodeKind, file: &str, lines: Option<(i64, i64)>) -> NodeRow {
    NodeRow {
        id: NodeId(id),
        symbol: LogosSymbol::parse(&format!("local {id}")).expect("local symbol parses"),
        kind,
        name: name.to_string(),
        file_path: Some(file.to_string()),
        start_line: lines.map(|l| l.0),
        end_line: lines.map(|l| l.1),
    }
}

fn edge(source: i64, target: i64, kind: EdgeKind) -> EdgeRow {
    EdgeRow {
        source: NodeId(source),
        target: NodeId(target),
        kind,
    }
}

/// A plain production function row: CC 2, 10 lines, nesting 1, no verdicts.
fn row(id: i64) -> FunctionMetricRow {
    FunctionMetricRow {
        id: NodeId(id),
        cyclomatic_complexity: Some(2),
        is_dead: Some(false),
        is_duplicate: Some(false),
        line_count: Some(10),
        max_nesting_depth: Some(1),
        clone_group: None,
        has_body: Some(true),
    }
}

/// The `member_access` set the ranking runs under: the fixtures' classes live
/// in `.rs` files, so the set declares `rs` — the engine is pure over the set it
/// is handed, and the ranking's subject is the counterfactual, not the
/// capability gate (pinned in `metrics/tests.rs`).
fn member_access() -> HashSet<String> {
    HashSet::from(["rs".to_string()])
}

/// One member's snapshot inputs.
#[derive(Clone)]
struct Member {
    nodes: Vec<NodeRow>,
    edges: Vec<EdgeRow>,
    functions: Vec<FunctionMetricRow>,
    test_ids: HashSet<NodeId>,
    duplicate_groups: Vec<Vec<NodeId>>,
}

impl Member {
    fn compute(&self) -> MetricSnapshot {
        let view = build_view(Granularity::ExcludeContains, &self.nodes, &self.edges);
        compute(
            &view,
            &self.nodes,
            &self.edges,
            &self.functions,
            &self.test_ids,
            Thresholds::default(),
            &member_access(),
        )
    }

    fn rank(&self) -> WhereToStart {
        where_to_start(
            &self.nodes,
            &self.edges,
            &self.functions,
            &self.test_ids,
            &self.duplicate_groups,
            Thresholds::default(),
            &member_access(),
            &self.compute(),
        )
    }

    fn signal(&self) -> i64 {
        i64::from(self.compute().aggregate_signal.expect("the fixture scores a signal"))
    }

    fn function_mut(&mut self, id: i64) -> &mut FunctionMetricRow {
        self.functions
            .iter_mut()
            .find(|f| f.id == NodeId(id))
            .expect("the function exists")
    }
}

/// A member with one unit of every kind offending:
///
/// - functions 1..=12 in `src/a/x.rs`; f1 nested 6 deep, f2 a brain method
///   (CC 20, 120 lines, nesting 5 — also deep), f3/f4/f5 an exact-duplicate
///   group of 30-line functions with test function 12 in it too, f6/f7 a
///   near-clone group of 40-line functions;
/// - class `Split` (20) with two methods sharing nothing (LCOM4 2), class
///   `Whole` (30) with one method, and class `Huge` (40) spanning 600 lines
///   (god by span).
fn mixed() -> Member {
    let mut nodes: Vec<NodeRow> = (1..=12)
        .map(|i| node(i, &format!("f{i:02}"), NodeKind::Function, "src/a/x.rs", Some((i * 10, i * 10 + 9))))
        .collect();
    nodes.extend([
        node(20, "Split", NodeKind::Class, "src/b/c.rs", Some((1, 50))),
        node(21, "left", NodeKind::Method, "src/b/c.rs", Some((2, 10))),
        node(22, "right", NodeKind::Method, "src/b/c.rs", Some((11, 20))),
        node(23, "state", NodeKind::Field, "src/b/c.rs", Some((21, 21))),
        node(30, "Whole", NodeKind::Class, "src/b/w.rs", Some((1, 20))),
        node(31, "only", NodeKind::Method, "src/b/w.rs", Some((2, 10))),
        node(40, "Huge", NodeKind::Class, "src/b/h.rs", Some((1, 600))),
    ]);
    let edges = vec![
        edge(20, 21, EdgeKind::Contains),
        edge(20, 22, EdgeKind::Contains),
        edge(20, 23, EdgeKind::Contains),
        edge(30, 31, EdgeKind::Contains),
    ];
    let mut functions: Vec<FunctionMetricRow> = (1..=12).chain([21, 22, 31]).map(row).collect();
    let mut member = Member {
        nodes,
        edges,
        functions: Vec::new(),
        test_ids: [NodeId(12)].into_iter().collect(),
        duplicate_groups: vec![vec![NodeId(3), NodeId(4), NodeId(5), NodeId(12)]],
    };
    for f in &mut functions {
        match f.id.0 {
            1 => f.max_nesting_depth = Some(6),
            2 => {
                f.cyclomatic_complexity = Some(20);
                f.line_count = Some(120);
                f.max_nesting_depth = Some(5);
            }
            3 | 4 | 5 | 12 => {
                f.is_duplicate = Some(true);
                f.line_count = Some(30);
            }
            6 | 7 => {
                f.clone_group = Some(NodeId(6));
                f.line_count = Some(40);
            }
            _ => {}
        }
    }
    member.functions = functions;
    member
}

fn unit(ranking: &WhereToStart, kind: FixUnitKind, id: i64) -> &FixUnit {
    ranking
        .units
        .iter()
        .find(|u| u.kind == kind && u.node_id == id)
        .unwrap_or_else(|| panic!("a {kind:?} unit for node {id} is ranked: {:#?}", ranking.units))
}

/// Assert a unit's stated points equal the re-run signal minus the current one.
fn assert_points(stated: &FixUnit, current: &Member, fixed: &Member) {
    let recovered = fixed.signal() - current.signal();
    assert!(recovered >= 1, "the fixture moves the signal for {}: {recovered}", stated.name);
    assert_eq!(
        stated.points, recovered,
        "{} states {} points; a re-run compute with it removed recovers {recovered}",
        stated.name, stated.points
    );
}

// ── FR-QM-17 AC 1: each unit kind's points equal a re-run compute ────────────

/// A function unit: with its nesting depth no longer recorded it is neither
/// deep nor a brain method. f2 is both, so it carries both dimensions.
#[test]
fn a_function_units_points_equal_a_rerun_compute_with_it_no_longer_offending() {
    let current = mixed();
    let ranking = current.rank();
    for id in [1, 2] {
        let mut fixed = current.clone();
        fixed.function_mut(id).max_nesting_depth = None;
        assert_points(unit(&ranking, FixUnitKind::Function, id), &current, &fixed);
    }
    assert_eq!(unit(&ranking, FixUnitKind::Function, 1).dimensions, ["nesting"]);
    assert_eq!(
        unit(&ranking, FixUnitKind::Function, 2).dimensions,
        ["nesting", "conciseness"]
    );
    assert_eq!(
        unit(&ranking, FixUnitKind::Function, 2).detail,
        "CC 20 · LOC 120 · nesting 5"
    );
}

/// The ranking gates Cohesion as the dimension does ([CR-171]): with no
/// language declaring `member_access`, `Split` (LCOM4 2) is never ranked for
/// Cohesion — a fix the signal cannot see is no place to start — while the
/// god-by-span `Huge` is still ranked for Focus, which needs no field access.
///
/// [CR-171]: ../../../docs/requests/CR-171-the-quality-signal-and-test-classification-stay-honest-on-a-thin-graph.md
#[test]
fn a_class_of_an_undeclared_language_is_never_ranked_for_cohesion() {
    let member = mixed();
    let undeclared = HashSet::new();
    let view = build_view(Granularity::ExcludeContains, &member.nodes, &member.edges);
    let current = compute(
        &view,
        &member.nodes,
        &member.edges,
        &member.functions,
        &member.test_ids,
        Thresholds::default(),
        &undeclared,
    );
    assert!(current.cohesion.is_none(), "no class is scoreable, so the dimension drops out");
    let ranking = where_to_start(
        &member.nodes,
        &member.edges,
        &member.functions,
        &member.test_ids,
        &member.duplicate_groups,
        Thresholds::default(),
        &undeclared,
        &current,
    );
    assert!(
        ranking.units.iter().all(|u| !u.dimensions.contains(&"cohesion")),
        "no unit is ranked for Cohesion: {:?}",
        ranking.units
    );
    assert_eq!(unit(&ranking, FixUnitKind::Class, 40).dimensions, ["focus"], "Focus is not gated");
    // The declared run, for contrast, does rank Split for Cohesion.
    assert_eq!(unit(&member.rank(), FixUnitKind::Class, 20).dimensions, ["cohesion"]);
}

/// A class unit, both ways a class offends: `Split`'s two methods made to share
/// a field (LCOM4 1, its `1/LCOM4` becomes 1), and `Huge` cut below the span
/// threshold (no longer god).
#[test]
fn a_class_units_points_equal_a_rerun_compute_with_it_no_longer_offending() {
    let current = mixed();
    let ranking = current.rank();

    let mut cohesive = current.clone();
    cohesive.edges.extend([edge(21, 23, EdgeKind::Accesses), edge(22, 23, EdgeKind::Accesses)]);
    let split = unit(&ranking, FixUnitKind::Class, 20);
    assert_points(split, &current, &cohesive);
    assert_eq!((split.dimensions.as_slice(), split.detail.as_str()), (&["cohesion"][..], "LCOM4 2"));

    let mut focused = current.clone();
    focused
        .nodes
        .iter_mut()
        .find(|n| n.id == NodeId(40))
        .unwrap()
        .end_line = Some(100);
    let huge = unit(&ranking, FixUnitKind::Class, 40);
    assert_points(huge, &current, &focused);
    assert_eq!(huge.dimensions, ["focus"]);
}

/// An exact-duplicate group of k lowers the redundant count by k − 1: the
/// lowest member keeps its verdict, the others lose theirs. The test member
/// (12) is outside the production scope, so the group's k is 3, not 4.
#[test]
fn a_duplicate_groups_points_equal_a_rerun_compute_with_k_minus_one_copies_gone() {
    let current = mixed();
    let ranking = current.rank();
    let mut fixed = current.clone();
    for id in [4, 5] {
        fixed.function_mut(id).is_duplicate = Some(false);
    }
    let group = unit(&ranking, FixUnitKind::DuplicateGroup, 3);
    assert_points(group, &current, &fixed);
    assert_eq!(group.dimensions, ["redundancy"]);
    assert_eq!(group.detail, "duplicate group · 3 members × 30 lines");
}

/// A dead member is redundant whatever the fix, so it is the copy that
/// survives: the two live members leave the redundant count (k − 1 = 2) — and
/// the figure is the same whichever member is the dead one, never a function
/// of id order. The stated points equal the re-run each way.
#[test]
fn a_dead_duplicate_survives_so_the_figure_never_depends_on_id_order() {
    let mut figures = Vec::new();
    for dead in [5, 3] {
        let mut current = mixed();
        current.function_mut(dead).is_dead = Some(true);
        let ranking = current.rank();
        let mut fixed = current.clone();
        for id in [3, 4, 5].into_iter().filter(|&id| id != dead) {
            fixed.function_mut(id).is_duplicate = Some(false);
        }
        let group = unit(&ranking, FixUnitKind::DuplicateGroup, 3);
        assert_points(group, &current, &fixed);
        figures.push(group.points);
    }
    assert_eq!(figures[0], figures[1], "the dead member's id changes nothing");
}

/// A near-clone group's members leave the near-cloned count.
#[test]
fn a_clone_groups_points_equal_a_rerun_compute_with_its_members_unclustered() {
    let current = mixed();
    let ranking = current.rank();
    let mut fixed = current.clone();
    for id in [6, 7] {
        fixed.function_mut(id).clone_group = None;
    }
    let group = unit(&ranking, FixUnitKind::CloneGroup, 6);
    assert_points(group, &current, &fixed);
    assert_eq!(group.dimensions, ["uniqueness"]);
    assert_eq!(group.detail, "clone group #6 · 2 members × 40 lines");
}

/// A duplicate group the scored rows do not bear out — the groups and the rows
/// are separate statements, and an annotation can commit between them — never
/// clears more copies than Redundancy counted: its members without the
/// duplicate verdict are left out, and a group left with one member recovers
/// nothing. (The unchecked count once underflowed here.)
#[test]
fn a_group_the_scored_rows_do_not_bear_out_is_never_cleared() {
    let mut member = mixed();
    member.duplicate_groups.push(vec![NodeId(8), NodeId(9), NodeId(10)]);
    member.function_mut(10).is_duplicate = Some(true); // one of three bears it out
    let ranking = member.rank();
    let groups: Vec<i64> = ranking
        .units
        .iter()
        .filter(|u| u.kind == FixUnitKind::DuplicateGroup)
        .map(|u| u.node_id)
        .collect();
    assert_eq!(groups, [3], "the group of one recovers nothing: {:#?}", ranking.units);
    let mut fixed = member.clone();
    for id in [4, 5] {
        fixed.function_mut(id).is_duplicate = Some(false);
    }
    assert_points(unit(&ranking, FixUnitKind::DuplicateGroup, 3), &member, &fixed);
}

/// Equality and Depth (and Acyclicity, Modularity) never carry points: a
/// member whose Equality and Depth both offend — one function far above the
/// mean complexity, a three-directory chain — ranks no entry under them.
#[test]
fn equality_and_depth_never_enter_the_ranking() {
    let mut member = mixed();
    member.nodes.extend([
        node(50, "top", NodeKind::Function, "src/c/t.rs", None),
        node(51, "mid", NodeKind::Function, "src/d/m.rs", None),
    ]);
    member.edges.extend([edge(50, 51, EdgeKind::Calls), edge(51, 1, EdgeKind::Calls)]);
    member.functions.extend([row(50), row(51)]);
    member.function_mut(50).cyclomatic_complexity = Some(60);
    let metrics = member.compute();
    assert!(metrics.depth.raw >= 3.0, "the fixture's Depth offends: {}", metrics.depth.raw);
    assert!(metrics.equality.raw > 0.0, "the fixture's Equality offends");

    let ranking = member.rank();
    assert!(!ranking.units.is_empty());
    for u in &ranking.units {
        for d in &u.dimensions {
            assert!(
                !["equality", "depth", "acyclicity", "modularity"].contains(d),
                "{} carries points under {d}",
                u.name
            );
        }
    }
}

// ── FR-QM-17 AC 2: ordering and byte-identity ────────────────────────────────

/// Points descending, then the first dimension's canonical order, then node
/// id — and two identical units tie on points and dimension, so the lower id
/// comes first.
#[test]
fn units_order_by_points_then_dimension_then_node_id() {
    let mut member = mixed();
    member.function_mut(8).max_nesting_depth = Some(6); // f8 now ties f1 exactly
    let ranking = member.rank();
    let key = |u: &FixUnit| {
        (
            -u.points,
            FixUnit::DIMENSIONS.iter().position(|d| *d == u.dimensions[0]).unwrap(),
            u.node_id,
        )
    };
    let keys: Vec<_> = ranking.units.iter().map(key).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "the ranking is in key order");

    let f1 = ranking.units.iter().position(|u| u.node_id == 1).unwrap();
    let f8 = ranking.units.iter().position(|u| u.node_id == 8).unwrap();
    assert_eq!(ranking.units[f1].points, ranking.units[f8].points, "the two tie");
    assert!(f1 < f8, "on a tie the lower node id comes first");
}

/// The cap: a member with more offending units than [`WHERE_TO_START_CAP`]
/// keeps exactly the cap, the highest-points ones.
#[test]
fn the_ranking_is_capped_keeping_the_highest_points() {
    let mut member = mixed();
    for id in 3..=11 {
        member.function_mut(id).max_nesting_depth = Some(5);
    }
    // Eleven function units, two class units and two group units: fifteen.
    let current = member.compute();
    let inputs = Inputs::new(
        &member.nodes,
        &member.edges,
        &member.functions,
        &member.test_ids,
        &member.duplicate_groups,
        &member_access(),
    );
    let all = rank(
        &inputs,
        &Thresholds::default(),
        &current,
        i64::from(current.aggregate_signal.unwrap()),
        &HashSet::new(),
    );
    assert_eq!(all.len(), 15, "every offending unit recovers a point here");
    let ranking = member.rank();
    assert_eq!(ranking.units.len(), WHERE_TO_START_CAP);
    let head: Vec<(i64, i64)> = all[..WHERE_TO_START_CAP].iter().map(|u| (u.node_id, u.points)).collect();
    let kept: Vec<(i64, i64)> = ranking.units.iter().map(|u| (u.node_id, u.points)).collect();
    assert_eq!(kept, head, "the cap keeps the head of the full order");
}

/// Runs on unchanged inputs are byte-identical, ranking and labels both.
#[test]
fn reruns_on_unchanged_inputs_are_byte_identical() {
    let member = labelled_member();
    let first = serde_json::to_string(&member.rank()).unwrap();
    let second = serde_json::to_string(&member.rank()).unwrap();
    assert_eq!(first, second);
    assert!(first.contains("\"labels\":[{"), "the fixture labels something: {first}");
}

/// A unit whose fix recovers no whole point of the integer signal is not
/// ranked — and the re-run confirms it recovers nothing.
#[test]
fn a_unit_recovering_no_whole_point_is_not_ranked() {
    let mut nodes = Vec::new();
    let mut functions = Vec::new();
    // An edgeless graph: under metric-semantics v8 the three graph dimensions
    // leave the mean (FR-QM-18), so the signal spans five dimensions and one
    // deep function needs a larger population to stay under half a point.
    for i in 1..=8000 {
        nodes.push(node(i, &format!("g{i}"), NodeKind::Function, "src/x.rs", None));
        functions.push(row(i));
    }
    functions[0].max_nesting_depth = Some(5);
    let member = Member {
        nodes,
        edges: Vec::new(),
        functions,
        test_ids: HashSet::new(),
        duplicate_groups: Vec::new(),
    };
    let mut fixed = member.clone();
    fixed.function_mut(1).max_nesting_depth = None;
    assert_eq!(fixed.signal(), member.signal(), "one deep function in 8000 moves no whole point");
    assert!(member.rank().units.is_empty());
}

/// An empty production graph has no signal, so nothing is ranked — but the
/// set is still a recorded one.
#[test]
fn no_production_graph_ranks_nothing_and_is_still_recorded() {
    let member = Member {
        nodes: Vec::new(),
        edges: Vec::new(),
        functions: Vec::new(),
        test_ids: HashSet::new(),
        duplicate_groups: Vec::new(),
    };
    let ranking = member.rank();
    assert!(ranking.recorded && ranking.units.is_empty());
    assert!(ranking.outliers.iter().all(|m| m.labels.is_empty() && m.fence.is_none()));
}

// ── FR-QM-17 AC 3: per-member Tukey labels ───────────────────────────────────

/// The fence is `Q3 + 1.5 × IQR` with linearly interpolated quartiles, exact:
/// over 1..=8, Q1 = 2.75 and Q3 = 6.25, so the fence is 11.5.
#[test]
fn the_fence_is_q3_plus_one_and_a_half_iqr_exactly() {
    let one_to_eight: Vec<i64> = (1..=8).collect();
    assert_eq!(upper_fence_eighths(&one_to_eight), Some(92), "8 × 11.5");
    // n = 9: Q1 = x[2], Q3 = x[6] exactly.
    let nine = [1, 1, 2, 3, 4, 5, 6, 9, 30];
    assert_eq!(upper_fence_eighths(&nine), Some(8 * 6 + 12 * (6 - 2)), "6 + 1.5 × 4 = 12");
    assert_eq!(upper_fence_eighths(&one_to_eight[..7]), None, "below the minimum size");
}

/// A member whose nesting distribution has one clear outlier (depth 9 among
/// depths 1–2) at or above `T_nest`: it is labelled, and the function unit it
/// belongs to carries the label.
fn labelled_member() -> Member {
    let mut member = mixed();
    member.function_mut(1).max_nesting_depth = Some(9);
    member
}

#[test]
fn an_outlier_above_the_fence_and_the_threshold_is_labelled() {
    let ranking = labelled_member().rank();
    let nesting = ranking.outliers.iter().find(|m| m.measure == "nesting").unwrap();
    assert!(nesting.units as usize >= OUTLIER_MIN_UNITS, "{} units", nesting.units);
    assert_eq!(nesting.threshold, Some(4));
    let labelled: Vec<i64> = nesting.labels.iter().map(|l| l.node_id).collect();
    assert!(labelled.contains(&1), "depth 9 is labelled: {nesting:?}");
    assert!(
        nesting.labels.iter().all(|l| l.value >= 4 && (l.value as f64) > nesting.fence.unwrap()),
        "every label is above the fence and at or above the threshold: {nesting:?}"
    );
    assert_eq!(nesting.labelled as usize, nesting.labels.len());
    assert_eq!(unit(&ranking, FixUnitKind::Function, 1).outliers, ["nesting"]);
}

/// Above the fence is not enough: a value above it but below the detection
/// threshold is never labelled — on a member whose nesting depths are all 1
/// except one 3 (< `T_nest` = 4), nothing is labelled, though 3 is far above
/// the fence of 1.
#[test]
fn a_value_above_the_fence_but_below_the_threshold_is_not_labelled() {
    let mut member = mixed();
    member.function_mut(1).max_nesting_depth = Some(3);
    member.function_mut(2).max_nesting_depth = Some(1);
    let ranking = member.rank();
    let nesting = ranking.outliers.iter().find(|m| m.measure == "nesting").unwrap();
    assert!(nesting.fence.unwrap() < 3.0, "3 is above the fence: {nesting:?}");
    assert!(nesting.labels.is_empty(), "but below T_nest, so never labelled: {nesting:?}");
}

/// Fewer than [`OUTLIER_MIN_UNITS`] units: no fence and no label, however
/// extreme the value.
#[test]
fn fewer_than_eight_units_emit_no_label() {
    let mut nodes = Vec::new();
    let mut functions = Vec::new();
    for i in 1..OUTLIER_MIN_UNITS as i64 {
        nodes.push(node(i, &format!("g{i}"), NodeKind::Function, "src/x.rs", None));
        functions.push(row(i));
    }
    functions[0].max_nesting_depth = Some(40);
    let member = Member {
        nodes,
        edges: Vec::new(),
        functions,
        test_ids: HashSet::new(),
        duplicate_groups: Vec::new(),
    };
    let ranking = member.rank();
    let nesting = ranking.outliers.iter().find(|m| m.measure == "nesting").unwrap();
    assert_eq!(nesting.units as usize, OUTLIER_MIN_UNITS - 1);
    assert_eq!(nesting.fence, None, "no fence below the minimum size");
    assert!(nesting.labels.is_empty());

    // One more unit, and the same value is labelled.
    let mut eight = member.clone();
    eight.nodes.push(node(99, "g99", NodeKind::Function, "src/x.rs", None));
    eight.functions.push(row(99));
    let nesting = eight.rank().outliers.into_iter().find(|m| m.measure == "nesting").unwrap();
    assert_eq!(nesting.labels.iter().map(|l| l.node_id).collect::<Vec<_>>(), [1]);
}

/// Calibration is per member ([BR-56]): the same depth-6 function is an
/// outlier in a member whose other functions are shallow and not in one whose
/// functions are uniformly deep — each member is judged on its own
/// distribution, and the second member's existence changes nothing about the
/// first's labels.
///
/// [BR-56]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn each_member_is_calibrated_on_its_own_distribution() {
    let shallow = mixed();
    let mut deep = mixed();
    for id in 3..=11 {
        deep.function_mut(id).max_nesting_depth = Some(6);
    }
    let labels_of = |m: &Member| -> Vec<i64> {
        m.rank()
            .outliers
            .into_iter()
            .find(|o| o.measure == "nesting")
            .unwrap()
            .labels
            .iter()
            .map(|l| l.node_id)
            .collect()
    };
    let alone = labels_of(&shallow);
    assert!(alone.contains(&1), "depth 6 stands out among shallow functions: {alone:?}");
    assert!(
        !labels_of(&deep).contains(&1),
        "depth 6 is ordinary where every function is that deep"
    );
    assert_eq!(labels_of(&shallow), alone, "the other member changed nothing here");
}

/// Every measure is reported, in the pinned order, each with its own threshold
/// (none for a group mass).
#[test]
fn every_measure_is_reported_in_its_pinned_order() {
    let ranking = mixed().rank();
    let measures: Vec<&str> = ranking.outliers.iter().map(|m| m.measure).collect();
    assert_eq!(
        measures,
        crate::models::quality::OutlierMeasure::MEASURES,
        "all seven, in order"
    );
    let t = Thresholds::default();
    let thresholds: Vec<Option<i64>> = ranking.outliers.iter().map(|m| m.threshold).collect();
    assert_eq!(
        thresholds,
        [Some(2), Some(t.god_methods), Some(t.god_span), Some(t.brain_cc), Some(t.nest), None, None]
    );
}

/// A class that is both low-cohesion and god is one unit carrying both
/// dimensions: its points equal a re-run with both offences removed — the
/// methods made to share a field **and** the span cut below `T_span`.
#[test]
fn a_class_that_is_both_low_cohesion_and_god_is_one_unit_with_both_dimensions() {
    let mut current = mixed();
    current.nodes.iter_mut().find(|n| n.id == NodeId(20)).unwrap().end_line = Some(600);
    let ranking = current.rank();
    let split = unit(&ranking, FixUnitKind::Class, 20);
    assert_eq!(split.dimensions, ["cohesion", "focus"]);
    assert_eq!(split.detail, "LCOM4 2 · 2 methods · span 600");

    let mut fixed = current.clone();
    fixed.edges.extend([edge(21, 23, EdgeKind::Accesses), edge(22, 23, EdgeKind::Accesses)]);
    fixed.nodes.iter_mut().find(|n| n.id == NodeId(20)).unwrap().end_line = Some(50);
    assert_points(split, &current, &fixed);
}

/// A member of `n` plain production functions (ids 1..=n) in one file.
fn plain_member(n: i64) -> Member {
    Member {
        nodes: (1..=n)
            .map(|i| node(i, &format!("g{i:03}"), NodeKind::Function, "src/x.rs", Some((i * 10, i * 10 + 5))))
            .collect(),
        edges: Vec::new(),
        functions: (1..=n).map(row).collect(),
        test_ids: HashSet::new(),
        duplicate_groups: Vec::new(),
    }
}

/// A group's mass is members × the mean **recorded** line count rounded half
/// up: lines 300, 301 and one unrecorded give a mean of 300.5 → 301, so the
/// mass is 3 × 301 = 903 — never a floor (900), never measured × mean (602).
/// Nine ordinary 2 × 10-line groups make the measure calibrate, and the big
/// group is its one outlier; its descriptor states the same rounded mean.
#[test]
fn a_group_mass_is_members_times_the_rounded_mean_of_recorded_lines() {
    let mut member = plain_member(21);
    for f in &mut member.functions {
        f.is_duplicate = Some(true);
    }
    member.duplicate_groups = (0..9).map(|g| vec![NodeId(2 * g + 1), NodeId(2 * g + 2)]).collect();
    member.duplicate_groups.push(vec![NodeId(19), NodeId(20), NodeId(21)]);
    member.function_mut(19).line_count = Some(300);
    member.function_mut(20).line_count = Some(301);
    member.function_mut(21).line_count = None;
    let ranking = member.rank();
    let mass = ranking.outliers.iter().find(|m| m.measure == "duplicate_mass").unwrap();
    assert_eq!(mass.units, 10);
    let labels: Vec<(i64, i64)> = mass.labels.iter().map(|l| (l.node_id, l.value)).collect();
    assert_eq!(labels, [(19, 903)], "{mass:?}");
    assert_eq!(
        unit(&ranking, FixUnitKind::DuplicateGroup, 19).detail,
        "duplicate group · 3 members × 301 lines"
    );
}

/// Twelve outliers on one measure: the persisted labels stop at
/// [`WORST_OFFENDER_CAP`](crate::metrics::WORST_OFFENDER_CAP) — the deepest
/// first — while `labelled` counts all twelve, and a ranked unit whose label the
/// cap cut (f61, the shallowest outlier, ranked first on the id tie-break) still
/// carries it: a unit's own labels never depend on the cap. Sixty shallow
/// functions keep the twelve beyond the upper quartile.
#[test]
fn labels_are_capped_but_a_ranked_unit_keeps_a_label_the_cap_cut() {
    let cap = crate::metrics::WORST_OFFENDER_CAP;
    let mut member = plain_member(72);
    for id in 61..=72 {
        member.function_mut(id).max_nesting_depth = Some(9 + (id - 61));
    }
    let ranking = member.rank();
    let nesting = ranking.outliers.iter().find(|m| m.measure == "nesting").unwrap();
    assert_eq!(nesting.labelled, 12);
    assert_eq!(nesting.labels.len(), cap);
    assert!(nesting.labels.iter().all(|l| l.node_id > 62), "the cap keeps the deepest: {nesting:?}");
    let first = &ranking.units[0];
    assert_eq!((first.node_id, first.outliers.as_slice()), (61, &["nesting"][..]));
}

/// A ranked unit carries every label of its own measures: f2 (CC 20, nesting
/// 5) is an outlier on both function measures, and `Huge` (a 600-line span
/// among nine containers of 10–50 lines) on the class `span` measure — while
/// `Split`'s 50-line span, above the fence but under `T_span`, is not.
#[test]
fn a_ranked_unit_carries_the_labels_of_each_of_its_measures() {
    let mut member = labelled_member();
    for id in 60..66 {
        member.nodes.push(node(id, &format!("Small{id}"), NodeKind::Class, "src/b/s.rs", Some((1, 10))));
    }
    let ranking = member.rank();
    assert_eq!(unit(&ranking, FixUnitKind::Function, 2).outliers, ["cc", "nesting"]);
    let span = ranking.outliers.iter().find(|m| m.measure == "span").unwrap();
    assert_eq!(span.units, 9);
    assert_eq!(span.labels.iter().map(|l| l.node_id).collect::<Vec<_>>(), [40]);
    assert_eq!(unit(&ranking, FixUnitKind::Class, 40).outliers, ["span"]);
}

/// A container whose lines are unrecorded has span 0, "unrecorded" — never a
/// measured zero in the `span` distribution: three line-less containers leave
/// the measure at nine units, and the fence it reports is the one those nine
/// give.
#[test]
fn an_unrecorded_span_never_enters_the_span_distribution() {
    let mut member = labelled_member();
    for id in 60..66 {
        member.nodes.push(node(id, &format!("Small{id}"), NodeKind::Class, "src/b/s.rs", Some((1, 10))));
    }
    let measured = member.rank().outliers.into_iter().find(|m| m.measure == "span").unwrap();
    for id in 70..73 {
        member.nodes.push(node(id, &format!("Lineless{id}"), NodeKind::Class, "src/b/l.rs", None));
    }
    let span = member.rank().outliers.into_iter().find(|m| m.measure == "span").unwrap();
    assert_eq!(span.units, 9, "the line-less containers are not counted");
    assert_eq!(span.fence, measured.fence, "nor do they move the fence");
}

/// A duplicate group whose every member is test code has no production
/// member: it is dropped, never ranked, and never counted as a group mass —
/// and the ranking does not stumble over it.
#[test]
fn a_duplicate_group_of_test_code_alone_is_dropped() {
    let mut member = mixed();
    member.nodes.push(node(13, "test_twin", NodeKind::Function, "src/a/x.rs", Some((200, 230))));
    let mut twin = row(13);
    twin.is_duplicate = Some(true);
    member.functions.push(twin);
    member.test_ids.insert(NodeId(13));
    let before = member.rank();
    member.duplicate_groups.push(vec![NodeId(12), NodeId(13)]);
    let after = member.rank();
    assert_eq!(after, before, "a test-only group changes nothing");
}

/// "At or above" the detection threshold: a depth of exactly `T_nest` (4),
/// above the fence of a member of depth-1 functions, is labelled.
#[test]
fn a_value_exactly_at_the_threshold_is_labelled() {
    let mut member = plain_member(14);
    member.function_mut(14).max_nesting_depth = Some(Thresholds::default().nest);
    let nesting = member.rank().outliers.into_iter().find(|m| m.measure == "nesting").unwrap();
    assert_eq!(nesting.labels.iter().map(|l| (l.node_id, l.value)).collect::<Vec<_>>(), [(14, 4)]);
}

/// Labels are ordered by value descending, then node id: among twenty depth-1
/// functions, depths 9, 6 and a second 9 label as f30, f32 (the two 9s, lower
/// id first), then f31.
#[test]
fn labels_order_by_value_descending_then_node_id() {
    let mut member = plain_member(32);
    for (id, depth) in [(30, 9), (31, 6), (32, 9)] {
        member.function_mut(id).max_nesting_depth = Some(depth);
    }
    let nesting = member.rank().outliers.into_iter().find(|m| m.measure == "nesting").unwrap();
    let order: Vec<(i64, i64)> = nesting.labels.iter().map(|l| (l.node_id, l.value)).collect();
    assert_eq!(order, [(30, 9), (32, 9), (31, 6)]);
}
