//! The CR-005 extended structural dimensions ([FR-QM-09]..[FR-QM-13]) and the
//! detection thresholds they read ([FR-QM-14], [ADR-21]).
//!
//! Five new dimensions widen the quality signal from five to ten, all over the
//! **production scope** ([FR-QM-08]): the test/doc exclusion the original five
//! already apply is threaded through here too — the caller passes the production
//! function slice and the `is_test` set, never test code.
//!
//! - **Nesting** ([FR-QM-09]) — `1 − (functions with max nesting ≥ T_nest) /
//!   functions`, floored at [`DIMENSION_FLOOR`].
//! - **Conciseness** ([FR-QM-10]) — `1 − brain-method ratio`, floored. A *brain
//!   method* meets all three of CC ≥ `T_cc` ∧ LOC ≥ `T_loc` ∧ nesting ≥ `T_bn`.
//! - **Cohesion** ([FR-QM-11]) — mean of `1/LCOM4` over production classes,
//!   floored; **n/a drop-out** when no class construct exists.
//! - **Focus** ([FR-QM-12]) — `1 − god-container ratio` over class-like
//!   containers, floored; **n/a drop-out** when no class-like container exists.
//! - **Uniqueness** ([FR-QM-13]) — `1 − near-clone ratio`, floored.
//!
//! # Floors, not short-circuits ([ADR-21])
//!
//! Each dimension normalizes into `[DIMENSION_FLOOR, 1]`: a ratio that reaches 1
//! (every function deeply nested, say) floors at 0.01 rather than 0, so a new
//! dimension drags the aggregate hard but can never *alone* collapse it. The
//! original five keep their [ADR-12] zero short-circuit — the asymmetry is
//! deliberate ([ADR-21]: systemic pathologies vs ratio heuristics).
//!
//! # Applicability ([ADR-21] drop-out)
//!
//! Cohesion and Focus return [`None`] when their construct is absent. Per
//! [ADR-21] applicability is the [ADR-09] declarative pattern, **realized
//! through the [`NodeKind`] taxonomy** the per-language `symbols` queries
//! populate: a language's queries map its class constructs to [`NodeKind::Class`]
//! (Java/Python/TypeScript classes) and its record/aggregate constructs to
//! [`NodeKind::Struct`] (Rust struct, Go type). So Cohesion scopes to `Class`
//! (LCOM4's field-sharing premise holds for stateful classes; Rust impl blocks
//! and Go method-sets are excluded exactly as [FR-QM-11] requires — they are
//! `Struct`), and Focus scopes to `Class ∪ Struct` (the class-like containers of
//! [FR-QM-12]). No core change per language and no separate manifest field is
//! needed — the kind a construct extracts to *is* its declared applicability.
//!
//! # Determinism ([NFR-RA-06], [ADR-08])
//!
//! Every reduction is integer-exact until a single trailing `f64` division;
//! LCOM4 unions methods in id order, the cohesion mean sums `1/LCOM4` over
//! classes in id order, and the thresholds hash is a [`blake3`] digest over a
//! fixed canonical string — byte-identical across the four CI targets.
//!
//! [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
//! [FR-QM-09]: ../../../docs/specs/requirements/FR-QM-09.md
//! [FR-QM-10]: ../../../docs/specs/requirements/FR-QM-10.md
//! [FR-QM-11]: ../../../docs/specs/requirements/FR-QM-11.md
//! [FR-QM-12]: ../../../docs/specs/requirements/FR-QM-12.md
//! [FR-QM-13]: ../../../docs/specs/requirements/FR-QM-13.md
//! [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
//! [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
//! [ADR-09]: ../../../docs/specs/architecture/decisions/ADR-09.md
//! [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
//! [ADR-21]: ../../../docs/specs/architecture/decisions/ADR-21.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::graph_store::{EdgeRow, FunctionMetricRow, NodeRow};
use crate::model::{EdgeKind, NodeId, NodeKind};
use crate::models::quality::MetricValue;

/// The floor each new dimension normalizes into `[FLOOR, 1]` ([FR-QM-14],
/// [ADR-21]): a ratio of 1 maps to 0.01, so the dimension drags but never alone
/// collapses the signal.
///
/// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
/// [ADR-21]: ../../../docs/specs/architecture/decisions/ADR-21.md
pub(super) const DIMENSION_FLOOR: f64 = 0.01;

/// The effective detection-threshold set the new dimensions read ([FR-QM-09]..
/// [FR-QM-13], [FR-QM-14]).
///
/// The defaults are the documented [CR-005] §5.1 values; the effective set is
/// composed from `rules.toml`'s `[metric_thresholds]` table by
/// [`MetricThresholds::effective`](crate::config::MetricThresholds::effective)
/// ([BR-25]). The [`hash`](Self::hash) is persisted in every snapshot so a tuning
/// change is visible and triggers the announced gate auto-re-baseline
/// ([FR-GV-10]).
///
/// The two near-clone parameters ([CR-013]) — `clone_similarity` and
/// `clone_min_tokens` — join the set: they are read by the near-clone clustering
/// pass ([FR-AN-06], [FR-EX-09]) that backs the Uniqueness dimension ([FR-QM-13]),
/// so tuning either re-baselines the gate exactly like the six structural
/// thresholds. `Eq` is therefore no longer derived (the similarity is an `f64`).
///
/// [CR-005]: ../../../docs/requests/CR-005-extended-structural-metrics.md
/// [CR-013]: ../../../docs/requests/CR-013-tunable-near-clone-thresholds.md
/// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
/// [FR-EX-09]: ../../../docs/specs/requirements/FR-EX-09.md
/// [FR-QM-13]: ../../../docs/specs/requirements/FR-QM-13.md
/// [BR-25]: ../../../docs/specs/software-spec.md#311-quality-metrics
/// [FR-GV-10]: ../../../docs/specs/requirements/FR-GV-10.md
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// `T_nest` — max nesting depth at/above which a function is "deeply nested"
    /// ([FR-QM-09]); default 4.
    pub nest: i64,
    /// `T_cc` — cyclomatic-complexity floor of a brain method ([FR-QM-10]);
    /// default 15.
    pub brain_cc: i64,
    /// `T_loc` — line-count floor of a brain method ([FR-QM-10]); default 100.
    pub brain_loc: i64,
    /// `T_bn` — max-nesting floor of a brain method ([FR-QM-10]); default 3.
    pub brain_nest: i64,
    /// `T_m` — method count at/above which a container is a god container
    /// ([FR-QM-12]); default 20.
    pub god_methods: i64,
    /// `T_span` — line span at/above which a container is a god container
    /// ([FR-QM-12]); default 500.
    pub god_span: i64,
    /// The Jaccard clone-similarity threshold ([FR-AN-06], [CR-013]); default
    /// 0.85, valid range `(0, 1]`. Two functions clone-pair at/above this.
    ///
    /// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
    /// [CR-013]: ../../../docs/requests/CR-013-tunable-near-clone-thresholds.md
    pub clone_similarity: f64,
    /// The minimum-token floor below which a function produces no clone shingles
    /// ([FR-EX-09], [CR-013]); default 50, a positive integer.
    ///
    /// [FR-EX-09]: ../../../docs/specs/requirements/FR-EX-09.md
    pub clone_min_tokens: i64,
}

impl Default for Thresholds {
    /// The documented [CR-005] §5.1 / [CR-013] defaults ([FR-QM-09]..[FR-QM-13]).
    fn default() -> Self {
        Self {
            nest: 4,
            brain_cc: 15,
            brain_loc: 100,
            brain_nest: 3,
            god_methods: 20,
            god_span: 500,
            clone_similarity: 0.85,
            clone_min_tokens: 50,
        }
    }
}

impl Thresholds {
    /// The effective-thresholds hash persisted in every snapshot ([FR-QM-14],
    /// [BR-25]).
    ///
    /// A [`blake3`] digest over a fixed-order `key=value` canonical string — a
    /// byte-identical hex string across the four CI targets ([NFR-RA-06]). A
    /// changed threshold changes the hash, which the gate detects as a baseline
    /// mismatch and auto-re-baselines against ([FR-GV-10]).
    ///
    /// The two near-clone parameters ([CR-013]) are appended **only when they
    /// differ from their documented defaults**. This keeps the default-set hash
    /// byte-identical to the pre-CR-013 build — an untuned repo never spuriously
    /// re-baselines on upgrade — while any tuning of either still moves the hash
    /// (the structural keys and the near-clone keys use disjoint name prefixes,
    /// so no tuning of one can ever forge another's canonical segment).
    ///
    /// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
    /// [BR-25]: ../../../docs/specs/software-spec.md#311-quality-metrics
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [CR-013]: ../../../docs/requests/CR-013-tunable-near-clone-thresholds.md
    /// [FR-GV-10]: ../../../docs/specs/requirements/FR-GV-10.md
    pub(super) fn hash(&self) -> String {
        // Fixed key order → a stable canonical form independent of struct layout.
        let mut canonical = format!(
            "t_nest={};t_cc={};t_loc={};t_bn={};t_m={};t_span={}",
            self.nest,
            self.brain_cc,
            self.brain_loc,
            self.brain_nest,
            self.god_methods,
            self.god_span,
        );
        // CR-013: fold the near-clone parameters into the hashed effective set.
        // Append-on-divergence preserves the pre-CR default-set hash exactly
        // (FR-QM-14 byte-identity), so existing baselines stay valid; comparing
        // the similarity by bit pattern is exact and dodges `clippy::float_cmp`.
        let d = Thresholds::default();
        if self.clone_similarity.to_bits() != d.clone_similarity.to_bits() {
            canonical.push_str(&format!(";clone_similarity={}", self.clone_similarity));
        }
        if self.clone_min_tokens != d.clone_min_tokens {
            canonical.push_str(&format!(";clone_min_tokens={}", self.clone_min_tokens));
        }
        blake3::hash(canonical.as_bytes()).to_hex().to_string()
    }
}

/// Normalize a "bad ratio" dimension into `[FLOOR, 1]`: `(1 − ratio)` floored.
/// `raw` carries the unfloored ratio so evolution shows which dimension moved
/// ([FR-QM-07]).
fn from_ratio(redundant: usize, total: usize) -> MetricValue {
    if total == 0 {
        // No production functions: nothing to flag → the dimension is a clean
        // 1.0 (FR-QM-09/10/13 "zero production functions → 1.0").
        return MetricValue {
            raw: 0.0,
            normalized: 1.0,
        };
    }
    let ratio = redundant as f64 / total as f64;
    MetricValue {
        raw: ratio,
        normalized: (1.0 - ratio).max(DIMENSION_FLOOR),
    }
}

/// Nesting ([FR-QM-09]) — `1 − (functions with max nesting ≥ T_nest) /
/// functions`, floored.
///
/// A function whose depth was never computed (`None`) is not deep — it is
/// excluded from the numerator, never coerced to a phantom deep function
/// ([NFR-CC-04]).
///
/// [FR-QM-09]: ../../../docs/specs/requirements/FR-QM-09.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub(super) fn nesting(production: &[&FunctionMetricRow], t: &Thresholds) -> MetricValue {
    let deep = production
        .iter()
        .filter(|f| f.max_nesting_depth.is_some_and(|d| d >= t.nest))
        .count();
    from_ratio(deep, production.len())
}

/// Conciseness ([FR-QM-10]) — `1 − brain-method ratio`, floored.
///
/// A brain method meets **all three** thresholds; failing any one (or carrying
/// a `None` input) excludes it ([NFR-CC-04] — never a fabricated metric).
///
/// [FR-QM-10]: ../../../docs/specs/requirements/FR-QM-10.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub(super) fn conciseness(production: &[&FunctionMetricRow], t: &Thresholds) -> MetricValue {
    let brain = production
        .iter()
        .filter(|f| {
            f.cyclomatic_complexity.is_some_and(|c| c >= t.brain_cc)
                && f.line_count.is_some_and(|l| l >= t.brain_loc)
                && f.max_nesting_depth.is_some_and(|d| d >= t.brain_nest)
        })
        .count();
    from_ratio(brain, production.len())
}

/// Uniqueness ([FR-QM-13]) — `1 − near-clone ratio`, floored.
///
/// A production function is near-cloned iff its `clone_group` is non-`None`
/// (the [S-043] annotation, [FR-AN-06]). Exact-duplicate detection and
/// Redundancy ([FR-QM-05]) are read independently and are unchanged.
///
/// [FR-QM-13]: ../../../docs/specs/requirements/FR-QM-13.md
/// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
/// [S-043]: ../../../docs/planning/journal.md#s-043-near-clone-detection-annotation
pub(super) fn uniqueness(production: &[&FunctionMetricRow]) -> MetricValue {
    let cloned = production
        .iter()
        .filter(|f| f.clone_group.is_some())
        .count();
    from_ratio(cloned, production.len())
}

/// The class-like container scaffolding the Cohesion and Focus dimensions read,
/// built once from the production node/edge snapshot ([FR-QM-11], [FR-QM-12]).
///
/// Containers are non-test [`NodeKind::Class`]/[`NodeKind::Struct`] nodes; each
/// carries its **production** member methods and fields (via [`EdgeKind::Contains`]),
/// so `is_test` methods are excluded from both dimensions — adding test methods
/// to a class leaves Cohesion and Focus byte-identical ([FR-QM-08], [UAT-QM-07]).
/// `Accesses` (method→field) and intra-class `Calls` edges feed LCOM4
/// connectivity.
pub(super) struct ContainerIndex {
    /// Class-like containers in id order (deterministic reduction, [ADR-08]).
    containers: Vec<Container>,
    /// Method → the fields it accesses ([`EdgeKind::Accesses`], class-scoped per
    /// [FR-EX-08]); an LCOM4 field-sharing input.
    ///
    /// [FR-EX-08]: ../../../docs/specs/requirements/FR-EX-08.md
    method_fields: HashMap<NodeId, BTreeSet<NodeId>>,
    /// Method → methods it calls ([`EdgeKind::Calls`]); filtered to intra-class
    /// pairs when LCOM4 consumes it.
    method_calls: HashMap<NodeId, BTreeSet<NodeId>>,
}

/// One class-like container with its production members and span.
struct Container {
    /// The container node's storage id (worst-offender / god-container reporting,
    /// [FR-QM-12], [UAT-GV-08]); reported in id order for determinism.
    id: NodeId,
    /// The container node kind (`Class` is cohesion-applicable; `Class`/`Struct`
    /// are both focus-applicable).
    kind: NodeKind,
    /// Production member methods in id order.
    methods: Vec<NodeId>,
    /// 1-based line span `end − start + 1`, or 0 when either line is unrecorded.
    span: i64,
}

/// A class-like container flagged as a **god container** ([FR-QM-12]): its
/// production method count ≥ `T_m` **or** its line span ≥ `T_span`. Backs both
/// the `no_god_containers` budget ([FR-GV-11] ext., [UAT-GV-08]) and the Focus
/// worst-offender list, so the budget, the report, and the Focus dimension agree
/// by construction. The caller enriches `id` to a name/file via the node set.
///
/// [FR-QM-12]: ../../../docs/specs/requirements/FR-QM-12.md
/// [FR-GV-11]: ../../../docs/specs/requirements/FR-GV-11.md
/// [UAT-GV-08]: ../../../docs/specs/requirements/UAT-GV-08.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GodContainer {
    /// The container node's storage id.
    pub id: NodeId,
    /// Production member-method count.
    pub method_count: u64,
    /// 1-based line span.
    pub span: i64,
}

impl ContainerIndex {
    /// Build the container scaffolding from the production node/edge snapshot.
    ///
    /// `nodes`/`edges` are the whole-graph snapshots (id-ordered and
    /// `(source,target,kind)`-ordered respectively); `test_ids` is the persisted
    /// `is_test` set excluded from the production scope ([FR-QM-08]). Method↔field
    /// access and intra-class calls are resolved here so the dimension methods are
    /// pure arithmetic over the result.
    pub(super) fn build(nodes: &[NodeRow], edges: &[EdgeRow], test_ids: &HashSet<NodeId>) -> Self {
        let kind_of: HashMap<NodeId, NodeKind> = nodes.iter().map(|n| (n.id, n.kind)).collect();

        // Class-like containers are enumerated from the node set (not from edges)
        // so a method-less container — e.g. a god-by-span empty struct — is still
        // counted by Focus. `nodes` is id-ordered (all_nodes ORDER BY id), so the
        // container order is deterministic (NFR-RA-06, ADR-08). A container in
        // test scope is excluded wholesale (FR-QM-08).
        let mut by_id: HashMap<NodeId, usize> = HashMap::new();
        let mut containers: Vec<Container> = Vec::new();
        for n in nodes {
            if matches!(n.kind, NodeKind::Class | NodeKind::Struct) && !test_ids.contains(&n.id) {
                let span = match (n.start_line, n.end_line) {
                    (Some(s), Some(e)) if e >= s => e - s + 1,
                    _ => 0,
                };
                by_id.insert(n.id, containers.len());
                containers.push(Container {
                    id: n.id,
                    kind: n.kind,
                    methods: Vec::new(),
                    span,
                });
            }
        }

        // Method → the fields it accesses (Accesses is class-scoped, FR-EX-08);
        // method → methods it calls. Both feed LCOM4 connectivity (FR-QM-11).
        let mut method_fields: HashMap<NodeId, BTreeSet<NodeId>> = HashMap::new();
        let mut method_calls: HashMap<NodeId, BTreeSet<NodeId>> = HashMap::new();

        for e in edges {
            match e.kind {
                // Attach production member methods to their container.
                EdgeKind::Contains => {
                    if let Some(&ci) = by_id.get(&e.source) {
                        if matches!(kind_of.get(&e.target), Some(NodeKind::Method))
                            && !test_ids.contains(&e.target)
                        {
                            containers[ci].methods.push(e.target);
                        }
                    }
                }
                EdgeKind::Accesses => {
                    method_fields.entry(e.source).or_default().insert(e.target);
                }
                EdgeKind::Calls => {
                    method_calls.entry(e.source).or_default().insert(e.target);
                }
                _ => {}
            }
        }

        // Deterministic LCOM4 union order within each container (NFR-RA-06).
        for c in &mut containers {
            c.methods.sort_unstable();
        }

        ContainerIndex {
            containers,
            method_fields,
            method_calls,
        }
    }

    /// Cohesion ([FR-QM-11]) — mean of `1/LCOM4` over production **classes**,
    /// floored; [`None`] (n/a drop-out) when no class has scoreable methods.
    ///
    /// LCOM4 is the number of connected components of a class's methods linked by
    /// shared field access ([`EdgeKind::Accesses`]) or intra-class
    /// [`EdgeKind::Calls`] ([FR-QM-11]). A class with no production methods is not
    /// scoreable (LCOM4 undefined) and is excluded; a repo with zero scoreable
    /// classes drops Cohesion out of the aggregate denominator ([ADR-21]).
    ///
    /// [FR-QM-11]: ../../../docs/specs/requirements/FR-QM-11.md
    /// [ADR-21]: ../../../docs/specs/architecture/decisions/ADR-21.md
    pub(super) fn cohesion(&self) -> Option<MetricValue> {
        let mut sum = 0.0_f64;
        let mut classes = 0_u64;
        for c in &self.containers {
            // Cohesion applies to Class only (Rust/Go aggregates are Struct —
            // LCOM4's field-sharing premise is contested there, FR-QM-11).
            if c.kind != NodeKind::Class || c.methods.is_empty() {
                continue;
            }
            let lcom4 = self.lcom4(c);
            sum += 1.0 / lcom4 as f64;
            classes += 1;
        }
        if classes == 0 {
            return None; // n/a drop-out: no scoreable class construct (ADR-21).
        }
        let mean = sum / classes as f64;
        // Cohesion is defined directly as the goodness mean `mean(1/LCOM4)`
        // ([FR-QM-11]) — unlike the ratio dimensions there is no separate
        // "bad ratio" pre-image. `raw` is the unfloored mean (the full-precision
        // signal evolution tracks); `normalized` is the same value after the only
        // normalization step, the [ADR-21] floor.
        Some(MetricValue {
            raw: mean,
            normalized: mean.max(DIMENSION_FLOOR),
        })
    }

    /// Focus ([FR-QM-12]) — `1 − god-container ratio` over class-like containers,
    /// floored; [`None`] (n/a drop-out) when there are no class-like containers.
    ///
    /// A container is **god** when its production method count ≥ `T_m` **or** its
    /// line span ≥ `T_span` ([FR-QM-12]). Class-like containers are `Class` and
    /// `Struct` (Java/Python/TS class, Rust struct+impl, Go type method-set).
    ///
    /// [FR-QM-12]: ../../../docs/specs/requirements/FR-QM-12.md
    pub(super) fn focus(&self, t: &Thresholds) -> Option<MetricValue> {
        let total = self.containers.len();
        if total == 0 {
            return None; // n/a drop-out: no class-like container exists (ADR-21).
        }
        let god = self
            .containers
            .iter()
            .filter(|c| c.methods.len() as i64 >= t.god_methods || c.span >= t.god_span)
            .count();
        Some(from_ratio(god, total))
    }

    /// The class-like containers over the god thresholds ([FR-QM-12]), in node-id
    /// order ([NFR-RA-06]). The exact set Focus counts as god, so the
    /// `no_god_containers` budget ([FR-GV-11] ext.) and the dimension never
    /// disagree. `containers` is already id-ordered (built from the id-ordered
    /// node set), so the returned list is deterministic with no extra sort.
    ///
    /// [FR-QM-12]: ../../../docs/specs/requirements/FR-QM-12.md
    /// [FR-GV-11]: ../../../docs/specs/requirements/FR-GV-11.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub(super) fn god_containers(&self, t: &Thresholds) -> Vec<GodContainer> {
        self.containers
            .iter()
            .filter(|c| c.methods.len() as i64 >= t.god_methods || c.span >= t.god_span)
            .map(|c| GodContainer {
                id: c.id,
                method_count: c.methods.len() as u64,
                span: c.span,
            })
            .collect()
    }

    /// The production **classes** whose LCOM4 ≥ 2 — the low-cohesion offenders the
    /// Cohesion worst-offender list reports ([FR-QM-11], [UAT-QM-13]/[CR-005]
    /// review detail). Same scope as [`cohesion`](Self::cohesion) (Class with
    /// production methods); returns `(class id, LCOM4)` in node-id order, so the
    /// report agrees with the dimension and is deterministic ([NFR-RA-06]).
    ///
    /// [FR-QM-11]: ../../../docs/specs/requirements/FR-QM-11.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub(super) fn low_cohesion_classes(&self) -> Vec<(NodeId, u64)> {
        let mut offenders: Vec<(NodeId, u64)> = self
            .containers
            .iter()
            .filter(|c| c.kind == NodeKind::Class && !c.methods.is_empty())
            .filter_map(|c| {
                let lcom4 = self.lcom4(c);
                (lcom4 >= 2).then_some((c.id, lcom4))
            })
            .collect();
        // `containers` is id-ordered, so `offenders` is already id-ascending; the
        // explicit sort documents the determinism contract and is a no-op here.
        offenders.sort_by_key(|&(id, _)| id);
        offenders
    }

    /// LCOM4 of one class: the number of connected components of its methods
    /// linked by shared field access or intra-class calls ([FR-QM-11]).
    ///
    /// Union-find over the class's methods in id order keeps the component count
    /// a pure function of the graph, order-independent and reproducible
    /// ([NFR-RA-06]). Returns at least 1 (a non-empty method set).
    fn lcom4(&self, c: &Container) -> u64 {
        let methods = &c.methods;
        let n = methods.len();
        debug_assert!(n > 0, "cohesion() skips method-less classes");
        let index_of: HashMap<NodeId, usize> =
            methods.iter().enumerate().map(|(i, &m)| (m, i)).collect();
        let mut uf = UnionFind::new(n);

        // Field sharing: methods touching the same field are one component. Group
        // accessing methods by field, then union each group. The component count
        // is order-invariant under union-find, but each group's member list is
        // built by iterating `method_fields` (a `HashMap`, arbitrary order), so we
        // sort it to ascending method-index (= ascending method-id) order — the
        // union sequence is then genuinely deterministic, not merely
        // count-invariant ([NFR-RA-06]; the `field_members` keys are already
        // id-ordered via the `BTreeMap`).
        let mut field_members: BTreeMap<NodeId, Vec<usize>> = BTreeMap::new();
        for (&method, fields) in &self.method_fields {
            if let Some(&mi) = index_of.get(&method) {
                for &field in fields {
                    field_members.entry(field).or_default().push(mi);
                }
            }
        }
        for members in field_members.values_mut() {
            members.sort_unstable();
            if let Some((&first, rest)) = members.split_first() {
                for &other in rest {
                    uf.union(first, other);
                }
            }
        }

        // Intra-class calls: a call between two methods of this class links them.
        for (&src, dsts) in &self.method_calls {
            if let Some(&si) = index_of.get(&src) {
                for &dst in dsts {
                    if let Some(&di) = index_of.get(&dst) {
                        uf.union(si, di);
                    }
                }
            }
        }

        uf.component_count() as u64
    }
}

/// A minimal union-find over `0..n` with union-by-size and path halving — a
/// local helper so LCOM4 component counting needs no external dependency.
struct UnionFind {
    parent: Vec<usize>,
    size: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            size: vec![1; n],
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]]; // path halving
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        // Union by size; ties broken toward the lower root for determinism.
        let (big, small) = if self.size[ra] >= self.size[rb] {
            (ra, rb)
        } else {
            (rb, ra)
        };
        self.parent[small] = big;
        self.size[big] += self.size[small];
    }

    fn component_count(&mut self) -> usize {
        // A root is its own parent. `find` flattens paths, so after calling it on
        // every element the distinct roots are exactly the elements that are their
        // own parent — counted in one pass with no heap allocation.
        (0..self.parent.len())
            .filter(|&x| self.find(x) == x)
            .count()
    }
}
