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
//! # The algorithm — exact prefix filtering ([CR-198])
//!
//! A naive all-pairs Jaccard is O(functions²), and so is counting every pair
//! that co-occurs in a shingle's postings list: a boilerplate shingle carried by
//! *n* functions alone yields *n*²/2 pairs. On this repository that counter
//! visited 241.7 M pairs and peaked a one-file sync at ~2.4 GB ([CR-198]). The
//! clustering therefore generates candidates by **prefix filtering**, the
//! standard exact set-similarity join, and verifies each one exactly:
//!
//! 1. **Global order.** Every shingle of the clone-eligible functions is ranked
//!    by ascending document frequency, ties broken by hash value — a total,
//!    deterministic order in which rare shingles come first and ubiquitous ones
//!    last.
//! 2. **Prefix.** A function of `s` shingles indexes only its first
//!    `s − o(s) + 1` shingles in that order, where `o(s)` is the smallest overlap
//!    `o ≥ 1` that passes the verdict comparison itself, `o / s ≥ threshold`
//!    ([`min_overlap`]). If a pair's Jaccard passes, its intersection `I` passes
//!    `I / s` for each side's size `s` (the union is at least `s`, and rounded
//!    division is monotone), so `I ≥ o(s)` on both sides. The least shared
//!    shingle in the order then sits within both prefixes — no qualifying pair is
//!    lost, and no float product `threshold · s` is ever rounded into a bound.
//! 3. **Length filter.** A pair is kept only if `min(|A|, |B|) / max(|A|, |B|)`
//!    passes the same comparison — necessary, since the Jaccard never exceeds it.
//! 4. **Exact verification.** Each surviving candidate's intersection is counted
//!    by merging the two sorted sets, and the pair is unioned only if
//!    `shared / (|A| + |B| − shared)` meets the threshold — the very expression
//!    the all-pairs counter applied, so every verdict is the same.
//!
//! A ubiquitous shingle sorts last and falls out of almost every prefix, so a
//! posting shared by *n* functions adds no *n*² work; the work counter
//! ([`ClusterWork`]) pins that growth in the hub-shingle test.
//!
//! [CR-198]: ../../../docs/requests/CR-198-near-clone-clustering-is-exact-under-prefix-filtering.md
//!
//! # Memory ([NFR-PE-06])
//!
//! Memory is linear in the input: the per-function shingle sets (the index
//! itself, re-keyed to ranks), the prefix postings (at most one entry per
//! indexed shingle), and the verified pairs. The all-pairs counter's
//! `{pair → count}` map — O(Σ|posting|²) entries, the whole of the old spike —
//! is gone. No fixed ceiling is claimed: the candidate count still depends on
//! how many functions share mid-frequency shingles within their prefixes.
//!
//! [NFR-PE-06]: ../../../docs/specs/requirements/NFR-PE-06.md
//!
//! # Parallelism ([S-229], [NFR-PE-08])
//!
//! The prefix postings are built once, serially; probing them is independent per
//! function, so the probe-and-verify step maps across the core-owned shared
//! worker pool ([`Runtime::worker_pool`], [CR-057]) one function per task. To pin
//! the work to that pool the caller runs [`cluster`] inside
//! `worker_pool().install(…)`, exactly as extraction and file-load already do;
//! called outside an `install` (e.g. an in-crate unit test) it transparently
//! uses the global rayon pool.
//!
//! [`Runtime::worker_pool`]: ../../runtime/struct.Runtime.html#method.worker_pool
//! [S-229]: ../../../docs/planning/journal.md#s-229-parallelize-the-annotation-compute-gated-stretch
//! [CR-057]: ../../../docs/requests/CR-057-indexing-performance-optimization.md
//! [NFR-PE-08]: ../../../docs/specs/requirements/NFR-PE-08.md
//!
//! # Determinism ([NFR-RA-06])
//!
//! The persisted result is thread-count-independent. The global order, the
//! processing order (by size, then node id) and so the prefix postings are pure
//! functions of the index; each verified pair is found exactly once, when the
//! later function of the two probes the postings, whichever worker runs it. The
//! group identifier is then the **minimum node id** of the component
//! (union-by-minimum), a pure function of *which* functions are connected —
//! independent of the order pairs are verified or unioned. So the persisted
//! `clone_group` value (a sorted [`BTreeMap`]) is byte-identical across runs,
//! across worker counts, and idempotent across re-passes, with no final
//! relabelling step.
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
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
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
    cluster_counted(index, threshold, min_shingles).0
}

/// The work one clustering run did — the counters the hub-shingle growth test
/// pins ([CR-198]): a posting shared by *n* functions must add no *n*² work.
///
/// [CR-198]: ../../../docs/requests/CR-198-near-clone-clustering-is-exact-under-prefix-filtering.md
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct ClusterWork {
    /// Prefix-posting entries visited while probing, after the length filter
    /// trimmed each posting: one per (function, earlier function, shared prefix
    /// shingle) examined.
    pub(super) probes: u64,
    /// Distinct candidate pairs verified by an exact intersection.
    pub(super) verified: u64,
}

/// One clone-eligible function: its id and its shingle set as ascending
/// global ranks (step 3 of [`cluster_counted`]).
struct Record {
    id: NodeId,
    ranks: Vec<usize>,
}

/// [`cluster_with`], also returning the [`ClusterWork`] the run did.
pub(super) fn cluster_counted(
    index: &[(NodeId, u64)],
    threshold: f64,
    min_shingles: usize,
) -> (CloneClustering, ClusterWork) {
    // 1. Per-node shingle sets over the clone-eligible nodes (≥ the floor). Keyed
    //    by node id, so nothing below depends on the order the rows arrive in.
    //    The table's PRIMARY KEY (node_id, hash) already makes each a set; the
    //    sort + dedup keeps that true for any caller.
    let mut sets: BTreeMap<NodeId, Vec<u64>> = BTreeMap::new();
    for &(node, hash) in index {
        sets.entry(node).or_default().push(hash);
    }
    sets.retain(|_, hashes| {
        hashes.sort_unstable();
        hashes.dedup();
        hashes.len() >= min_shingles
    });

    // 2. The global order: ascending document frequency over the eligible sets,
    //    ties broken by hash value, so it is total and deterministic. Rare
    //    shingles rank first; a ubiquitous one ranks last and so falls out of
    //    almost every prefix.
    let mut frequency: HashMap<u64, usize> = HashMap::new();
    for hashes in sets.values() {
        for &hash in hashes {
            *frequency.entry(hash).or_insert(0) += 1;
        }
    }
    let mut order: Vec<(usize, u64)> = frequency.into_iter().map(|(hash, df)| (df, hash)).collect();
    order.sort_unstable();
    let rank: HashMap<u64, usize> = order
        .iter()
        .enumerate()
        .map(|(rank, &(_, hash))| (hash, rank))
        .collect();

    // 3. The records in processing order — ascending size, then node id — each
    //    set re-keyed to ascending ranks. Ranks are a bijection of the hashes, so
    //    a merge over ranks counts exactly the shared shingles.
    let mut records: Vec<Record> = sets
        .into_iter()
        .map(|(id, hashes)| {
            let mut ranks: Vec<usize> = hashes.iter().map(|hash| rank[hash]).collect();
            ranks.sort_unstable();
            Record { id, ranks }
        })
        .collect();
    records.sort_unstable_by_key(|record| (record.ranks.len(), record.id));
    let sizes: Vec<usize> = records.iter().map(|record| record.ranks.len()).collect();

    // 4. The prefix postings: rank → the positions of the records whose prefix
    //    holds it. Built in position order, so each posting ascends by position
    //    and therefore by size.
    let mut postings: Vec<Vec<usize>> = vec![Vec::new(); order.len()];
    let prefix_lens: Vec<usize> = records
        .iter()
        .enumerate()
        .map(|(pos, record)| {
            let size = record.ranks.len();
            let len = min_overlap(size, threshold).map_or(0, |overlap| size - overlap + 1);
            for &r in &record.ranks[..len] {
                postings[r].push(pos);
            }
            len
        })
        .collect();

    // 5. Probe and verify, independently per record: each record probes its
    //    prefix's postings for EARLIER records only, so a pair is found once,
    //    from its later side. The length filter trims each posting's leading run
    //    of records too small to pass (sizes ascend along it); the survivors are
    //    deduplicated and verified by an exact intersection under the verdict's
    //    own comparison. `collect` keeps position order, so the result is the
    //    same at every worker count (NFR-RA-06).
    let probed: Vec<(Vec<(NodeId, NodeId)>, ClusterWork)> = (0..records.len())
        .into_par_iter()
        .map(|pos| {
            let record = &records[pos];
            let size = sizes[pos];
            let mut work = ClusterWork::default();
            let mut candidates: Vec<usize> = Vec::new();
            for &r in &record.ranks[..prefix_lens[pos]] {
                let posting = &postings[r];
                let end = posting.partition_point(|&other| other < pos);
                let start =
                    posting[..end].partition_point(|&other| !meets(sizes[other], size, threshold));
                work.probes += (end - start) as u64;
                candidates.extend_from_slice(&posting[start..end]);
            }
            candidates.sort_unstable();
            candidates.dedup();
            work.verified = candidates.len() as u64;
            let pairs = candidates
                .into_iter()
                .filter(|&other| {
                    let shared = intersection(&records[other].ranks, &record.ranks);
                    meets(shared, sizes[other] + size - shared, threshold)
                })
                .map(|other| (records[other].id, record.id))
                .collect();
            (pairs, work)
        })
        .collect();

    // 6. Union the verified pairs. Union-by-minimum makes each component's root a
    //    pure function of *which* pairs connect, never the order they are unioned
    //    (see [`UnionFind`]), so the group ids are identical at every worker
    //    count (NFR-RA-06).
    let mut uf = UnionFind::default();
    let mut work = ClusterWork::default();
    for (pairs, record_work) in probed {
        work.probes += record_work.probes;
        work.verified += record_work.verified;
        for (a, b) in pairs {
            uf.union(a, b);
        }
    }

    // 7. The connected components are the near-clone groups. Every node in the
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

    (
        CloneClustering {
            group_of,
            group_count: roots.len(),
        },
        work,
    )
}

/// The clone verdict's comparison: does `shared / union` meet `threshold`?
///
/// The one comparison every bound in [`cluster_counted`] goes through — the
/// verdict itself, the length filter and the prefix length ([`min_overlap`]) —
/// so no bound can round differently from the verdict it guards ([CR-198]).
///
/// [CR-198]: ../../../docs/requests/CR-198-near-clone-clustering-is-exact-under-prefix-filtering.md
#[inline]
fn meets(shared: usize, union: usize, threshold: f64) -> bool {
    shared as f64 / union as f64 >= threshold
}

/// The smallest overlap `o ≥ 1` a set of `size` shingles needs with any partner
/// for the pair to pass the verdict: the least `o` with `meets(o, size)`, or
/// `None` when not even a full overlap passes (a threshold above 1).
///
/// Any passing pair has `meets(shared, union)` with `union ≥ size`; division is
/// monotone under rounding, so `meets(shared, size)` holds too and
/// `shared ≥ o`. The real-valued `⌈threshold · size⌉` is only the starting
/// guess: the product rounds (`0.56 · 25` lands above 14, yet `14 / 25` passes
/// `0.56`), so the walk settles on the integer the comparison itself admits.
fn min_overlap(size: usize, threshold: f64) -> Option<usize> {
    if size == 0 || !meets(size, size, threshold) {
        return None;
    }
    let guess = (threshold * size as f64).ceil();
    let mut overlap = if guess >= 1.0 {
        (guess as usize).min(size)
    } else {
        1
    };
    while overlap > 1 && meets(overlap - 1, size, threshold) {
        overlap -= 1;
    }
    // Terminates: `meets(size, size)` holds, checked above.
    while !meets(overlap, size, threshold) {
        overlap += 1;
    }
    Some(overlap)
}

/// The number of elements two ascending, duplicate-free slices share.
fn intersection(a: &[usize], b: &[usize]) -> usize {
    let (mut i, mut j, mut shared) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                shared += 1;
                i += 1;
                j += 1;
            }
        }
    }
    shared
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
    /// dial the worker count.
    fn cluster_on(index: &[(NodeId, u64)], workers: usize) -> CloneClustering {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .expect("pool builds");
        pool.install(|| super::cluster(index, DEFAULT_CLONE_SIMILARITY, DEFAULT_CLONE_MIN_TOKENS))
    }

    /// A clustering fixture large enough to spread across workers: two genuine
    /// near-clone groups (12 members each, identical 40-shingle sets) plus 16
    /// solo functions with distinct sets, and a single "hub" shingle every one of
    /// the 40 functions carries. The hub is one 40-node posting — C(40,2) = 780
    /// pairs to the all-pairs counter — but far too weak a signal (Jaccard ≈ 0.02)
    /// to group the distinct sets, so the correct result is exactly two groups
    /// however the work divides.
    fn hub_stress_index() -> Vec<(NodeId, u64)> {
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
    /// across worker counts. The hub-stress fixture is clustered under 1, 2, 4,
    /// 8, and 16 workers — each run divides the probe-and-verify step differently
    /// — yet every node's group id, the group count, and the cloned count must
    /// equal the single-worker baseline.
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    #[test]
    fn clustering_is_byte_identical_across_worker_counts() {
        let idx = hub_stress_index();
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
    /// parallel probe-and-verify step carries no data race or run-to-run
    /// nondeterminism (the `--threads > 1` stress).
    #[test]
    fn clustering_is_stable_under_repeated_multiworker_runs() {
        let idx = hub_stress_index();
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

    // ── CR-198: prefix filtering is exact against the all-pairs oracle ───────

    /// The **pre-[CR-198] all-pairs counter**, kept verbatim as a test-only
    /// oracle (its keyspace sharding dropped: sharding never changed a count, so
    /// one map is the same algorithm). It counts the shared shingles of every pair
    /// co-occurring in any posting and unions those whose Jaccard meets the
    /// threshold. Also returns the pair visits it made — Σ C(|posting|, 2), the
    /// quadratic work [`ClusterWork`] replaces.
    ///
    /// [CR-198]: ../../../docs/requests/CR-198-near-clone-clustering-is-exact-under-prefix-filtering.md
    fn all_pairs_oracle(
        index: &[(NodeId, u64)],
        threshold: f64,
        min_shingles: usize,
    ) -> (CloneClustering, u64) {
        let mut sizes: BTreeMap<NodeId, usize> = BTreeMap::new();
        for &(node, _) in index {
            *sizes.entry(node).or_insert(0) += 1;
        }
        let mut postings: BTreeMap<u64, Vec<NodeId>> = BTreeMap::new();
        for &(node, hash) in index {
            if sizes.get(&node).is_some_and(|&size| size >= min_shingles) {
                postings.entry(hash).or_default().push(node);
            }
        }
        let mut visits = 0u64;
        let mut counts: HashMap<(NodeId, NodeId), usize> = HashMap::new();
        for nodes in postings.values() {
            for (i, &a) in nodes.iter().enumerate() {
                for &b in &nodes[i + 1..] {
                    visits += 1;
                    let pair = if a <= b { (a, b) } else { (b, a) };
                    *counts.entry(pair).or_insert(0) += 1;
                }
            }
        }
        let mut uf = UnionFind::default();
        for (&(a, b), &intersection) in &counts {
            let union = sizes[&a] + sizes[&b] - intersection;
            let jaccard = intersection as f64 / union as f64;
            if jaccard >= threshold {
                uf.union(a, b);
            }
        }
        let nodes: Vec<NodeId> = uf.parent.keys().copied().collect();
        let mut group_of: BTreeMap<NodeId, NodeId> = BTreeMap::new();
        let mut roots: BTreeSet<NodeId> = BTreeSet::new();
        for node in nodes {
            let root = uf.find(node);
            group_of.insert(node, root);
            roots.insert(root);
        }
        (
            CloneClustering {
                group_of,
                group_count: roots.len(),
            },
            visits,
        )
    }

    /// One rayon pool per worker count, built once and reused across a test.
    fn pools(workers: &[usize]) -> Vec<(usize, rayon::ThreadPool)> {
        workers
            .iter()
            .map(|&n| {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(n)
                    .build()
                    .expect("pool builds");
                (n, pool)
            })
            .collect()
    }

    /// Assert the prefix-filtered clustering equals the all-pairs oracle on
    /// `idx` at every `(threshold, floor)`, twice per worker count. Returns how
    /// many of those settings formed at least one group, so a caller can refuse
    /// a vacuous fixture set.
    fn assert_matches_oracle(
        label: &str,
        idx: &[(NodeId, u64)],
        thresholds: &[f64],
        floors: &[usize],
        pools: &[(usize, rayon::ThreadPool)],
    ) -> usize {
        let mut grouped = 0;
        for &threshold in thresholds {
            for &floor in floors {
                let (expected, _) = all_pairs_oracle(idx, threshold, floor);
                if expected.group_count() > 0 {
                    grouped += 1;
                }
                for (workers, pool) in pools {
                    for run in 0..2 {
                        let got = pool.install(|| cluster_with(idx, threshold, floor));
                        assert_eq!(
                            got, expected,
                            "{label}: threshold {threshold}, floor {floor}, \
                             {workers} workers, run {run} differs from all-pairs"
                        );
                    }
                }
            }
        }
        grouped
    }

    /// Two functions whose Jaccard is exactly `shared / (|A| + |B| − shared)`:
    /// `shared` common shingles, then `extra_a` and `extra_b` of their own. Each
    /// side's own shingles have document frequency 1 and so rank FIRST in the
    /// global order — the arrangement that puts the shared shingles as deep in
    /// each prefix as they can go.
    fn pair_fixture(shared: u64, extra_a: u64, extra_b: u64) -> Vec<(NodeId, u64)> {
        let mut a = set(100, shared);
        a.extend(set(10_000, extra_a));
        let mut b = set(100, shared);
        b.extend(set(20_000, extra_b));
        index(&[(10, &a), (20, &b)])
    }

    /// The near-clone fixtures of this module and of `annotate/tests.rs`,
    /// rebuilt by shape, as `(label, index)`.
    fn every_clone_fixture() -> Vec<(&'static str, Vec<(NodeId, u64)>)> {
        let with = |mut base: Vec<u64>, extra: &[u64]| {
            base.extend_from_slice(extra);
            base
        };
        vec![
            ("empty", Vec::new()),
            (
                "identical pair + unrelated",
                index(&[(10, &set(100, 20)), (20, &set(100, 20)), (30, &set(900, 20))]),
            ),
            (
                "near pair (one edited shingle each)",
                index(&[(10, &with(set(100, 20), &[500])), (20, &with(set(100, 20), &[501]))]),
            ),
            ("far pair (Jaccard 0.5)", pair_fixture(20, 10, 10)),
            ("just above 0.85 (18/21)", pair_fixture(18, 2, 1)),
            ("just below 0.85 (17/21)", pair_fixture(17, 3, 1)),
            (
                "transitive triple",
                index(&[(30, &set(100, 20)), (20, &set(100, 20)), (10, &set(100, 20))]),
            ),
            (
                "below and at the floor",
                index(&[
                    (10, &set(100, 10)),
                    (20, &set(100, 10)),
                    (30, &set(500, 11)),
                    (40, &set(500, 11)),
                ]),
            ),
            (
                "duplicate-floor mix (long pair, unrelated, short pair)",
                index(&[
                    (1, &set(100, 20)),
                    (2, &set(100, 20)),
                    (3, &set(900, 20)),
                    (4, &set(300, 5)),
                    (5, &set(300, 5)),
                ]),
            ),
            ("hub stress (two groups + solos on one hub)", hub_stress_index()),
            ("exactly 0.85 (17/20)", pair_fixture(17, 1, 2)),
            ("exactly 0.85 as a subset (17 ⊂ 20)", pair_fixture(17, 3, 0)),
            ("exactly 0.56 as a subset (14 ⊂ 25)", pair_fixture(14, 11, 0)),
        ]
    }

    /// CR-198 / [NFR-RA-06]: on every clone fixture, at the default and at
    /// re-tuned thresholds and floors, across worker counts and runs, the
    /// prefix-filtered clustering is identical to the all-pairs oracle.
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    #[test]
    fn prefix_filtering_matches_the_all_pairs_oracle_on_every_clone_fixture() {
        let pools = pools(&[1, 2, 4, 8]);
        let thresholds = [0.28, 0.5, 0.56, 0.7, DEFAULT_CLONE_SIMILARITY, 0.9, 0.95, 1.0];
        let floors = [1, 5, MIN_CLONE_SHINGLES, 21];
        let mut grouped = 0;
        for (label, idx) in every_clone_fixture() {
            grouped += assert_matches_oracle(label, &idx, &thresholds, &floors, &pools);
        }
        assert!(grouped >= 40, "the fixtures must actually form groups ({grouped})");
    }

    /// A deterministic xorshift64 stream for the randomised oracle tables.
    struct XorShift(u64);

    impl XorShift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// A seeded random shingle table shaped like a real one: functions drawn
    /// from a few families (a shared body each, with shingles dropped and a few
    /// mid-frequency extras added, so pairs straddle every threshold), plus hub
    /// shingles most functions carry, as test boilerplate does.
    fn random_table(seed: u64) -> Vec<(NodeId, u64)> {
        let mut rng = XorShift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let families: Vec<Vec<u64>> = (0..12)
            .map(|f| set(100_000 + f * 1_000, 8 + rng.below(50)))
            .collect();
        let mut rows: Vec<(i64, Vec<u64>)> = Vec::new();
        for id in 1..=220 {
            let family = &families[rng.below(families.len() as u64) as usize];
            let drop_one_in = 4 + rng.below(30);
            let mut hashes: Vec<u64> =
                family.iter().copied().filter(|_| rng.below(drop_one_in) != 0).collect();
            for _ in 0..rng.below(6) {
                hashes.push(rng.below(400));
            }
            for hub in 0..4 {
                if rng.below(4) != 0 {
                    hashes.push(9_000_000 + hub);
                }
            }
            rows.push((id, hashes));
        }
        let refs: Vec<(i64, &[u64])> = rows.iter().map(|(id, hs)| (*id, hs.as_slice())).collect();
        index(&refs)
    }

    /// CR-198: on seeded random tables — near-threshold pairs, transitive chains,
    /// hub shingles — the prefix-filtered clustering equals the all-pairs oracle
    /// at every threshold and floor, across worker counts.
    #[test]
    fn prefix_filtering_matches_the_all_pairs_oracle_on_random_tables() {
        let pools = pools(&[1, 4]);
        let thresholds = [0.3, 0.56, 0.7, 0.8, DEFAULT_CLONE_SIMILARITY, 0.9, 1.0];
        let floors = [1, MIN_CLONE_SHINGLES, 30];
        let mut grouped = 0;
        for seed in 1..=8 {
            let idx = random_table(seed);
            grouped += assert_matches_oracle(
                &format!("random table seed {seed}"),
                &idx,
                &thresholds,
                &floors,
                &pools,
            );
        }
        assert!(grouped >= 100, "the random tables must actually form groups ({grouped})");
    }

    /// CR-198 / UAT-QM-12: a pair whose Jaccard is **exactly** the 0.85 default
    /// groups and a pair just below it does not, as under the all-pairs counter.
    /// The subset case is the tight one for the prefix: `|A| = 20`, `B ⊂ A` with
    /// 17 shingles, so `A` needs all 17 and its prefix is `20 − 17 + 1 = 4` — its
    /// 3 own (rarest) shingles plus the first shared one. A prefix one shorter
    /// sees only A's own shingles and loses the pair.
    #[test]
    fn a_pair_at_exactly_the_threshold_groups_and_just_below_does_not() {
        let t = DEFAULT_CLONE_SIMILARITY;
        for (label, idx) in [
            ("17/20", pair_fixture(17, 1, 2)),
            ("17 ⊂ 20", pair_fixture(17, 3, 0)),
            ("34/40", pair_fixture(34, 2, 4)),
            ("34 ⊂ 40", pair_fixture(34, 6, 0)),
        ] {
            let got = cluster_with(&idx, t, MIN_CLONE_SHINGLES);
            assert_eq!(got.group_of(NodeId(20)), Some(NodeId(10)), "{label} is exactly {t}: it pairs");
            assert_eq!(got, all_pairs_oracle(&idx, t, MIN_CLONE_SHINGLES).0, "{label}");
        }
        for (label, idx) in [
            ("17/21", pair_fixture(17, 3, 1)),
            ("16 ⊂ 19 (0.842)", pair_fixture(16, 3, 0)),
            ("33/39 (0.846)", pair_fixture(33, 3, 3)),
        ] {
            let got = cluster_with(&idx, t, MIN_CLONE_SHINGLES);
            assert_eq!(got.group_count(), 0, "{label} is just below {t}: no group");
            assert_eq!(got, all_pairs_oracle(&idx, t, MIN_CLONE_SHINGLES).0, "{label}");
        }
    }

    /// CR-198: the prefix length and the length filter never trust a rounded
    /// product. `0.56 · 25` rounds to `14.000000000000002`, so `⌈0.56 · 25⌉ = 15`,
    /// yet `14 / 25` passes `0.56`. A 14-shingle subset of a 25-shingle set is
    /// therefore a pair at exactly the threshold; a prefix or a length bound taken
    /// from the product drops it. The same at `0.28 · 25` and `0.55 · 100`.
    #[test]
    fn a_rounded_product_never_tightens_the_prefix_or_the_length_filter() {
        for (t, shared, size) in [(0.56, 14u64, 25u64), (0.28, 7, 25), (0.55, 55, 100)] {
            assert!(
                (t * size as f64).ceil() as u64 > shared,
                "the fixture must sit where the product rounds up ({t} · {size})"
            );
            assert_eq!(min_overlap(size as usize, t), Some(shared as usize), "o({size}) at {t}");
            let idx = pair_fixture(shared, size - shared, 0);
            let got = cluster_with(&idx, t, 1);
            assert_eq!(
                got.group_of(NodeId(20)),
                Some(NodeId(10)),
                "{shared} ⊂ {size} is exactly {t}: it pairs"
            );
            assert_eq!(got, all_pairs_oracle(&idx, t, 1).0);
        }
    }

    /// CR-198 / CR-013: re-tuning `clone_similarity` or `clone_min_tokens`
    /// re-derives the prefix and the floor — nothing is cached from the defaults
    /// — and moves the verdicts exactly as the all-pairs counter does.
    #[test]
    fn retuned_thresholds_re_derive_the_prefix_and_the_floor() {
        // The prefix length follows the similarity: o(20) is 17 at 0.85, 10 at 0.5.
        assert_eq!(min_overlap(20, DEFAULT_CLONE_SIMILARITY), Some(17));
        assert_eq!(min_overlap(20, 0.5), Some(10));
        assert_eq!(min_overlap(20, 1.0), Some(20));
        assert_eq!(min_overlap(20, 1.5), None, "no overlap passes a threshold above 1");
        assert_eq!(min_overlap(0, 0.5), None);

        // Jaccard 0.5: no group at the default, a group at 0.5, none at 0.95.
        let far = pair_fixture(20, 10, 10);
        for (similarity, groups) in [(DEFAULT_CLONE_SIMILARITY, 0), (0.5, 1), (0.95, 0)] {
            let got = super::cluster(&far, similarity, DEFAULT_CLONE_MIN_TOKENS);
            assert_eq!(got.group_count(), groups, "Jaccard 0.5 at threshold {similarity}");
            let floor = min_shingles_for(DEFAULT_CLONE_MIN_TOKENS);
            assert_eq!(got, all_pairs_oracle(&far, similarity, floor).0);
        }

        // Five identical shingles: below the default floor, eligible at a 1-token floor.
        let short = index(&[(10, &set(100, 5)), (20, &set(100, 5))]);
        for (min_tokens, groups) in [(DEFAULT_CLONE_MIN_TOKENS, 0), (1, 1)] {
            let got = super::cluster(&short, DEFAULT_CLONE_SIMILARITY, min_tokens);
            assert_eq!(got.group_count(), groups, "clone_min_tokens {min_tokens}");
            let floor = min_shingles_for(min_tokens);
            assert_eq!(got, all_pairs_oracle(&short, DEFAULT_CLONE_SIMILARITY, floor).0);
        }
    }

    /// The hub-shingle table: `n` functions with distinct 40-shingle bodies, all
    /// carrying the one `hub` shingle, and every tenth function followed by a
    /// copy of its body (a genuine clone pair), so the run has real work to
    /// verify. Body hashes sit in `1_000_000..`, so a `hub` of 0 ranks below every
    /// body by hash value and only its document frequency can sort it last.
    fn hub_table(n: i64, hub: u64) -> Vec<(NodeId, u64)> {
        let mut rows: Vec<(i64, Vec<u64>)> = Vec::new();
        for id in 1..=n {
            let body_of = if id % 10 == 0 { id - 1 } else { id };
            let mut hashes = set(1_000_000 + body_of as u64 * 100, 40);
            hashes.push(hub);
            rows.push((id, hashes));
        }
        let refs: Vec<(i64, &[u64])> = rows.iter().map(|(id, hs)| (*id, hs.as_slice())).collect();
        index(&refs)
    }

    /// CR-198 / [FR-AN-06]: a hub shingle shared by *n* functions adds no *n*²
    /// work. At *n* = 1,000 and 2,000 the prefix-filtered run stays within a
    /// linear budget and grows by well under the 4× a doubling costs a quadratic
    /// counter, with the verdicts the all-pairs oracle gives. The oracle's own
    /// work — every pair of the hub posting — breaks both bounds, so the bounds
    /// discriminate: the pre-change algorithm fails this test. The hub is run at
    /// the lowest and the highest hash value, so it is the document-frequency
    /// half of the global order — not where the hub's hash happens to sort —
    /// that keeps it out of the prefixes.
    ///
    /// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
    #[test]
    fn a_hub_shingle_grows_candidate_work_sub_quadratically() {
        for hub in [0, u64::MAX] {
            assert_hub_work_is_sub_quadratic(hub);
        }
    }

    /// The body of [`a_hub_shingle_grows_candidate_work_sub_quadratically`] for
    /// one `hub` hash.
    fn assert_hub_work_is_sub_quadratic(hub: u64) {
        let budget = |n: i64| 4 * n as u64;
        let mut work = Vec::new();
        let mut oracle_work = Vec::new();
        for n in [1_000, 2_000] {
            let idx = hub_table(n, hub);
            let (got, done) = cluster_counted(&idx, DEFAULT_CLONE_SIMILARITY, MIN_CLONE_SHINGLES);
            let (expected, visits) = all_pairs_oracle(&idx, DEFAULT_CLONE_SIMILARITY, MIN_CLONE_SHINGLES);
            assert_eq!(got, expected, "hub {hub}, n = {n}: verdicts equal all-pairs");
            assert_eq!(got.group_count(), n as u64 / 10, "hub {hub}, n = {n}: one group per pair");
            let total = done.probes + done.verified;
            assert!(total > 0, "hub {hub}, n = {n}: the planted pairs are real work");
            assert!(total <= budget(n), "hub {hub}, n = {n}: work {total} ({done:?}) exceeds 4n");
            assert!(visits > budget(n), "hub {hub}, n = {n}: all-pairs work {visits} breaks the budget");
            work.push(total);
            oracle_work.push(visits);
        }
        // Doubling n: linear work doubles, quadratic work quadruples.
        assert!(
            work[1] * 10 <= work[0] * 25,
            "hub {hub}: work grew {} → {} on doubling n: not sub-quadratic",
            work[0],
            work[1]
        );
        assert!(
            oracle_work[1] * 10 > oracle_work[0] * 25,
            "the all-pairs work {} → {} must fail the same growth bound",
            oracle_work[0],
            oracle_work[1]
        );
    }

    /// Copy every `.rs` file under `from` into `to`, keeping the relative layout.
    fn copy_rust_sources(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).expect("mkdir");
        for entry in std::fs::read_dir(from).expect("read_dir") {
            let path = entry.expect("dir entry").path();
            let target = to.join(path.file_name().expect("file name"));
            if path.is_dir() {
                copy_rust_sources(&path, &target);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                std::fs::copy(&path, &target).expect("copy");
            }
        }
    }

    /// CR-198 / [NFR-RA-06]: on a real slice of THIS repository — the annotation,
    /// governance and metrics sources, test-heavy and boilerplate-rich like the code that
    /// made the all-pairs counter quadratic — indexed end to end, the
    /// prefix-filtered clustering of the persisted shingle table equals the
    /// all-pairs oracle at the default and re-tuned thresholds, across worker
    /// counts. (The whole repository's table is compared at sprint time, on a
    /// `git archive` export: the oracle's pair map alone needs gigabytes there.)
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    #[test]
    fn prefix_filtering_matches_the_all_pairs_oracle_on_a_slice_of_this_repository() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for module in ["annotate", "governance", "metrics"] {
            copy_rust_sources(&src.join(module), &tmp.path().join("src").join(module));
        }
        let engine = crate::Engine::start(tmp.path()).expect("engine starts");
        let result = engine.index();
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        let shingles = engine
            .runtime()
            .expect("runtime")
            .submit_read(|store| store.shingle_index())
            .expect("shingle index reads");
        let functions: BTreeSet<NodeId> = shingles.iter().map(|&(node, _)| node).collect();
        eprintln!("repository slice: {} shingled functions", functions.len());
        assert!(functions.len() >= 300, "a real slice: {} shingled functions", functions.len());

        let pools = pools(&[1, 4]);
        let thresholds = [0.5, 0.7, DEFAULT_CLONE_SIMILARITY, 0.95];
        let grouped =
            assert_matches_oracle("repository slice", &shingles, &thresholds, &[MIN_CLONE_SHINGLES], &pools);
        assert_eq!(grouped, thresholds.len(), "the slice forms groups at every threshold");
    }

    /// **Sprint-time measurement, not a gate test** ([CR-198]): cluster a whole
    /// shingle table exported from an indexed store and time the run serially (a
    /// one-worker pool) against the machine's worker count, asserting identical
    /// verdicts. With `LOGOS_CLONE_ORACLE=1` it also runs the all-pairs oracle —
    /// gigabytes on this repository — and asserts equality.
    ///
    /// `LOGOS_CLONE_SHINGLES` names a TSV of `node_id<TAB>hash` rows, e.g.
    /// `sqlite3 -separator $'\t' .logos/logos.db 'SELECT node_id, hash FROM shingles'`
    /// (the signed `hash` is the stored form of the `u64`).
    ///
    /// [CR-198]: ../../../docs/requests/CR-198-near-clone-clustering-is-exact-under-prefix-filtering.md
    #[test]
    #[ignore = "sprint-time measurement: set LOGOS_CLONE_SHINGLES to an exported shingle TSV"]
    fn measure_serial_against_parallel_clustering_on_an_exported_table() {
        let path = std::env::var("LOGOS_CLONE_SHINGLES").expect("LOGOS_CLONE_SHINGLES is set");
        let text = std::fs::read_to_string(&path).expect("table reads");
        let mut idx: Vec<(NodeId, u64)> = text
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let (node, hash) = line.split_once('\t').expect("node<TAB>hash");
                (
                    NodeId(node.parse().expect("node id")),
                    hash.parse::<i64>().expect("hash") as u64,
                )
            })
            .collect();
        idx.sort_unstable();
        let workers = std::thread::available_parallelism().map_or(4, usize::from);
        let mut baseline: Option<CloneClustering> = None;
        for (n, pool) in pools(&[1, workers]) {
            for run in 0..3 {
                let started = std::time::Instant::now();
                let (got, work) = pool.install(|| {
                    cluster_counted(&idx, DEFAULT_CLONE_SIMILARITY, MIN_CLONE_SHINGLES)
                });
                println!(
                    "workers={n} run={run} ms={} rows={} groups={} cloned={} {work:?}",
                    started.elapsed().as_millis(),
                    idx.len(),
                    got.group_count(),
                    got.cloned_count()
                );
                match &baseline {
                    Some(first) => assert_eq!(&got, first, "workers={n} run={run}"),
                    None => baseline = Some(got),
                }
            }
        }
        if std::env::var("LOGOS_CLONE_ORACLE").is_ok_and(|v| v == "1") {
            let started = std::time::Instant::now();
            let (expected, visits) =
                all_pairs_oracle(&idx, DEFAULT_CLONE_SIMILARITY, MIN_CLONE_SHINGLES);
            println!("oracle ms={} pair_visits={visits}", started.elapsed().as_millis());
            assert_eq!(baseline.as_ref(), Some(&expected), "all-pairs oracle differs");
        }
    }
}
