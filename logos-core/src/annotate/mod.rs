//! Pass 3 of the pipeline — the annotation engine ([annotation-engine],
//! S-014, [ADR-10]).
//!
//! [`run`] computes four whole-graph annotations over a consistent snapshot
//! and writes them to **native node columns** ([FR-AN-04], no sidecar table):
//!
//! 0. **Test classification** ([FR-AN-05], [CR-001]) — `is_test` is the
//!    disjunction of three *positive* signals (never call-graph inference,
//!    [ADR-18]): the persisted extraction-time test-marker evidence
//!    ([FR-EX-06], S-027), a test path convention (`tests/`, `*_test.*`,
//!    `*.spec.*`, …), or a `[semantics].test_markers` match. Computed first
//!    because it feeds the dead-code live-root set below, and persisted as the
//!    single source of truth every other test-aware detector reads — metrics
//!    scope ([FR-QM-08]), `test_gaps` ([FR-GV-08]), dead-code roots
//!    ([FR-AN-01]).
//! 1. **Dead-code** ([FR-AN-01]) — reachability over `calls` (+ `routes_to`)
//!    from the live root set: every *exported* declaration (exported-is-live),
//!    every framework `route` node, every `[semantics].entry_points` name, and
//!    every `is_test = true` node (a test is a live root by construction, so an
//!    unreferenced test helper is never `is_dead`, [FR-AN-01]/[CR-001]).
//!    A `function`/`method` outside the live set is `is_dead = true` — **but
//!    only for a language that declares the reachability capability** ([CR-043],
//!    [ADR-39]). A callable whose language does not declare it (its binder
//!    coverage is unproven, so reachability over a partial edge set is not a
//!    trustworthy signal) renders `is_dead = NULL` ("not computed", [NFR-CC-04])
//!    rather than a fabricated `true`. The walk is unchanged; the capability
//!    only gates which callables earn a verdict, so a capable language is
//!    byte-identical to before the gate.
//! 2. **Duplicates** ([FR-AN-02]) — `function`/`method` nodes sharing a
//!    normalised AST-shape fingerprint (captured by Pass 1, identifiers /
//!    whitespace / comments stripped) are all `is_duplicate = true`.
//! 2b. **Near-clones** ([FR-AN-06], [CR-005]) — a deterministic union-find over
//!    the id-ordered inverted shingle index ([FR-EX-09]) groups functions whose
//!    Jaccard shingle similarity meets the clone threshold; each member's
//!    `clone_group` column records the stable group id (the component's minimum
//!    node id). This runs **beside** exact-duplicate detection ([ADR-21]) and
//!    never touches `is_duplicate` or the Redundancy inputs — near and exact
//!    detection are separate columns (see [`clone`]).
//! 3. **Layer membership** ([FR-AN-03]) — `rules.toml` `[[layers]]` globs
//!    assign each file (and so each node) its layer; `layer` / `boundary`
//!    policy nodes are materialised (`derived = 1`), and every dependency edge
//!    crossing a `[[boundaries]]` declaration is flagged with a derived
//!    `forbidden_dependency` edge. The same derived edge is also materialised
//!    for every `Imports`/`References` edge a `[[forbidden_imports]]` glob pair
//!    bans ([FR-GV-12], [CR-002]) — two sources, one idempotent pass.
//!
//! # Shape: snapshot → compute → one atomic commit
//!
//! Mirrors the resolution pass ([`crate::resolve`]): one reader-pool snapshot,
//! pure in-memory compute, one writer-actor batch ([ADR-02], [NFR-RA-07]).
//!
//! **Mechanism note (component-contract deviation, ratified at review):** the
//! [annotation-engine] component sheet names the hydrated petgraph
//! ([graph-hydration], [ADR-05]) as the reachability substrate. This pass
//! instead BFS-walks a direct `calls`/`routes_to` adjacency map built from the
//! same id-ordered edge snapshot — behaviourally identical and deterministic —
//! because the Engine-owned hydration *cache* is keyed by `(scope,
//! last_sync_at)` and is not reachable from inside a pipeline run (the stamp
//! only advances after the run commits). It also needs per-*node* identity for
//! the verdict columns, where the symbol-level [`GraphView`](crate::hydrate)
//! collapses same-symbol nodes onto one vertex. [ADR-05]'s operative rule —
//! whole-graph traversal in memory, never recursive SQL — is honoured.
//! The batch **clears all derived nodes/edges first** and re-materialises
//! them, so the pass is idempotent — re-running never accumulates stale
//! policy artifacts ([annotation-engine] "clears derived edges at the start
//! of each check"). Derived artifacts from the previous run are filtered out
//! of the compute inputs for the same reason.
//!
//! # Honesty posture
//!
//! Annotations inherit resolution honesty ([NFR-RA-05]): a `calls` edge the
//! resolver could not bind keeps its callee out of the reachable set, so
//! exported-is-live exists precisely to bias that failure mode toward *false
//! live* (a missed dead flag) rather than *false dead* ([AR-05]). Verdict
//! columns are tri-state: `NULL` means "not computed / not applicable", never
//! a silent `false` ([NFR-CC-04]).
//!
//! # Determinism ([NFR-RA-06])
//!
//! The snapshot arrives in `id` order, duplicate groups use a `BTreeMap`,
//! layer assignment is first-glob-wins in declaration order, and policy
//! nodes/edges are inserted in declaration/snapshot order — same graph + same
//! `rules.toml` always produces byte-identical annotations.
//!
//! [annotation-engine]: ../../../docs/specs/architecture/components/annotation-engine.md
//! [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
//! [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
//! [AR-05]: ../../../docs/specs/architecture.md#13-risk-register
//! [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
//! [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
//! [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
//! [FR-AN-04]: ../../../docs/specs/requirements/FR-AN-04.md
//! [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
//! [FR-EX-06]: ../../../docs/specs/requirements/FR-EX-06.md
//! [FR-GV-08]: ../../../docs/specs/requirements/FR-GV-08.md
//! [FR-GV-12]: ../../../docs/specs/requirements/FR-GV-12.md
//! [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
//! [ADR-18]: ../../../docs/specs/architecture/decisions/ADR-18.md
//! [CR-001]: ../../../docs/requests/CR-001-test-aware-quality-metrics.md
//! [CR-002]: ../../../docs/requests/CR-002-extended-architecture-contracts.md
//! [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md
//! [ADR-39]: ../../../docs/specs/architecture/decisions/ADR-39.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
//! [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::config::Rules;
use crate::extract::escape_name;
use crate::graph_store::{AnnotationNodeRow, EdgeRow, FileRecord, NewNode};
use crate::model::{EdgeKind, LogosSymbol, NodeId, NodeKind};
use crate::models::pipeline::AnnotationStats;
use crate::runtime::Runtime;

/// The consistent graph state one annotation run computes over.
struct Snapshot {
    nodes: Vec<AnnotationNodeRow>,
    edges: Vec<EdgeRow>,
    files: Vec<FileRecord>,
    /// The inverted near-clone shingle index ([FR-EX-09]) in `(node_id, hash)`
    /// order — the input to near-clone clustering ([FR-AN-06], [`clone`]).
    ///
    /// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
    /// [FR-EX-09]: ../../../docs/specs/requirements/FR-EX-09.md
    shingles: Vec<(NodeId, u64)>,
}

/// One node's computed annotation verdicts, ready to commit.
struct NodeVerdict {
    id: NodeId,
    is_dead: Option<bool>,
    is_duplicate: Option<bool>,
    is_test: bool,
    layer: Option<String>,
    /// The stable near-clone group id (the component minimum node id), or `None`
    /// when the function is in no near-clone group ([FR-AN-06]).
    ///
    /// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
    clone_group: Option<NodeId>,
    /// Whether this verdict *differs from the stored snapshot value* — the
    /// incremental-commit selector (S-024-HF). `false` means the recompute
    /// reproduced what the graph already holds, so the write is a no-op and is
    /// skipped on a `sync` (a full `index` writes regardless).
    changed: bool,
}

/// A policy node to materialise (`derived = 1`).
struct PolicyNode {
    symbol: LogosSymbol,
    kind: NodeKind,
    name: String,
}

/// Run the annotation pass: snapshot, compute, and commit atomically.
///
/// `entry_points` is `[semantics].entry_points` (matched against node names);
/// `test_markers` is `[semantics].test_markers` (the third `is_test` disjunct,
/// [FR-AN-05]); `rules` is the loaded `rules.toml` architecture contract
/// (defaulted when the file is absent — no layers means no policy nodes and
/// `NULL` layer membership everywhere).
///
/// `reachable_exts` is the set of file extensions (normalised lower-case, no
/// dot) whose language declares the *reachability capability* ([CR-043],
/// [ADR-39], built by [`LanguageRegistry::reachability_extensions`]). A callable
/// whose file extension is **not** in the set emits `is_dead = NULL` ("not
/// computed", [NFR-CC-04]) instead of a fabricated verdict — the dead-code pass
/// is honest only for languages whose binder coverage is proven ([FR-AN-01]).
/// The reachability walk itself is unchanged: it still runs over every bound
/// edge, so a capable language's verdict is byte-identical to before the gate
/// ([NFR-RA-06]); the gate only decides which callables earn a `Some` verdict.
///
/// [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md
/// [ADR-39]: ../../../docs/specs/architecture/decisions/ADR-39.md
/// [`LanguageRegistry::reachability_extensions`]: ../plugin/struct.LanguageRegistry.html#method.reachability_extensions
///
/// # Incremental commit (S-024-HF)
///
/// `incremental` is `true` on a `sync`/`reconcile` and `false` on a full
/// `index`. The **compute is always whole-graph** — dead-code reachability,
/// duplicate fingerprint groups, and near-clone clusters are graph-global, so a
/// one-file change can legitimately flip a verdict on an *untouched* node, and
/// only a whole-graph recompute sees that (the equivalence invariant
/// `tests/indexing.rs::assert_sync_matches_reindex` pins it). What `incremental`
/// changes is the **commit**: the verdict of every node is recomputed, but only
/// the nodes whose verdict actually *differs from the stored snapshot value* are
/// written back. Because the snapshot already holds each unchanged node's correct
/// verdict (from its last index/sync), skipping the no-op writes leaves the graph
/// byte-identical to a full re-annotate while turning the commit from O(graph)
/// `UPDATE`s into O(changed) — the cost the [NFR-PE-03] single-file-sync budget
/// could not absorb. A full `index` writes every verdict (`incremental = false`),
/// keeping the cold path's whole-graph commit unchanged.
///
/// [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
///
/// # Errors
/// Returns an error if the snapshot read, a layer glob compilation, a policy
/// symbol assembly, or the commit batch fails (the batch rolls back
/// wholesale, [NFR-RA-07]).
///
/// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
pub fn run(
    runtime: &Runtime,
    rules: &Rules,
    entry_points: &[String],
    test_markers: &[String],
    reachable_exts: &HashSet<String>,
    incremental: bool,
) -> Result<AnnotationStats> {
    let snap = runtime.submit_read(|store| {
        Ok(Snapshot {
            nodes: store.annotation_nodes()?,
            edges: store.all_edges()?,
            files: store.indexed_files()?,
            shingles: store.shingle_index()?,
        })
    })?;

    // Previous-run derived artifacts are cleared in the commit below; exclude
    // them (and edges touching them) from the compute inputs so the pass is
    // a pure function of the *extracted* graph + rules.
    let derived_ids: HashSet<NodeId> = snap
        .nodes
        .iter()
        .filter(|n| n.derived)
        .map(|n| n.id)
        .collect();
    let nodes: Vec<&AnnotationNodeRow> = snap.nodes.iter().filter(|n| !n.derived).collect();
    let edges: Vec<&EdgeRow> = snap
        .edges
        .iter()
        .filter(|e| {
            e.kind != EdgeKind::ForbiddenDependency
                && !derived_ids.contains(&e.source)
                && !derived_ids.contains(&e.target)
        })
        .collect();

    // ── Compute ──────────────────────────────────────────────────────────
    // is_test first: it is a dead-code live-root input (FR-AN-01) as well as a
    // persisted verdict (FR-AN-05). Positive evidence only — never inferred
    // from call relationships (ADR-18).
    //
    // Note (S-229): only near-clone clustering and the per-node verdict loop below
    // run on the shared worker pool. is_test, dead-code reachability (`live_set`),
    // and exact-duplicate detection (`duplicate_set`) are left serial by measured
    // decision — on the Logos repo they cost ≤ a few milliseconds each (vs. ~1.9 s
    // for clustering), and the reachability BFS is inherently sequential; adding
    // fork/join there would cost more than it saves (CR-057 measure-first).
    let test_ids: HashSet<NodeId> = nodes
        .iter()
        .filter(|n| is_test_marked(n, test_markers))
        .map(|n| n.id)
        .collect();
    let live = live_set(&nodes, &edges, entry_points, &test_ids);
    let duplicate_ids = duplicate_set(&nodes);
    // Near-clone clustering (FR-AN-06, CR-005): a pure function of the inverted
    // shingle index, computed beside — never inside — exact-duplicate detection
    // (ADR-21), so the `is_duplicate` set above is unaffected. CR-013: the
    // similarity threshold and token floor come from the effective
    // `[metric_thresholds]` set — the SAME hashed set the gate scores under — so
    // tuning either re-baselines the gate exactly like a structural threshold.
    let clone_thresholds = rules.metric_thresholds.effective();
    // Near-clone clustering is the dominant annotation cost on a real repo
    // (S-229): its O(Σ|posting|²) counting fans out across the core-owned shared
    // worker pool. Running it inside `worker_pool().install(…)` pins the rayon
    // parallelism to that one pool — exactly as extraction and file-load do
    // (AQ-04) — rather than spawning a competing global pool. The verdicts stay
    // byte-identical across worker counts (NFR-RA-06, see [`clone`]).
    let clone_clusters = runtime.worker_pool().install(|| {
        clone::cluster(
            &snap.shingles,
            clone_thresholds.clone_similarity,
            clone_thresholds.clone_min_tokens,
        )
    });
    let file_layers = assign_file_layers(rules, &snap.files)?;
    let layer_by_file: HashMap<i64, &str> = file_layers
        .iter()
        .filter_map(|(id, layer)| layer.as_deref().map(|l| (*id, l)))
        .collect();

    // Per-node verdict computation (S-229): each node's verdict is an independent
    // pure function of the shared read-only inputs (`live`, `duplicate_ids`,
    // `test_ids`, `layer_by_file`, `clone_clusters`), so it maps across the same
    // core-owned worker pool. `par_iter().collect()` preserves input order — and
    // `nodes` arrives in `id` order — so the verdict vector is byte-identical to
    // the serial loop regardless of worker count (NFR-RA-06). The `stats` counters
    // are then folded **serially** over that ordered vector: integer accumulation
    // is exact, and a serial fold keeps the tallies trivially deterministic.
    let verdicts: Vec<NodeVerdict> = runtime.worker_pool().install(|| {
        nodes
            .par_iter()
            .map(|node| {
                let callable = matches!(node.kind, NodeKind::Function | NodeKind::Method);
                // Tri-state honesty (NFR-CC-04): only callables get a dead-code
                // verdict, and only fingerprinted callables get a duplicate verdict
                // — everything else stays NULL rather than a fake `false`.
                // Dead-code is further gated on the language declaring the
                // reachability capability (CR-043, ADR-39): a callable whose
                // language cannot reliably bind its calls renders NULL ("not
                // computed") rather than a fabricated `true`, so the binding gap
                // never masquerades as dead code (FR-AN-01).
                let is_dead = (callable
                    && reachability_capable(node.file_path.as_deref(), reachable_exts))
                .then(|| !live.contains(&node.id));
                let is_duplicate = node
                    .fingerprint
                    .is_some()
                    .then(|| duplicate_ids.contains(&node.id));
                let layer = node
                    .file_id
                    .and_then(|f| layer_by_file.get(&f))
                    .map(|l| (*l).to_string());
                // is_test is a definite boolean (FR-AN-05): membership in the set
                // computed above, not a tri-state verdict.
                let is_test = test_ids.contains(&node.id);
                // Near-clone group membership (FR-AN-06): the stable group id (the
                // component minimum node id), or None when the function is in no
                // group. Written for every node — None included — so a re-pass
                // clears stale membership (idempotent, NFR-RA-06).
                let clone_group = clone_clusters.group_of(node.id);

                // The incremental-commit selector (S-024-HF): does the recomputed
                // verdict differ from what the snapshot already stores? A
                // whole-graph recompute catches cross-file flips on untouched nodes
                // (so a far node going dead *is* written), while a node whose
                // verdict is unchanged is a no-op write skipped on a `sync`.
                let changed = is_dead != node.is_dead
                    || is_duplicate != node.is_duplicate
                    || is_test != node.is_test
                    || layer.as_deref() != node.layer_membership.as_deref()
                    || clone_group != node.clone_group;

                NodeVerdict {
                    id: node.id,
                    is_dead,
                    is_duplicate,
                    is_test,
                    layer,
                    clone_group,
                    changed,
                }
            })
            .collect()
    });

    let mut stats = AnnotationStats::default();
    for v in &verdicts {
        stats.nodes_annotated += 1;
        stats.dead += u64::from(v.is_dead == Some(true));
        stats.duplicates += u64::from(v.is_duplicate == Some(true));
        stats.tests += u64::from(v.is_test);
    }
    stats.clones = clone_clusters.cloned_count();
    stats.clone_groups = clone_clusters.group_count();

    let layer_nodes = layer_policy_nodes(rules)?;
    let boundary_nodes = boundary_policy_nodes(rules)?;
    // FR-GV-12: `forbidden_dependency` edges now have two sources — boundary
    // crossings AND `[[forbidden_imports]]` glob matches. Dedupe so a pair
    // banned by both is materialised as a single derived edge (and the
    // idempotency guarantee — no duplicate derived edges on re-run — holds for
    // either source, NFR-RA-06).
    let mut forbidden = forbidden_pairs(rules, &nodes, &edges, &layer_by_file);
    let mut forbidden_seen: HashSet<(NodeId, NodeId)> = forbidden.iter().copied().collect();
    for pair in forbidden_import_pairs(rules, &nodes, &edges)? {
        if forbidden_seen.insert(pair) {
            forbidden.push(pair);
        }
    }
    stats.layer_nodes = layer_nodes.len() as u64;
    stats.boundary_nodes = boundary_nodes.len() as u64;

    // ── Commit: one transaction through the writer actor (ADR-02) ───────
    let forbidden_inserted = runtime.submit_write(move |w| {
        // Idempotency: clear last run's derived artifacts before
        // re-materialising (FR-AN-03).
        w.clear_derived()?;

        for v in &verdicts {
            // Incremental commit (S-024-HF): a `sync` writes only the verdicts that
            // changed; a full `index` writes every node. The skipped rows already
            // hold their correct verdict, so the committed graph is byte-identical
            // to a whole-graph re-annotate either way.
            if incremental && !v.changed {
                continue;
            }
            w.set_node_annotations(
                v.id,
                v.is_dead,
                v.is_duplicate,
                v.is_test,
                v.layer.as_deref(),
                v.clone_group,
            )?;
        }
        for (file_id, layer) in &file_layers {
            w.set_file_layer(*file_id, layer.as_deref())?;
        }
        for policy in layer_nodes.iter().chain(boundary_nodes.iter()) {
            let symbol_id = w.upsert_symbol(&policy.symbol)?;
            w.insert_node(&NewNode {
                derived: true,
                ..NewNode::plain(symbol_id, policy.kind, &policy.name)
            })?;
        }
        let mut inserted = 0u64;
        for (source, target) in &forbidden {
            if w.insert_derived_edge(*source, *target, EdgeKind::ForbiddenDependency)? {
                inserted += 1;
            }
        }
        Ok(inserted)
    })?;
    stats.forbidden_edges = forbidden_inserted;

    Ok(stats)
}

/// The live set ([FR-AN-01]): roots ∪ everything reachable from them over
/// `calls` and `routes_to` edges.
///
/// Roots are every exported node (exported-is-live — the public API is live
/// by definition), every framework `route` node (an entry point by
/// construction), every node whose *name* appears in `[semantics].entry_points`,
/// every node in `test_ids` (a test is a live root by construction, so an
/// unreferenced test helper is never `is_dead` — [FR-AN-01], [CR-001]), and
/// every node carrying a **framework-dispatch live-root marker** — a
/// `RoutesTo` self-edge planted by the dispatch pass ([`crate::resolve::dispatch`],
/// [CR-043], [ADR-39]) on a method invoked only through an external framework
/// (trait-impl dispatch, `#[tool]` tool dispatch). The marker keeps such a
/// live-but-uncallable method out of the dead set without fabricating a call
/// edge, preserving the false-live bias ([NFR-RA-05], [AR-05]).
///
/// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
/// [CR-001]: ../../../docs/requests/CR-001-test-aware-quality-metrics.md
/// [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md
/// [ADR-39]: ../../../docs/specs/architecture/decisions/ADR-39.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [AR-05]: ../../../docs/specs/architecture.md#13-risk-register
fn live_set(
    nodes: &[&AnnotationNodeRow],
    edges: &[&EdgeRow],
    entry_points: &[String],
    test_ids: &HashSet<NodeId>,
) -> HashSet<NodeId> {
    let entry_names: HashSet<&str> = entry_points.iter().map(String::as_str).collect();
    let mut adjacency: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    // Nodes carrying the dispatch live-root marker (a `RoutesTo` self-edge): a
    // shape no genuine framework route ever produces, so it unambiguously marks
    // a framework-dispatched entry point ([`crate::resolve::dispatch`], CR-043).
    let mut dispatch_roots: HashSet<NodeId> = HashSet::new();
    for edge in edges {
        if matches!(edge.kind, EdgeKind::Calls | EdgeKind::RoutesTo) {
            adjacency.entry(edge.source).or_default().push(edge.target);
        }
        if edge.kind == EdgeKind::RoutesTo && edge.source == edge.target {
            dispatch_roots.insert(edge.source);
        }
    }

    let mut live: HashSet<NodeId> = HashSet::new();
    let mut queue: VecDeque<NodeId> = VecDeque::new();
    for node in nodes {
        let is_root = node.exported
            || node.kind == NodeKind::Route
            || entry_names.contains(node.name.as_str())
            || test_ids.contains(&node.id)
            || dispatch_roots.contains(&node.id);
        if is_root && live.insert(node.id) {
            queue.push_back(node.id);
        }
    }
    while let Some(current) = queue.pop_front() {
        if let Some(callees) = adjacency.get(&current) {
            for &callee in callees {
                if live.insert(callee) {
                    queue.push_back(callee);
                }
            }
        }
    }
    live
}

/// Whether `file_path`'s language declares the reachability capability ([CR-043],
/// [ADR-39]) — the gate on emitting a dead-code verdict. A callable in a
/// non-declaring (or unidentifiable) language renders `is_dead = NULL` ("not
/// computed", [NFR-CC-04]) rather than a fabricated `true`, because reachability
/// over a partial bound-edge set is not a trustworthy signal there.
///
/// Keyed by the file extension (normalised lower-case, no dot, matching
/// [`LanguageRegistry::reachability_extensions`]). A node with no file path, or
/// an extension whose plugin does not declare the capability, is **not** capable
/// — NULL is the honest tri-state for "not declared", never a silent verdict.
///
/// [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md
/// [ADR-39]: ../../../docs/specs/architecture/decisions/ADR-39.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
fn reachability_capable(file_path: Option<&str>, reachable_exts: &HashSet<String>) -> bool {
    file_path
        .and_then(|p| std::path::Path::new(p).extension())
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| reachable_exts.contains(&ext))
}

/// Whether a node is test code under the unified [FR-AN-05] definition — the
/// disjunction of three **positive** signals, never call-graph inference
/// ([ADR-18]):
///
/// 1. **Extraction evidence** — the persisted `test_evidence` captured by
///    Pass 1 ([FR-EX-06], S-027), the only layer that sees in-file test
///    attributes (Rust `#[test]`/`#[cfg(test)]` modules, JS/TS `it`/`describe`
///    callbacks, …) a path/name convention cannot.
/// 2. **Test path convention** — the file matches the shared
///    [`crate::navigate::is_test_path`] heuristic (`tests/`/`test/`/`__tests__/`
///    segments, `*_test.*`, `test_*.py`, `*.spec.*`/`*.test.*`).
/// 3. **`[semantics].test_markers` match** — the node name equals a marker or
///    carries it as a `<marker>_` / `_<marker>` affix (the ratified
///    2026-06-06 surface, shared with the former `test_gaps` marking).
///
/// This is the single computation behind the persisted `is_test` column, so
/// `test_gaps` ([FR-GV-08]) and the metrics scope filter ([FR-QM-08]) — which
/// read that column — can never disagree with the annotation (CR-001 CRA-01).
///
/// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
/// [FR-EX-06]: ../../../docs/specs/requirements/FR-EX-06.md
/// [FR-GV-08]: ../../../docs/specs/requirements/FR-GV-08.md
/// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
/// [ADR-18]: ../../../docs/specs/architecture/decisions/ADR-18.md
fn is_test_marked(node: &AnnotationNodeRow, test_markers: &[String]) -> bool {
    node.test_evidence
        || node
            .file_path
            .as_deref()
            .is_some_and(crate::navigate::is_test_path)
        || test_markers.iter().any(|marker| {
            node.name == *marker
                || node.name.starts_with(&format!("{marker}_"))
                || node.name.ends_with(&format!("_{marker}"))
        })
}

/// The duplicate set ([FR-AN-02]): every callable whose fingerprint is shared
/// by at least one other callable. `BTreeMap` keeps group iteration (and so
/// the run) deterministic ([NFR-RA-06]).
///
/// [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn duplicate_set(nodes: &[&AnnotationNodeRow]) -> HashSet<NodeId> {
    let mut groups: BTreeMap<&str, Vec<NodeId>> = BTreeMap::new();
    for node in nodes {
        if let Some(fp) = node.fingerprint.as_deref() {
            groups.entry(fp).or_default().push(node.id);
        }
    }
    groups
        .into_values()
        .filter(|members| members.len() >= 2)
        .flatten()
        .collect()
}

/// Assign each indexed file its `[[layers]]` band ([FR-AN-03]):
/// first-matching-layer wins, in declaration order; `None` when no glob
/// matches (or no layers are declared).
///
/// # Errors
/// Returns an error if a layer glob fails validation/compilation — `rules.toml`
/// was already validated at load time, so this is defence in depth.
///
/// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
fn assign_file_layers(rules: &Rules, files: &[FileRecord]) -> Result<Vec<(i64, Option<String>)>> {
    let mut matchers = Vec::with_capacity(rules.layers.len());
    for layer in &rules.layers {
        let set = crate::config::compile_globs(&layer.paths)
            .with_context(|| format!("compiling layer '{}' globs", layer.name))?;
        matchers.push((layer.name.as_str(), set));
    }

    Ok(files
        .iter()
        .map(|file| {
            let layer = matchers
                .iter()
                .find(|(_, set)| set.is_match(&file.path))
                .map(|(name, _)| (*name).to_string());
            (file.id, layer)
        })
        .collect())
}

/// The `layer` policy nodes to materialise, one per `[[layers]]` declaration
/// ([FR-AN-03]).
///
/// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
fn layer_policy_nodes(rules: &Rules) -> Result<Vec<PolicyNode>> {
    rules
        .layers
        .iter()
        .map(|layer| {
            Ok(PolicyNode {
                symbol: policy_symbol("layer", &[&layer.name])?,
                kind: NodeKind::Layer,
                name: layer.name.clone(),
            })
        })
        .collect()
}

/// The `boundary` policy nodes to materialise, one per `[[boundaries]]`
/// declaration ([FR-AN-03]). Named `from->to` for search/display.
///
/// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
fn boundary_policy_nodes(rules: &Rules) -> Result<Vec<PolicyNode>> {
    rules
        .boundaries
        .iter()
        .map(|boundary| {
            Ok(PolicyNode {
                symbol: policy_symbol("boundary", &[&boundary.from, &boundary.to])?,
                kind: NodeKind::Boundary,
                name: format!("{}->{}", boundary.from, boundary.to),
            })
        })
        .collect()
}

/// The dependency edges that cross a forbidden boundary ([FR-AN-03]): every
/// non-`contains` extracted edge whose source file sits in a boundary's
/// `from` layer and whose target file sits in its `to` layer.
///
/// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
fn forbidden_pairs(
    rules: &Rules,
    nodes: &[&AnnotationNodeRow],
    edges: &[&EdgeRow],
    layer_by_file: &HashMap<i64, &str>,
) -> Vec<(NodeId, NodeId)> {
    if rules.boundaries.is_empty() {
        return Vec::new();
    }
    let node_layer: HashMap<NodeId, &str> = nodes
        .iter()
        .filter_map(|n| {
            n.file_id
                .and_then(|f| layer_by_file.get(&f))
                .map(|layer| (n.id, *layer))
        })
        .collect();

    let mut seen: HashSet<(NodeId, NodeId)> = HashSet::new();
    let mut pairs = Vec::new();
    for edge in edges {
        // Lexical containment is not a dependency (FR-DB-06 posture).
        if edge.kind == EdgeKind::Contains {
            continue;
        }
        let (Some(&from), Some(&to)) = (node_layer.get(&edge.source), node_layer.get(&edge.target))
        else {
            continue; // an endpoint outside every layer violates no boundary
        };
        let crosses = rules
            .boundaries
            .iter()
            .any(|b| b.from == from && b.to == to);
        if crosses && seen.insert((edge.source, edge.target)) {
            pairs.push((edge.source, edge.target));
        }
    }
    pairs
}

/// The `Imports`/`References` edges a `[[forbidden_imports]]` glob pair bans
/// ([FR-GV-12]): the source file matches the `from` glob and the target file
/// matches the `to` glob. Materialised as `forbidden_dependency` edges
/// alongside boundary crossings, through the same idempotent pass (cleared +
/// rebuilt each run). v1 matches resolved intra-workspace edges — both
/// endpoints carry a file path; an external-package target is an
/// unresolved-reference-ledger row, not a graph edge, so it is out of scope
/// ([CR-002] CRA-01).
///
/// Globs are matched against the **project-root-relative** file path, the same
/// surface `[[layers]]` globs match ([FR-AN-03], [NFR-SE-04]).
///
/// # Errors
/// Returns an error if a `from`/`to` glob fails compilation — `rules.toml` is
/// validated at load, so this is defence in depth.
///
/// [FR-GV-12]: ../../../docs/specs/requirements/FR-GV-12.md
/// [CR-002]: ../../../docs/requests/CR-002-extended-architecture-contracts.md
/// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
/// [NFR-SE-04]: ../../../docs/specs/requirements/NFR-SE-04.md
fn forbidden_import_pairs(
    rules: &Rules,
    nodes: &[&AnnotationNodeRow],
    edges: &[&EdgeRow],
) -> Result<Vec<(NodeId, NodeId)>> {
    if rules.forbidden_imports.is_empty() {
        return Ok(Vec::new());
    }
    // Compile each (from, to) glob pair once for the pass (FR-GV-12).
    let mut matchers = Vec::with_capacity(rules.forbidden_imports.len());
    for fi in &rules.forbidden_imports {
        let from = crate::config::compile_globs(std::slice::from_ref(&fi.from))
            .with_context(|| format!("compiling forbidden_imports from '{}'", fi.from))?;
        let to = crate::config::compile_globs(std::slice::from_ref(&fi.to))
            .with_context(|| format!("compiling forbidden_imports to '{}'", fi.to))?;
        matchers.push((from, to));
    }
    let file_of: HashMap<NodeId, &str> = nodes
        .iter()
        .filter_map(|n| n.file_path.as_deref().map(|p| (n.id, p)))
        .collect();

    let mut seen: HashSet<(NodeId, NodeId)> = HashSet::new();
    let mut pairs = Vec::new();
    for edge in edges {
        // Only import/reference edges, never `calls` (FR-GV-12: a finer linter
        // than boundaries, which act on every dependency kind).
        if !matches!(edge.kind, EdgeKind::Imports | EdgeKind::References) {
            continue;
        }
        let (Some(&src_file), Some(&dst_file)) =
            (file_of.get(&edge.source), file_of.get(&edge.target))
        else {
            continue; // an unbound endpoint — not a resolved intra-workspace edge
        };
        let banned = matchers
            .iter()
            .any(|(from, to)| from.is_match(src_file) && to.is_match(dst_file));
        if banned && seen.insert((edge.source, edge.target)) {
            pairs.push((edge.source, edge.target));
        }
    }
    Ok(pairs)
}

/// Assemble the canonical symbol of a derived policy node:
/// `logos policy rules . <kind>/<segment>/…` — the `policy` manager namespace
/// keeps these synthetic identities disjoint from every extracted symbol, and
/// [`escape_name`] makes arbitrary `rules.toml` names assemble into a valid
/// SCIP string ([FR-AN-03]).
///
/// # Errors
/// Returns an error only if the assembled string is rejected by the SCIP
/// codec — escaping makes that unreachable for any non-empty name.
///
/// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
fn policy_symbol(kind: &str, segments: &[&str]) -> Result<LogosSymbol> {
    let mut raw = format!("logos policy rules . {kind}/");
    for segment in segments {
        raw.push_str(&escape_name(segment));
        raw.push('/');
    }
    LogosSymbol::parse(&raw).with_context(|| format!("assembling the {kind} policy symbol"))
}

mod clone;

#[cfg(test)]
mod tests;
