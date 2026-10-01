//! Navigation read-models — the navigation-service result types
//! (S-013, [FR-NV-01..13]).
//!
//! Countless on purpose: the header used to say "the eight", and two
//! consecutive stories added to this file without noticing.
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
use crate::models::quality::CrossFileAbsence;

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
    /// The resolved edge set this answer was computed over ([FR-NV-14]):
    /// present on every answer, empty or not.
    ///
    /// [FR-NV-14]: ../../../docs/specs/requirements/FR-NV-14.md
    pub resolution_denominator: ResolutionDenominator,
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
    /// The resolved edge set this answer was computed over ([FR-NV-14]):
    /// present on every answer, empty or not.
    ///
    /// [FR-NV-14]: ../../../docs/specs/requirements/FR-NV-14.md
    pub resolution_denominator: ResolutionDenominator,
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
    /// The resolved edge set this answer was computed over ([FR-NV-14]):
    /// present on every answer, empty or not.
    ///
    /// [FR-NV-14]: ../../../docs/specs/requirements/FR-NV-14.md
    pub resolution_denominator: ResolutionDenominator,
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

// ── Impact-set intersection across planned work items ([FR-NV-11]) ──────────
//
// The scheduling question, made deterministic: given several work items each
// naming the symbols it intends to change, which pairs' transitive impact sets
// overlap — and on which symbols. Overlapping items cannot safely run in
// parallel; disjoint ones can. Everything below is derived from the SAME
// depth-bounded traversal `impact` runs ([FR-NV-06]), so the two answers can
// never disagree.
//
// [FR-NV-11]: ../../../docs/specs/requirements/FR-NV-11.md
// [FR-NV-06]: ../../../docs/specs/requirements/FR-NV-06.md

/// One unit of planned work, naming the symbols it intends to change
/// ([FR-NV-11]) — the *input* to [`ImpactIntersectionResult`].
///
/// This is the one type in this module that is parsed rather than serialised:
/// every surface (CLI `--item`, the MCP tool, the `/api/v1` route) accepts the
/// same `<id>=<symbol>[,<symbol>…]` spelling, and [`WorkItem::from_specs`] is
/// the single parser all three share so the surfaces cannot drift (ADR-01 —
/// the adapters parse nothing of their own).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WorkItem {
    /// The item's identifier as the caller spelled it (a story id, a branch
    /// name, a free-text label — Logos never interprets it).
    pub id: String,
    /// The symbols this item intends to change, as given.
    pub symbols: Vec<String>,
}

impl WorkItem {
    /// Parse one `<id>=<symbol>[,<symbol>…]` spec.
    ///
    /// Splits on the FIRST `=` only, so a symbol containing `=` survives; the
    /// symbol list is comma-separated, and blank entries are dropped. `None`
    /// when the spec carries no `=`, or an empty id — the caller reports that
    /// as a warning rather than guessing what was meant ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub fn parse_spec(spec: &str) -> Option<WorkItem> {
        let (id, rest) = spec.split_once('=')?;
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        Some(WorkItem {
            id: id.to_string(),
            symbols: rest
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        })
    }

    /// Parse a whole `--item` list into work items, merging repeated ids.
    ///
    /// Repeating an id accumulates its symbols (`--item A=f --item A=g` is
    /// `A` naming both), which is also the escape hatch for a symbol that
    /// genuinely contains a comma. Order follows first appearance, so the
    /// reported pair order is the caller's own. Returns the items alongside one
    /// warning per unparseable spec — a malformed spec is never silently
    /// dropped ([NFR-CC-04]).
    pub fn from_specs<S: AsRef<str>>(specs: &[S]) -> (Vec<WorkItem>, Vec<String>) {
        let (mut items, mut warnings): (Vec<WorkItem>, Vec<String>) = (Vec::new(), Vec::new());
        for spec in specs {
            let spec = spec.as_ref();
            let Some(parsed) = WorkItem::parse_spec(spec) else {
                warnings.push(format!(
                    "ignored work item {spec:?}: expected <id>=<symbol>[,<symbol>...]"
                ));
                continue;
            };
            match items.iter_mut().find(|existing| existing.id == parsed.id) {
                Some(existing) => existing.symbols.extend(parsed.symbols),
                None => items.push(parsed),
            }
        }
        // A symbol named twice under one id is one intention, not two: leaving
        // the repeat in would double an entry in every per-item projection and
        // buy a second "did you mean" lookup for the same miss. Order is first
        // appearance, so the caller's spelling order still drives the payload.
        for item in &mut items {
            let mut seen = std::collections::HashSet::new();
            item.symbols.retain(|symbol| seen.insert(symbol.clone()));
        }
        (items, warnings)
    }
}

/// Impact-set intersection across a set of planned work items ([FR-NV-11]).
#[derive(Debug, Default, Serialize)]
pub struct ImpactIntersectionResult {
    /// Depth bound applied to every item's impact set (default 3) — the same
    /// bound `impact` applies ([FR-NV-06]).
    pub depth: u32,
    /// One row per work item, in the order supplied: what it declared, what
    /// resolved, and how large its impact set turned out to be.
    pub items: Vec<WorkItemImpact>,
    /// Every unordered pair whose impact sets intersect, naming the shared
    /// symbols. **These items cannot safely proceed in parallel.** Ordered by
    /// the items' input positions (left, then right).
    pub intersecting: Vec<ItemIntersection>,
    /// Every unordered pair whose impact sets are disjoint — safely parallel.
    /// Same ordering.
    pub safe_parallel: Vec<ItemPair>,
    /// What this answer can and cannot see ([NFR-CC-04]).
    pub coverage: IntersectionCoverage,
    /// The resolved edge set this answer was computed over ([FR-NV-14]):
    /// present on every answer, empty or not.
    ///
    /// [FR-NV-14]: ../../../docs/specs/requirements/FR-NV-14.md
    pub resolution_denominator: ResolutionDenominator,
    /// Degradation channel (ADR-14) — also where a malformed `--item` spec and
    /// a single-item call are reported.
    pub warnings: Vec<String>,
}

/// One work item's declared symbols and the size of the impact set they span.
#[derive(Debug, Default, Serialize)]
pub struct WorkItemImpact {
    /// The item id as given.
    pub item: String,
    /// The symbols it declared, as given.
    pub declared: Vec<String>,
    /// The distinct nodes those symbols resolved to (deterministic, symbol
    /// asc). Two spellings naming one node appear once.
    ///
    /// Declared symbols that resolved to nothing are NOT mirrored here: they
    /// are a coverage limit, and [`IntersectionCoverage::unresolved`] is the
    /// one place limits are stated — with the "did you mean" suggestions this
    /// row could not carry ([NFR-CC-04]).
    pub resolved: Vec<SymbolRef>,
    /// Size of the transitive impact set: the resolved seeds plus everything
    /// upstream and downstream of them within `depth`.
    pub impact_set_size: u32,
}

/// Two work items whose impact sets intersect, and the symbols they share.
#[derive(Debug, Default, Serialize)]
pub struct ItemIntersection {
    /// The earlier-supplied item.
    pub left: String,
    /// The later-supplied item.
    pub right: String,
    /// How many symbols the two impact sets share, in full — never the length
    /// of the truncated `shared` list ([NFR-CC-04]).
    pub shared_total: u32,
    /// The shared symbols: those either item declared directly first, then by
    /// symbol ascending; truncated to a bounded payload.
    pub shared: Vec<SharedSymbol>,
    /// How many shared symbols `shared` omitted — `0` when the list is whole.
    pub shared_elided: u32,
}

/// One symbol two work items' impact sets have in common.
#[derive(Debug, Serialize)]
pub struct SharedSymbol {
    /// The shared symbol.
    #[serde(flatten)]
    pub symbol: SymbolRef,
    /// Which of the two items named this symbol *directly*, in input order.
    /// Empty when both merely reach it — a weaker signal than a declared
    /// collision, and distinguished so a consumer can rank them.
    pub declared_by: Vec<String>,
}

/// An unordered pair of work items, named by id.
#[derive(Debug, Default, Serialize)]
pub struct ItemPair {
    /// The earlier-supplied item.
    pub left: String,
    /// The later-supplied item.
    pub right: String,
}

/// The coverage limits of one intersection answer ([NFR-CC-04]).
///
/// The result is only as complete as the graph it was computed over, and this
/// is where that is said out loud rather than left for the reader to infer.
#[derive(Debug, Default, Serialize)]
pub struct IntersectionCoverage {
    /// The standing statement of what the answer can and cannot see.
    pub statement: String,
    /// Declared symbols no node matched, with the item that declared them.
    /// A non-empty list means "disjoint" is weaker than it looks.
    pub unresolved: Vec<UnresolvedDeclaration>,
    /// Work items that ended with no resolved seed at all: they are disjoint
    /// from everything by construction, which says nothing about the code.
    pub items_without_resolved_symbols: Vec<String>,
}

/// One declared symbol the graph does not know ([NFR-CC-04]).
#[derive(Debug, Serialize)]
pub struct UnresolvedDeclaration {
    /// The item that declared it.
    pub item: String,
    /// The symbol text as given.
    pub symbol: String,
    /// "Did you mean" names for it ([FR-NV-09]) — empty when nothing is close.
    pub suggestions: Vec<String>,
}

// ── Structural precedent ([FR-NV-12]) ───────────────────────────────────────
//
// "Which existing code plays the same structural role as the thing I am about
// to write." The blast-radius questions ([FR-NV-06], [FR-NV-11]) answer what a
// change breaks; this one answers what it should look like — the dominant
// question once a plan has already settled the scope.
//
// The whole design risk is in the word *analogous*. A similarity notion that
// cannot explain itself is a fuzzy score wearing a graph costume, so there is
// no score here at all: analogy is a fixed set of three NAMED graph facts
// ([`PrecedentFacet`]), every result says which of them it matched and through
// which nodes, and the ranking is a lexicographic comparison of counts that are
// all printed on the payload ([FR-NV-12], [NFR-CC-04]).
//
// [FR-NV-12]: ../../../docs/specs/requirements/FR-NV-12.md
// [FR-NV-11]: ../../../docs/specs/requirements/FR-NV-11.md
// [FR-NV-06]: ../../../docs/specs/requirements/FR-NV-06.md

/// One named notion of structural similarity ([FR-NV-12]).
///
/// These three are the whole vocabulary — the query never invents a fourth
/// reason and never blends them into a number. They are declared in the order
/// the ranking breaks ties on, strongest first: sharing a supertype is a
/// declared contract, being wired up by the same node is a declared
/// registration, and sharing callees is inferred from behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrecedentFacet {
    /// The candidate implements or extends a trait/interface/superclass the
    /// target also implements or extends — a shared declared contract.
    SharedSupertype,
    /// Some third node *does something with* both the candidate and the target
    /// the same way: a registry, dispatcher, factory or route table that calls,
    /// instantiates, references, routes to, or is typed by both. This is the
    /// facet that finds sibling arms of one capability.
    ///
    /// A module's `use` list is deliberately **not** a registration — see
    /// `is_registration_edge` for the measurement that settled it.
    SharedRegistration,
    /// The candidate and the target call the same functions — a matching call
    /// shape. Counted only from [`MIN_SHARED_CALLEES`] shared callees up.
    ///
    /// [`MIN_SHARED_CALLEES`]: crate::models::navigation::MIN_SHARED_CALLEES
    SharedCallee,
}

impl PrecedentFacet {
    /// Every facet, in ranking precedence order — the same order the derived
    /// [`Ord`] gives, since both follow the declaration above.
    ///
    /// The accumulator and the ranking no longer read this: they key on the
    /// facet itself and take their order from `Ord`. What is left needs an
    /// *enumeration* rather than an order — the wire-vocabulary check, and any
    /// consumer that wants to name the whole notion. A fourth variant fails to
    /// compile in [`as_str`](Self::as_str), `precedent_explanation` and
    /// `precedent_direction`, and `precedent_facet_all_lists_every_variant`
    /// then catches it missing from this list.
    pub const ALL: [PrecedentFacet; 3] = [
        PrecedentFacet::SharedSupertype,
        PrecedentFacet::SharedRegistration,
        PrecedentFacet::SharedCallee,
    ];

    /// The wire name — identical to the `serde` representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            PrecedentFacet::SharedSupertype => "shared_supertype",
            PrecedentFacet::SharedRegistration => "shared_registration",
            PrecedentFacet::SharedCallee => "shared_callee",
        }
    }
}

/// How many shared callees the call-shape facet requires before it counts
/// ([FR-NV-12]).
///
/// One shared callee is a coincidence of using the same helper; two or more is
/// a call shape. The threshold is public, stated on every payload, and the
/// candidates it drops are **counted** on the coverage block rather than
/// silently discarded ([NFR-CC-04]) — a hidden filter is exactly the opaque
/// behaviour this query exists not to have.
pub const MIN_SHARED_CALLEES: usize = 2;

/// Nodes structurally analogous to one symbol or file ([FR-NV-12]).
#[derive(Debug, Default, Serialize)]
pub struct PrecedentResult {
    /// The target as the caller spelled it.
    pub query: String,
    /// How the query text was resolved.
    pub target_kind: PrecedentTargetKind,
    /// The resolved target symbol, when the query named one.
    pub target_symbol: Option<SymbolRef>,
    /// The resolved project-relative file, when the query named one.
    pub target_file: Option<String>,
    /// The similarity notion, stated in full on every answer — the reader never
    /// has to trust an undocumented ranking ([FR-NV-12]).
    pub notion: String,
    /// The ranking rule, likewise stated in full.
    pub ranked_by: String,
    /// The analogous nodes, best precedent first.
    pub precedents: Vec<Precedent>,
    /// How many analogous nodes were found in total, before the limit.
    pub total_found: u32,
    /// How many `total_found` omitted — `0` when the list is whole.
    pub elided: u32,
    /// Why the answer is empty, when it is. `None` whenever `precedents` is
    /// non-empty. An empty result **always** carries one: the query never
    /// relaxes the notion to manufacture a low-confidence guess ([FR-NV-12]).
    pub empty_reason: Option<EmptyPrecedent>,
    /// What this answer can and cannot see ([NFR-CC-04]).
    pub coverage: PrecedentCoverage,
    /// "Did you mean" names for an unresolved target ([FR-NV-09]).
    pub suggestions: Vec<String>,
    /// The resolved edge set this answer was computed over ([FR-NV-14]):
    /// present on every answer, empty or not.
    ///
    /// [FR-NV-14]: ../../../docs/specs/requirements/FR-NV-14.md
    pub resolution_denominator: ResolutionDenominator,
    /// Degradation channel ([ADR-14]).
    pub warnings: Vec<String>,
}

/// What the query text turned out to name.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrecedentTargetKind {
    /// The graph knows nothing by that symbol, name, or path.
    #[default]
    Unresolved,
    /// One symbol; its own structure is compared.
    Symbol,
    /// A project-relative file; the structure of every symbol it defines is
    /// compared, and the precedents are the analogous **symbols** — a sibling
    /// file surfaces as a cluster of its symbols, each naming its file.
    File,
}

/// One structurally analogous node, with the reasons it qualifies.
#[derive(Debug, Serialize)]
pub struct Precedent {
    /// The analogous symbol.
    #[serde(flatten)]
    pub symbol: SymbolRef,
    /// The counted graph facts this result was ranked on — the same numbers
    /// the ranking rule names, so the order is reproducible by hand.
    pub rank: PrecedentRank,
    /// Why it is analogous: one entry per matched facet, each naming the nodes
    /// the analogy runs through ([FR-NV-12] AC 2).
    pub reasons: Vec<PrecedentReason>,
}

/// The counted facts behind one precedent's position ([FR-NV-12] AC 1).
///
/// Deliberately four separate integers rather than one number: a composite
/// score would be exactly the opaque ranking the requirement forbids, and these
/// are compared lexicographically in declaration order.
#[derive(Debug, Default, Serialize)]
pub struct PrecedentRank {
    /// How many of the three facets matched (1–3).
    pub facets: u32,
    /// Distinct traits/interfaces/superclasses shared with the target.
    pub shared_supertypes: u32,
    /// Distinct nodes that depend on both the candidate and the target the
    /// same way.
    pub shared_registrations: u32,
    /// Distinct functions both call.
    pub shared_callees: u32,
}

/// One reason a node is analogous ([FR-NV-12] AC 2).
#[derive(Debug, Serialize)]
pub struct PrecedentReason {
    /// Which notion matched.
    pub facet: PrecedentFacet,
    /// The reason in words, naming the nodes it runs through.
    pub explanation: String,
    /// The shared nodes themselves — the trait, the registrar, the callees —
    /// round-trippable into any other navigation tool. Bounded.
    pub via: Vec<SymbolRef>,
    /// How many shared nodes there are in full, never the length of `via`.
    pub via_total: u32,
    /// How many `via` omitted — `0` when the list is whole.
    pub via_elided: u32,
}

/// Why a precedent answer is empty ([FR-NV-12] AC 4).
///
/// The requirement is explicit that an empty result states its reason rather
/// than degrading to a low-confidence guess, so the reason is a **closed
/// vocabulary** a consumer can branch on, not free prose that will be reworded.
#[derive(Debug, Clone, Serialize)]
pub struct EmptyPrecedent {
    /// Which of the closed vocabulary applies.
    pub code: EmptyPrecedentCode,
    /// The same reason in words, with the numbers behind it.
    pub detail: String,
}

/// The closed vocabulary of [`EmptyPrecedent::code`] ([FR-NV-12] AC 4).
///
/// A type rather than a `String` for the reason the doc above gives: the
/// vocabulary is *promised* closed, and only an enum makes that promise
/// checkable — a consumer matching on it is told by the compiler when a code is
/// added, and a typo cannot reach the wire. This mirrors
/// [`DegradedCause`](crate::federation::open_state::DegradedCause), the other
/// closed reason-code vocabulary in the read model, down to the
/// serialized-value pinning test; two spellings of one idea is exactly what the
/// `precedent` query exists to stop.
///
/// The wire format is unchanged — `rename_all` reproduces the same snake_case
/// strings the `String` codes carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmptyPrecedentCode {
    /// The query text named nothing in the graph.
    TargetUnresolved,
    /// The graph itself is empty — nothing has been indexed.
    GraphEmpty,
    /// The target resolved, but no node for it survived into the compared view.
    TargetAbsentFromView,
    /// The target has none of the three structural attachments to compare on.
    NoStructuralAnchors,
    /// The target's anchors exist but nothing else in the graph shares them.
    /// The opposite claim from [`Self::AnchorsAreUbiquitous`], and a different
    /// next action, which is why the two are separate codes.
    AnchorsAreUnshared,
    /// Every anchor was shared so widely it was discarded as uninformative.
    AnchorsAreUbiquitous,
    /// A degraded path: the underlying query failed.
    QueryFailed,
    /// A degraded path: results could not be read back.
    ResultsUnavailable,
}

impl EmptyPrecedentCode {
    /// The wire name — identical to the `serde` representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            EmptyPrecedentCode::TargetUnresolved => "target_unresolved",
            EmptyPrecedentCode::GraphEmpty => "graph_empty",
            EmptyPrecedentCode::TargetAbsentFromView => "target_absent_from_view",
            EmptyPrecedentCode::NoStructuralAnchors => "no_structural_anchors",
            EmptyPrecedentCode::AnchorsAreUnshared => "anchors_are_unshared",
            EmptyPrecedentCode::AnchorsAreUbiquitous => "anchors_are_ubiquitous",
            EmptyPrecedentCode::QueryFailed => "query_failed",
            EmptyPrecedentCode::ResultsUnavailable => "results_unavailable",
        }
    }
}

/// The coverage limits of one precedent answer ([NFR-CC-04]).
#[derive(Debug, Default, Serialize)]
pub struct PrecedentCoverage {
    /// The standing statement of what the answer can and cannot see.
    pub statement: String,
    /// The symbols whose structure was compared: the target itself in symbol
    /// mode, the file's own symbols in file mode. Bounded — a file with
    /// hundreds of symbols is compared in full but not listed in full.
    pub compared: Vec<SymbolRef>,
    /// How many compared symbols this list omits, whether they fell past the
    /// listing bound or past the comparison bound itself. `0` when the list is
    /// whole.
    pub compared_elided: u32,
    /// Shared nodes discarded as ubiquitous — a helper called from everywhere
    /// is not evidence of analogy, and saying which ones were dropped is how a
    /// surprising empty answer stays diagnosable.
    pub ubiquitous_anchors: Vec<UbiquitousAnchor>,
    /// How many candidate nodes shared at least one anchor before the
    /// call-shape threshold and the facet filters were applied.
    pub candidates_considered: u32,
    /// How many candidates were dropped for matching on a **single** shared
    /// callee and nothing else — the [`MIN_SHARED_CALLEES`] threshold, made
    /// visible rather than silent.
    pub dropped_single_callee_matches: u32,
}

/// A shared node too widely shared to be evidence ([NFR-CC-04]).
#[derive(Debug, Serialize)]
pub struct UbiquitousAnchor {
    /// The canonical symbol of the over-shared node.
    pub symbol: String,
    /// Its human-facing name.
    pub name: String,
    /// The facet it would have contributed to.
    pub facet: PrecedentFacet,
    /// How many nodes share it — above the stated bound named in `notion`.
    pub sharers: u32,
}
// ── Branch and merge symbol overlap ([FR-NV-13]) ────────────────────────────
//
// The integration question, made deterministic: given the refs about to be
// merged, which symbols does more than one of them modify — and, once a merge
// result is stated, which of a ref's symbols did that result not carry.
//
// A clean merge is not a complete merge. In Sprint 63 five branches contributed
// to one capability roster, git merged them without a conflict, and two of the
// arms never reached the roster: nothing in the working tree, the test suite or
// the merge output said so. This is the query that says so.
//
// [FR-NV-13]: ../../../docs/specs/requirements/FR-NV-13.md

/// Branch and merge symbol overlap across a set of git refs ([FR-NV-13]).
#[derive(Debug, Default, Serialize)]
pub struct BranchOverlapResult {
    /// The commit every ref was diffed against — the octopus merge-base of the
    /// refs, or the caller's `--base` when one was given. `None` when it could
    /// not be resolved, in which case nothing below was computed.
    pub base: Option<String>,
    /// How `base` was arrived at, in words — so a reader never has to guess
    /// whether the comparison point was chosen or supplied.
    pub base_origin: String,
    /// One row per requested ref, in the order supplied.
    pub refs: Vec<RefChangeSummary>,
    /// The symbols **more than one ref modifies**, naming the refs
    /// ([FR-NV-13] AC 1). Ordered by contention (most refs first), then by
    /// canonical symbol, and truncated to a bounded payload.
    pub contended: Vec<ContendedSymbol>,
    /// How many symbols more than one ref modifies, in full — never the length
    /// of the truncated `contended` list ([NFR-CC-04]).
    pub contended_total: u32,
    /// How many contended symbols `contended` omitted — `0` when whole.
    pub contended_elided: u32,
    /// The stated merge result and what it did not carry ([FR-NV-13] AC 2).
    /// `None` when no merge result was stated: the collision half of this query
    /// stands on its own, before any merge exists.
    pub merge: Option<MergeCheck>,
    /// What this answer can and cannot see ([NFR-CC-04]).
    pub coverage: OverlapCoverage,
    /// The resolved edge set this answer was computed over ([FR-NV-14]):
    /// present on every answer, empty or not.
    ///
    /// [FR-NV-14]: ../../../docs/specs/requirements/FR-NV-14.md
    pub resolution_denominator: ResolutionDenominator,
    /// Degradation channel ([ADR-14]) — also where an unresolvable ref, a
    /// missing `git`, and a single-ref call are reported.
    pub warnings: Vec<String>,
}

/// What one ref changed relative to the base.
#[derive(Debug, Default, Serialize)]
pub struct RefChangeSummary {
    /// The ref as the caller spelled it; Logos never rewrites it.
    #[serde(rename = "ref")]
    pub git_ref: String,
    /// The commit it resolved to, or `None` when it did not resolve.
    pub commit: Option<String>,
    /// Files this ref changed relative to the base.
    pub files_changed: u32,
    /// Indexed symbols this ref's changed line ranges landed in.
    pub symbols_modified: u32,
}

/// One symbol that more than one of the refs modifies ([FR-NV-13] AC 1).
#[derive(Debug, Serialize)]
pub struct ContendedSymbol {
    /// The contended symbol.
    #[serde(flatten)]
    pub symbol: SymbolRef,
    /// The refs whose changes land inside this symbol, in the order supplied.
    pub modified_by: Vec<String>,
    /// The refs in the same set that do **not** touch it, in the order
    /// supplied. A shared append point that only some siblings reached is the
    /// silent-drop shape: it is what Sprint 63's roster looked like from the
    /// outside. A **smell, not a proof** — see [`OverlapCoverage::statement`].
    pub absent_from: Vec<String>,
}

/// A stated merge result, and what it did not carry ([FR-NV-13] AC 2).
///
/// Deliberately not `Default`: the payload distinguishes `merge: null` (none was
/// stated) from a merge block, and an all-empty block reads as a clean bill of
/// health. There should be no one-line way to write the answer the tests exist
/// to prevent.
#[derive(Debug, Serialize)]
pub struct MergeCheck {
    /// The merge result as the caller spelled it.
    #[serde(rename = "ref")]
    pub git_ref: String,
    /// The commit it resolved to, or `None` when it did not resolve.
    pub commit: Option<String>,
    /// Files the merge result changed relative to the same base.
    pub files_changed: u32,
    /// Symbols a ref modified that the merge result does not change **at all** —
    /// the ref's work on them is not in the merge. Bounded like `contended`.
    ///
    /// A symbol the merge result *does* change is never listed here, even where
    /// the merge kept only one contributing ref's version of it. That case is
    /// not detectable by comparing which symbols changed, and it is precisely
    /// the shape a clean merge hides — read `contended`/`absent_from` for it.
    pub lost_symbols: Vec<LostSymbol>,
    /// How many such symbols there are in full ([NFR-CC-04]).
    pub lost_symbols_total: u32,
    /// How many `lost_symbols` omitted — `0` when whole.
    pub lost_symbols_elided: u32,
    /// Files a ref changed that the merge result does not change at all. The
    /// coarser twin of `lost_symbols`, and the only one that can speak for a
    /// file the index does not cover ([NFR-CC-04]). Bounded like the lists
    /// above — and counted, because a file the index holds no symbol for has no
    /// twin in `lost_symbols` to compensate for a silent truncation.
    pub lost_files: Vec<LostFile>,
    /// How many such files there are in full.
    pub lost_files_total: u32,
    /// How many `lost_files` omitted — `0` when whole.
    pub lost_files_elided: u32,
}

/// One symbol a ref modified that the stated merge result does not carry.
#[derive(Debug, Serialize)]
pub struct LostSymbol {
    /// The symbol.
    #[serde(flatten)]
    pub symbol: SymbolRef,
    /// The refs that modified it, in the order supplied.
    pub modified_by: Vec<String>,
    /// Whether the merge result changed the symbol's file at all. `true` is the
    /// stronger signal: the merge took *some* of that file and not this.
    pub merge_changed_the_file: bool,
}

/// One file a ref changed that the stated merge result does not change.
#[derive(Debug, Serialize)]
pub struct LostFile {
    /// Project-relative path.
    pub path: String,
    /// The refs that changed it, in the order supplied.
    pub modified_by: Vec<String>,
}

/// The coverage limits of one branch-overlap answer ([NFR-CC-04]).
///
/// This query joins two sources with different horizons — git, which sees every
/// byte of every ref, and the code graph, which sees one indexed snapshot. Every
/// way that join can under-report is named here rather than left to the reader.
#[derive(Debug, Default, Serialize)]
pub struct OverlapCoverage {
    /// The standing statement of what the answer can and cannot see.
    pub statement: String,
    /// `HEAD` at query time — a *label* for the snapshot, not its identity.
    /// Logos indexes the working tree, so the spans used for attribution may
    /// include uncommitted edits this commit id does not describe.
    /// `files_with_drifted_spans` is the field that answers the question
    /// honestly; this one is for saying where in history the answer sits.
    /// `None` when it could not be resolved.
    pub indexed_snapshot: Option<String>,
    /// Changed files whose content at a ref differs from **the working tree the
    /// spans were read from**, so the spans used to attribute their hunks may
    /// have moved. Bounded.
    pub files_with_drifted_spans: Vec<String>,
    /// Changed files the index holds no span-bearing symbol for — an unindexed
    /// language, an excluded path, a data file. Their changes are invisible to
    /// the symbol half of this answer. Bounded.
    pub files_without_indexed_symbols: Vec<String>,
    /// Changed line ranges that fell inside no indexed symbol span. Each is a
    /// change this answer could not attribute.
    pub unattributed_hunks: u32,
    /// Refs that did not resolve to a commit; they contribute nothing.
    pub unresolved_refs: Vec<String>,
    /// Refs that resolved but whose diff against the base failed — a partial
    /// clone with unfetched blobs, a killed subprocess. They contribute nothing
    /// and, unlike a ref that genuinely changed nothing, they are **excluded
    /// from `absent_from`**: "we did not look" is not "this ref did not touch
    /// it", and `absent_from` is the field a reader acts on.
    pub refs_not_diffed: Vec<String>,
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
    /// Unix-seconds timestamp of the last full index that built this graph
    /// (FR-NV-07, CR-130), read from the durable `project_metadata` record
    /// (`last_full_index_at`) — so a read-only process that did no indexing
    /// itself still reports it. `None` when no full index is recorded for the
    /// project, including a project whose walk admits no file: absent is
    /// reported as absent, never as `0` (NFR-CC-04).
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
    ///
    /// One ratio over every language, so a language binding nothing across a
    /// file boundary is averaged into the rest — [CR-142]'s defect survived 74
    /// sprints behind it. Read it beside
    /// [`resolution_by_language`](Self::resolution_by_language), never alone.
    ///
    /// [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
    pub resolution_coverage: f64,
    /// Resolution coverage **per language, with its denominator** ([FR-RS-09],
    /// [S-441]): one row for every language tagged on an indexed file, in
    /// language-name order ([NFR-RA-06]). A language present in the index is
    /// never an absent row — a data grammar with nothing to resolve still
    /// appears, and says so through its named state. Empty exactly when no
    /// indexed file carries a language.
    ///
    /// [FR-RS-09]: ../../../docs/specs/requirements/FR-RS-09.md
    /// [S-441]: ../../../docs/planning/journal.md#s-441-resolution-coverage-is-reported-per-language-with-its-denominator
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub resolution_by_language: Vec<LanguageResolution>,
    /// Total physical lines of code across the admitted file set, from the
    /// index-time roll-up ([FR-IX-12], [CR-085]) — the same quantity the
    /// beyond-envelope advisory ([NFR-PE-09]) uses, refreshed on full `index`.
    /// `None` when no full index has computed the roll-up for this graph: an
    /// honest absent, never a fabricated `0` ([NFR-CC-04]). Invariant:
    /// `total = source + test` whenever all three are `Some`.
    pub total_line_count: Option<u64>,
    /// Source (non-test) physical LOC, derived as `total − test` — never counted
    /// independently ([FR-IX-12]). `None` in lock-step with [`Self::total_line_count`].
    pub source_line_count: Option<u64>,
    /// Test physical LOC: the roll-up total restricted to files matched by the
    /// [FR-AN-05] test-path conventions. `None` in lock-step with
    /// [`Self::total_line_count`].
    pub test_line_count: Option<u64>,
    /// The freshness/staleness statement (ADR-11 best-effort contract).
    pub freshness: String,
    /// Degradation channel (ADR-14).
    pub warnings: Vec<String>,
}

/// One language's resolution coverage, with its denominator ([FR-RS-09],
/// [S-441], [CR-142] D3) — a row of [`StatusInfo::resolution_by_language`].
///
/// `language` is the grammar as the plugin substrate records it on each file
/// (`files.language`), the token [`LanguageCount`] uses. That is the resolver's
/// own unit — `module_separator` and `import_strategy` are declared per plugin
/// — so `.js` files read under `typescript`, the plugin that parses them, and
/// `.jsx` under `tsx`. [CR-142] §3.1 split those rows by file extension; this
/// row is their sum.
///
/// [FR-RS-09]: ../../../docs/specs/requirements/FR-RS-09.md
/// [S-441]: ../../../docs/planning/journal.md#s-441-resolution-coverage-is-reported-per-language-with-its-denominator
/// [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LanguageResolution {
    /// The language name, e.g. `"rust"`, `"tsx"`.
    pub language: String,
    /// Indexed files tagged with this language — every one of them, including
    /// files that contributed no node, since presence in the index is what
    /// earns the row.
    pub files: u64,
    /// The `Calls` relation — what `callers` and `impact` traverse.
    pub calls: RelationResolution,
    /// The `Imports` relation.
    pub imports: RelationResolution,
    /// Why this language's unbound `Calls` rows stay unbound, by reason (S-468,
    /// [FR-RS-10], [CR-150] §3.2 C) — present on the `status` row of a
    /// **package-shaped** language (Java, and Kotlin since S-472) and absent
    /// from every other row.
    ///
    /// A status-only extension: the reasons are decided by re-walking each
    /// unbound row through the binder, which needs the whole graph, so the
    /// relational answers — which attach these rows from the three aggregate
    /// reads alone ([S-442]) — carry the counts above and not this.
    ///
    /// [FR-RS-10]: ../../../docs/specs/requirements/FR-RS-10.md
    /// [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
    /// [S-442]: ../../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_residue: Option<CallResidue>,
}

/// The reasons a package-shaped language's unbound `Calls` rows stay unbound
/// (S-468, [FR-RS-10], [CR-150] §3.2 C), each counted over one denominator —
/// [`unbound`](Self::unbound), the language's `calls.references − calls.bound`.
///
/// The reasons partition the denominator: `unbound = Σ reasons + unclassified`.
///
/// | reason | the row stays unbound because |
/// |---|---|
/// | `no-receiver-evidence` | the file proves no receiver type (a bare Method-form row, or a bare call naming no import) |
/// | `external-type` | the receiver's type is declared by no file of this repository — the JDK, a library, a generated type, or (outside a workspace) another member |
/// | `type-in-another-member` | the receiver's type is declared by another workspace member (workspace scope only) |
/// | `overload-ambiguous` | the type, or the nearest supertype level holding the name, declares two or more callables of that name — or two static imports each supply one |
/// | `type-ambiguous` | the type's name reaches two declarations here (a `src/main` and a `src/test` class of one name) |
/// | `supertype-unreached` | the type is here, and neither it nor any supertype reached here declares the name — the chain leaves the repository, stops at an interface, or cycles |
///
/// **Scope.** One repository's graph cannot tell another member's type from a
/// library's, so a `status` read outside a workspace has `scope: "repository"`
/// and no `type-in-another-member` entry — its `external-type` includes them.
/// A workspace `status` compares every member's declared types and moves those
/// rows to `type-in-another-member`, with `scope: "workspace"`.
///
/// [FR-RS-10]: ../../../docs/specs/requirements/FR-RS-10.md
/// [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallResidue {
    /// Unbound `Calls` rows of the language — the denominator of every reason.
    pub unbound: u64,
    /// Rows per reason; every reason of the scope is present, a `0` included —
    /// each is a count the classification made.
    pub reasons: std::collections::BTreeMap<CallResidueReason, u64>,
    /// Unbound rows no reason is assigned to: a capture-before-delete row
    /// awaiting its target, or a row the ledger holds unbound that the binder
    /// binds now (a graph bound by an older binary, or a sync that did not
    /// re-select it). `0` on a graph freshly indexed by this binary.
    pub unclassified: u64,
    /// Over what the external/other-member split was decided.
    pub scope: ResidueScope,
    /// The `external-type` rows grouped by the fully-qualified names each could
    /// be — what a workspace sorts into `type-in-another-member`. Not
    /// serialised.
    #[serde(skip)]
    pub(crate) external_candidates: std::collections::BTreeMap<Vec<Vec<String>>, u64>,
    /// Every fully-qualified name a top-level type of this repository is
    /// declared under — the other half of that comparison. Not serialised.
    #[serde(skip)]
    pub(crate) declared_types: Vec<Vec<String>>,
}

/// Why a package-shaped `Calls` row stays unbound — a key of
/// [`CallResidue::reasons`], serialised as its kebab-case token (S-468,
/// [FR-RS-10]). Declared in token order, so the map's order is the tokens'.
///
/// [FR-RS-10]: ../../../docs/specs/requirements/FR-RS-10.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallResidueReason {
    /// The receiver's type is declared by no file of this repository.
    ExternalType,
    /// The file proves no receiver type.
    NoReceiverEvidence,
    /// The deciding level declares two or more callables of that name, or two
    /// imports each supply one.
    OverloadAmbiguous,
    /// Neither the type nor a supertype reached here declares the name.
    SupertypeUnreached,
    /// The type's name reaches two declarations here.
    TypeAmbiguous,
    /// Another workspace member declares the receiver's type (workspace scope
    /// only).
    TypeInAnotherMember,
}

/// Over what a [`CallResidue`]'s external/other-member split was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResidueScope {
    /// One repository's graph: another member's type reads `external-type`.
    Repository,
    /// A workspace `status`, which compared every member's declared types.
    Workspace,
}

impl CallResidue {
    /// The reasons a repository-scoped readout counts, in token order.
    pub const REPOSITORY_REASONS: [CallResidueReason; 5] = [
        CallResidueReason::ExternalType,
        CallResidueReason::NoReceiverEvidence,
        CallResidueReason::OverloadAmbiguous,
        CallResidueReason::SupertypeUnreached,
        CallResidueReason::TypeAmbiguous,
    ];

    /// Move every `external-type` row whose type could be one `elsewhere`
    /// declares — another member's declared names — to
    /// `type-in-another-member`, and mark the residue workspace-scoped. A
    /// candidate matches when it, or a leading run of its segments (an outer
    /// type, for a nested one), is declared elsewhere.
    pub(crate) fn split_by_workspace(&mut self, elsewhere: &dyn Fn(&[String]) -> bool) {
        let moved: u64 = self
            .external_candidates
            .iter()
            .filter(|(candidates, _)| {
                candidates
                    .iter()
                    .any(|fqn| (1..=fqn.len()).any(|k| elsewhere(&fqn[..k])))
            })
            .map(|(_, rows)| rows)
            .sum();
        if let Some(external) = self.reasons.get_mut(&CallResidueReason::ExternalType) {
            *external -= moved;
        }
        self.reasons
            .insert(CallResidueReason::TypeInAnotherMember, moved);
        self.scope = ResidueScope::Workspace;
    }
}

/// One relation class's resolution over one language: the ledger's numerator
/// over its denominator, and the resolved edges split by whether they cross a
/// file boundary ([FR-RS-09]).
///
/// Two populations, deliberately side by side and deliberately not reconciled.
/// `bound / references` is the **ledger's** ratio, the per-language slice of
/// [`StatusInfo::refs_resolved`] over [`StatusInfo::refs_total`]. The locality
/// split is the **resolved edge set's**: an edge is unique per
/// `(source, target, kind)` and one whose target lies in no indexed file has no
/// locality, so `same_file_edges + cross_file_edges` need not equal `bound`.
///
/// The cross-file figure and its absence are produced together by
/// [`CrossFileAbsence::classify`], so exactly one of the two is `Some`: a
/// cross-file count is only ever a count of something that exists, never a `0`
/// read as a measurement ([NFR-CC-04], [NFR-RA-05]). This is the **typed
/// resolution denominator** the relational answers attach next ([S-442]).
///
/// Deliberately **not** `Default`: a defaulted row would carry neither the
/// figure nor its absence — the one state this type exists to rule out — so
/// the only constructor is [`measured`](Self::measured).
///
/// [FR-RS-09]: ../../../docs/specs/requirements/FR-RS-09.md
/// [S-442]: ../../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelationResolution {
    /// Ledger rows of this class recorded by the language's files — the
    /// denominator.
    pub references: u64,
    /// Of those rows, the ones currently bound — the numerator.
    pub bound: u64,
    /// Resolved edges of this class, leaving the language's nodes, whose two
    /// endpoints share a file. Always a plain count, `0` included — unlike the
    /// cross-file figure it is never replaced by a named state, and a `0` here
    /// can sit beside a cross-file figure (a class whose every edge crosses a
    /// file boundary).
    pub same_file_edges: u64,
    /// Resolved edges of this class, leaving the language's nodes, that cross a
    /// file boundary; `None` exactly when there are none, and
    /// [`cross_file_absence`](Self::cross_file_absence) names why.
    pub cross_file_edges: Option<u64>,
    /// Why there is no cross-file figure; `None` exactly when there is one.
    pub cross_file_absence: Option<CrossFileAbsence>,
}

impl RelationResolution {
    /// Assemble one class's row from its four counts, classifying the
    /// cross-file figure through [`CrossFileAbsence::classify`] — the one
    /// place a cross-file count becomes a figure or a named state.
    #[must_use]
    pub fn measured(
        references: u64,
        bound: u64,
        same_file_edges: u64,
        cross_file_edges: u64,
    ) -> Self {
        let (cross_file_edges, cross_file_absence) =
            CrossFileAbsence::classify(references, bound, same_file_edges, cross_file_edges);
        Self {
            references,
            bound,
            same_file_edges,
            cross_file_edges,
            cross_file_absence,
        }
    }
}

/// The **resolution denominator** a relational answer was computed over
/// ([FR-NV-14], [S-442], [CR-143]): the [`LanguageResolution`] row of every
/// language the answer is anchored in.
///
/// A relational answer is a traversal of the resolved edge set, and an empty or
/// short traversal has two causes a bare list cannot tell apart — *nothing
/// depends on this* and *nothing could be resolved here*. This is the field that
/// tells them apart, on the answer, where a machine consumer reads it: beside
/// `total: 0` for a TypeScript symbol it reads `calls.cross_file_absence:
/// same-file-only`, and beside a Rust answer it reads the cross-file figure the
/// traversal ran over. It rides **every** answer, empty or not — a partial set
/// read as complete is the same false clearance with a count in front of it
/// ([CR-143] D3).
///
/// # Keyed by the anchor, not by the result
///
/// The anchors are what the query named and resolved — the symbol of
/// `callers`/`callees`/`impact`, every declared symbol of `impact_intersection`,
/// the changed files of `affected`, every changed file of `branch_overlap` the
/// index holds — and each anchor contributes the row of its file's
/// `files.language`, the same key [`StatusInfo::resolution_by_language`] uses.
/// The rows are that read-model's rows, computed by the same
/// `resolve::coverage_by_language`, so the readout and the answer can never
/// disagree ([CR-143] §3.5: computed once, consumed twice).
///
/// A row counts edges **leaving** its language's nodes. An inbound answer
/// (`callers`, `impact` upstream, `affected`) whose dependents live in a second
/// language is therefore bounded by that language's row too, which this field
/// does not carry; a `typescript` anchor called from `tsx` is the common case.
///
/// # Exactly one of the rows or their absence
///
/// `languages` is non-empty exactly when `absence` is `None`, so the one state
/// this type exists to rule out — no row and no reason — has no spelling.
/// [`measured`](Self::measured) is the only constructor that decides between
/// them. [`Default`] is the `n/a` state, **not** an empty list: a defaulted
/// answer (the [ADR-14] degraded path) states that no denominator was read,
/// rather than an absence of one.
///
/// [FR-NV-14]: ../../../docs/specs/requirements/FR-NV-14.md
/// [S-442]: ../../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
/// [CR-143]: ../../../docs/requests/CR-143-a-relational-answer-states-its-resolution-denominator.md
/// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolutionDenominator {
    /// One row per language an anchor of the answer lies in, in language-name
    /// order ([NFR-RA-06]). Empty exactly when [`absence`](Self::absence) names
    /// why.
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub languages: Vec<LanguageResolution>,
    /// Why there is no row; `None` exactly when there is one.
    pub absence: Option<DenominatorAbsence>,
}

impl ResolutionDenominator {
    /// Select the rows for an answer's resolved anchors — one entry per anchor,
    /// carrying its file's language, or `None` when the anchor has no file or
    /// its file records no language — out of the per-language `rows`
    /// `resolve::coverage_by_language` read.
    ///
    /// The one place a denominator becomes rows or a named absence: no anchor
    /// resolved is [`DenominatorAbsence::Unindexed`]; anchors that resolved
    /// and select no row are [`DenominatorAbsence::NoLanguageRecorded`]. An
    /// anchor with no language beside anchors that have one selects nothing
    /// and is not reported separately.
    #[must_use]
    pub fn measured(rows: Vec<LanguageResolution>, anchors: &[Option<String>]) -> Self {
        if anchors.is_empty() {
            return Self::absent(DenominatorAbsence::Unindexed);
        }
        let wanted: std::collections::BTreeSet<&str> =
            anchors.iter().flatten().map(String::as_str).collect();
        let mut languages: Vec<LanguageResolution> = rows
            .into_iter()
            .filter(|row| wanted.contains(row.language.as_str()))
            .collect();
        if languages.is_empty() {
            return Self::absent(DenominatorAbsence::NoLanguageRecorded {
                anchors: anchors.len() as u64,
            });
        }
        // The read-model already orders by name; re-sorting here makes the
        // order this type's own guarantee rather than its producer's.
        languages.sort_by(|a, b| a.language.cmp(&b.language));
        Self {
            languages,
            absence: None,
        }
    }

    /// The denominator of an answer that read none — the [ADR-14] degraded
    /// path, or a denominator read that failed beside a successful answer.
    ///
    /// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
    #[must_use]
    pub fn not_available() -> Self {
        Self::absent(DenominatorAbsence::NotAvailable)
    }

    /// The clause a relational answer's own words gain when a language it is
    /// anchored in binds no `Calls` edge across a file boundary — `None` when
    /// every such language carries a cross-file figure ([FR-NV-12] AC 4,
    /// [CR-143] §3.7).
    ///
    /// An empty answer phrased about the code — `precedent`'s "no structural
    /// anchors", the query view's "No callers of …" — would state a coverage
    /// gap as a property of the user's code where the resolver never binds a
    /// cross-file call: the structure such a call would supply is unresolved
    /// rather than absent. Both sites append this one clause, so the two
    /// surfaces cannot word the same condition twice (R5). The named state is
    /// the row's own serialised tag, so the clause speaks the closed lexicon
    /// rather than a spelling of its own.
    ///
    /// Only the two states that establish **unresolved** earn it:
    /// `same-file-only` and `no-resolved-edges`. `no-references-recorded`
    /// means the language's files recorded no call at all, so a target that
    /// calls nothing is truly absent of calls, and saying "unresolved" would
    /// name a cause the condition does not establish (R1).
    ///
    /// [FR-NV-12]: ../../../docs/specs/requirements/FR-NV-12.md
    /// [CR-143]: ../../../docs/requests/CR-143-a-relational-answer-states-its-resolution-denominator.md
    #[must_use]
    pub fn unresolved_calls_clause(&self) -> Option<String> {
        let gaps: Vec<String> = self
            .languages
            .iter()
            .filter_map(|row| {
                let absence = row.calls.cross_file_absence?;
                if matches!(absence, CrossFileAbsence::NoReferencesRecorded) {
                    return None;
                }
                let absence = serde_json::to_value(absence).ok()?;
                Some(format!(
                    "{} binds no Calls edge across a file boundary ({}; {} of {} Calls \
                     reference(s) bound)",
                    row.language,
                    absence["cause"].as_str()?,
                    row.calls.bound,
                    row.calls.references
                ))
            })
            .collect();
        (!gaps.is_empty()).then(|| gaps.join(", and "))
    }

    fn absent(absence: DenominatorAbsence) -> Self {
        Self {
            languages: Vec::new(),
            absence: Some(absence),
        }
    }
}

impl Default for ResolutionDenominator {
    /// [`not_available`](Self::not_available) — never an empty row list with
    /// no reason, which is the state this type rules out.
    fn default() -> Self {
        Self::not_available()
    }
}

/// Why a relational answer's [`ResolutionDenominator`] carries **no language
/// row** ([FR-NV-14], [S-442]) — a classifier over one question, R0 of
/// [`absence`](crate::models::quality::absence).
///
/// It lives here rather than beside [`CrossFileAbsence`] because two of its
/// spellings are lexicon words reused as written — `unindexed` and `n/a` —
/// and `models/quality.rs` must hold no sentinel outside the lexicon's own
/// declaration. Each arm's wording is an [`absence::SENTINELS`] spelling.
///
/// [`absence::SENTINELS`]: crate::models::quality::absence::SENTINELS
/// [FR-NV-14]: ../../../docs/specs/requirements/FR-NV-14.md
/// [S-442]: ../../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "cause", rename_all = "kebab-case")]
pub enum DenominatorAbsence {
    /// Nothing the answer is anchored on is in the index — the symbol resolved
    /// to no node, or no changed path is an indexed file. R1: that is the
    /// whole of what the condition establishes. R3: no command is named,
    /// because a misspelt symbol is as likely as an unindexed one and the
    /// answer's `suggestions` already speak to it.
    Unindexed,
    /// The anchors resolved, and none lies in a file the index records a
    /// language for (a file-less node, or a file no plugin tagged), so there is
    /// no row to select. R2: carries how many anchors were looked up.
    NoLanguageRecorded {
        /// How many resolved anchors selected no row.
        anchors: u64,
    },
    /// No denominator was read: the answer degraded, the denominator's own
    /// read failed (the answer's `warnings` say why), or the question named
    /// nothing to anchor on — no changed file, no declared symbol, no ref that
    /// changed anything. R1: names no cause.
    #[serde(rename = "n/a")]
    NotAvailable,
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
#[derive(Debug, Clone, Default, Serialize)]
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
    /// The resolved edge set this answer was computed over ([FR-NV-14]):
    /// present on every answer, empty or not.
    ///
    /// [FR-NV-14]: ../../../docs/specs/requirements/FR-NV-14.md
    pub resolution_denominator: ResolutionDenominator,
    /// Degradation channel (ADR-14).
    pub warnings: Vec<String>,
}

/// One dependent file in an [`AffectedResult`] closure.
#[derive(Debug, Clone, Serialize)]
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

    /// A repository-scoped residue with `external` rows, grouped by the
    /// candidate names each could be.
    fn residue(external: &[(&[&[&str]], u64)]) -> CallResidue {
        let external_candidates: std::collections::BTreeMap<Vec<Vec<String>>, u64> = external
            .iter()
            .map(|(candidates, rows)| {
                let names = candidates
                    .iter()
                    .map(|fqn| fqn.iter().map(|s| (*s).to_string()).collect())
                    .collect();
                (names, *rows)
            })
            .collect();
        let external_rows = external_candidates.values().sum::<u64>();
        let mut reasons: std::collections::BTreeMap<CallResidueReason, u64> =
            CallResidue::REPOSITORY_REASONS.iter().map(|r| (*r, 0)).collect();
        reasons.insert(CallResidueReason::ExternalType, external_rows);
        CallResidue {
            unbound: external_rows,
            reasons,
            unclassified: 0,
            scope: ResidueScope::Repository,
            external_candidates,
            declared_types: Vec::new(),
        }
    }

    fn split(mut residue: CallResidue, elsewhere: &[&[&str]]) -> CallResidue {
        let elsewhere: Vec<Vec<String>> = elsewhere
            .iter()
            .map(|fqn| fqn.iter().map(|s| (*s).to_string()).collect())
            .collect();
        residue.split_by_workspace(&|fqn: &[String]| elsewhere.iter().any(|e| e == fqn));
        residue
    }

    #[test]
    fn a_workspace_split_moves_only_the_rows_whose_type_another_member_declares() {
        let moved = split(
            residue(&[
                (&[&["com", "x", "Mailer"]], 3),
                (&[&["java", "util", "List"]], 2),
            ]),
            &[&["com", "x", "Mailer"]],
        );
        assert_eq!(moved.reasons[&CallResidueReason::ExternalType], 2);
        assert_eq!(moved.reasons[&CallResidueReason::TypeInAnotherMember], 3);
        assert_eq!(moved.scope, ResidueScope::Workspace);
    }

    #[test]
    fn a_nested_candidate_matches_through_its_declared_outer_type() {
        let moved = split(
            residue(&[(&[&["com", "x", "Outer", "Inner"]], 1)]),
            &[&["com", "x", "Outer"]],
        );
        assert_eq!(moved.reasons[&CallResidueReason::TypeInAnotherMember], 1);
        assert_eq!(moved.reasons[&CallResidueReason::ExternalType], 0);
    }

    #[test]
    fn a_row_matches_when_any_of_its_candidates_is_declared_elsewhere() {
        let moved = split(
            residue(&[(&[&["com", "app", "Util"], &["com", "shared", "Util"]], 1)]),
            &[&["com", "shared", "Util"]],
        );
        assert_eq!(moved.reasons[&CallResidueReason::TypeInAnotherMember], 1);
    }

    #[test]
    fn a_workspace_split_with_nothing_declared_elsewhere_states_a_zero_and_its_scope() {
        for start in [residue(&[(&[&["java", "util", "List"]], 4)]), residue(&[])] {
            let external = start.reasons[&CallResidueReason::ExternalType];
            let moved = split(start, &[&["com", "x", "Mailer"]]);
            assert_eq!(moved.reasons[&CallResidueReason::ExternalType], external);
            assert_eq!(moved.reasons.get(&CallResidueReason::TypeInAnotherMember), Some(&0));
            assert_eq!(moved.scope, ResidueScope::Workspace);
        }
    }
}
