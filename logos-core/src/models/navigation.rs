//! Navigation read-models — the eight navigation-service result types
//! (S-013, [FR-NV-01..09]).
//!
//! Each struct corresponds to one `Engine` navigation method (ADR-01).
//! All types derive [`serde::Serialize`] so CLI and MCP adapters can
//! serialise them to JSON without any core knowledge of the wire format.
//!
//! # Shared conventions
//!
//! - Every result echoes the caller's `query` text and carries `warnings`
//!   (the infallible-surface degradation channel, ADR-14) plus
//!   `suggestions` — the "did you mean" names for an unknown symbol
//!   ([FR-NV-09]): empty result + suggestions, never an error.
//! - Code is **opt-in** (`include_code`) and `None` by default
//!   ([FR-NV-02], [FR-NV-04]) — navigation saves tokens by *not* shipping
//!   source unless asked.
//!
//! [FR-NV-01..09]: ../../../docs/specs/requirements/FR-NV-01.md
//! [FR-NV-02]: ../../../docs/specs/requirements/FR-NV-02.md
//! [FR-NV-04]: ../../../docs/specs/requirements/FR-NV-04.md
//! [FR-NV-09]: ../../../docs/specs/requirements/FR-NV-09.md

use serde::Serialize;

use crate::model::{EdgeKind, NodeKind};

/// Result of an FTS5-ranked full-text search over the code graph (FR-NV-01).
#[derive(Debug, Default, Serialize)]
pub struct SearchResult {
    /// The search text as given.
    pub query: String,
    /// Matches, best-first (FTS5 bm25 rank), at most `limit` (default 20).
    pub hits: Vec<SymbolRef>,
    /// "Did you mean" names when there are no hits (FR-NV-09).
    pub suggestions: Vec<String>,
    /// Degradation channel (ADR-14): a failed read is reported, not panicked.
    pub warnings: Vec<String>,
}

/// Deterministic context bundle for a task description (FR-NV-02).
///
/// One call replaces several ad-hoc file reads; the token-saving thesis
/// (AS-02, BN-01). Built FTS-seed → hop-expand → centrality-rank → cap.
#[derive(Debug, Default, Serialize)]
pub struct ContextBundle {
    /// The task text the bundle was seeded from.
    pub task: String,
    /// Hop depth used for the neighbourhood expansion (OQ-05: default 1).
    pub hops: u32,
    /// The ranked bundle, capped at `max_nodes` (default 25).
    pub nodes: Vec<ContextNode>,
    /// Distinct files the bundle covers, sorted.
    pub files: Vec<String>,
    /// The dogfood metric seed (NFR-OO-03): each distinct file in the bundle
    /// is one naïve `Read` an agent no longer needs.
    pub est_reads_replaced: u32,
    /// "Did you mean" names when the task seeds nothing (FR-NV-09).
    pub suggestions: Vec<String>,
    /// Degradation channel (ADR-14).
    pub warnings: Vec<String>,
}

/// One ranked member of a [`ContextBundle`].
#[derive(Debug, Serialize)]
pub struct ContextNode {
    /// The symbol this entry describes.
    #[serde(flatten)]
    pub symbol: SymbolRef,
    /// Combined rank: FTS match-score (seeds) + normalised degree centrality.
    pub score: f64,
    /// `true` when this node was an FTS seed (vs a hop-expanded neighbour).
    pub seed: bool,
    /// Source text of the declaration — only when `include_code=true`.
    pub code: Option<String>,
}

/// Explore result — neighbourhood source grouped by file (FR-NV-03).
#[derive(Debug, Default, Serialize)]
pub struct ExploreResult {
    /// The query text as given.
    pub query: String,
    /// The symbol the walk was anchored on, when one resolved.
    pub anchor: Option<SymbolRef>,
    /// Per-file groups (anchor's file first), at most `max_files` (default 10).
    pub files: Vec<FileGroup>,
    /// How many files the neighbourhood actually spans (pre-cap honesty).
    pub total_files: u32,
    /// "Did you mean" names when nothing resolves (FR-NV-09).
    pub suggestions: Vec<String>,
    /// Degradation channel (ADR-14).
    pub warnings: Vec<String>,
}

/// One file's worth of neighbourhood symbols in an [`ExploreResult`].
#[derive(Debug, Serialize)]
pub struct FileGroup {
    /// Project-relative file path.
    pub file: String,
    /// The neighbourhood symbols defined in this file, with their source.
    pub symbols: Vec<ExploreSymbol>,
}

/// One symbol inside a [`FileGroup`], carrying its declaration source.
#[derive(Debug, Serialize)]
pub struct ExploreSymbol {
    /// The symbol this entry describes.
    #[serde(flatten)]
    pub symbol: SymbolRef,
    /// 1-based end line of the declaration, when recorded.
    pub end_line: Option<u32>,
    /// The declaration's source text (`explore` returns source, FR-NV-03);
    /// `None` when the file cannot be read back (e.g. deleted since indexing).
    pub code: Option<String>,
}

/// Full node info for a single symbol (FR-NV-04).
#[derive(Debug, Default, Serialize)]
pub struct NodeInfo {
    /// The symbol text as given.
    pub query: String,
    /// The resolved node, or `None` for an unknown symbol (FR-NV-09).
    pub node: Option<NodeDetail>,
    /// "Did you mean" names when the symbol is unknown (FR-NV-09).
    pub suggestions: Vec<String>,
    /// Degradation channel (ADR-14).
    pub warnings: Vec<String>,
}

/// The metadata payload of a resolved [`NodeInfo`].
#[derive(Debug, Serialize)]
pub struct NodeDetail {
    /// The symbol this entry describes.
    #[serde(flatten)]
    pub symbol: SymbolRef,
    /// 1-based end line of the declaration, when recorded.
    pub end_line: Option<u32>,
    /// The declaration signature text (FR-NV-04). `None` until the
    /// extraction layer records signatures (no `nodes.signature` column yet);
    /// the field is in the wire contract now so adapters never reshape.
    pub signature: Option<String>,
    /// Native node annotations (dead-code, duplicate, layer). Populated by
    /// the annotation engine (S-014); empty until those columns land.
    pub annotations: Vec<String>,
    /// Every immediate edge, both directions, all kinds.
    pub edges: Vec<EdgeSummary>,
    /// Source text of the declaration — only when `include_code=true`.
    pub code: Option<String>,
}

/// One immediate edge in a [`NodeDetail`].
#[derive(Debug, Serialize)]
pub struct EdgeSummary {
    /// Whether the edge points at this node (`in`) or away from it (`out`).
    pub direction: EdgeDirection,
    /// The relationship kind.
    pub kind: EdgeKind,
    /// The node at the other end.
    pub other: SymbolRef,
}

/// Edge orientation relative to the queried node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeDirection {
    /// Inbound — the other node points at the queried node.
    In,
    /// Outbound — the queried node points at the other node.
    Out,
}

/// Direct callers of a symbol (FR-NV-05).
#[derive(Debug, Default, Serialize)]
pub struct CallersResult {
    /// The symbol text as given.
    pub query: String,
    /// The node the query resolved to, or `None` for an unknown symbol.
    pub resolved: Option<SymbolRef>,
    /// How many direct callers exist in total (pre-limit honesty).
    pub total: u32,
    /// Direct callers, at most `limit` (default 50).
    pub callers: Vec<SymbolRef>,
    /// "Did you mean" names when the symbol is unknown (FR-NV-09).
    pub suggestions: Vec<String>,
    /// Degradation channel (ADR-14).
    pub warnings: Vec<String>,
}

/// Direct callees of a symbol (FR-NV-05).
#[derive(Debug, Default, Serialize)]
pub struct CalleesResult {
    /// The symbol text as given.
    pub query: String,
    /// The node the query resolved to, or `None` for an unknown symbol.
    pub resolved: Option<SymbolRef>,
    /// How many direct callees exist in total (pre-limit honesty).
    pub total: u32,
    /// Direct callees, at most `limit` (default 50).
    pub callees: Vec<SymbolRef>,
    /// "Did you mean" names when the symbol is unknown (FR-NV-09).
    pub suggestions: Vec<String>,
    /// Degradation channel (ADR-14).
    pub warnings: Vec<String>,
}

/// Transitive impact of changing a symbol — BOTH directions, labeled
/// (FR-NV-06, DL-03).
#[derive(Debug, Default, Serialize)]
pub struct ImpactResult {
    /// The symbol text as given.
    pub query: String,
    /// The node the query resolved to, or `None` for an unknown symbol.
    pub resolved: Option<SymbolRef>,
    /// Traversal depth bound applied to both directions (default 3).
    pub depth: u32,
    /// What `upstream` means — fixed to "breaks if changed" (DL-03).
    pub upstream_label: String,
    /// Transitive callers/referencers, nearest-first.
    pub upstream: Vec<ImpactEntry>,
    /// What `downstream` means — fixed to "depends on" (DL-03).
    pub downstream_label: String,
    /// Transitive callees/dependencies, nearest-first.
    pub downstream: Vec<ImpactEntry>,
    /// What `docs` means — fixed to "documented by" (FR-NV-10): the doc-aware
    /// dimension of impact.
    pub docs_label: String,
    /// The documentation sections that reference the queried symbol — the docs
    /// a change to it may oblige updating ([FR-NV-10], S-037). Empty (never an
    /// error) when no doc→code edge points at the symbol. Deterministic order
    /// (symbol asc).
    pub docs: Vec<TraceLink>,
    /// "Did you mean" names when the symbol is unknown (FR-NV-09).
    pub suggestions: Vec<String>,
    /// Degradation channel (ADR-14).
    pub warnings: Vec<String>,
}

/// One reachable symbol in an [`ImpactResult`] direction set.
#[derive(Debug, Serialize)]
pub struct ImpactEntry {
    /// The symbol this entry describes.
    #[serde(flatten)]
    pub symbol: SymbolRef,
    /// BFS distance from the queried symbol (1 = direct).
    pub distance: u32,
}

/// Current index and sync health of the code graph (FR-NV-07).
#[derive(Debug, Default, Serialize)]
pub struct StatusInfo {
    /// `true` once the graph holds at least one indexed file or node.
    pub indexed: bool,
    /// Indexed file count.
    pub file_count: u64,
    /// Graph node count.
    pub node_count: u64,
    /// Graph edge count.
    pub edge_count: u64,
    /// On-disk path of the canonical store.
    pub db_path: String,
    /// Size of the canonical store in bytes (main file + WAL sidecar).
    pub db_size_bytes: u64,
    /// Unix-seconds timestamp of the last full index run (FR-NV-07).
    /// In-process for now: populated when this engine ran the index; the
    /// persisted `project_metadata` column is a later story.
    pub last_full_index_at: Option<String>,
    /// Unix-seconds timestamp of the last observed store write (file mtime;
    /// best-effort — the persisted `last_sync_at` column is a later story).
    pub last_sync_at: Option<String>,
    /// The persisted monotonic graph revision (FR-SY-09, ADR-32): advanced on
    /// every completed `index` and every graph-mutating `sync`, `0` before the
    /// first index. The durable, cross-process "has the graph changed?" signal
    /// the native wiki tier consumes — readable by a second process opening the
    /// same `logos.db`.
    pub graph_revision: u64,
    /// Whole reference ledger size (S-011).
    pub refs_total: u64,
    /// Ledger rows currently bound to an edge.
    pub refs_resolved: u64,
    /// Ledger rows persisted for retry — never fabricated (NFR-RA-05).
    pub refs_unresolved: u64,
    /// The resolution bound-ratio (FR-RS-04); `1.0` for an empty ledger.
    pub resolution_coverage: f64,
    /// The freshness/staleness statement (ADR-11 best-effort contract).
    pub freshness: String,
    /// Degradation channel (ADR-14).
    pub warnings: Vec<String>,
}

/// The per-project **language composition** read-model ([FR-UI-10], [CR-021]):
/// the languages **actually present** in the indexed graph, each with its graph
/// node/symbol count and the number of files that contributed those nodes.
///
/// This is distinct from the plugin-registry listing of [`LanguagesInfo`]
/// ([FR-PL-06]): that lists every loaded grammar regardless of project use,
/// whereas this reports only languages with at least one indexed node, derived
/// from the hydrated graph / [graph-store]. A registered-but-unused grammar is
/// absent; an un-indexed root yields an empty composition (the dashboard's
/// honest empty state, [NFR-CC-04]). It is produced by a non-persisting façade
/// accessor so a dashboard GET never computes-and-persists ([ADR-28],
/// [FR-UI-03]).
///
/// [FR-UI-10]: ../../../docs/specs/requirements/FR-UI-10.md
/// [FR-PL-06]: ../../../docs/specs/requirements/FR-PL-06.md
/// [FR-UI-03]: ../../../docs/specs/requirements/FR-UI-03.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
/// [CR-021]: ../../../docs/requests/CR-021-dashboard-redesign-quality-coverage-rollups.md
/// [graph-store]: ../../../docs/specs/architecture/components/graph-store.md
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct LanguageComposition {
    /// One entry per language present in the graph, in deterministic order
    /// (node count descending, then language name ascending — [NFR-RA-06]).
    /// Empty for an un-indexed root.
    pub languages: Vec<LanguageCount>,
}

/// One language's footprint in the indexed graph ([FR-UI-10]). Both counts are
/// graph facts read from the `nodes`/`files` tables — never fabricated
/// ([NFR-RA-05]).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct LanguageCount {
    /// The language name as the plugin substrate records it on each file
    /// (`files.language`, e.g. `"rust"`) — the same token the registry listing
    /// uses, so the Dashboard can reconcile the two views.
    pub language: String,
    /// Graph nodes (symbols) attributed to this language — the magnitude the
    /// Dashboard's Languages card sizes by ([FR-UI-09]).
    pub nodes: u64,
    /// Distinct indexed files that contributed those nodes.
    pub files: u64,
}

/// Reverse-transitive closure of files affected by a changed set
/// (FR-CL-04, DL-08).
///
/// The closure is whole (not depth-bounded): its consumer is CI deciding
/// "what to retest", where a missed transitive dependent is a missed test
/// run. The changed files themselves are echoed in [`changed`], not listed
/// as affected — the union is trivially available to the caller.
///
/// [`changed`]: AffectedResult::changed
#[derive(Debug, Default, Serialize)]
pub struct AffectedResult {
    /// The changed files as resolved (normalised project-relative form).
    pub changed: Vec<String>,
    /// Whether the closure was narrowed to test-marked files (FR-CL-04).
    pub tests_only: bool,
    /// Dependent files reachable by reverse traversal over calls/imports/
    /// references, nearest-first then path order (deterministic, NFR-RA-06).
    pub affected: Vec<AffectedFile>,
    /// Changed paths not present in the indexed graph — reported, not erred.
    pub unknown: Vec<String>,
    /// Degradation channel (ADR-14).
    pub warnings: Vec<String>,
}

/// One dependent file in an [`AffectedResult`] closure.
#[derive(Debug, Serialize)]
pub struct AffectedFile {
    /// Project-relative file path.
    pub file: String,
    /// Minimal reverse-edge hops from the changed set (1 = direct dependent).
    pub distance: u32,
    /// Whether the file is test-marked. Path-convention heuristic until
    /// native test annotations land (S-020 test-gap analysis refines this).
    pub is_test: bool,
}

// ── Traceability (FR-NV-10, S-037) ──────────────────────────────────────────

/// One end of a traceability link: the linked node plus the doc edge kind that
/// connects it ([FR-NV-10], S-037). The `via` field distinguishes a generic
/// `doc_reference` (a markdown mention bound to code, [FR-DG-04]) from a typed
/// `traces_to` (a swe-skills `Requirement`/`Adr`/`Story` trace, [FR-DG-07]).
///
/// [FR-NV-10]: ../../../docs/specs/requirements/FR-NV-10.md
/// [FR-DG-04]: ../../../docs/specs/requirements/FR-DG-04.md
/// [FR-DG-07]: ../../../docs/specs/requirements/FR-DG-07.md
#[derive(Debug, Clone, Serialize)]
pub struct TraceLink {
    /// The linked node — code for an `implements` answer, a doc section for a
    /// `referencing_docs` answer.
    #[serde(flatten)]
    pub symbol: SymbolRef,
    /// The documentation edge kind connecting the queried node to this one.
    pub via: EdgeKind,
}

/// Which code implements a documentation/requirement node ([FR-NV-10], S-037):
/// the code symbols a doc node points at over `doc_reference`/`traces_to` edges.
///
/// Returns an **empty** `implementors` (never an error) when the node resolves
/// but has no outgoing doc→code edge, and an empty result plus `suggestions`
/// when the doc node is unknown ([FR-NV-09] graceful contract).
#[derive(Debug, Default, Serialize)]
pub struct ImplementorsResult {
    /// The documentation/requirement text as given.
    pub query: String,
    /// The documentation node the query resolved to, or `None` if unknown.
    pub resolved: Option<SymbolRef>,
    /// The implementing code symbols, deterministic order (symbol asc).
    pub implementors: Vec<TraceLink>,
    /// "Did you mean" names when the doc node is unknown (FR-NV-09).
    pub suggestions: Vec<String>,
    /// Degradation channel (ADR-14).
    pub warnings: Vec<String>,
}

/// Which documents reference a code symbol ([FR-NV-10], S-037): the doc sections
/// that point at the symbol over `doc_reference`/`traces_to` edges.
///
/// Returns an **empty** `docs` (never an error) when the symbol resolves but no
/// doc references it, and an empty result plus `suggestions` when the symbol is
/// unknown ([FR-NV-09] graceful contract).
#[derive(Debug, Default, Serialize)]
pub struct ReferencingDocsResult {
    /// The symbol text as given.
    pub query: String,
    /// The code node the query resolved to, or `None` if unknown.
    pub resolved: Option<SymbolRef>,
    /// The referencing documentation sections, deterministic order (symbol asc).
    pub docs: Vec<TraceLink>,
    /// "Did you mean" names when the symbol is unknown (FR-NV-09).
    pub suggestions: Vec<String>,
    /// Degradation channel (ADR-14).
    pub warnings: Vec<String>,
}

// ── Shared primitives ──────────────────────────────────────────────────────

/// A lightweight reference to a symbol (used in many read-models).
#[derive(Debug, Clone, Serialize)]
pub struct SymbolRef {
    /// The canonical SCIP symbol string — round-trips into any navigation
    /// tool's `symbol` argument.
    pub symbol: String,
    /// The human-facing name.
    pub name: String,
    /// The node ontology kind (wire form: lower-case snake_case).
    pub kind: NodeKind,
    /// Project-relative defining file, when bound.
    pub file: Option<String>,
    /// 1-based start line of the declaration, when recorded.
    pub line: Option<u32>,
}

// ── Graph-elements (FR-UI-08, ADR-29) ───────────────────────────────────────

/// The presentation layer a graph node belongs to, for the web canvas's
/// layer filters and node coloring (frontend-design §4.4): code, documentation,
/// or config/artifact ([FR-UI-08], [ADR-29]).
///
/// Derived from the node's [`NodeKind`] — never stored, never fabricated
/// ([NFR-RA-05]). The accessor hydrates the presentation-only
/// [`Visualization`](crate::Granularity::Visualization) view ([ADR-34],
/// [FR-UI-08]), which keeps the non-code layers, so a node renders as [`Code`],
/// [`Doc`], or [`Artifact`] per its kind. The metric/algorithm views remain the
/// code subgraph (see `hydrate::view::build_view`), so surfacing these layers to
/// the canvas never moves the aggregate signal ([FR-DG-06], [ADR-19]).
///
/// [`Code`]: GraphLayer::Code
/// [`Doc`]: GraphLayer::Doc
/// [`Artifact`]: GraphLayer::Artifact
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphLayer {
    /// A code symbol (function, type, module, …).
    Code,
    /// A documentation node (doc file/section, requirement, ADR, story).
    Doc,
    /// A config/artifact node (config file/section, shell function, route, …).
    Artifact,
}

impl GraphLayer {
    /// Resolve a lower-case wire name (`code`/`doc`/`artifact` — the `serde`
    /// representation) back to its layer, or `None` for an unrecognised token.
    /// The single source of truth for parsing the layer wire form, shared by the
    /// canvas's server-side `layers` re-budgeting filter (S-122, [FR-UI-15]) and
    /// the structured-query `layer` field filter (S-120). An unrecognised token is
    /// dropped rather than erroring, so a malformed canvas request degrades to a
    /// looser filter, never a 4xx.
    ///
    /// [FR-UI-15]: ../../../docs/specs/requirements/FR-UI-15.md
    pub fn from_wire(wire: &str) -> Option<GraphLayer> {
        match wire {
            "code" => Some(GraphLayer::Code),
            "doc" => Some(GraphLayer::Doc),
            "artifact" => Some(GraphLayer::Artifact),
            _ => None,
        }
    }
}

impl From<NodeKind> for GraphLayer {
    /// Pure, never-fabricated classification ([NFR-RA-05]): a documentation kind
    /// renders in the [`Doc`](GraphLayer::Doc) layer, a config/artifact kind in
    /// [`Artifact`](GraphLayer::Artifact), and every other kind as
    /// [`Code`](GraphLayer::Code). The single source of truth for the canvas's
    /// code/doc/artifact split ([FR-UI-08]) — shared by the graph-elements
    /// hydration and the Decisions-panel identity header (S-121).
    fn from(kind: NodeKind) -> Self {
        if kind.is_doc() {
            GraphLayer::Doc
        } else if kind.is_config() {
            GraphLayer::Artifact
        } else {
            GraphLayer::Code
        }
    }
}

/// The semantic **cluster zoom** tier a graph-elements snapshot is taken at
/// (S-124, [FR-UI-15], [ADR-36]): the Google-Maps-style module → file → symbol
/// altitude ladder the canvas drives from its tracked zoom. Each tier selects an
/// **existing** hydration view ([ADR-34], [FR-DB-05]) — no clustering algorithm is
/// invented:
///
/// - [`Module`](GraphGranularity::Module) — the module-rollup view
///   ([`Granularity::Module`](crate::Granularity::Module)): vertices are modules,
///   dependency edges aggregated. The lowest-detail tier (far zoom-out).
/// - [`File`](GraphGranularity::File) — the file-rollup view
///   ([`Granularity::File`](crate::Granularity::File)): vertices are files. The
///   mid tier.
/// - [`Symbol`](GraphGranularity::Symbol) — the presentation-only visualization
///   view ([`Granularity::Visualization`](crate::Granularity::Visualization)):
///   one vertex per symbol, all three layers. The highest-detail tier and the
///   **default** (an unparameterized request behaves exactly as before S-124).
///
/// The module/file rollup tiers are the **code subgraph** by construction —
/// documentation and config/artifact files/modules are excluded at hydration
/// ([FR-DG-06], [FR-CG-05]) — so a cluster tier never surfaces a doc/artifact
/// node. Reading any of these views for presentation is metric-neutral: none is
/// on a metric/cycle/DSM/dead-code path, so the aggregate signal stays
/// byte-identical ([ADR-34], [FR-QM-08]).
///
/// [FR-UI-15]: ../../../docs/specs/requirements/FR-UI-15.md
/// [FR-DB-05]: ../../../docs/specs/requirements/FR-DB-05.md
/// [FR-DG-06]: ../../../docs/specs/requirements/FR-DG-06.md
/// [FR-CG-05]: ../../../docs/specs/requirements/FR-CG-05.md
/// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
/// [ADR-34]: ../../../docs/specs/architecture/decisions/ADR-34.md
/// [ADR-36]: ../../../docs/specs/architecture/decisions/ADR-36.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphGranularity {
    /// Module-rollup clusters (the lowest-detail tier).
    Module,
    /// File-rollup clusters (the mid tier).
    File,
    /// Symbol-level vertices (the highest-detail tier; the **default**).
    #[default]
    Symbol,
}

impl GraphGranularity {
    /// Resolve a lower-case wire token (`module`/`file`/`symbol` — the `serde`
    /// representation) back to its tier, or `None` for an unrecognised token. The
    /// single source of truth for parsing the canvas's `granularity` cluster-zoom
    /// parameter (S-124, [FR-UI-15]); an unrecognised token is dropped by the
    /// caller so a malformed request degrades to the default tier rather than a
    /// 4xx, mirroring [`GraphLayer::from_wire`].
    ///
    /// [FR-UI-15]: ../../../docs/specs/requirements/FR-UI-15.md
    pub fn from_wire(wire: &str) -> Option<GraphGranularity> {
        match wire {
            "module" => Some(GraphGranularity::Module),
            "file" => Some(GraphGranularity::File),
            "symbol" => Some(GraphGranularity::Symbol),
            _ => None,
        }
    }
}

/// One node in a [`GraphElements`] snapshot — a presentation-shaped vertex of
/// the hydrated graph for the interactive canvas ([FR-UI-08], [ADR-29]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GraphElementNode {
    /// Stable identity — the canonical SCIP symbol string at the symbol tier, or
    /// the file path / module key at the file/module rollup tiers (S-124). The
    /// canvas's node id; round-trips into the navigation tools' `symbol` argument
    /// for a symbol-tier node.
    pub id: String,
    /// Human-facing label (the node name, file path, or module name).
    pub label: String,
    /// The node ontology kind (wire form: lower-case snake_case), or `null` for a
    /// **rollup cluster** vertex (a file/module aggregate has no single kind —
    /// S-124). Always present at the symbol ([`Visualization`]) tier.
    ///
    /// [`Visualization`]: crate::Granularity::Visualization
    pub kind: Option<NodeKind>,
    /// The presentation layer this node renders in ([`GraphLayer`]). A rollup
    /// cluster is the code subgraph by construction (docs/artifacts excluded at
    /// hydration, [FR-DG-06]), so it renders in the [`Code`](GraphLayer::Code)
    /// layer.
    pub layer: GraphLayer,
}

/// One edge in a [`GraphElements`] snapshot — a typed, directed relationship
/// between two rendered nodes ([FR-UI-08], [ADR-29]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GraphElementEdge {
    /// The [`GraphElementNode::id`] the edge points **from**.
    pub source: String,
    /// The [`GraphElementNode::id`] the edge points **to**.
    pub target: String,
    /// The relationship kind, for the canvas's edge-type filters and line styles
    /// (wire form: lower-case snake_case), or `null` for a **rollup cluster** edge
    /// (an aggregated dependency edge spans one or more underlying kinds and has no
    /// single type — S-124). Always present at the symbol ([`Visualization`]) tier.
    ///
    /// [`Visualization`]: crate::Granularity::Visualization
    pub edge_type: Option<EdgeKind>,
}

/// A read-only, presentation-shaped nodes+edges snapshot of the hydrated graph
/// for the web surface's interactive canvas ([FR-UI-08], [FR-DB-05], [ADR-29]).
///
/// Whole-graph (`seed = None`) or seed-scoped (the connected neighbourhood of a
/// seed symbol), bounded by a visible-element [`cap`](GraphElements::cap): when
/// the in-scope graph exceeds the cap the most-connected nodes are kept and the
/// remainder reported as elided, so the frontend shows an honest "N more not
/// shown" notice rather than silently truncating ([NFR-CC-04]). Built from the
/// cached hydrated view — a pure reader, persists nothing ([ADR-28], [ADR-29]).
#[derive(Debug, Default, Serialize)]
pub struct GraphElements {
    /// The seed symbol the snapshot was scoped to, echoed back; `None` for the
    /// whole-graph snapshot.
    pub seed: Option<String>,
    /// The semantic cluster-zoom tier the snapshot was taken at, echoed back
    /// (S-124, [FR-UI-15], [ADR-36]): `module`/`file`/`symbol`. Defaults to
    /// [`Symbol`](GraphGranularity::Symbol) — an unparameterized request is the
    /// pre-S-124 symbol-tier snapshot.
    pub granularity: GraphGranularity,
    /// The visible-element cap applied to the selection.
    pub cap: u32,
    /// In-scope node count **before** the cap — the denominator for
    /// [`elided_nodes`](GraphElements::elided_nodes).
    pub total_nodes: u32,
    /// In-scope edge count (both endpoints in scope) **before** the cap — the
    /// denominator for [`elided_edges`](GraphElements::elided_edges).
    pub total_edges: u32,
    /// How many in-scope nodes were elided by the cap (`total_nodes − nodes`),
    /// never silently dropped ([NFR-CC-04]).
    pub elided_nodes: u32,
    /// How many in-scope edges were elided because an endpoint was capped out
    /// (`total_edges − edges`).
    pub elided_edges: u32,
    /// The rendered nodes, deterministically ordered by `id` ([NFR-RA-06]).
    pub nodes: Vec<GraphElementNode>,
    /// The rendered edges among the rendered nodes, deterministically ordered
    /// ([NFR-RA-06]).
    pub edges: Vec<GraphElementEdge>,
    /// Degradation channel (ADR-14): a failed read is reported, not panicked.
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical [`GraphLayer`] derivation (S-121, FR-UI-08) classifies a
    /// kind into exactly one presentation layer — code by default, doc for the
    /// documentation kinds, artifact for the config kinds — never fabricated
    /// ([NFR-RA-05]). The single source of truth shared by the canvas hydration
    /// and the Decisions-panel identity header.
    #[test]
    fn graph_layer_from_node_kind_classifies_code_doc_and_artifact() {
        assert_eq!(GraphLayer::from(NodeKind::Function), GraphLayer::Code);
        assert_eq!(GraphLayer::from(NodeKind::Struct), GraphLayer::Code);
        assert_eq!(GraphLayer::from(NodeKind::Requirement), GraphLayer::Doc);
        assert_eq!(GraphLayer::from(NodeKind::DocSection), GraphLayer::Doc);
        assert_eq!(GraphLayer::from(NodeKind::ConfigFile), GraphLayer::Artifact);
        assert_eq!(GraphLayer::from(NodeKind::ShellFunction), GraphLayer::Artifact);
    }

    /// Every kind maps to a layer consistent with its own `is_doc`/`is_config`
    /// classification — the derivation agrees with its inputs across the whole
    /// taxonomy, so a newly added kind cannot silently fall through.
    #[test]
    fn graph_layer_agrees_with_kind_classification_for_all_kinds() {
        for kind in NodeKind::ALL {
            let expected = if kind.is_doc() {
                GraphLayer::Doc
            } else if kind.is_config() {
                GraphLayer::Artifact
            } else {
                GraphLayer::Code
            };
            assert_eq!(GraphLayer::from(kind), expected, "{}", kind.as_str());
        }
    }

    /// `from_wire` is the inverse of the snake_case serde form for the three
    /// layers and `None` for anything else — the contract the canvas's server-side
    /// `layers` re-budgeting filter rests on (S-122, FR-UI-15): a malformed token
    /// is dropped, never an error. Round-trips against the serialized form so it
    /// can never drift from the wire representation.
    #[test]
    fn graph_layer_roundtrips_through_wire_name() {
        for layer in [GraphLayer::Code, GraphLayer::Doc, GraphLayer::Artifact] {
            let wire = serde_json::to_string(&layer).unwrap();
            let token = wire.trim_matches('"');
            assert_eq!(GraphLayer::from_wire(token), Some(layer), "{token}");
        }
        assert_eq!(GraphLayer::from_wire("code"), Some(GraphLayer::Code));
        assert_eq!(GraphLayer::from_wire("doc"), Some(GraphLayer::Doc));
        assert_eq!(GraphLayer::from_wire("artifact"), Some(GraphLayer::Artifact));
        assert_eq!(GraphLayer::from_wire("Code"), None, "case-sensitive wire form");
        assert_eq!(GraphLayer::from_wire("not_a_layer"), None);
        assert_eq!(GraphLayer::from_wire(""), None);
    }

    /// `GraphGranularity::from_wire` is the inverse of the snake_case serde form
    /// for the three cluster-zoom tiers and `None` for anything else — the contract
    /// the canvas's `granularity` parameter rests on (S-124, FR-UI-15): a malformed
    /// token is dropped so the request degrades to the default tier, never a 4xx.
    /// Round-trips against the serialized form so it cannot drift from the wire
    /// representation, and pins the default tier to `Symbol` (the pre-S-124
    /// behaviour of an unparameterized request).
    #[test]
    fn graph_granularity_roundtrips_through_wire_name_and_defaults_to_symbol() {
        for tier in [
            GraphGranularity::Module,
            GraphGranularity::File,
            GraphGranularity::Symbol,
        ] {
            let wire = serde_json::to_string(&tier).unwrap();
            let token = wire.trim_matches('"');
            assert_eq!(GraphGranularity::from_wire(token), Some(tier), "{token}");
        }
        assert_eq!(GraphGranularity::from_wire("module"), Some(GraphGranularity::Module));
        assert_eq!(GraphGranularity::from_wire("file"), Some(GraphGranularity::File));
        assert_eq!(GraphGranularity::from_wire("symbol"), Some(GraphGranularity::Symbol));
        assert_eq!(GraphGranularity::from_wire("Module"), None, "case-sensitive wire form");
        assert_eq!(GraphGranularity::from_wire("symbols"), None);
        assert_eq!(GraphGranularity::from_wire(""), None);
        assert_eq!(GraphGranularity::default(), GraphGranularity::Symbol);
    }
}
