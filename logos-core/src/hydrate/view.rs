//! The hydrated [`GraphView`] and the SQLite→petgraph builder ([ADR-05],
//! [FR-DB-06], [NFR-RA-06]).
//!
//! A [`GraphView`] is a derived, in-memory [`petgraph::graph::DiGraph`] built
//! from a snapshot of the canonical store's nodes and edges. It is **ephemeral**
//! — never persisted, always rebuildable ([ADR-05]) — and it is what whole-graph
//! algorithms (`tarjan_scc`, `condensation`, `toposort`, hand-rolled
//! longest-path) run on, since those are awkward and slow as recursive SQL.
//!
//! # Granularities ([FR-DB-06])
//!
//! Four views, selected by [`Granularity`](super::Granularity):
//!
//! - **`ExcludeContains`** — symbol-level vertices; every dependency edge *except*
//!   the lexical [`EdgeKind::Contains`]. This is *the* dependency graph metrics
//!   and SCC run on ([FR-DB-06]: "the dependency graph used for metrics contains
//!   no `contains` edges").
//! - **`Symbol`** — symbol-level vertices; **all** edge kinds including
//!   `Contains`. The complete lexical-plus-dependency graph (used for
//!   neighbourhood explore where lexical nesting matters, never for dependency
//!   metrics). Still the **code subgraph** — non-code vertices/edges are dropped.
//! - **`File`** — file-rollup: vertices are files, dependency edges (exclude
//!   `Contains`) lifted to the file that defines each endpoint and deduplicated
//!   with a multiplicity weight.
//! - **`Module`** — module-rollup: vertices are modules; membership is derived by
//!   walking `Contains` up to the nearest enclosing module ([FR-DB-06]: "module
//!   rollup is derived via `contains`"); dependency edges (exclude `Contains`)
//!   lifted to modules and deduplicated with a weight.
//! - **`Visualization`** — symbol-level vertices that, alone among the views,
//!   **keep** the non-code layers (doc/config/artifact vertices and the
//!   cross-layer `DocReference`/`TracesTo`/`ArtifactRef`/`ArtifactBinding` edges).
//!   Presentation-only: hydrated solely by the web graph-elements accessor, never
//!   a metric/algorithm path, so the code subgraph above is untouched and the
//!   aggregate signal is byte-identical ([ADR-34], [FR-UI-08]).
//!
//! # Determinism ([NFR-RA-06])
//!
//! Vertices are created while iterating nodes in ascending-`id` order (the order
//! [`all_nodes`](crate::graph_store::GraphStore::all_nodes) guarantees) and
//! aggregated edges are emitted in sorted `(source_index, target_index)` order,
//! so the assigned [`NodeIndex`] values — and therefore the output of any graph
//! algorithm — are reproducible across runs and thread counts.
//!
//! [ADR-05]: ../../../docs/specs/architecture/decisions/ADR-05.md
//! [FR-DB-06]: ../../../docs/specs/requirements/FR-DB-06.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::{BTreeMap, HashMap, HashSet};

use petgraph::graph::{DiGraph, NodeIndex};

use crate::graph_store::{EdgeRow, NodeRow};
use crate::model::{EdgeKind, NodeId, NodeKind};

use super::Granularity;

/// The sentinel vertex key for a node bound to no file / no module — keeps a
/// rollup edge meaningful rather than silently dropping the endpoint.
const UNBOUND: &str = "<unbound>";

/// Defensive cap on the `Contains`-ancestry walk when resolving module
/// membership. Lexical containment is a tree (no cycles), so this is only ever
/// reached on a corrupt store; it bounds the walk instead of looping forever.
const MAX_CONTAINS_DEPTH: usize = 4096;

// Rough per-element byte estimates for the cache byte-budget bound ([AQ-02]).
// These approximate the heap footprint of a vertex/edge in the petgraph plus its
// owned strings; they are deliberately conservative rather than exact — the
// byte budget is a soft RSS guard, not an allocator-accurate accounting.
const VERTEX_OVERHEAD: usize = 64;
const EDGE_OVERHEAD: usize = 48;
const VIEW_BASE: usize = 256;

/// A vertex in a hydrated [`GraphView`].
///
/// For symbol-level views (`ExcludeContains` / `Symbol` / `Visualization`) a
/// vertex is one symbol and [`kind`](Vertex::kind) / [`node_id`](Vertex::node_id)
/// are populated. For the file/module rollups a vertex is an aggregate (a file
/// path or a module), so those fields are `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vertex {
    /// The stable identity of this vertex within its view: a symbol string
    /// (symbol-level), a file path (file rollup), or a module key (module
    /// rollup).
    pub key: String,
    /// A human-facing label (the node `name`, the file path, or the module
    /// name).
    pub label: String,
    /// The node kind, for symbol-level vertices; `None` for rollup aggregates.
    pub kind: Option<NodeKind>,
    /// The backing graph-store node id, for symbol-level vertices; `None` for
    /// rollup aggregates.
    pub node_id: Option<NodeId>,
}

/// The weight on an edge in a hydrated [`GraphView`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeData {
    /// The relationship kind for a symbol-level edge; `None` for a rollup edge
    /// that aggregates one or more underlying dependency edges of possibly mixed
    /// kinds.
    pub kind: Option<EdgeKind>,
    /// How many underlying graph-store edges this edge represents: always `1` for
    /// symbol-level views; the deduplicated multiplicity for a rollup edge.
    pub weight: u32,
}

/// A derived, in-memory petgraph view of the code graph at one granularity.
///
/// Held behind an `Arc` in the Engine's hydration cache ([super]); cloning the
/// `Arc` is how repeated aggregate runs reuse a hydrated view without rebuilding
/// it ([NFR-PE-07]).
///
/// [NFR-PE-07]: ../../../docs/specs/requirements/NFR-PE-07.md
#[derive(Debug)]
pub struct GraphView {
    granularity: Granularity,
    graph: DiGraph<Vertex, EdgeData>,
    by_key: HashMap<String, NodeIndex>,
    estimated_bytes: usize,
}

impl GraphView {
    /// The granularity this view was built at.
    pub fn granularity(&self) -> Granularity {
        self.granularity
    }

    /// The underlying petgraph, for running graph algorithms
    /// (`petgraph::algo::tarjan_scc`, `condensation`, `toposort`, …).
    pub fn graph(&self) -> &DiGraph<Vertex, EdgeData> {
        &self.graph
    }

    /// Number of vertices in the view.
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Number of edges in the view.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Look up a vertex by its stable [`Vertex::key`].
    pub fn index_of(&self, key: &str) -> Option<NodeIndex> {
        self.by_key.get(key).copied()
    }

    /// The estimated heap footprint of this view, used by the cache byte-budget
    /// bound ([AQ-02]). Approximate, not allocator-exact.
    ///
    /// [AQ-02]: ../../../docs/specs/architecture.md#14-open-questions
    pub fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }
}

/// Build a [`GraphView`] at `granularity` from a node + edge snapshot.
///
/// Pure and deterministic: same inputs (in the `all_nodes`/`all_edges` order)
/// always produce the same graph, indices and all ([NFR-RA-06]). Does no I/O —
/// the caller fetches the snapshot from the RO pool and hands it here.
///
/// Every view **except [`Granularity::Visualization`]** is the **code subgraph**:
/// **non-code** nodes ([`NodeKind::is_non_code`] — the documentation kinds of
/// [ADR-19] **and** the config/artifact kinds of [ADR-25]) and documentation-kind
/// edges ([`EdgeKind::is_documentation`]) are dropped at build time. This exists
/// for the whole-graph *algorithm* consumers — metrics, cycle detection, and DSM —
/// which must see code only so adding or removing documentation **or config
/// artifacts** leaves their output byte-identical ([FR-DG-06], [FR-CG-05],
/// [ADR-19], [ADR-25]). It is the same scope-at-construction shape the metric
/// graph uses for `is_test` ([FR-QM-08]); non-code kinds are filtered here rather
/// than downstream because the file/module rollups erase `node.kind`. The config
/// layer adds no edge kind (`Contains`-only, [CR-010]), so a config `Contains`
/// edge needs no edge-kind filter — it drops with its excluded endpoints.
///
/// [`Granularity::Visualization`](super::Granularity::Visualization) is the lone,
/// **presentation-only** exception ([ADR-34], [FR-UI-08]): it admits the non-code
/// vertices and the cross-layer `DocReference`/`TracesTo`/`ArtifactRef`/
/// `ArtifactBinding` edges so the web canvas can render all three layers. It is
/// reached **only** through the read-only graph-elements accessor, never a
/// metric/algorithm path, so the code-subgraph scope above — the exclusion
/// predicate and its single audit point — is untouched and the aggregate signal
/// stays byte-identical.
///
/// [ADR-34]: ../../../docs/specs/architecture/decisions/ADR-34.md
/// [FR-UI-08]: ../../../docs/specs/requirements/FR-UI-08.md
///
/// [ADR-25]: ../../../docs/specs/architecture/decisions/ADR-25.md
/// [FR-CG-05]: ../../../docs/specs/requirements/FR-CG-05.md
/// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
///
/// Doc-aware navigation and traceability ([FR-NV-10], [S-037]) do **not** read
/// the documentation graph through these views — they query the doc↔code edges
/// in the canonical store directly — so excluding doc kinds here costs them
/// nothing. A future view that must traverse doc↔code edges would add its own
/// granularity rather than re-admit docs into the algorithm scope.
///
/// [FR-NV-10]: ../../../docs/specs/requirements/FR-NV-10.md
/// [S-037]: ../../../docs/planning/journal.md#s-037-doc-aware-navigation-and-traceability-queries
///
/// [FR-DG-06]: ../../../docs/specs/requirements/FR-DG-06.md
/// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
/// [ADR-19]: ../../../docs/specs/architecture/decisions/ADR-19.md
pub fn build_view(granularity: Granularity, nodes: &[NodeRow], edges: &[EdgeRow]) -> GraphView {
    match granularity {
        Granularity::ExcludeContains => build_symbol_level(
            granularity,
            nodes,
            edges,
            SymbolScope {
                include_contains: false,
                admit_non_code: false,
            },
        ),
        Granularity::Symbol => build_symbol_level(
            granularity,
            nodes,
            edges,
            SymbolScope {
                include_contains: true,
                admit_non_code: false,
            },
        ),
        Granularity::File => build_rollup(granularity, nodes, edges, RollupKind::File),
        Granularity::Module => build_rollup(granularity, nodes, edges, RollupKind::Module),
        // The presentation-only visualization view (ADR-34): include the lexical
        // Contains edges AND admit the non-code layers + cross-layer edges. Reached
        // only by graph_elements(), never a metric/algorithm path.
        Granularity::Visualization => build_symbol_level(
            granularity,
            nodes,
            edges,
            SymbolScope {
                include_contains: true,
                admit_non_code: true,
            },
        ),
    }
}

/// The two scope flags that select what a symbol-level build keeps.
///
/// Holding both in one value (rather than two positional bools) keeps the three
/// symbol-level granularities self-documenting at the call site and makes the
/// metric-neutrality contract explicit: every code-subgraph view sets
/// `admit_non_code: false`, so its exclusion behaviour — the dropped non-code
/// vertices and documentation/artifact edges — is byte-for-byte what it was
/// before the visualization view existed.
#[derive(Debug, Clone, Copy)]
struct SymbolScope {
    /// Keep the lexical `Contains` and member-access `Accesses` edges (the full
    /// `Symbol` and `Visualization` views); the `ExcludeContains` dependency
    /// view drops both.
    include_contains: bool,
    /// Admit non-code vertices ([`NodeKind::is_non_code`]) and the cross-layer
    /// documentation/artifact edges ([`EdgeKind::is_documentation`] /
    /// [`EdgeKind::is_config_reference`]) — the presentation-only
    /// `Visualization` view ([ADR-34]). `false` for every code-subgraph
    /// (metric/algorithm) view, leaving the exclusion predicate and its single
    /// audit point untouched and the aggregate signal byte-identical
    /// ([FR-DG-06], [FR-CG-05], [FR-QM-08]).
    ///
    /// [ADR-34]: ../../../docs/specs/architecture/decisions/ADR-34.md
    admit_non_code: bool,
}

/// Symbol-level view: one vertex per symbol, edges drawn directly.
///
/// [`SymbolScope`] selects what is kept: `include_contains` distinguishes the
/// full `Symbol` view (all code edge kinds) from the `ExcludeContains`
/// dependency view (every kind but `Contains`/`Accesses`); `admit_non_code`
/// distinguishes the presentation-only `Visualization` view (which keeps the
/// non-code vertices and the cross-layer doc/artifact edges) from the
/// code-subgraph views (which drop them at this single audit point — [ADR-34]).
fn build_symbol_level(
    granularity: Granularity,
    nodes: &[NodeRow],
    edges: &[EdgeRow],
    scope: SymbolScope,
) -> GraphView {
    let mut graph = DiGraph::<Vertex, EdgeData>::new();
    let mut by_key: HashMap<String, NodeIndex> = HashMap::new();
    // Every node maps to the vertex of its symbol (collapsing the rare case of
    // two nodes sharing one canonical symbol onto a single vertex).
    let mut node_to_index: HashMap<NodeId, NodeIndex> = HashMap::new();

    for node in nodes {
        if !scope.admit_non_code && node.kind.is_non_code() {
            continue; // non-code vertex (doc — FR-DG-06 — or config — FR-CG-05) —
                      // excluded from the code subgraph; its incident edges drop
                      // with it below, keeping the signal byte-identical. The
                      // visualization view (admit_non_code) keeps it for the canvas.
        }
        let key = node.symbol.as_str().to_string();
        let idx = *by_key.entry(key.clone()).or_insert_with(|| {
            graph.add_node(Vertex {
                key: key.clone(),
                label: node.name.clone(),
                kind: Some(node.kind),
                node_id: Some(node.id),
            })
        });
        node_to_index.entry(node.id).or_insert(idx);
    }

    for edge in edges {
        if !scope.include_contains && edge.kind == EdgeKind::Contains {
            continue;
        }
        // The CR-005 member-access `Accesses` edge is a structural field-usage
        // fact, not a code-coupling dependency: excluded from the dependency
        // view the five original metrics run on (FR-EX-08, ADR-21) — kept in the
        // full symbol view (include_contains) where it is navigable, exactly as
        // Contains is. Admitting Accesses therefore leaves the original signal
        // byte-identical (metric-neutrality).
        if !scope.include_contains && edge.kind == EdgeKind::Accesses {
            continue;
        }
        if !scope.admit_non_code && edge.kind.is_documentation() {
            continue; // doc-kind edge — excluded from the code subgraph (FR-DG-06);
                      // the visualization view keeps it as a cross-layer edge.
        }
        // The CR-011 cross-artifact edges (ArtifactRef/ArtifactBinding) are fenced
        // here at the SAME hydration audit point as the non-code node predicate
        // above: an artifact reference is never a code-coupling dependency, and an
        // ArtifactBinding is exactly the cross-layer edge whose non-code endpoint
        // would otherwise leak into the signal. Excluding it by kind — beside the
        // is_documentation fence — keeps aggregate_signal, cycles, DSM, and
        // dead-code byte-identical with the wiring present (FR-CG-05, ADR-26,
        // UAT-CG-04). (Its artifact endpoint is also dropped as a non-code vertex,
        // so this is the explicit, first-class half of a two-filter guard.)
        if !scope.admit_non_code && edge.kind.is_config_reference() {
            continue;
        }
        // An endpoint missing from `node_to_index` means the node was filtered
        // out — a non-code vertex above (doc or config), since extraction never
        // emits a dangling edge — so its incident edges drop with it, keeping the
        // code subgraph closed. The config layer is `Contains`-only (CR-010), so a
        // config edge is always a Contains between two config nodes: in the
        // code-subgraph views it is excluded both as a Contains (when
        // !include_contains) and via the missing endpoint, leaving the signal
        // byte-identical (FR-CG-05); the visualization view (admit_non_code) keeps
        // both config endpoints, so these Contains edges are admitted there.
        let (Some(&src), Some(&dst)) = (
            node_to_index.get(&edge.source),
            node_to_index.get(&edge.target),
        ) else {
            continue;
        };
        graph.add_edge(
            src,
            dst,
            EdgeData {
                kind: Some(edge.kind),
                weight: 1,
            },
        );
    }

    finish(granularity, graph, by_key)
}

/// The two rollup flavours, distinguished by how a node maps to its aggregate.
#[derive(Debug, Clone, Copy)]
enum RollupKind {
    File,
    Module,
}

/// File/module rollup view: vertices are aggregates, dependency edges lifted to
/// the aggregate and deduplicated with a multiplicity weight; `Contains` is
/// never an edge here ([FR-DB-06]).
fn build_rollup(
    granularity: Granularity,
    nodes: &[NodeRow],
    edges: &[EdgeRow],
    rollup: RollupKind,
) -> GraphView {
    // Module rollup needs the lexical-containment parent map and per-node kinds
    // to find each node's enclosing module; file rollup ignores both.
    let parent_of = match rollup {
        RollupKind::Module => contains_parent_map(edges),
        RollupKind::File => HashMap::new(),
    };
    // Documentation nodes are excluded from the index the module walk climbs, so
    // a `Contains` ancestry chain can never resolve a doc-kind ancestor and mint a
    // doc vertex in the rollup — the same code-subgraph scope the vertex loop
    // below enforces (FR-DG-06). (Doc `Contains` is doc→doc by construction, so
    // this is defence-in-depth against a malformed chain, not a live path.)
    let node_index: HashMap<NodeId, &NodeRow> = nodes
        .iter()
        .filter(|n| !n.kind.is_non_code())
        .map(|n| (n.id, n))
        .collect();

    let mut graph = DiGraph::<Vertex, EdgeData>::new();
    let mut by_key: HashMap<String, NodeIndex> = HashMap::new();
    let mut node_to_index: HashMap<NodeId, NodeIndex> = HashMap::new();

    for node in nodes {
        if node.kind.is_non_code() {
            continue; // non-code node (doc — FR-DG-06 — or config — FR-CG-05) —
                      // kept out of the rollup so a doc/config file or module
                      // never becomes a DSM/metric vertex. The kind is only
                      // visible here, pre-rollup, so the scope filter must run
                      // before the aggregate erases it.
        }
        let (key, label) = match rollup {
            RollupKind::File => file_key(node),
            RollupKind::Module => module_key(node, &parent_of, &node_index),
        };
        let idx = *by_key.entry(key.clone()).or_insert_with(|| {
            graph.add_node(Vertex {
                key: key.clone(),
                label,
                kind: None,
                node_id: None,
            })
        });
        node_to_index.entry(node.id).or_insert(idx);
    }

    // Aggregate dependency edges into a deterministic (src, dst) -> weight map,
    // dropping self-loops (an aggregate depending on itself is not a coupling).
    let mut aggregated: BTreeMap<(usize, usize), u32> = BTreeMap::new();
    for edge in edges {
        if edge.kind == EdgeKind::Contains
            || edge.kind == EdgeKind::Accesses
            || edge.kind.is_documentation()
            || edge.kind.is_config_reference()
        {
            continue; // lexical, member-access, doc-kind, or cross-artifact edge —
                      // none is a code coupling (FR-CG-05, ADR-26, the rollup twin
                      // of the symbol-level artifact fence above)
        }
        // A doc endpoint was never given a rollup vertex above, so its incident
        // edges fall away here, keeping the rollup a pure code subgraph (FR-DG-06).
        let (Some(&src), Some(&dst)) = (
            node_to_index.get(&edge.source),
            node_to_index.get(&edge.target),
        ) else {
            continue;
        };
        if src == dst {
            continue;
        }
        *aggregated.entry((src.index(), dst.index())).or_insert(0) += 1;
    }
    for ((src, dst), weight) in aggregated {
        graph.add_edge(
            NodeIndex::new(src),
            NodeIndex::new(dst),
            EdgeData { kind: None, weight },
        );
    }

    finish(granularity, graph, by_key)
}

/// The file-rollup vertex key/label for a node: its defining file, or the
/// `<unbound>` sentinel when the node is bound to no file.
fn file_key(node: &NodeRow) -> (String, String) {
    match &node.file_path {
        Some(path) => (path.clone(), path.clone()),
        None => (UNBOUND.to_string(), UNBOUND.to_string()),
    }
}

/// The module-rollup vertex key/label for a node.
///
/// Walks `Contains` ancestry (inclusive) to the nearest enclosing
/// [`NodeKind::Module`] and keys on that module's symbol. With no module
/// ancestor it falls back to the node's file (`file:<path>`) and finally to the
/// `<unbound>` sentinel — so every node maps to exactly one module vertex.
fn module_key(
    node: &NodeRow,
    parent_of: &HashMap<NodeId, NodeId>,
    node_index: &HashMap<NodeId, &NodeRow>,
) -> (String, String) {
    let mut current = node;
    let mut seen: HashSet<NodeId> = HashSet::new();
    for _ in 0..MAX_CONTAINS_DEPTH {
        if current.kind == NodeKind::Module {
            return (
                format!("module:{}", current.symbol.as_str()),
                current.name.clone(),
            );
        }
        if !seen.insert(current.id) {
            break; // cycle in a corrupt store — stop the walk
        }
        match parent_of.get(&current.id).and_then(|p| node_index.get(p)) {
            Some(parent) => current = parent,
            None => break,
        }
    }
    // No enclosing module: fall back to the file, then to the sentinel.
    match &node.file_path {
        Some(path) => (format!("file:{path}"), path.clone()),
        None => (UNBOUND.to_string(), UNBOUND.to_string()),
    }
}

/// Build the child→parent map from `Contains` edges (`source` contains
/// `target`), so module resolution can walk a node up to its container.
fn contains_parent_map(edges: &[EdgeRow]) -> HashMap<NodeId, NodeId> {
    let mut parent_of = HashMap::new();
    for edge in edges {
        if edge.kind == EdgeKind::Contains {
            // Lexical nesting is a tree: a node has at most one container. Keep
            // the first (deterministic, since edges arrive sorted).
            parent_of.entry(edge.target).or_insert(edge.source);
        }
    }
    parent_of
}

/// Finalise a built graph: compute its estimated footprint and wrap it up.
fn finish(
    granularity: Granularity,
    graph: DiGraph<Vertex, EdgeData>,
    by_key: HashMap<String, NodeIndex>,
) -> GraphView {
    let mut estimated_bytes = VIEW_BASE;
    for vertex in graph.node_weights() {
        estimated_bytes += VERTEX_OVERHEAD + vertex.key.len() + vertex.label.len();
        // by_key holds a second copy of each key.
        estimated_bytes += vertex.key.len();
    }
    estimated_bytes += graph.edge_count() * EDGE_OVERHEAD;
    GraphView {
        granularity,
        graph,
        by_key,
        estimated_bytes,
    }
}
