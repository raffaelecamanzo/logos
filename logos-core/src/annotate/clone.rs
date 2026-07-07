//! The near-clone clustering sub-pass (CR-005, [FR-AN-06], [ADR-21]).
//!
//! Groups functions into **near-clone groups** from the winnowed shingle
//! fingerprints Pass 1 persisted ([FR-EX-09], the `shingles` inverted index):
//! two functions are *clone-paired* when the Jaccard similarity of their shingle
//! sets meets [`CLONE_SIMILARITY_THRESHOLD`] and both clear the eligibility floor
//! ([`MIN_CLONE_SHINGLES`]); a near-clone *group* is a connected component of the
//! resulting pair graph, computed by a deterministic union-find.
//!
//! This sits **beside** exact-duplicate detection ([`super::duplicate_set`],
//! [FR-AN-02]), never inside it ([ADR-21]): the AST-shape fingerprint answers
//! "byte-for-byte the same shape?" while a shingle *set* and its Jaccard
//! similarity answer "near the same shape?". The two verdicts are independent
//! columns; this pass never reads or writes `is_duplicate`.
//!
//! # The algorithm — inverted-index candidate generation
//!
//! A naive all-pairs Jaccard is O(functions²). Instead we walk each shingle's
//! **postings list** (the eligible functions carrying that hash): only functions
//! that co-occur in some posting can share a shingle, so only those pairs become
//! candidates. For each candidate pair we accumulate the count of shared
//! shingles, then `Jaccard(A, B) = shared / (|A| + |B| − shared)` — the union
//! size derived from the two set cardinalities and the intersection, never
//! materialised.
//!
//! # Parallelism ([S-229], [NFR-PE-08])
//!
//! The shared-shingle counting step is O(Σ|posting|²) and, on the Logos repo
//! itself, dominates the whole annotation phase (the other Pass-3 computations —
//! reachability, exact-duplicate, the per-node verdict loop — measure at ≤ a few
//! milliseconds combined). It fans out across the core-owned shared worker pool
//! ([`Runtime::worker_pool`], [CR-057]) by **partitioning the pair keyspace**: the
//! pool runs one task per worker, and each task scans *every* posting but counts
//! only the pairs whose canonical key routes to its shard ([`pair_shard`]). Every
//! pair therefore lands in exactly one shard's map, so the shard maps are
//! **disjoint** — there is no cross-worker sum-merge, and total memory is one copy
//! of the keyspace (≈ O(distinct pairs)), which holds the run within the ≤1 GB
//! indexing ceiling ([NFR-PE-06]) that a per-worker full-copy fold would breach.
//! The trade is deliberate: the cheap pair *generation* is repeated per shard
//! (each shard re-scans every posting and hashes every pair) while only the
//! expensive map *inserts* divide across cores — so the sharding bounds *memory*,
//! not the enumeration, and the speedup plateaus below linear (measured, S-225).
//! To pin the work to the shared pool the caller runs [`cluster`] inside
//! `worker_pool().install(…)`, exactly as extraction and file-load already do;
//! called outside an `install` (e.g. an in-crate unit test) it transparently uses
//! the global rayon pool.
//!
//! [`Runtime::worker_pool`]: ../../runtime/struct.Runtime.html#method.worker_pool
//! [S-229]: ../../../docs/planning/journal.md#s-229-parallelize-the-annotation-compute-gated-stretch
//! [CR-057]: ../../../docs/requests/CR-057-indexing-performance-optimization.md
//! [NFR-PE-08]: ../../../docs/specs/requirements/NFR-PE-08.md
//! [NFR-PE-06]: ../../../docs/specs/requirements/NFR-PE-06.md
//!
//! # Determinism ([NFR-RA-06])
//!
//! The persisted result is thread-count-independent. Postings are built in
//! node-id order, and each pair routes to its shard by [`pair_shard`], a pure
//! function of the pair: *which* shard counts a pair varies with the shard count,
//! but the **union of the disjoint shard maps is always the complete
//! `{pair → count}` set** — every shard scans every posting, so a pair's every
//! co-occurrence reaches its one shard and is fully summed there. The group
//! identifier is then the **minimum node id** of the component (union-by-minimum),
//! a pure function of *which* functions are connected — independent of the order
//! pairs are counted or unioned. So the persisted `clone_group` value (a sorted
//! [`BTreeMap`]) is byte-identical across runs, across worker counts, and
//! idempotent across re-passes, with no final relabelling step.
//!
//! [annotation-engine]: ../../../docs/specs/architecture/components/annotation-engine.md
//! [ADR-21]: ../../../docs/specs/architecture/decisions/ADR-21.md
//! [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
//! [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
//! [FR-EX-09]: ../../../docs/specs/requirements/FR-EX-09.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rayon::prelude::*;

use crate::extract::shingle::{K_GRAM, WINDOW};
use crate::model::NodeId;

/// The documented [FR-AN-06] near-clone defaults (`0.85` similarity, `50`-token
/// floor). They mirror [`Thresholds::default`](crate::metrics::Thresholds) — the
/// single source of truth — and exist here only so the in-crate clone tests can
/// drive the documented behaviour without reaching into the metrics module. A
/// unit test pins them equal to the metrics defaults so the two can never drift.
///
/// Since [CR-013] the *effective* values flow in from the `rules.toml`
/// `[metric_thresholds]` keys `clone_similarity`/`clone_min_tokens` via
/// [`MetricThresholds::effective`](crate::config::MetricThresholds::effective);
/// the annotation pass passes them to [`cluster`], so tuning either re-baselines
/// the gate exactly like a structural threshold ([BR-25]).
///
/// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
/// [CR-013]: ../../../docs/requests/CR-013-tunable-near-clone-thresholds.md
/// [BR-25]: ../../../docs/specs/software-spec.md#311-quality-metrics
#[cfg(test)]
pub(super) const DEFAULT_CLONE_SIMILARITY: f64 = 0.85;
/// The [FR-AN-06] default minimum-token floor (50 normalized tokens) — the lower
/// bound on a function's body size for it to be clone-eligible, so trivial
/// boilerplate is never reported as a clone. Tunable since [CR-013]
/// (`clone_min_tokens`). Test-only: the production default flows in from
/// [`Thresholds::default`](crate::metrics::Thresholds) via the effective set.
#[cfg(test)]
pub(super) const DEFAULT_CLONE_MIN_TOKENS: i64 = 50;

/// The clone-eligibility floor in **shingles** for a given minimum-token floor.
///
/// The merged S-042 contract persists `shingles(node_id, hash)` with **no token
/// count**, so the floor is applied to the shingle-set cardinality, the size
/// signal the inverted index does provide. A body of `T` normalized tokens
/// yields `T − K_GRAM + 1` k-grams, and winnowing selects at least
/// `⌈(k-grams − WINDOW + 1) / WINDOW⌉` fingerprints (the Schleimer–Wilkerson–Aiken
/// worst case — one global minimum dominates at most `WINDOW` consecutive
/// windows). For the documented 50-token default under the fixed winnowing
/// constants ([`K_GRAM`] = 5, [`WINDOW`] = 4) this is `⌈43 / 4⌉ = 11`,
/// byte-identical to the pre-[CR-013] constant.
///
/// Saturating arithmetic keeps a validated-positive but sub-winnowing floor
/// (`clone_min_tokens` in `1..K_GRAM+WINDOW−2`) from underflowing, and the result
/// is floored at 1 so a function always needs at least one shingle to be
/// clone-eligible — a body too short to fingerprint can never pair.
///
/// [CR-013]: ../../../docs/requests/CR-013-tunable-near-clone-thresholds.md
pub(super) const fn min_shingles_for(min_tokens: i64) -> usize {
    let tokens = if min_tokens < 0 { 0 } else { min_tokens as usize };
    // `saturating_add` so a huge `clone_min_tokens` cannot overflow the `+ 2`
    // step on a narrow `usize` (e.g. a 32-bit target) — the whole derivation
    // stays saturating end-to-end, as the doc above promises.
    let floor = tokens
        .saturating_add(2)
        .saturating_sub(K_GRAM + WINDOW)
        .div_ceil(WINDOW);
    if floor < 1 {
        1
    } else {
        floor
    }
}

/// The default clone-eligibility floor in shingles (`⌈43 / 4⌉ = 11`), retained
/// for the in-crate clone tests; the production path derives the floor from the
/// effective `clone_min_tokens` via [`min_shingles_for`].
#[cfg(test)]
pub(super) const MIN_CLONE_SHINGLES: usize = min_shingles_for(DEFAULT_CLONE_MIN_TOKENS);

/// The clustering result: which near-clone group each clustered function belongs
/// to, and how many distinct groups formed.
pub(super) struct CloneClustering {
    /// node id → its group's stable identifier (the minimum node id of the
    /// connected component). Only functions in a group of ≥ 2 appear; an absent
    /// id is in no near-clone group.
    group_of: BTreeMap<NodeId, NodeId>,
    /// The number of distinct near-clone groups (components of size ≥ 2).
    group_count: usize,
}

impl CloneClustering {
    /// The stable clone-group identifier for `id`, or `None` when the function
    /// belongs to no near-clone group ([FR-AN-06]).
    pub(super) fn group_of(&self, id: NodeId) -> Option<NodeId> {
        self.group_of.get(&id).copied()
    }

    /// The number of functions belonging to some near-clone group.
    pub(super) fn cloned_count(&self) -> u64 {
        self.group_of.len() as u64
    }

    /// The number of distinct near-clone groups.
    pub(super) fn group_count(&self) -> u64 {
        self.group_count as u64
    }
}

/// Cluster the id-ordered inverted shingle index ([FR-EX-09]) into near-clone
/// groups under the effective near-clone parameters ([FR-AN-06], [CR-013]).
///
/// `index` is `(node_id, hash)` rows in `(node_id, hash)` order, exactly as
/// [`shingle_index`](crate::graph_store::GraphStore::shingle_index) yields them.
/// `similarity` is the Jaccard clone-similarity threshold and `min_tokens` the
/// minimum-token floor — both from the effective `[metric_thresholds]` set
/// (defaults [`DEFAULT_CLONE_SIMILARITY`] / [`DEFAULT_CLONE_MIN_TOKENS`]). The
/// token floor is mapped to the in-index shingle floor by [`min_shingles_for`].
///
/// [CR-013]: ../../../docs/requests/CR-013-tunable-near-clone-thresholds.md
pub(super) fn cluster(index: &[(NodeId, u64)], similarity: f64, min_tokens: i64) -> CloneClustering {
    cluster_with(index, similarity, min_shingles_for(min_tokens))
}

/// The thresholded core of [`cluster`], parameterised so tests can drive a small
/// floor without depending on the production constant.
fn cluster_with(index: &[(NodeId, u64)], threshold: f64, min_shingles: usize) -> CloneClustering {
    // 1. Per-node shingle-set cardinality. The index arrives in (node_id, hash)
    //    order and the table's PRIMARY KEY (node_id, hash) dedupes, so a node's
    //    rows are contiguous and the count is the set size.
    let mut sizes: BTreeMap<NodeId, usize> = BTreeMap::new();
    for &(node, _) in index {
        *sizes.entry(node).or_insert(0) += 1;
    }

    // 2. The inverted index over clone-eligible nodes only (≥ the floor), with
    //    postings in node-id order — deterministic candidate generation.
    let mut postings: BTreeMap<u64, Vec<NodeId>> = BTreeMap::new();
    for &(node, hash) in index {
        if sizes.get(&node).is_some_and(|&size| size >= min_shingles) {
            postings.entry(hash).or_default().push(node);
        }
    }

    // 3. Shared-shingle counts for every candidate pair co-occurring in a
    //    posting — the O(Σ|posting|²) hot loop, and on a real repo the dominant
    //    cost of the whole annotation phase (S-229). It fans out across the
    //    core-owned worker pool by **partitioning the pair keyspace**: the pool
    //    runs one task per worker, and task `s` scans every posting but counts
    //    only the pairs whose hash routes to shard `s` ([`pair_shard`]). Each pair
    //    therefore lands in exactly one shard's map — the shard maps are disjoint,
    //    so there is no cross-worker merge and total memory is one copy of the
    //    keyspace (≈ O(distinct pairs)), holding the run within the ≤1 GB indexing
    //    ceiling (NFR-PE-06) that a per-worker full-copy fold would breach.
    //
    //    Which shard a pair routes to depends on the shard count, but the *union*
    //    of all shard maps is the complete `{pair → total count}` set regardless
    //    of how many shards there are — every shard sees every posting, so a
    //    pair's every co-occurrence reaches its one shard. The counts, and so the
    //    verdicts, are byte-identical across worker counts (NFR-RA-06).
    let shards = rayon::current_num_threads().max(1);
    let posting_lists: Vec<&[NodeId]> = postings.values().map(Vec::as_slice).collect();
    let shard_counts: Vec<HashMap<(NodeId, NodeId), usize>> = (0..shards)
        .into_par_iter()
        .map(|shard| {
            let mut local: HashMap<(NodeId, NodeId), usize> = HashMap::new();
            for nodes in &posting_lists {
                for (i, &a) in nodes.iter().enumerate() {
                    for &b in &nodes[i + 1..] {
                        // Postings are id-sorted, so a < b; canonicalise the key
                        // defensively all the same.
                        let pair = if a <= b { (a, b) } else { (b, a) };
                        if pair_shard(pair, shards) == shard {
                            *local.entry(pair).or_insert(0) += 1;
                        }
                    }
                }
            }
            local
        })
        .collect();

    // 4. Union the pairs whose Jaccard meets the threshold. Iterating the shard
    //    maps in any order is safe: union-by-minimum makes each component's root a
    //    pure function of *which* pairs connect, never the order they are unioned
    //    (see [`UnionFind`]), so the group ids are identical regardless of shard
    //    count or map-iteration order (NFR-RA-06).
    let mut uf = UnionFind::default();
    for local in &shard_counts {
        for (&(a, b), &intersection) in local {
            // |A ∪ B| = |A| + |B| − |A ∩ B|; both sizes are present (the pair came
            // from eligible postings) and the union is ≥ max(|A|, |B|) ≥ 1.
            let union = sizes[&a] + sizes[&b] - intersection;
            let jaccard = intersection as f64 / union as f64;
            if jaccard >= threshold {
                uf.union(a, b);
            }
        }
    }

    // 5. The connected components are the near-clone groups. Every node in the
    //    forest was unioned (so every component has ≥ 2 members); the root is the
    //    component minimum, the stable group identifier.
    let nodes: Vec<NodeId> = uf.parent.keys().copied().collect();
    let mut group_of: BTreeMap<NodeId, NodeId> = BTreeMap::new();
    let mut roots: BTreeSet<NodeId> = BTreeSet::new();
    for node in nodes {
        let root = uf.find(node);
        group_of.insert(node, root);
        roots.insert(root);
    }

    CloneClustering {
        group_of,
        group_count: roots.len(),
    }
}

/// Route a canonical pair to one of `shards` keyspace shards for the parallel
/// shared-shingle count (step 3 of [`cluster_with`]).
///
/// A pure function of the pair and the shard count: the same pair always routes
/// to the same shard for a given `shards`, so every co-occurrence of a pair is
/// counted in one place and its total is complete. The two odd multipliers plus
/// an xor-shift finaliser spread the small, dense node ids across the whole `u64`
/// so the low bits used by the range map are well-distributed; the
/// multiply-high range reduction (`h * shards >> 64`, Lemire) maps that hash
/// uniformly onto `0..shards` without a per-pair division on the hot path.
#[inline]
fn pair_shard((a, b): (NodeId, NodeId), shards: usize) -> usize {
    let mut h = (a.0 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= (b.0 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    (((h as u128) * (shards as u128)) >> 64) as usize
}

/// A disjoint-set forest with **union-by-minimum-id**: a component's root is
/// always its smallest member, so [`find`](Self::find) returns a stable,
/// order-independent representative ([NFR-RA-06]).
#[derive(Default)]
struct UnionFind {
    parent: BTreeMap<NodeId, NodeId>,
}

impl UnionFind {
    /// The representative (component minimum) of `x`, with path compression.
    fn find(&mut self, x: NodeId) -> NodeId {
        let mut root = x;
        while let Some(&parent) = self.parent.get(&root) {
            if parent == root {
                break;
            }
            root = parent;
        }
        // Compress the path so repeated lookups stay flat.
        let mut current = x;
        while let Some(&parent) = self.parent.get(&current) {
            if parent == root {
                break;
            }
            self.parent.insert(current, root);
            current = parent;
        }
        root
    }

    /// Merge the components of `a` and `b`, keeping the smaller id as the root.
    fn union(&mut self, a: NodeId, b: NodeId) {
        self.parent.entry(a).or_insert(a);
        self.parent.entry(b).or_insert(b);
        let root_a = self.find(a);
        let root_b = self.find(b);
        if root_a == root_b {
            return;
        }
        // Union by minimum id: the smaller root stays root, so every component's
        // representative is its global minimum — a stable group identifier
        // regardless of the order pairs were unioned (NFR-RA-06).
        let (root, child) = if root_a <= root_b {
            (root_a, root_b)
        } else {
            (root_b, root_a)
        };
        self.parent.insert(child, root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cluster under the documented defaults — the behaviour the bulk of these
    /// tests pin. Shadows [`super::cluster`] so the existing default-behaviour
    /// cases read unchanged; the tunable-parameter cases call `super::cluster`
    /// directly with explicit values.
    fn cluster(index: &[(NodeId, u64)]) -> CloneClustering {
        super::cluster(index, DEFAULT_CLONE_SIMILARITY, DEFAULT_CLONE_MIN_TOKENS)
    }

    /// Build a `(node_id, hash)` index from `(id, hashes)` pairs — the shape
    /// [`shingle_index`](crate::graph_store::GraphStore::shingle_index) yields,
    /// already sorted by `(node_id, hash)`.
    fn index(rows: &[(i64, &[u64])]) -> Vec<(NodeId, u64)> {
        let mut out = Vec::new();
        for &(id, hashes) in rows {
            let mut hs = hashes.to_vec();
            hs.sort_unstable();
            hs.dedup();
            for h in hs {
                out.push((NodeId(id), h));
            }
        }
        out.sort_by_key(|&(NodeId(id), h)| (id, h));
        out
    }

    /// A shingle set of `n` distinct hashes starting at `base` — a body well
    /// above the eligibility floor.
    fn set(base: u64, n: u64) -> Vec<u64> {
        (base..base + n).collect()
    }

    /// Cluster `index` inside a fresh rayon pool of exactly `workers` threads —
    /// the 1→N harness for the S-229 parallel-clustering equivalence and stress
    /// tests. `worker_pool().install(…)` in production pins the compute to the
    /// core-owned pool; here a standalone pool stands in for it so the test can
    /// dial the worker (and therefore keyspace-shard) count.
    fn cluster_on(index: &[(NodeId, u64)], workers: usize) -> CloneClustering {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .expect("pool builds");
        pool.install(|| super::cluster(index, DEFAULT_CLONE_SIMILARITY, DEFAULT_CLONE_MIN_TOKENS))
    }

    /// A clustering fixture large enough to drive the keyspace sharding: two
    /// genuine near-clone groups (12 members each, identical 40-shingle sets) plus
    /// 16 solo functions with distinct sets, and a single "hub" shingle every one
    /// of the 40 functions carries. The hub yields one 40-node posting →
    /// C(40,2) = 780 candidate pairs spread across shards, but is far too weak a
    /// signal (Jaccard ≈ 0.02) to group the distinct sets — so the correct result
    /// is exactly two groups regardless of how the pairs shard out.
    fn shard_stress_index() -> Vec<(NodeId, u64)> {
        const HUB: u64 = 9_999_999;
        let mut rows: Vec<(i64, Vec<u64>)> = Vec::new();
        for id in 1..=12 {
            rows.push((id, set(100, 40)));
        }
        for id in 13..=24 {
            rows.push((id, set(200, 40)));
        }
        for id in 25..=40 {
            let base = 1_000 + (id as u64) * 100;
            rows.push((id, set(base, 40)));
        }
        for (_, hs) in &mut rows {
            hs.push(HUB);
        }
        let refs: Vec<(i64, &[u64])> = rows.iter().map(|(id, hs)| (*id, hs.as_slice())).collect();
        index(&refs)
    }

    /// S-229 / [NFR-RA-06]: the parallel near-clone clustering is byte-identical
    /// across worker counts. The shard-stress fixture is clustered under 1, 2, 4,
    /// 8, and 16 workers — since the shard count tracks the worker count, each run
    /// partitions the pair keyspace differently — yet every node's group id, the
    /// group count, and the cloned count must equal the single-worker baseline.
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    #[test]
    fn clustering_is_byte_identical_across_worker_counts() {
        let idx = shard_stress_index();
        let baseline = cluster_on(&idx, 1);
        // The baseline must actually carry the two expected groups, or the
        // equivalence check would be vacuous.
        assert_eq!(baseline.group_count(), 2, "the fixture forms two near-clone groups");
        assert_eq!(baseline.cloned_count(), 24, "24 of 40 functions are clustered");

        for workers in [2, 4, 8, 16] {
            let got = cluster_on(&idx, workers);
            assert_eq!(
                got.group_count(),
                baseline.group_count(),
                "group_count differs at {workers} workers"
            );
            assert_eq!(
                got.cloned_count(),
                baseline.cloned_count(),
                "cloned_count differs at {workers} workers"
            );
            for id in 1..=40 {
                assert_eq!(
                    got.group_of(NodeId(id)),
                    baseline.group_of(NodeId(id)),
                    "group_of({id}) differs at {workers} workers (NFR-RA-06)"
                );
            }
        }
    }

    /// S-229: repeated multi-worker clusterings are stable — 50 runs of the same
    /// index under an 8-worker pool all reproduce the first result, so the
    /// parallel keyspace-sharded counting carries no data race or run-to-run
    /// nondeterminism (the `--threads > 1` stress).
    #[test]
    fn clustering_is_stable_under_repeated_multiworker_runs() {
        let idx = shard_stress_index();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(8)
            .build()
            .expect("pool builds");
        let first =
            pool.install(|| super::cluster(&idx, DEFAULT_CLONE_SIMILARITY, DEFAULT_CLONE_MIN_TOKENS));
        for run in 0..50 {
            let again = pool
                .install(|| super::cluster(&idx, DEFAULT_CLONE_SIMILARITY, DEFAULT_CLONE_MIN_TOKENS));
            assert_eq!(again.group_count(), first.group_count(), "run {run}");
            for id in 1..=40 {
                assert_eq!(again.group_of(NodeId(id)), first.group_of(NodeId(id)), "run {run}");
            }
        }
    }

    /// FR-AN-06: two functions with identical shingle sets (Jaccard 1.0) land in
    /// one group; an unrelated function (disjoint set) lands in none. The group
    /// identifier is the minimum member id.
    #[test]
    fn identical_pair_groups_unrelated_does_not() {
        let a = set(100, 20);
        let unrelated = set(900, 20); // disjoint hashes
        let idx = index(&[(10, &a), (20, &a), (30, &unrelated)]);

        let clusters = cluster(&idx);

        assert_eq!(clusters.group_of(NodeId(10)), Some(NodeId(10)));
        assert_eq!(
            clusters.group_of(NodeId(20)),
            Some(NodeId(10)),
            "the group id is the minimum member id (FR-AN-06)"
        );
        assert_eq!(
            clusters.group_of(NodeId(30)),
            None,
            "an unrelated function is in no near-clone group"
        );
        assert_eq!(clusters.group_count(), 1);
        assert_eq!(clusters.cloned_count(), 2);
    }

    /// UAT-QM-12: a one-shingle edit keeps Jaccard above the 0.85 default, so the
    /// near clones still group; a heavier divergence falls below and does not.
    #[test]
    fn similarity_threshold_separates_near_from_far() {
        // 20 shared + 1 unique each → shared 20, union 22, Jaccard ≈ 0.909 ≥ 0.85.
        let mut near_a = set(100, 20);
        near_a.push(500);
        let mut near_b = set(100, 20);
        near_b.push(501);
        // 20 shared + 10 unique each → union 40, Jaccard 0.5 < 0.85.
        let mut far_a = set(100, 20);
        far_a.extend(set(600, 10));
        let mut far_b = set(100, 20);
        far_b.extend(set(700, 10));

        let near = cluster(&index(&[(10, &near_a), (20, &near_b)]));
        assert_eq!(near.group_of(NodeId(10)), Some(NodeId(10)));
        assert_eq!(near.group_of(NodeId(20)), Some(NodeId(10)));

        let far = cluster(&index(&[(10, &far_a), (20, &far_b)]));
        assert_eq!(far.group_of(NodeId(10)), None);
        assert_eq!(far.group_of(NodeId(20)), None);
        assert_eq!(far.group_count(), 0);
    }

    /// UAT-QM-12 / FR-AN-06: the 0.85 default is the exact gate — a pair whose
    /// Jaccard sits just above it groups, a pair just below does not. Both pairs
    /// clear the eligibility floor, so only the similarity decides.
    #[test]
    fn threshold_boundary_groups_just_above_and_excludes_just_below() {
        // Just above: 18 shared, |A| = 20, |B| = 19 → union 21, 18/21 ≈ 0.857 ≥ 0.85.
        let mut above_a = set(100, 18);
        above_a.extend([500, 501]);
        let mut above_b = set(100, 18);
        above_b.push(600);
        let above = cluster(&index(&[(10, &above_a), (20, &above_b)]));
        assert_eq!(above.group_of(NodeId(10)), Some(NodeId(10)));
        assert_eq!(above.group_of(NodeId(20)), Some(NodeId(10)));
        assert_eq!(above.group_count(), 1);

        // Just below: 17 shared, |A| = 20, |B| = 18 → union 21, 17/21 ≈ 0.810 < 0.85.
        let mut below_a = set(100, 17);
        below_a.extend([500, 501, 502]);
        let mut below_b = set(100, 17);
        below_b.push(600);
        let below = cluster(&index(&[(10, &below_a), (20, &below_b)]));
        assert_eq!(below.group_of(NodeId(10)), None);
        assert_eq!(below.group_of(NodeId(20)), None);
        assert_eq!(below.group_count(), 0);
    }

    /// FR-AN-06: clone-pairing is transitive — a↔b and b↔c collapse into a single
    /// connected component of three, identified by the minimum id.
    #[test]
    fn transitive_pairs_form_one_component() {
        let a = set(100, 20);
        let b = set(100, 20);
        let c = set(100, 20);
        let clusters = cluster(&index(&[(30, &a), (20, &b), (10, &c)]));

        for id in [10, 20, 30] {
            assert_eq!(
                clusters.group_of(NodeId(id)),
                Some(NodeId(10)),
                "all three share one group rooted at the minimum id"
            );
        }
        assert_eq!(clusters.group_count(), 1);
        assert_eq!(clusters.cloned_count(), 3);
    }

    /// FR-AN-06 floor: two functions below the eligibility floor never pair, even
    /// with identical sets — trivial bodies are not a clone signal.
    #[test]
    fn below_floor_functions_are_excluded() {
        // A set one below the production floor.
        let tiny = set(100, (MIN_CLONE_SHINGLES - 1) as u64);
        let clusters = cluster(&index(&[(10, &tiny), (20, &tiny)]));
        assert_eq!(clusters.group_of(NodeId(10)), None);
        assert_eq!(clusters.group_of(NodeId(20)), None);
        assert_eq!(clusters.group_count(), 0);

        // Exactly at the floor, the same pair groups — the boundary is inclusive.
        let at_floor = set(100, MIN_CLONE_SHINGLES as u64);
        let grouped = cluster(&index(&[(10, &at_floor), (20, &at_floor)]));
        assert_eq!(grouped.group_of(NodeId(10)), Some(NodeId(10)));
        assert_eq!(grouped.group_of(NodeId(20)), Some(NodeId(10)));
    }

    /// NFR-RA-06: clustering is a pure, order-independent function of the index —
    /// re-running and shuffling the row order yield byte-identical group ids.
    #[test]
    fn clustering_is_deterministic_and_order_independent() {
        let a = set(100, 20);
        let b = set(100, 20);
        let forward = index(&[(10, &a), (20, &b)]);
        let mut reversed = forward.clone();
        reversed.reverse();

        let first = cluster(&forward);
        let second = cluster(&reversed);
        for id in [10, 20] {
            assert_eq!(first.group_of(NodeId(id)), second.group_of(NodeId(id)));
        }
        assert_eq!(first.group_count(), second.group_count());
    }

    /// An empty index produces no groups — the empty-tree / no-shingles case.
    #[test]
    fn empty_index_yields_no_groups() {
        let clusters = cluster(&[]);
        assert_eq!(clusters.group_count(), 0);
        assert_eq!(clusters.cloned_count(), 0);
        assert_eq!(clusters.group_of(NodeId(1)), None);
    }

    /// CR-013: the in-crate clone defaults mirror the metrics `Thresholds`
    /// defaults exactly — the guard that keeps the two definitions from drifting
    /// (the metrics struct is the single source of truth).
    #[test]
    fn defaults_match_metrics_thresholds_default() {
        let d = crate::metrics::Thresholds::default();
        assert_eq!(
            DEFAULT_CLONE_SIMILARITY.to_bits(),
            d.clone_similarity.to_bits()
        );
        assert_eq!(DEFAULT_CLONE_MIN_TOKENS, d.clone_min_tokens);
    }

    /// CR-013: the default token floor maps to the pre-CR shingle floor of 11
    /// (`⌈43 / 4⌉`), so clustering under the defaults is byte-identical to the
    /// pre-CR build; a sub-winnowing positive floor saturates to 1 rather than
    /// underflowing.
    #[test]
    fn min_shingles_for_is_byte_identical_at_default_and_saturates_tiny() {
        assert_eq!(min_shingles_for(DEFAULT_CLONE_MIN_TOKENS), 11);
        assert_eq!(min_shingles_for(DEFAULT_CLONE_MIN_TOKENS), MIN_CLONE_SHINGLES);
        // A floor too short to fingerprint (≤ K_GRAM + WINDOW − 2 = 7) still
        // demands at least one shingle — it never underflows.
        for tiny in [1, 5, 7] {
            assert_eq!(min_shingles_for(tiny), 1, "tiny floor saturates to 1");
        }
        // A larger floor demands proportionally more shingles.
        assert!(min_shingles_for(100) > min_shingles_for(50));
    }

    /// CR-013: a tuned `clone_similarity` moves the grouping boundary — a pair
    /// that does not group at the 0.85 default groups under a permissive 0.5
    /// threshold, and the same pair stops grouping under a strict 0.95 one.
    #[test]
    fn tuned_similarity_shifts_the_grouping_boundary() {
        // 20 shared + 10 unique each → union 40, Jaccard 0.5: below 0.85, at 0.5.
        let mut a = set(100, 20);
        a.extend(set(600, 10));
        let mut b = set(100, 20);
        b.extend(set(700, 10));
        let idx = index(&[(10, &a), (20, &b)]);

        let strict = super::cluster(&idx, 0.95, DEFAULT_CLONE_MIN_TOKENS);
        assert_eq!(strict.group_of(NodeId(10)), None, "0.5 < 0.95 → no group");

        let permissive = super::cluster(&idx, 0.5, DEFAULT_CLONE_MIN_TOKENS);
        assert_eq!(
            permissive.group_of(NodeId(10)),
            Some(NodeId(10)),
            "0.5 ≥ 0.5 → the pair groups (FR-AN-06 tunable similarity)"
        );
        assert_eq!(permissive.group_of(NodeId(20)), Some(NodeId(10)));
    }

    /// CR-013: a tuned `clone_min_tokens` moves the eligibility floor — a pair of
    /// short identical bodies that the default 50-token floor excludes becomes
    /// clone-eligible under a low floor.
    #[test]
    fn tuned_min_tokens_shifts_the_eligibility_floor() {
        // Five identical shingles — below the default floor of 11, above the
        // floor a small `clone_min_tokens` yields.
        let small = set(100, 5);
        let idx = index(&[(10, &small), (20, &small)]);

        let default_floor = super::cluster(&idx, DEFAULT_CLONE_SIMILARITY, DEFAULT_CLONE_MIN_TOKENS);
        assert_eq!(
            default_floor.group_of(NodeId(10)),
            None,
            "5 shingles < default floor of 11 → excluded"
        );

        // A 1-token floor maps to a 1-shingle floor, so the 5-shingle pair is
        // eligible and groups (Jaccard 1.0).
        let low_floor = super::cluster(&idx, DEFAULT_CLONE_SIMILARITY, 1);
        assert_eq!(low_floor.group_of(NodeId(10)), Some(NodeId(10)));
        assert_eq!(low_floor.group_of(NodeId(20)), Some(NodeId(10)));
    }
}
