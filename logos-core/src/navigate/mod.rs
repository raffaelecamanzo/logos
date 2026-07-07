//! The navigation service — fallible bodies of the eight navigation tools
//! (S-013, [navigation-service], [FR-NV-01..09]).
//!
//! Every function here is a **best-effort-fresh point query** ([ADR-11],
//! [NFR-DM-02]): it reads the latest committed WAL snapshot through the
//! runtime's read-only pool ([ADR-02]) and **never reconciles the working
//! tree per call** ([FR-RC-05]) — background sync (and the one-time
//! auto-index prologue, [FR-IX-07]) are what keep the graph current. A
//! stale-by-a-debounce answer costs the agent at most one re-read; a
//! per-call discovery walk would blow the sub-100 ms budget ([NFR-PE-01]).
//!
//! # Query routing ([ADR-05])
//!
//! Anything answerable by an index is a SQL point query on the
//! [`GraphStore`] seam (`search`, `node`, `callers`, `callees`, `status`);
//! anything needing whole-graph traversal (`context`, `explore`, `impact`)
//! runs on the cached hydrated petgraph view ([graph-hydration]), keyed by
//! the engine's sync stamp so repeated navigation amortises one hydration
//! ([NFR-PE-07]).
//!
//! # Unknown symbols ([FR-NV-09])
//!
//! A symbol that resolves to nothing yields an *empty* result carrying
//! "did you mean" suggestions — never an error. The [`Engine`] wrappers add
//! the final infallible-surface layer ([ADR-14]): any `Err` from these
//! bodies degrades to a warning-carrying read-model.
//!
//! [navigation-service]: ../../../docs/specs/architecture/components/navigation-service.md
//! [graph-hydration]: ../../../docs/specs/architecture/components/graph-hydration.md
//! [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
//! [ADR-05]: ../../../docs/specs/architecture/decisions/ADR-05.md
//! [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
//! [FR-NV-01..09]: ../../../docs/specs/requirements/FR-NV-01.md
//! [FR-RC-05]: ../../../docs/specs/requirements/FR-RC-05.md
//! [FR-IX-07]: ../../../docs/specs/requirements/FR-IX-07.md
//! [NFR-DM-02]: ../../../docs/specs/requirements/NFR-DM-02.md
//! [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
//! [NFR-PE-07]: ../../../docs/specs/requirements/NFR-PE-07.md

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Component, Path};

use anyhow::Result;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::Direction;

use crate::engine::Engine;
use crate::graph_store::{GraphStore, NodeRow};
use crate::hydrate::{EdgeData, Granularity, GraphView, Vertex};
use crate::model::{EdgeKind, NodeId, NodeKind};
use crate::models::navigation::{
    AffectedFile, AffectedResult, CalleesResult, CallersResult, ContextBundle, ContextNode,
    EdgeDirection, EdgeSummary, ExploreResult, ExploreSymbol, FileGroup, GraphElementEdge,
    GraphElementNode, GraphElements, GraphGranularity, GraphLayer, ImpactEntry, ImpactResult,
    ImplementorsResult,
    NodeDetail, NodeInfo, ReferencingDocsResult, SearchResult, StatusInfo, SymbolRef, TraceLink,
};

#[cfg(test)]
mod tests;

/// Default `search` result cap ([FR-NV-01]).
const DEFAULT_SEARCH_LIMIT: usize = 20;
/// Default `callers`/`callees` result cap ([FR-NV-05]).
const DEFAULT_ADJACENCY_LIMIT: usize = 50;
/// Default `context` bundle cap ([FR-NV-02]).
const DEFAULT_MAX_NODES: usize = 25;
/// Default `explore` file-group cap ([FR-NV-03]).
const DEFAULT_MAX_FILES: usize = 10;
/// Default `impact` traversal depth ([FR-NV-06]).
const DEFAULT_IMPACT_DEPTH: usize = 3;
/// Default visible-element cap for `graph_elements` ([FR-UI-08], [ADR-29]): the
/// level-of-detail bound the whole-graph canvas opens within. Elements beyond it
/// are reported as elided, never silently dropped ([NFR-CC-04]).
///
/// Lowered from 500 to 250 by [CR-030]/S-119 so the canvas **opens sparser** and
/// is easier to read and click; the honest "N more not shown" notice still names
/// the remainder, and both the `?cap=` override and the additive "Expand
/// neighbours" control widen the rendered set back out on demand.
const DEFAULT_GRAPH_ELEMENT_CAP: usize = 250;
/// The **separate reserved budget** the documentation-intent overlay spends on
/// governing-doc nodes when it is active ([FR-UI-16], [ADR-37], S-128). It is
/// deliberately **small** relative to [`DEFAULT_GRAPH_ELEMENT_CAP`] and computed
/// entirely **outside** the structural degree ranking, so the intent overlay can
/// never crowd out the code anchors — the [CR-014] doc-flooding guard the fitness
/// test pins. A tuning constant to settle during dogfood ([ADR-37] open question);
/// surplus governing-doc nodes beyond it are reported as elided, never silently
/// dropped ([NFR-CC-04]).
///
/// [CR-014]: ../../../docs/requests/CR-014-context-seed-doc-flooding.md
const INTENT_OVERLAY_BUDGET: usize = 60;
/// `context` expansion hop depth — OQ-05 resolved this to a 1-hop default;
/// the configurable-to-2 knob rides the config component in a later story.
const CONTEXT_HOPS: u32 = 1;
/// How many FTS seeds **of each kind class** (code vs non-code) anchor a
/// `context` bundle before expansion. Kept per-class — not a single global cut —
/// so doc-node FTS dominance cannot starve the code anchors (CR-014, [FR-NV-02]).
const SEED_LIMIT: i64 = 8;
/// The widened FTS pool the seed window is kind-balanced from. A common prose
/// token matches far more text-dense doc/requirement/story bodies than code
/// symbols, so an all-kinds top-`SEED_LIMIT` cut can be *all* docs and leave the
/// code symbols that also match unseen. Fetching a generous pool and partitioning
/// it by kind class (see [`context`]) guarantees the code symbols that match are
/// retrieved even when the doc matches outrank them; bounded so the match cost
/// stays within the navigation budget ([NFR-PE-01]).
const SEED_POOL: i64 = SEED_LIMIT * 16;
/// How many "did you mean" names an unknown symbol earns ([FR-NV-09]).
const SUGGEST_LIMIT: i64 = 5;
/// The DL-03 upstream label: who breaks if the symbol changes.
const UPSTREAM_LABEL: &str = "breaks if changed";
/// The DL-03 downstream label: what the symbol depends on.
const DOWNSTREAM_LABEL: &str = "depends on";
/// The FR-NV-10 doc-aware-impact label: which docs reference the symbol.
const DOCS_LABEL: &str = "documented by";

// ── The eight tools ─────────────────────────────────────────────────────────

/// `search` — FTS5-ranked, kind-filtered symbol search ([FR-NV-01]).
pub(crate) fn search(
    engine: &Engine,
    query: &str,
    kind: Option<NodeKind>,
    limit: Option<usize>,
) -> Result<SearchResult> {
    let runtime = engine.nav_runtime()?;
    // Saturating: a pathological huge `limit` must never wrap into SQLite's
    // "negative LIMIT = unbounded".
    let limit = i64::try_from(limit.unwrap_or(DEFAULT_SEARCH_LIMIT)).unwrap_or(i64::MAX);
    // Raw user text is phrase-quoted before it reaches FTS5 so punctuation (e.g.
    // the `-` in `web-surface`) can never be misread as an FTS operator and yield
    // an opaque empty result. `store.search` itself stays raw-FTS — `context`
    // hands it a hand-built expression (`seed_query`), so the neutralisation lives
    // here, at the raw-input boundary.
    let fts_query = phrase_query(query);
    let (rows, suggestions) = runtime.submit_read(|store| {
        let rows = match &fts_query {
            Some(q) => store.search(q, kind, limit)?,
            None => Vec::new(),
        };
        // Empty result → best-effort "did you mean" (FR-NV-09). `suggest` does its
        // own tokenisation, so it takes the raw `query`, not the phrase form.
        let suggestions = if rows.is_empty() {
            store.suggest(query, SUGGEST_LIMIT)?
        } else {
            Vec::new()
        };
        Ok((rows, suggestions))
    })?;
    Ok(SearchResult {
        query: query.to_string(),
        hits: rows.iter().map(symbol_ref).collect(),
        suggestions,
        warnings: Vec::new(),
    })
}

/// `context` — deterministic seed → expand → centrality-rank bundle
/// ([FR-NV-02]): FTS5-seed the task text, expand the neighbourhood 1 hop
/// (OQ-05) along **all** code edge kinds (calls/imports/contains — the `Symbol`
/// view) **and** the doc↔code edges read from the canonical store (see
/// [`doc_neighbours`]), so the documentation explaining the seeded/expanded
/// code joins the bundle as a non-seed neighbour; rank by match-score +
/// normalised degree centrality, cap at `max_nodes`.
pub(crate) fn context(
    engine: &Engine,
    task: &str,
    max_nodes: Option<usize>,
    include_code: bool,
) -> Result<ContextBundle> {
    let runtime = engine.nav_runtime()?;
    let max_nodes = max_nodes.unwrap_or(DEFAULT_MAX_NODES);

    // A task is prose, not a symbol: FTS5's default AND semantics would
    // require one name to contain *every* word. Seed with OR-of-prefix-terms
    // instead, so any task token can anchor the bundle.
    let fts_query = seed_query(task);
    let (seeds, suggestions) = runtime.submit_read(|store| {
        let pool = match &fts_query {
            Some(query) => store.search(query, None, SEED_POOL)?,
            None => Vec::new(),
        };
        // Kind-balance the seed window (CR-014): for a common prose token a
        // docs-rich graph ranks its text-dense doc/requirement/story bodies above
        // every code symbol, so an all-kinds top-`SEED_LIMIT` cut can be entirely
        // documentation and starve the code anchors. Partition the wider pool into
        // code and non-code, capping each class at `SEED_LIMIT` while preserving
        // FTS rank order within the class — code symbols keep their slots
        // regardless of doc dominance, and doc nodes keep theirs so a documentation
        // node can still anchor a bundle (FR-NV-02).
        let cap = SEED_LIMIT as usize;
        let mut code: Vec<NodeRow> = Vec::new();
        let mut docs: Vec<NodeRow> = Vec::new();
        for row in pool {
            let bucket = if row.kind.is_non_code() {
                &mut docs
            } else {
                &mut code
            };
            if bucket.len() < cap {
                bucket.push(row);
            }
        }
        // Code seeds first: their match-score (1/(1+position)) is then at least as
        // strong as before the fix, so a bundle a query returns today stays a
        // superset after it (NFR-RA-06); the doc seeds follow and anchor in turn.
        code.append(&mut docs);
        let seeds = code;
        let suggestions = if seeds.is_empty() {
            store.suggest(task, SUGGEST_LIMIT)?
        } else {
            Vec::new()
        };
        Ok((seeds, suggestions))
    })?;
    if seeds.is_empty() {
        return Ok(ContextBundle {
            task: task.to_string(),
            hops: CONTEXT_HOPS,
            suggestions,
            ..ContextBundle::default()
        });
    }

    // The full lexical-plus-dependency view: FR-NV-02 expands along
    // calls/imports/contains, and `Symbol` is the one granularity in which
    // `contains` appears as an edge.
    let view = engine.hydrate(Granularity::Symbol)?;
    let graph = view.graph();

    // Seed scores decay by FTS rank position: 1, 1/2, 1/3, …
    let mut candidates: HashMap<NodeIndex, (f64, bool)> = HashMap::new();
    let mut frontier: Vec<NodeIndex> = Vec::new();
    // Non-code seeds (doc/requirement/story/config) have no vertex in the
    // code-only hydrated view, so they cannot anchor through `index_of` — the
    // CR-014 defect silently dropped them, emptying the bundle for a prose task
    // that matched only documentation. They anchor directly as ranked nodes keyed
    // by store id, and the doc seeds among them expand along doc→code edges so a
    // documentation anchor still reaches its implementing symbols ([FR-NV-02]).
    let mut seed_anchors: Vec<RankedNode> = Vec::new();
    let mut doc_seed_ids: Vec<NodeId> = Vec::new();
    for (position, seed) in seeds.iter().enumerate() {
        let match_score = 1.0 / (1.0 + position as f64);
        match view.index_of(seed.symbol.as_str()) {
            // A code seed anchors at its vertex. Two seeds can collapse onto one
            // vertex (shared symbol): the higher-ranked score wins (the code-first
            // order makes the first occurrence the strongest) and the vertex enters
            // the frontier once.
            Some(idx) => {
                if let std::collections::hash_map::Entry::Vacant(slot) = candidates.entry(idx) {
                    slot.insert((match_score, true));
                    frontier.push(idx);
                }
            }
            None => {
                seed_anchors.push(RankedNode {
                    node_id: seed.id,
                    key: seed.symbol.as_str().to_string(),
                    score: match_score,
                    seed: true,
                });
                if seed.kind.is_doc() {
                    doc_seed_ids.push(seed.id);
                }
            }
        }
    }

    // Fold each doc seed's implementing code symbols (its outbound doc→code edges)
    // into the expansion frontier, so a documentation anchor reaches the code it
    // documents — and that code's 1-hop neighbourhood and centrality — exactly as
    // a code seed would ([FR-NV-02]). A target already seeded as code keeps its
    // (stronger) seed score via the vacancy guard.
    for symbol in doc_seed_code_targets(engine, &doc_seed_ids)? {
        if let Some(idx) = view.index_of(&symbol) {
            if let std::collections::hash_map::Entry::Vacant(slot) = candidates.entry(idx) {
                slot.insert((0.0, false));
                frontier.push(idx);
            }
        }
    }

    // Hop expansion: undirected neighbours, no score of their own (their
    // rank comes from centrality alone).
    for _ in 0..CONTEXT_HOPS {
        let mut next: Vec<NodeIndex> = Vec::new();
        for &idx in &frontier {
            for neighbour in graph.neighbors_undirected(idx) {
                if let std::collections::hash_map::Entry::Vacant(slot) = candidates.entry(neighbour)
                {
                    slot.insert((0.0, false));
                    next.push(neighbour);
                }
            }
        }
        frontier = next;
    }

    // Degree centrality, normalised over the candidate set (≥1 guard so an
    // all-isolated candidate set divides cleanly).
    let degree_of = |idx: NodeIndex| -> usize {
        graph.neighbors_directed(idx, Direction::Incoming).count()
            + graph.neighbors_directed(idx, Direction::Outgoing).count()
    };
    let max_degree = candidates
        .keys()
        .map(|&idx| degree_of(idx))
        .max()
        .unwrap_or(1)
        .max(1);

    // Code candidates carry the view-derived score (FTS match + normalised
    // degree centrality); each keeps its symbol key for the deterministic
    // tie-break and its store id so the doc graph can be queried for it.
    let mut ranked: Vec<RankedNode> = candidates
        .iter()
        .filter_map(|(&idx, &(match_score, seed))| {
            let node_id = graph[idx].node_id?;
            let centrality = degree_of(idx) as f64 / max_degree as f64;
            Some(RankedNode {
                node_id,
                key: graph[idx].key.clone(),
                score: match_score + centrality,
                seed,
            })
        })
        .collect();

    // Doc-aware expansion (FR-NV-02): the hydrated `Symbol` view is the code
    // subgraph — S-036 drops every documentation node and doc↔code edge at
    // build time so metrics/cycle/DSM stay byte-identical w.r.t. docs (FR-DG-06,
    // ADR-19; see the doc comment on `hydrate::view::build_view`). The doc graph
    // therefore lives ONLY in the canonical store, so — exactly as `node` and
    // `referencing_docs` do — read the doc↔code edges there: every DocSection/
    // DocFile documenting a seed or expanded code symbol joins the candidate set
    // as a non-seed expansion neighbour BEFORE ranking and the `max_nodes` cut,
    // so docs are ranked and budget-bound alongside the code nodes.
    let code_ids: Vec<NodeId> = ranked.iter().map(|node| node.node_id).collect();
    // The non-code seeds anchor alongside the code candidates, ranked and
    // budget-bound through the same sort and `max_nodes` cut (CR-014, [FR-NV-02]).
    ranked.extend(seed_anchors);
    ranked.extend(doc_neighbours(engine, &code_ids, max_degree)?);

    // Cross-artifact expansion (FR-CG-11, CR-011): the artifact graph — like the
    // doc graph — is fenced out of the hydrated code subgraph for metric
    // neutrality, so it lives only in the canonical store. Reading the store's
    // `ArtifactRef`/`ArtifactBinding` edges for each candidate code symbol pulls
    // in the artifact node it binds (a route handler's `ApiOperation`) and that
    // node's spec section, exactly as `context` expands along the doc edges.
    ranked.extend(artifact_neighbours(engine, &code_ids, max_degree)?);

    // A documentation node can surface both as a seed anchor (FTS match-score)
    // and as a doc neighbour of the seeded code (degree centrality); merge the two
    // by store id so it scores like a code seed — match-score + centrality — and
    // appears once. Disjoint kinds keep every other id collision-free, so this is
    // a no-op for them ([FR-NV-02], [NFR-RA-06]).
    let mut ranked = merge_ranked(ranked);

    // Deterministic ranking: score desc, then symbol key asc (NFR-RA-06 — same
    // graph state, same bundle, regardless of HashMap/query iteration order).
    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.key.cmp(&b.key))
    });
    ranked.truncate(max_nodes);

    // Materialise rows for the selected vertices (one pooled read) — code and
    // doc nodes alike resolve through the same store lookup.
    let ids: Vec<NodeId> = ranked.iter().map(|node| node.node_id).collect();
    let rows = fetch_rows(engine, &ids)?;

    let mut nodes = Vec::with_capacity(ranked.len());
    let mut files: BTreeSet<String> = BTreeSet::new();
    for node in &ranked {
        let Some(row) = rows.get(&node.node_id) else {
            continue;
        };
        if let Some(file) = &row.file_path {
            files.insert(file.clone());
        }
        let code = if include_code {
            read_code(engine.root(), row)
        } else {
            None
        };
        nodes.push(ContextNode {
            symbol: symbol_ref(row),
            score: node.score,
            seed: node.seed,
            code,
        });
    }

    let files: Vec<String> = files.into_iter().collect();
    Ok(ContextBundle {
        task: task.to_string(),
        hops: CONTEXT_HOPS,
        est_reads_replaced: files.len() as u32,
        files,
        nodes,
        suggestions: Vec::new(),
        warnings: Vec::new(),
    })
}

/// `explore` — walk the neighbourhood around `query` and return source
/// grouped by file ([FR-NV-03]).
pub(crate) fn explore(
    engine: &Engine,
    query: &str,
    max_files: Option<usize>,
) -> Result<ExploreResult> {
    let runtime = engine.nav_runtime()?;
    let max_files = max_files.unwrap_or(DEFAULT_MAX_FILES);

    // Anchor resolution is forgiving (explore is the fuzzy tool): exact
    // symbol/name first, else the best FTS match.
    let (anchor, suggestions) = runtime.submit_read(|store| {
        if let Some(row) = resolve_symbol(store, query)? {
            return Ok((Some(row), Vec::new()));
        }
        let mut hits = store.search(query, None, 1)?;
        if let Some(row) = hits.pop() {
            return Ok((Some(row), Vec::new()));
        }
        Ok((None, store.suggest(query, SUGGEST_LIMIT)?))
    })?;
    let Some(anchor) = anchor else {
        return Ok(ExploreResult {
            query: query.to_string(),
            suggestions,
            ..ExploreResult::default()
        });
    };

    // 1-hop undirected neighbourhood on the full lexical view, anchor included.
    let view = engine.hydrate(Granularity::Symbol)?;
    let graph = view.graph();
    let mut ids: Vec<NodeId> = vec![anchor.id];
    if let Some(idx) = view.index_of(anchor.symbol.as_str()) {
        let mut seen: BTreeSet<NodeId> = BTreeSet::new();
        seen.insert(anchor.id);
        for neighbour in graph.neighbors_undirected(idx) {
            if let Some(node_id) = graph[neighbour].node_id {
                if seen.insert(node_id) {
                    ids.push(node_id);
                }
            }
        }
    }
    let rows = fetch_rows(engine, &ids)?;

    // Group by file, deterministic: anchor's file first, then path order.
    let mut groups: BTreeMap<String, Vec<&NodeRow>> = BTreeMap::new();
    for id in &ids {
        let Some(row) = rows.get(id) else { continue };
        let Some(file) = &row.file_path else { continue };
        groups.entry(file.clone()).or_default().push(row);
    }
    let total_files = groups.len() as u32;

    let anchor_file = anchor.file_path.clone();
    let mut ordered: Vec<(String, Vec<&NodeRow>)> = groups.into_iter().collect();
    if let Some(anchor_file) = &anchor_file {
        if let Some(pos) = ordered.iter().position(|(file, _)| file == anchor_file) {
            let group = ordered.remove(pos);
            ordered.insert(0, group);
        }
    }
    ordered.truncate(max_files);

    let files = ordered
        .into_iter()
        .map(|(file, mut rows)| {
            rows.sort_by_key(|row| (row.start_line, row.id.get()));
            // One disk read per group, sliced per symbol.
            let text = file_text(engine.root(), &file);
            FileGroup {
                symbols: rows
                    .into_iter()
                    .map(|row| ExploreSymbol {
                        symbol: symbol_ref(row),
                        end_line: line_u32(row.end_line),
                        code: text
                            .as_deref()
                            .and_then(|t| slice_span(t, row.start_line, row.end_line)),
                    })
                    .collect(),
                file,
            }
        })
        .collect();

    Ok(ExploreResult {
        query: query.to_string(),
        anchor: Some(symbol_ref(&anchor)),
        files,
        total_files,
        suggestions: Vec::new(),
        warnings: Vec::new(),
    })
}

/// `node` — everything about one symbol: metadata, immediate edges, code
/// opt-in ([FR-NV-04]).
pub(crate) fn node(engine: &Engine, symbol: &str, include_code: bool) -> Result<NodeInfo> {
    let runtime = engine.nav_runtime()?;
    let resolved = runtime.submit_read(|store| {
        let Some(row) = resolve_symbol(store, symbol)? else {
            return Ok(Err(store.suggest(symbol, SUGGEST_LIMIT)?));
        };
        let inbound = store.neighbours_in(row.id)?;
        let outbound = store.neighbours_out(row.id)?;
        Ok(Ok((row, inbound, outbound)))
    })?;
    let (row, inbound, outbound) = match resolved {
        Ok(found) => found,
        Err(suggestions) => {
            return Ok(NodeInfo {
                query: symbol.to_string(),
                suggestions,
                ..NodeInfo::default()
            });
        }
    };

    let edges = inbound
        .iter()
        .map(|(kind, other)| (EdgeDirection::In, *kind, other))
        .chain(
            outbound
                .iter()
                .map(|(kind, other)| (EdgeDirection::Out, *kind, other)),
        )
        .map(|(direction, kind, other)| EdgeSummary {
            direction,
            kind,
            other: symbol_ref(other),
        })
        .collect();

    let code = if include_code {
        read_code(engine.root(), &row)
    } else {
        None
    };
    Ok(NodeInfo {
        query: symbol.to_string(),
        node: Some(NodeDetail {
            symbol: symbol_ref(&row),
            end_line: line_u32(row.end_line),
            // No `nodes.signature` column exists yet (extraction-layer gap);
            // the wire field is in place so adapters never reshape (FR-NV-04).
            signature: None,
            // Native annotation columns land with the annotation engine
            // (S-014, FR-AN-04); the field is part of the read-model contract
            // already so adapters need no reshape later.
            annotations: Vec::new(),
            edges,
            code,
        }),
        suggestions: Vec::new(),
        warnings: Vec::new(),
    })
}

/// `callers` — direct callers, limit honoured ([FR-NV-05]).
pub(crate) fn callers(
    engine: &Engine,
    symbol: &str,
    limit: Option<usize>,
) -> Result<CallersResult> {
    let (resolved, total, rows, suggestions) =
        adjacency(engine, symbol, limit, AdjacencyKind::Callers)?;
    Ok(CallersResult {
        query: symbol.to_string(),
        resolved,
        total,
        callers: rows,
        suggestions,
        warnings: Vec::new(),
    })
}

/// `callees` — direct callees, limit honoured ([FR-NV-05]).
pub(crate) fn callees(
    engine: &Engine,
    symbol: &str,
    limit: Option<usize>,
) -> Result<CalleesResult> {
    let (resolved, total, rows, suggestions) =
        adjacency(engine, symbol, limit, AdjacencyKind::Callees)?;
    Ok(CalleesResult {
        query: symbol.to_string(),
        resolved,
        total,
        callees: rows,
        suggestions,
        warnings: Vec::new(),
    })
}

/// `impact` — BOTH labeled direction sets, depth-bounded ([FR-NV-06], DL-03):
/// upstream transitive callers/referencers ("breaks if changed") and
/// downstream transitive callees ("depends on"), on the dependency view
/// (`ExcludeContains` — lexical nesting is not impact).
pub(crate) fn impact(engine: &Engine, symbol: &str, depth: Option<usize>) -> Result<ImpactResult> {
    let runtime = engine.nav_runtime()?;
    let depth = depth.unwrap_or(DEFAULT_IMPACT_DEPTH);

    // The same pooled read resolves the symbol AND collects the doc sections
    // referencing it — the doc-aware dimension of impact (FR-NV-10): which docs
    // a change to the symbol may oblige updating. Empty when none point at it.
    let (resolved, docs, suggestions) =
        runtime.submit_read(|store| match resolve_symbol(store, symbol)? {
            Some(row) => {
                let docs = doc_links(store.neighbours_in(row.id)?, Endpoint::DocSource);
                Ok((Some(row), docs, Vec::new()))
            }
            None => Ok((None, Vec::new(), store.suggest(symbol, SUGGEST_LIMIT)?)),
        })?;
    let Some(row) = resolved else {
        return Ok(ImpactResult {
            query: symbol.to_string(),
            depth: depth as u32,
            upstream_label: UPSTREAM_LABEL.to_string(),
            downstream_label: DOWNSTREAM_LABEL.to_string(),
            docs_label: DOCS_LABEL.to_string(),
            suggestions,
            ..ImpactResult::default()
        });
    };

    let view = engine.hydrate(Granularity::ExcludeContains)?;
    let (upstream, downstream) = match view.index_of(row.symbol.as_str()) {
        Some(start) => (
            impact_entries(engine, &view, start, Direction::Incoming, depth)?,
            impact_entries(engine, &view, start, Direction::Outgoing, depth)?,
        ),
        // The vertex can be missing only if the symbol landed after this
        // view's snapshot; best-effort answers empty rather than failing.
        None => (Vec::new(), Vec::new()),
    };

    Ok(ImpactResult {
        query: symbol.to_string(),
        resolved: Some(symbol_ref(&row)),
        depth: depth as u32,
        upstream_label: UPSTREAM_LABEL.to_string(),
        upstream,
        downstream_label: DOWNSTREAM_LABEL.to_string(),
        downstream,
        docs_label: DOCS_LABEL.to_string(),
        docs,
        suggestions: Vec::new(),
        warnings: Vec::new(),
    })
}

/// `implements` — which code implements a documentation/requirement node
/// ([FR-NV-10], S-037): resolve the doc node, then project its outgoing
/// `doc_reference`/`traces_to` edges that land on a **code** node (a doc→doc
/// link is navigation, not implementation). An unknown node yields an empty
/// result plus "did you mean" suggestions; a known node with no such edge
/// yields an empty `implementors` — never an error ([FR-NV-09]).
///
/// [FR-NV-10]: ../../../docs/specs/requirements/FR-NV-10.md
pub(crate) fn implements(engine: &Engine, doc: &str) -> Result<ImplementorsResult> {
    let runtime = engine.nav_runtime()?;
    let resolved = runtime.submit_read(|store| {
        let Some(row) = resolve_symbol(store, doc)? else {
            return Ok(Err(store.suggest(doc, SUGGEST_LIMIT)?));
        };
        let outbound = store.neighbours_out(row.id)?;
        Ok(Ok((row, outbound)))
    })?;
    let (row, outbound) = match resolved {
        Ok(found) => found,
        Err(suggestions) => {
            return Ok(ImplementorsResult {
                query: doc.to_string(),
                suggestions,
                ..ImplementorsResult::default()
            });
        }
    };
    Ok(ImplementorsResult {
        query: doc.to_string(),
        resolved: Some(symbol_ref(&row)),
        implementors: doc_links(outbound, Endpoint::CodeTarget),
        ..ImplementorsResult::default()
    })
}

/// `referencing_docs` — which documents reference a symbol ([FR-NV-10], S-037):
/// resolve the node, then project its incoming `doc_reference`/`traces_to`
/// edges whose source is a **documentation** node. An unknown symbol yields an
/// empty result plus suggestions; a known symbol no doc points at yields an
/// empty `docs` — never an error ([FR-NV-09]).
///
/// [FR-NV-10]: ../../../docs/specs/requirements/FR-NV-10.md
pub(crate) fn referencing_docs(engine: &Engine, symbol: &str) -> Result<ReferencingDocsResult> {
    let runtime = engine.nav_runtime()?;
    let resolved = runtime.submit_read(|store| {
        let Some(row) = resolve_symbol(store, symbol)? else {
            return Ok(Err(store.suggest(symbol, SUGGEST_LIMIT)?));
        };
        let inbound = store.neighbours_in(row.id)?;
        Ok(Ok((row, inbound)))
    })?;
    let (row, inbound) = match resolved {
        Ok(found) => found,
        Err(suggestions) => {
            return Ok(ReferencingDocsResult {
                query: symbol.to_string(),
                suggestions,
                ..ReferencingDocsResult::default()
            });
        }
    };
    Ok(ReferencingDocsResult {
        query: symbol.to_string(),
        resolved: Some(symbol_ref(&row)),
        docs: doc_links(inbound, Endpoint::DocSource),
        ..ReferencingDocsResult::default()
    })
}

/// `status` — index health from one pooled read plus store-file metadata
/// ([FR-NV-07]). Deliberately skips the auto-index prologue: the health
/// probe must *report* an unindexed graph, not silently build one.
pub(crate) fn status(engine: &Engine) -> Result<StatusInfo> {
    let runtime = engine.nav_runtime_no_prologue()?;
    let counts = runtime.submit_read(|store| store.counts())?;
    let indexed = counts.files > 0 || counts.nodes > 0;

    // Store-file metadata is the best-effort `last_sync_at` source until the
    // persisted timestamp column lands with a later story; the WAL sidecar
    // (when present) carries the most recent committed write.
    let db_path = runtime.db_path();
    let wal_path = db_path.with_extension("db-wal");
    let (mut size, mut mtime) = file_facts(db_path);
    let (wal_size, wal_mtime) = file_facts(&wal_path);
    size += wal_size;
    mtime = match (mtime, wal_mtime) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };

    // The beyond-envelope advisory (NFR-PE-09): `index` recorded the LOC it
    // ingested; if the repo materially exceeds the performance envelope, repeat
    // the same one-line advisory `index` emits — honest expectations, never an
    // error (ADR-14). A repo never indexed (no recorded LOC) is silent.
    let mut warnings = Vec::new();
    let indexed_loc = runtime.submit_read(|store| store.project_metadata(crate::perf::INDEXED_LOC_KEY))?;
    if let Some(advisory) = indexed_loc
        .and_then(|raw| raw.parse::<u64>().ok())
        .and_then(crate::perf::envelope_advisory)
    {
        warnings.push(advisory);
    }

    let refs_unresolved = counts.refs_total.saturating_sub(counts.refs_resolved);
    let resolution_coverage = if counts.refs_total == 0 {
        1.0
    } else {
        counts.refs_resolved as f64 / counts.refs_total as f64
    };

    let freshness = if indexed {
        "best-effort fresh (ADR-11): navigation serves the latest committed snapshot and \
         never reconciles per call (FR-RC-05); evaluation tools reconcile-then-score"
            .to_string()
    } else {
        "unindexed: run `logos index` (or any navigation tool, which auto-indexes first, \
         FR-IX-07) to build the graph"
            .to_string()
    };

    // The persisted monotonic graph revision (FR-SY-09, ADR-32): a single
    // read-only point query on the RO pool — status never advances it. `0` on a
    // never-indexed store.
    let graph_revision = runtime.submit_read(|store| store.graph_revision())?;

    Ok(StatusInfo {
        indexed,
        file_count: counts.files,
        node_count: counts.nodes,
        edge_count: counts.edges,
        db_path: db_path.display().to_string(),
        db_size_bytes: size,
        last_full_index_at: engine.last_full_index_at().map(|secs| secs.to_string()),
        last_sync_at: mtime.map(|secs| secs.to_string()),
        graph_revision,
        refs_total: counts.refs_total,
        refs_resolved: counts.refs_resolved,
        refs_unresolved,
        resolution_coverage,
        freshness,
        warnings,
    })
}

/// `graph_elements` — a read-only, presentation-shaped nodes+edges snapshot of
/// the hydrated graph for the web surface's interactive canvas ([FR-UI-08],
/// [FR-DB-05], [ADR-29]).
///
/// Whole-graph (`seed = None`) or seed-scoped (`seed = Some(symbol)` — the
/// connected neighbourhood reachable from the seed). Both honour `cap`, the
/// visible-element / level-of-detail bound: when the in-scope graph is larger
/// than the cap, the most-connected nodes are kept (degree desc, symbol-key asc
/// tie-break for determinism — [NFR-RA-06]) and the remainder is reported as
/// elided so the frontend can show an honest "N more not shown" notice rather
/// than silently truncating ([NFR-CC-04]).
///
/// A **pure reader** over the cached hydrated view: it only calls
/// [`Engine::hydrate`], which reads through the RO pool and the
/// `(scope, last_sync_at)` cache — there is no compute-and-persist path, so a
/// call mutates no store ([ADR-28], [ADR-29]). It hydrates the presentation-only
/// [`Granularity::Visualization`](crate::Granularity::Visualization) view
/// ([ADR-34], [FR-UI-08]), which keeps the non-code vertices and the cross-layer
/// `DocReference`/`TracesTo`/`ArtifactRef`/`ArtifactBinding` edges, so every
/// element is layer-tagged code/doc/artifact for the canvas. That view is never
/// consumed by a metric/algorithm path, so the aggregate signal stays
/// byte-identical ([FR-DG-06], [ADR-19]); an unknown seed yields an honest empty
/// snapshot with a warning, never a fabricated node ([NFR-RA-05]).
///
/// `layers` / `edge_types` are the **server-side re-budgeting filters** (S-122,
/// [FR-UI-15]): when present they narrow the candidate set **before** the
/// degree-rank+truncate, not after. A `layers` filter drops out-of-scope-layer
/// vertices from the in-scope set, so the visible-element budget (`cap`) is
/// re-spent over the remaining layers — deselecting a layer *backfills* the freed
/// slots with previously-elided nodes of the remaining scope rather than merely
/// shrinking the graph. An `edge_types` filter restricts which edges count toward
/// each node's degree (so the ranking itself re-budgets to the remaining edge
/// structure) and which edges are materialised and tallied. `None` means "no
/// filter" (all layers / all edge types); an empty slice means "filter everything
/// out" — the honest empty graph a user who deselected every layer/edge expects.
/// The elided counts re-base on the filtered snapshot, so the "N more not shown"
/// notice stays correct for the filtered scope ([NFR-CC-04]).
///
/// `granularity` is the **semantic cluster-zoom tier** (S-124, [FR-UI-15],
/// [ADR-36]) — the Google-Maps-style module → file → symbol altitude ladder the
/// canvas drives from its zoom. It selects which **existing** hydration view
/// ([ADR-34], [FR-DB-05]) to read — no clustering algorithm is invented:
/// [`GraphGranularity::Module`] → the module-rollup view
/// ([`Granularity::Module`]), [`GraphGranularity::File`] → the file-rollup view
/// ([`Granularity::File`]), and [`GraphGranularity::Symbol`] (the default, so
/// `None` behaves exactly as before S-124) → the visualization view. At the two
/// rollup tiers a vertex is a **cluster** (a module/file aggregate) carrying no
/// `kind`, and an edge is an aggregated dependency carrying no `edge_type`; both
/// serialise as `null`. The rollup views are the **code subgraph** by
/// construction — documentation and config/artifact files/modules are excluded at
/// hydration ([FR-DG-06], [FR-CG-05]) — so a cluster tier never surfaces a
/// doc/artifact node. Every tier reads through the same read-only cached-view path
/// and feeds no metric/algorithm consumer, so the aggregate signal stays
/// byte-identical at every tier ([ADR-34], [FR-QM-08]).
///
/// `intent_overlay` is the **bounded documentation-intent overlay** (S-128,
/// [FR-UI-16], [ADR-37]). With it `false` (the default) the snapshot is
/// byte-identical to the pre-S-128 output. With it `true`, **after** the
/// structural degree-rank+truncate has chosen the in-scope code nodes, the
/// governing-doc nodes adjacent — via the existing `DocReference`/`TracesTo`
/// intent edges ([FR-DG-04]) — to those kept code nodes are admitted up to a
/// **separate reserved budget** ([`INTENT_OVERLAY_BUDGET`]) computed **outside**
/// the structural ranking. The ranking, the main visible budget, and the kept code
/// set are all left untouched (the overlay only *appends*), so the overlay can
/// never starve the code anchors — the [CR-014] doc-flooding guard. It reads only
/// the already-hydrated visualization view (no new hydration, edge kind, binding,
/// query verb, or metric path), so every metric/cycle/DSM/dead-code scope stays
/// byte-identical ([ADR-34], [FR-DG-06], [NFR-RA-05]). The honest elided counts
/// account for the overlay scope ([NFR-CC-04]). At the module/file rollup tiers the
/// view is the code subgraph (docs excluded at hydration, [FR-DG-06]), so there are
/// no doc candidates and the overlay is a natural no-op.
///
/// [CR-014]: ../../../docs/requests/CR-014-context-seed-doc-flooding.md
pub(crate) fn graph_elements(
    engine: &Engine,
    seed: Option<&str>,
    cap: Option<usize>,
    layers: Option<&[GraphLayer]>,
    edge_types: Option<&[EdgeKind]>,
    granularity: Option<GraphGranularity>,
    intent_overlay: bool,
) -> Result<GraphElements> {
    let cap = cap.unwrap_or(DEFAULT_GRAPH_ELEMENT_CAP);

    // The cluster-zoom tier (S-124, ADR-36) selects which EXISTING hydration view
    // to read — `None` is the symbol tier (the visualization view), so an
    // unparameterized call is byte-identical to the pre-S-124 behaviour. The
    // module/file rollup views are the code subgraph (docs/artifacts excluded at
    // hydration, FR-DG-06/FR-CG-05); the visualization view alone keeps the
    // non-code layers. No tier is consumed by a metric/algorithm path, so reading
    // any of them for presentation is metric-neutral (ADR-34, FR-QM-08).
    let tier = granularity.unwrap_or_default();
    let view_granularity = match tier {
        GraphGranularity::Module => Granularity::Module,
        GraphGranularity::File => Granularity::File,
        GraphGranularity::Symbol => Granularity::Visualization,
    };
    let view = engine.hydrate(view_granularity)?;
    let graph = view.graph();

    // The re-budgeting filter predicates (S-122, FR-UI-15). `None` ⇒ everything
    // allowed; an (even empty) slice ⇒ only the listed layers/edge types.
    //
    // A **rollup cluster** vertex (module/file tier, S-124) carries no `kind`, so
    // it has no per-layer identity; it is the code subgraph by construction, so it
    // is filtered as the [`GraphLayer::Code`] layer — deselecting "doc"/"artifact"
    // leaves the code-only clusters, deselecting "code" (or all layers) honestly
    // empties the tier. A rollup-aggregate **edge** carries no `edge_type` (it
    // spans mixed kinds), so the edge-type filter — which names concrete kinds —
    // cannot apply to it and is a no-op at the rollup tiers (it always passes).
    // At the symbol tier every vertex/edge carries a kind, so both predicates
    // behave exactly as they did for S-122.
    let layer_allowed = |kind: Option<NodeKind>| -> bool {
        match layers {
            None => true,
            Some(allowed) => {
                let layer = kind.map(GraphLayer::from).unwrap_or(GraphLayer::Code);
                allowed.contains(&layer)
            }
        }
    };
    let edge_allowed = |kind: Option<EdgeKind>| -> bool {
        match (edge_types, kind) {
            // No filter, or an aggregate edge with no filterable kind ⇒ allowed.
            (None, _) | (Some(_), None) => true,
            (Some(allowed), Some(k)) => allowed.contains(&k),
        }
    };

    // The in-scope candidate set: the whole graph, or the seed's connected
    // neighbourhood. An unknown seed is honest-empty, not an error ([FR-NV-09]
    // contract spirit): no vertex to anchor on, so nothing is fabricated. At a
    // rollup tier the seed is a cluster key (a file path / module key); a symbol
    // string that names no cluster simply does not resolve, yielding the same
    // honest-empty snapshot.
    let (mut in_scope, seed_echo, seed_dist): (Vec<NodeIndex>, Option<String>, Option<HashMap<NodeIndex, u32>>) = match seed {
        None => (graph.node_indices().collect::<Vec<_>>(), None, None),
        Some(s) => match view.index_of(s) {
            Some(start) => {
                let reach = reachable_from(graph, start);
                let dist: HashMap<NodeIndex, u32> = reach.iter().copied().collect();
                let nodes: Vec<NodeIndex> = reach.into_iter().map(|(idx, _)| idx).collect();
                (nodes, Some(s.to_string()), Some(dist))
            }
            None => {
                return Ok(GraphElements {
                    seed: Some(s.to_string()),
                    granularity: tier,
                    cap: cap as u32,
                    warnings: vec![format!("unknown seed symbol: {s}")],
                    ..GraphElements::default()
                });
            }
        },
    };

    // Re-budgeting (S-122, FR-UI-15): apply the layer filter to the in-scope set
    // *before* the degree-rank+truncate, so the cap is re-spent over the remaining
    // layers and previously-elided nodes of the remaining scope backfill the freed
    // budget — the graph stays full, not merely smaller. A no-op when `layers` is
    // `None` (the whole candidate set survives, byte-identical to the old path).
    in_scope.retain(|&idx| layer_allowed(graph[idx].kind));

    let mut total_nodes = in_scope.len();
    let in_scope_set: HashSet<NodeIndex> = in_scope.iter().copied().collect();

    // In-scope edges (both endpoints in scope, of an allowed type) — the honest
    // denominator for the elided-edge count, computed before the cap narrows the
    // rendered set and re-based on the active edge-type filter.
    let mut total_edges = graph
        .edge_indices()
        .filter(|&e| {
            // The denominator must match what `materialise` below keeps: an edge of
            // an allowed type whose endpoints are both in scope. `edge_allowed` now
            // admits a `None`-kind edge (the rollup-aggregate edge of the module/
            // file tiers, S-124) — which IS materialised at those tiers — so the two
            // counts stay consistent. (At the symbol/visualization tier every edge
            // carries a kind, so this is unchanged from S-122.)
            edge_allowed(graph[e].kind)
                && graph
                    .edge_endpoints(e)
                    .is_some_and(|(a, b)| in_scope_set.contains(&a) && in_scope_set.contains(&b))
        })
        .count();

    // Degree counts only edges of an allowed type, so an `edge_types` filter
    // re-budgets the ranking itself (a node whose connectivity came from a
    // deselected type drops in rank, admitting previously-elided nodes). With no
    // filter every incident edge counts — identical to the prior
    // `edges_directed(...).count()` semantics (petgraph yields one walk step per
    // edge, so this never double-counts a multi-edge neighbour).
    let degree_of = |idx: NodeIndex| -> usize {
        graph
            .edges_directed(idx, Direction::Incoming)
            .filter(|e| edge_allowed(e.weight().kind))
            .count()
            + graph
                .edges_directed(idx, Direction::Outgoing)
                .filter(|e| edge_allowed(e.weight().kind))
                .count()
    };

    // Level-of-detail selection. The whole-graph (unseeded) view keeps the
    // most-connected nodes first — the structurally important hubs — exactly as
    // before (S-122; byte-identical, preserving the metric-neutrality fitness,
    // ADR-34/FR-QM-08). A **seed-scoped** view instead ranks by BFS proximity to
    // the seed: `reachable_from` of a hub is often most of the graph, so the old
    // global-degree rank could truncate the (low-degree) seed itself out of the
    // cap — leaving focus indistinguishable from the whole graph and nothing for
    // the canvas to ring/center. Proximity ranking keeps the seed (distance 0,
    // always retained) and its nearest neighbourhood; degree breaks ties within a
    // ring, key is the final deterministic tie-break ([NFR-RA-06]).
    let mut ranked = in_scope;
    match &seed_dist {
        None => ranked.sort_by(|&a, &b| {
            degree_of(b)
                .cmp(&degree_of(a))
                .then_with(|| graph[a].key.cmp(&graph[b].key))
        }),
        Some(dist) => ranked.sort_by(|&a, &b| {
            dist.get(&a)
                .copied()
                .unwrap_or(u32::MAX)
                .cmp(&dist.get(&b).copied().unwrap_or(u32::MAX))
                .then_with(|| degree_of(b).cmp(&degree_of(a)))
                .then_with(|| graph[a].key.cmp(&graph[b].key))
        }),
    }
    ranked.truncate(cap);
    let kept: HashSet<NodeIndex> = ranked.iter().copied().collect();

    // Materialise the kept nodes, deterministically ordered by id ([NFR-RA-06]).
    // A symbol-tier vertex carries its `kind` and renders in its kind's layer; a
    // rollup-cluster vertex (module/file tier, S-124) carries no kind and renders
    // in the [`GraphLayer::Code`] layer (the rollups are the code subgraph by
    // construction — docs/artifacts excluded at hydration, FR-DG-06).
    let mut nodes: Vec<GraphElementNode> = ranked
        .iter()
        .map(|&idx| {
            let v = &graph[idx];
            GraphElementNode {
                id: v.key.clone(),
                label: v.label.clone(),
                kind: v.kind,
                layer: v.kind.map(layer_of).unwrap_or(GraphLayer::Code),
            }
        })
        .collect();
    nodes.sort_by(|a, b| a.id.cmp(&b.id));

    // Edges among the kept nodes only, of an allowed type, deterministically
    // ordered ([NFR-RA-06]). A symbol-tier edge carries its `edge_type`; a
    // rollup-aggregate edge (module/file tier) carries `None` and always passes the
    // edge-type filter (it spans mixed kinds — the filter cannot name it, S-124).
    let mut edges: Vec<GraphElementEdge> = graph
        .edge_indices()
        .filter_map(|e| {
            let (a, b) = graph.edge_endpoints(e)?;
            if !kept.contains(&a) || !kept.contains(&b) {
                return None;
            }
            let kind = graph[e].kind;
            if !edge_allowed(kind) {
                return None;
            }
            Some(GraphElementEdge {
                source: graph[a].key.clone(),
                target: graph[b].key.clone(),
                edge_type: kind,
            })
        })
        .collect();
    // A rollup-aggregate edge has no kind; `edge_render_order` orders it before any
    // typed edge (`None` < `Some`) so the ordering stays total and deterministic.
    edges.sort_by(edge_render_order);

    // ── Bounded documentation-intent overlay (S-128, FR-UI-16, ADR-37) ──────────
    // After the structural degree-rank+truncate has chosen the in-scope code nodes
    // (`kept`), admit the governing-doc nodes adjacent — via the existing
    // `DocReference`/`TracesTo` intent edges (FR-DG-04) — to those kept code nodes,
    // up to a SEPARATE reserved budget (INTENT_OVERLAY_BUDGET) computed OUTSIDE the
    // structural ranking. The ranking, the main visible budget, and `kept` are all
    // left untouched (the overlay only APPENDS), so the code-anchor set can never be
    // starved by it — the CR-014 doc-flooding guard (fitness test). Off by default:
    // when `intent_overlay` is false the block is skipped and the snapshot is
    // byte-identical to the pre-S-128 output. Reads ONLY the already-hydrated
    // visualization view (`graph`) — no new hydration, edge kind, binding, query
    // verb, or metric path — so every metric/cycle/DSM/dead-code scope stays
    // byte-identical (ADR-34, FR-DG-06, NFR-RA-05). At the module/file rollup tiers
    // the view is the code subgraph (docs excluded at hydration, FR-DG-06), so there
    // are no doc candidates and the overlay is a natural no-op.
    if intent_overlay {
        // Governing-doc candidates: a doc-layer vertex NOT already rendered, joined
        // by a `DocReference`/`TracesTo` edge to a kept CODE node. Tracked with its
        // kept-code anchor count so the most-referenced docs rank first; the key is
        // the deterministic tie-break (NFR-RA-06).
        let mut anchored: HashMap<NodeIndex, usize> = HashMap::new();
        for e in graph.edge_indices() {
            // Only the two intent edge kinds carry documentation adjacency (FR-DG-04).
            if !graph[e].kind.is_some_and(|k| k.is_documentation()) {
                continue;
            }
            let Some((a, b)) = graph.edge_endpoints(e) else {
                continue;
            };
            // Orient to (doc-candidate, kept-code-anchor). Intent edges run doc→code,
            // but checking both orientations keeps this robust and naturally ignores
            // doc→doc references (a doc is never a kept *code* anchor).
            for (cand, anchor) in [(a, b), (b, a)] {
                let cand_is_unrendered_doc =
                    graph[cand].kind.is_some_and(|k| k.is_doc()) && !kept.contains(&cand);
                let anchor_is_kept_code = kept.contains(&anchor)
                    && graph[anchor].kind.is_some_and(|k| layer_of(k) == GraphLayer::Code);
                if cand_is_unrendered_doc && anchor_is_kept_code {
                    *anchored.entry(cand).or_insert(0) += 1;
                }
            }
        }

        // Rank candidates: most kept-code anchors first, key asc as the deterministic
        // tie-break (NFR-RA-06). The reserved budget bounds how many are admitted.
        let mut candidates: Vec<NodeIndex> = anchored.keys().copied().collect();
        candidates.sort_by(|&a, &b| {
            anchored[&b]
                .cmp(&anchored[&a])
                .then_with(|| graph[a].key.cmp(&graph[b].key))
        });
        let admit = candidates.len().min(INTENT_OVERLAY_BUDGET);
        let admitted: HashSet<NodeIndex> = candidates[..admit].iter().copied().collect();

        // Honest counts (NFR-CC-04): a candidate NOT already in the structural
        // in-scope set is a NEW scope member, so it widens the node denominator; a
        // candidate already in scope was counted there (an evicted doc the main pass
        // already tallied as elided). With the denominator widened the unchanged
        // `elided = total − rendered` formula below recovers the correct elided tally
        // for every case (admitted/elided × in-scope/out-of-scope) with no double
        // counting — including the surplus governing docs beyond the budget.
        total_nodes += candidates.iter().filter(|&&d| !in_scope_set.contains(&d)).count();

        // Materialise the admitted doc nodes (each carries a kind in the
        // visualization view, so none renders as a cluster). Re-sort the merged set
        // by id so the snapshot stays deterministically ordered (NFR-RA-06).
        for &d in &candidates[..admit] {
            let v = &graph[d];
            nodes.push(GraphElementNode {
                id: v.key.clone(),
                label: v.label.clone(),
                kind: v.kind,
                layer: v.kind.map(layer_of).unwrap_or(GraphLayer::Code),
            });
        }
        nodes.sort_by(|a, b| a.id.cmp(&b.id));

        // The intent edges the overlay introduces: `DocReference`/`TracesTo` edges
        // with both endpoints now rendered and at least one an admitted doc (edges
        // among `kept` were already handled by the main pass, so this never
        // duplicates one). Surfacing these intent edges IS the overlay's purpose
        // (FR-UI-16), so they render regardless of the `edge_types` filter. An edge
        // the structural pass already tallied (allowed type + both endpoints in
        // scope) merely moves from elided to rendered; otherwise it widens the edge
        // denominator — the same honest accounting as the nodes. Pushed straight onto
        // `edges` (no intermediate vec) and re-sorted with the shared comparator.
        for e in graph.edge_indices() {
            let kind = graph[e].kind;
            if !kind.is_some_and(|k| k.is_documentation()) {
                continue;
            }
            let Some((a, b)) = graph.edge_endpoints(e) else {
                continue;
            };
            let both_rendered = (kept.contains(&a) || admitted.contains(&a))
                && (kept.contains(&b) || admitted.contains(&b));
            let touches_admitted = admitted.contains(&a) || admitted.contains(&b);
            if !both_rendered || !touches_admitted {
                continue;
            }
            let already_counted =
                edge_allowed(kind) && in_scope_set.contains(&a) && in_scope_set.contains(&b);
            if !already_counted {
                total_edges += 1;
            }
            edges.push(GraphElementEdge {
                source: graph[a].key.clone(),
                target: graph[b].key.clone(),
                edge_type: kind,
            });
        }
        edges.sort_by(edge_render_order);
    }

    // `kept ⊆ in_scope`, so each delta is non-negative today; `saturating_sub`
    // keeps the elided counts panic-free if a future refactor decouples the sets.
    let elided_nodes = total_nodes.saturating_sub(nodes.len()) as u32;
    let elided_edges = total_edges.saturating_sub(edges.len()) as u32;

    Ok(GraphElements {
        seed: seed_echo,
        granularity: tier,
        cap: cap as u32,
        total_nodes: total_nodes as u32,
        total_edges: total_edges as u32,
        elided_nodes,
        elided_edges,
        nodes,
        edges,
        warnings: Vec::new(),
    })
}

/// The connected neighbourhood of `start` over **undirected** adjacency — the
/// canvas's seed-scoped view (a symbol and everything it links to or is linked
/// from, transitively). Each node is paired with its **BFS distance** from
/// `start` (`start` itself is distance 0, first); deterministic per the view's
/// stable [`NodeIndex`] assignment ([NFR-RA-06]). The distance drives the
/// seed-scoped level-of-detail ranking in [`graph_elements`] so the cap keeps
/// the seed and its nearest neighbourhood rather than the globally-highest-degree
/// hubs of the (often near-whole-graph) reachable set.
fn reachable_from(graph: &DiGraph<Vertex, EdgeData>, start: NodeIndex) -> Vec<(NodeIndex, u32)> {
    let mut seen: HashSet<NodeIndex> = HashSet::new();
    let mut queue: VecDeque<(NodeIndex, u32)> = VecDeque::new();
    let mut out: Vec<(NodeIndex, u32)> = Vec::new();
    seen.insert(start);
    queue.push_back((start, 0));
    while let Some((idx, dist)) = queue.pop_front() {
        out.push((idx, dist));
        for neighbour in graph.neighbors_undirected(idx) {
            if seen.insert(neighbour) {
                queue.push_back((neighbour, dist + 1));
            }
        }
    }
    out
}

/// The presentation layer a node renders in, derived from its [`NodeKind`] —
/// the canvas's code/doc/artifact colouring ([FR-UI-08], frontend-design §4.4).
/// Pure classification, never fabricated ([NFR-RA-05]).
fn layer_of(kind: NodeKind) -> GraphLayer {
    GraphLayer::from(kind)
}

/// The deterministic total order for rendered graph edges ([NFR-RA-06]): by source
/// key, then target key, then edge-kind wire name — a kind-less rollup-aggregate
/// edge (`None`) sorts before any typed edge (`Some`) so the order stays total.
/// Shared by the structural pass and the documentation-intent overlay (S-128) so the
/// two sort sites can never drift.
fn edge_render_order(a: &GraphElementEdge, b: &GraphElementEdge) -> std::cmp::Ordering {
    a.source
        .cmp(&b.source)
        .then_with(|| a.target.cmp(&b.target))
        .then_with(|| a.edge_type.map(|k| k.as_str()).cmp(&b.edge_type.map(|k| k.as_str())))
}

/// `affected` — the **whole** reverse-transitive closure of files depending
/// on a changed set ([FR-CL-04], DL-08), on the file-rollup dependency view.
///
/// Unlike [`impact`] this is file-granular and **not** depth-bounded: the
/// consumer is CI deciding "what to retest", where a missed transitive
/// dependent is a missed test run. `tests_only` narrows the closure to
/// test-marked files (path-convention heuristic until native test
/// annotations land with the test-gap story).
///
/// [FR-CL-04]: ../../../docs/specs/requirements/FR-CL-04.md
pub(crate) fn affected(
    engine: &Engine,
    files: &[String],
    tests_only: bool,
) -> Result<AffectedResult> {
    // Same prologue contract as the other navigation queries (FR-IX-07).
    engine.nav_runtime()?;
    let view = engine.hydrate(Granularity::File)?;
    let graph = view.graph();

    // Resolve each changed path to its file vertex; "./" prefixes normalise
    // to the stored project-relative form. Unknown paths are reported, never
    // an error (the ADR-14 graceful-degradation posture).
    let mut changed: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut seeds: Vec<NodeIndex> = Vec::new();
    for raw in files {
        let path = raw.strip_prefix("./").unwrap_or(raw);
        match view.index_of(path) {
            Some(idx) => {
                if !changed.iter().any(|c| c == path) {
                    changed.push(path.to_string());
                    seeds.push(idx);
                }
            }
            None => unknown.push(raw.clone()),
        }
    }

    // Multi-source reverse BFS, unbounded: distance = minimal hops from ANY
    // changed seed. Reverse direction because an edge F→C means "F depends
    // on C", so dependents of the changed set sit on incoming edges.
    let mut distance: HashMap<NodeIndex, u32> = seeds.iter().map(|&s| (s, 0)).collect();
    let mut queue: VecDeque<NodeIndex> = seeds.into_iter().collect();
    let mut reached: Vec<(NodeIndex, u32)> = Vec::new();
    while let Some(current) = queue.pop_front() {
        let d = distance[&current];
        for neighbour in graph.neighbors_directed(current, Direction::Incoming) {
            if let std::collections::hash_map::Entry::Vacant(slot) = distance.entry(neighbour) {
                slot.insert(d + 1);
                reached.push((neighbour, d + 1));
                queue.push_back(neighbour);
            }
        }
    }

    let mut affected: Vec<AffectedFile> = reached
        .into_iter()
        .filter_map(|(idx, dist)| {
            let file = graph[idx].key.clone();
            // The `<unbound>` sentinel aggregates file-less nodes — not a file.
            if file.starts_with('<') {
                return None;
            }
            let is_test = is_test_path(&file);
            (!tests_only || is_test).then_some(AffectedFile {
                file,
                distance: dist,
                is_test,
            })
        })
        .collect();
    // Nearest-first then path order — deterministic across runs (NFR-RA-06).
    affected.sort_by(|a, b| {
        a.distance
            .cmp(&b.distance)
            .then_with(|| a.file.cmp(&b.file))
    });

    Ok(AffectedResult {
        changed,
        tests_only,
        affected,
        unknown,
        warnings: Vec::new(),
    })
}

/// Whether a project-relative path is test-marked by naming convention
/// ([FR-CL-04] `--tests-only`): a `tests`/`test`/`__tests__`/`spec` path
/// segment, or a filename matching the per-language test idioms (`*_test.*`,
/// `test_*.py`, `*.test.*`/`*.spec.*`, `*Test(s).java`, Ruby RSpec `*_spec.rb`).
/// Deterministic and language-blind; the native test annotation (test-gap
/// analysis story) will supersede it.
pub(crate) fn is_test_path(path: &str) -> bool {
    let p = Path::new(path);
    if p.components().any(|c| {
        matches!(c, Component::Normal(seg)
            if seg == "tests" || seg == "test" || seg == "__tests__" || seg == "spec")
    }) {
        return true;
    }
    let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let stem = name.split('.').next().unwrap_or(name);
    stem.ends_with("_test")
        || stem.ends_with("_spec")
        || stem.ends_with("Test")
        || stem.ends_with("Tests")
        || (stem.starts_with("test_") && name.ends_with(".py"))
        || name
            .split('.')
            .rev()
            .nth(1)
            .is_some_and(|tag| tag == "test" || tag == "spec")
}

// ── Shared plumbing ─────────────────────────────────────────────────────────

/// Which `calls`-edge side an adjacency query projects.
enum AdjacencyKind {
    Callers,
    Callees,
}

/// `true` for the two documentation traceability edge kinds ([FR-NV-10]): a
/// generic resolved doc reference or a typed `Requirement`/`Adr`/`Story` trace.
fn is_doc_edge(kind: EdgeKind) -> bool {
    matches!(kind, EdgeKind::DocReference | EdgeKind::TracesTo)
}

/// Which endpoint kind a traceability projection keeps — the half of a doc edge
/// that is *not* the queried node.
enum Endpoint {
    /// `implements`: keep the code targets of a doc node's outbound edges.
    CodeTarget,
    /// `referencing_docs` / doc-aware impact: keep the documentation sources of
    /// a node's inbound edges.
    DocSource,
}

impl Endpoint {
    /// Whether a neighbour of `kind` belongs in this projection.
    fn admits(&self, kind: NodeKind) -> bool {
        match self {
            Endpoint::CodeTarget => !kind.is_doc(),
            Endpoint::DocSource => kind.is_doc(),
        }
    }
}

/// Project a node's neighbour set to the [`TraceLink`]s reachable over a
/// documentation edge whose other endpoint matches `keep` ([FR-NV-10], S-037).
/// Deterministic: symbol ascending, edge kind as the tie-break (NFR-RA-06).
fn doc_links(neighbours: Vec<(EdgeKind, NodeRow)>, keep: Endpoint) -> Vec<TraceLink> {
    let mut links: Vec<TraceLink> = neighbours
        .into_iter()
        .filter(|(kind, other)| is_doc_edge(*kind) && keep.admits(other.kind))
        .map(|(kind, other)| TraceLink {
            symbol: symbol_ref(&other),
            via: kind,
        })
        .collect();
    links.sort_by(|a, b| {
        a.symbol
            .symbol
            .cmp(&b.symbol.symbol)
            .then_with(|| a.via.as_i32().cmp(&b.via.as_i32()))
    });
    links
}

/// The shared payload of an adjacency query:
/// `(resolved node, pre-limit total, page, suggestions)`.
type AdjacencyPage = (Option<SymbolRef>, u32, Vec<SymbolRef>, Vec<String>);

/// Shared body of [`callers`]/[`callees`]: resolve, fetch the full direct
/// set, report the pre-limit total, truncate to `limit` ([FR-NV-05]).
fn adjacency(
    engine: &Engine,
    symbol: &str,
    limit: Option<usize>,
    kind: AdjacencyKind,
) -> Result<AdjacencyPage> {
    let runtime = engine.nav_runtime()?;
    let limit = limit.unwrap_or(DEFAULT_ADJACENCY_LIMIT);
    runtime.submit_read(|store| {
        let Some(row) = resolve_symbol(store, symbol)? else {
            return Ok((None, 0, Vec::new(), store.suggest(symbol, SUGGEST_LIMIT)?));
        };
        let mut adjacent = match kind {
            AdjacencyKind::Callers => store.callers(row.id)?,
            AdjacencyKind::Callees => store.callees(row.id)?,
        };
        let total = adjacent.len() as u32;
        adjacent.truncate(limit);
        Ok((
            Some(symbol_ref(&row)),
            total,
            adjacent.iter().map(symbol_ref).collect(),
            Vec::new(),
        ))
    })
}

/// Build the FTS5 seed query for a prose task string ([FR-NV-02]): each
/// identifier-ish token becomes a quoted prefix term, OR-combined —
/// `"hydrate graph view"` → `"hydrate"* OR "graph"* OR "view"*` — so any
/// token can seed the bundle and user text stays inert FTS syntax (the
/// quoting neutralises operators; the value is a bound parameter besides,
/// NFR-SE-02). `None` when the task holds no usable token. Capped at 16
/// tokens to bound the match cost on a long task description.
/// Wrap raw user `query` as a single FTS5 phrase so punctuation (e.g. the `-` in
/// `web-surface`) can never be misread as an FTS operator; `None` for an
/// empty/whitespace query (a well-defined no-op, not an opaque FTS `syntax
/// error`). Embedded `"` is doubled per FTS5 string quoting; the value is a bound
/// parameter besides ([NFR-SE-02]). Mirrors [`crate::wiki::fts_phrase_query`] and
/// the [graph_store] `suggest` quoting precedent. Unlike [`seed_query`] (prose →
/// OR-of-prefix-terms), this keeps the query's adjacency: a symbol name is matched
/// as one phrase, not scattered tokens.
fn phrase_query(query: &str) -> Option<String> {
    if query.trim().is_empty() {
        return None;
    }
    Some(format!("\"{}\"", query.replace('"', "\"\"")))
}

fn seed_query(task: &str) -> Option<String> {
    let terms: Vec<String> = task
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|token| !token.is_empty())
        .take(16)
        .map(|token| format!("\"{token}\"*"))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

/// Resolve a navigation `symbol` argument to a node: exact canonical-symbol
/// match first, then exact name match (lowest id wins — deterministic).
/// `None` means unknown → the caller assembles the graceful empty result.
fn resolve_symbol(store: &dyn GraphStore, text: &str) -> Result<Option<NodeRow>> {
    if let Some(row) = store.node_by_symbol(text)? {
        return Ok(Some(row));
    }
    Ok(store.nodes_by_name(text)?.into_iter().next())
}

/// Depth-bounded BFS from `start` in `direction`, materialised to
/// [`ImpactEntry`] rows sorted nearest-first then by symbol (deterministic,
/// NFR-RA-06).
fn impact_entries(
    engine: &Engine,
    view: &GraphView,
    start: NodeIndex,
    direction: Direction,
    max_depth: usize,
) -> Result<Vec<ImpactEntry>> {
    let graph = view.graph();
    let mut distance: HashMap<NodeIndex, u32> = HashMap::new();
    distance.insert(start, 0);
    let mut queue: VecDeque<NodeIndex> = VecDeque::new();
    queue.push_back(start);
    let mut reached: Vec<(NodeIndex, u32)> = Vec::new();
    while let Some(current) = queue.pop_front() {
        let d = distance[&current];
        if d as usize >= max_depth {
            continue;
        }
        for neighbour in graph.neighbors_directed(current, direction) {
            if let std::collections::hash_map::Entry::Vacant(slot) = distance.entry(neighbour) {
                slot.insert(d + 1);
                reached.push((neighbour, d + 1));
                queue.push_back(neighbour);
            }
        }
    }

    let ids: Vec<NodeId> = reached
        .iter()
        .filter_map(|(idx, _)| graph[*idx].node_id)
        .collect();
    let rows = fetch_rows(engine, &ids)?;

    let mut entries: Vec<ImpactEntry> = reached
        .into_iter()
        .filter_map(|(idx, dist)| {
            let row = rows.get(&graph[idx].node_id?)?;
            Some(ImpactEntry {
                symbol: symbol_ref(row),
                distance: dist,
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        a.distance
            .cmp(&b.distance)
            .then_with(|| a.symbol.symbol.cmp(&b.symbol.symbol))
    });
    Ok(entries)
}

/// One ranked entry in a `context` bundle, unifying code vertices (scored from
/// the hydrated view) and documentation neighbours (scored from the store's
/// doc↔code edges) so both flow through one deterministic sort, one `max_nodes`
/// cut, and one row materialisation ([FR-NV-02], [NFR-RA-06]).
struct RankedNode {
    node_id: NodeId,
    /// The canonical symbol key — the deterministic tie-break on equal scores.
    key: String,
    score: f64,
    seed: bool,
}

/// Merge ranked candidates that share a store id into one entry, summing their
/// scores and OR-ing the seed flag ([FR-NV-02]). The only id collision the
/// `context` pipeline produces is a documentation node that anchored as a seed
/// (FTS match-score) **and** re-surfaced as a doc neighbour of the seeded code
/// (degree centrality); summing gives it the same match-score + centrality a code
/// seed earns, and the OR keeps it flagged a seed. Every other source contributes
/// disjoint kinds (code vertices, doc neighbours, artifact neighbours), so those
/// ids pass through untouched. A [`BTreeMap`] keyed by id fixes the order
/// regardless of source/iteration order ([NFR-RA-06]).
fn merge_ranked(nodes: Vec<RankedNode>) -> Vec<RankedNode> {
    let mut by_id: BTreeMap<NodeId, RankedNode> = BTreeMap::new();
    for node in nodes {
        match by_id.entry(node.node_id) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(node);
            }
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                let merged = slot.get_mut();
                merged.score += node.score;
                merged.seed |= node.seed;
            }
        }
    }
    by_id.into_values().collect()
}

/// The implementing code symbols of a set of documentation seed nodes
/// ([FR-NV-02]): each doc seed's outbound `DocReference`/`TracesTo` edges that
/// land on a **code** node — the same doc→code projection [`implements`] keeps
/// ([FR-NV-10]). Returned as canonical symbol keys, deduplicated and ordered by a
/// [`BTreeSet`] so the caller folds them into the hydrated-view frontier
/// deterministically ([NFR-RA-06]). Empty input short-circuits the pooled read.
fn doc_seed_code_targets(engine: &Engine, doc_ids: &[NodeId]) -> Result<Vec<String>> {
    if doc_ids.is_empty() {
        return Ok(Vec::new());
    }
    let doc_ids = doc_ids.to_vec();
    let runtime = engine.nav_runtime_no_prologue()?;
    let symbols: BTreeSet<String> = runtime.submit_read(move |store| {
        let mut symbols: BTreeSet<String> = BTreeSet::new();
        for id in &doc_ids {
            for (kind, other) in store.neighbours_out(*id)? {
                // The code half of a doc↔code edge — the projection `implements`
                // keeps (FR-NV-10): a code node reached over a doc reference/trace.
                if is_doc_edge(kind) && !other.kind.is_doc() {
                    symbols.insert(other.symbol.as_str().to_string());
                }
            }
        }
        Ok(symbols)
    })?;
    Ok(symbols.into_iter().collect())
}

/// The documentation neighbours of a `context` candidate code set ([FR-NV-02]).
///
/// S-036's hydrated views are the **code subgraph**: documentation nodes and
/// doc↔code edges are dropped at build time so metrics, cycle detection, and DSM
/// stay byte-identical w.r.t. docs ([FR-DG-06], [ADR-19], and the doc comment on
/// [`build_view`](crate::hydrate::view::build_view)). The doc graph lives only in
/// the canonical store, so this mirrors [`node`]/[`referencing_docs`] and reads
/// the store's inbound doc edges for each candidate code symbol, returning every
/// documenting node as a non-seed [`RankedNode`]. Its score is pure centrality
/// (match-score `0` — an expansion neighbour, never a seed), normalised by the
/// same `max_degree` as the code candidates so a doc that documents several
/// candidates outranks one documenting a single isolated symbol. Deterministic:
/// a [`BTreeMap`] keyed by doc id fixes the order regardless of which code id's
/// query surfaced the doc first ([NFR-RA-06]).
fn doc_neighbours(
    engine: &Engine,
    code_ids: &[NodeId],
    max_degree: usize,
) -> Result<Vec<RankedNode>> {
    if code_ids.is_empty() {
        return Ok(Vec::new());
    }
    let code_ids = code_ids.to_vec();
    let runtime = engine.nav_runtime_no_prologue()?;
    // doc id -> (row, how many distinct candidate code symbols it documents)
    let docs: BTreeMap<NodeId, (NodeRow, usize)> = runtime.submit_read(move |store| {
        let mut docs: BTreeMap<NodeId, (NodeRow, usize)> = BTreeMap::new();
        // A (doc, code) pair counts once toward the doc's degree even when both a
        // `DocReference` and a `TracesTo` edge connect it — degree is the count of
        // distinct candidate symbols documented, not of edges.
        let mut counted: BTreeSet<(NodeId, NodeId)> = BTreeSet::new();
        for id in &code_ids {
            for (kind, other) in store.neighbours_in(*id)? {
                // The doc-source half of a doc↔code edge — the same projection
                // `referencing_docs` keeps (FR-NV-10): a documentation node
                // reached over a `DocReference`/`TracesTo` edge.
                if is_doc_edge(kind) && other.kind.is_doc() {
                    let doc_id = other.id;
                    let first_link = counted.insert((doc_id, *id));
                    let entry = docs.entry(doc_id).or_insert_with(|| (other, 0));
                    if first_link {
                        entry.1 += 1;
                    }
                }
            }
        }
        Ok(docs)
    })?;

    Ok(docs
        .into_iter()
        .map(|(node_id, (row, degree))| RankedNode {
            node_id,
            key: row.symbol.as_str().to_string(),
            // Normalise onto the same [0, 1] centrality scale as code neighbours
            // (degree / max_degree), clamped so a doc documenting many candidate
            // symbols is at most as central as the most-connected code node — never
            // out-of-scale: max_degree counts code vertices only, so an unclamped
            // doc fan-in could exceed 1.0 and wrongly dominate the ranking.
            score: (degree as f64 / max_degree as f64).min(1.0),
            seed: false,
        })
        .collect())
}

/// Maximum `Contains` hops walked from a bound artifact node to its enclosing
/// spec section/file ([`artifact_neighbours`]) — a defensive bound far above any
/// real artifact nesting (an `ApiOperation` sits two levels under its
/// `ConfigFile`: `ConfigFile` → `ApiPath` → `ApiOperation`), so a malformed
/// `Contains` cycle terminates instead of spinning.
const ARTIFACT_ANCESTOR_HOPS: usize = 8;

/// The cross-artifact neighbours of a `context` candidate code set ([FR-NV-02],
/// [FR-CG-11], CR-011) — the artifact twin of [`doc_neighbours`].
///
/// S-068/S-069's `ArtifactRef`/`ArtifactBinding` edges are fenced out of the
/// hydrated code subgraph at the same audit point as the non-code node predicate
/// ([`EdgeKind::is_config_reference`], [UAT-CG-04]), so — exactly like the doc
/// graph — the artifact graph lives only in the canonical store. This reads each
/// candidate code symbol's `is_config_reference` edges (in **and** out, since a
/// binding points artifact→code), pulls in the artifact node on the other end (a
/// route handler's `ApiOperation`), and walks that node's `Contains` ancestors
/// (its `ApiPath`/`ConfigFile` — the "spec section") so a handler's bundle pulls
/// in its `ApiOperation` and spec section deterministically.
///
/// Each artifact node's score is pure centrality (an expansion neighbour, never a
/// seed), normalised by the same `max_degree` as the code candidates and clamped
/// to `[0, 1]`, so an `ApiOperation` (or shared `ConfigFile`) reachable from
/// several candidates outranks one reached from a single isolated symbol. A
/// [`BTreeMap`] keyed by node id fixes the order regardless of which candidate's
/// query surfaced the artifact first ([NFR-RA-06]).
///
/// [FR-NV-02]: ../../../docs/specs/requirements/FR-NV-02.md
/// [FR-CG-11]: ../../../docs/specs/requirements/FR-CG-11.md
/// [UAT-CG-04]: ../../../docs/specs/requirements/UAT-CG-04.md
fn artifact_neighbours(
    engine: &Engine,
    code_ids: &[NodeId],
    max_degree: usize,
) -> Result<Vec<RankedNode>> {
    if code_ids.is_empty() {
        return Ok(Vec::new());
    }
    let code_ids = code_ids.to_vec();
    let runtime = engine.nav_runtime_no_prologue()?;
    // artifact node id -> (row, the distinct candidate code symbols it reached
    // through). The set's size is the centrality degree; the BTreeMap fixes order.
    let reached: BTreeMap<NodeId, (NodeRow, BTreeSet<NodeId>)> =
        runtime.submit_read(move |store| {
            let mut reached: BTreeMap<NodeId, (NodeRow, BTreeSet<NodeId>)> = BTreeMap::new();
            for code_id in &code_ids {
                // A binding is artifact→code, so the artifact endpoint is on the code
                // node's inbound side; reading both sides keeps the read symmetric for
                // any future artifact→code direction without a second pass.
                let edges = store
                    .neighbours_in(*code_id)?
                    .into_iter()
                    .chain(store.neighbours_out(*code_id)?);
                for (kind, artifact) in edges {
                    if !kind.is_config_reference() {
                        continue;
                    }
                    // Pull the artifact node and its enclosing spec section/file.
                    for node in artifact_ancestry(store, artifact)? {
                        reached
                            .entry(node.id)
                            .or_insert_with(|| (node, BTreeSet::new()))
                            .1
                            .insert(*code_id);
                    }
                }
            }
            Ok(reached)
        })?;

    Ok(reached
        .into_iter()
        .map(|(node_id, (row, code_links))| RankedNode {
            node_id,
            key: row.symbol.as_str().to_string(),
            score: (code_links.len() as f64 / max_degree as f64).min(1.0),
            seed: false,
        })
        .collect())
}

/// An artifact node plus its `Contains` ancestors — the node itself, then each
/// enclosing scope reached by walking inbound `Contains` edges upward
/// (`ApiOperation` → `ApiPath` → `ConfigFile`), bounded by
/// [`ARTIFACT_ANCESTOR_HOPS`] and a visited set so a corrupt `Contains` cycle
/// terminates. The "spec section" a handler's bundle pulls in alongside its
/// `ApiOperation` ([FR-CG-11]).
fn artifact_ancestry(store: &dyn GraphStore, node: NodeRow) -> Result<Vec<NodeRow>> {
    let mut chain = vec![node];
    let mut seen: BTreeSet<NodeId> = BTreeSet::new();
    seen.insert(chain[0].id);
    for _ in 0..ARTIFACT_ANCESTOR_HOPS {
        let current = chain.last().expect("chain is non-empty");
        // The `Contains` parent is the source of the one inbound `Contains` edge.
        let parent = store
            .neighbours_in(current.id)?
            .into_iter()
            .find(|(kind, _)| *kind == EdgeKind::Contains)
            .map(|(_, parent)| parent);
        match parent {
            Some(parent) if seen.insert(parent.id) => chain.push(parent),
            _ => break,
        }
    }
    Ok(chain)
}

/// Fetch the [`NodeRow`]s for `ids` in one pooled read, keyed by id.
fn fetch_rows(engine: &Engine, ids: &[NodeId]) -> Result<HashMap<NodeId, NodeRow>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids = ids.to_vec();
    let runtime = engine.nav_runtime_no_prologue()?;
    runtime.submit_read(move |store| {
        let mut rows = HashMap::with_capacity(ids.len());
        for id in ids {
            if let Some(row) = store.node(id)? {
                rows.insert(id, row);
            }
        }
        Ok(rows)
    })
}

/// Map a store row to the wire-facing [`SymbolRef`].
fn symbol_ref(row: &NodeRow) -> SymbolRef {
    SymbolRef {
        symbol: row.symbol.as_str().to_string(),
        name: row.name.clone(),
        kind: row.kind,
        file: row.file_path.clone(),
        line: line_u32(row.start_line),
    }
}

/// A stored 1-based line number as the wire `u32`, dropping nonsense values.
fn line_u32(line: Option<i64>) -> Option<u32> {
    line.and_then(|l| u32::try_from(l).ok()).filter(|&l| l > 0)
}

/// Read the declaration source for `row` from the worktree, best-effort.
///
/// Returns `None` when the node has no file/line binding, the path escapes
/// the project root (defensive — paths in the store are project-relative by
/// construction), or the file is unreadable/shorter than the recorded span
/// (deleted or rewritten since indexing — the best-effort contract tolerates
/// that drift rather than erroring, [NFR-DM-02]).
fn read_code(root: &Path, row: &NodeRow) -> Option<String> {
    let text = file_text(root, row.file_path.as_deref()?)?;
    slice_span(&text, row.start_line, row.end_line)
}

/// Read a project-relative file from the worktree, refusing any path that
/// could escape `root` — lexically (absolute, `..`, a Windows prefix) **and**
/// physically: the canonicalised target must still live under the
/// canonicalised root, so a repo-controlled symlink pointing outside the
/// project cannot leak foreign file content into a navigation result.
fn file_text(root: &Path, rel: &str) -> Option<String> {
    let rel = Path::new(rel);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
    {
        return None;
    }
    // Canonicalise BOTH sides: the root itself may sit behind a symlink
    // (macOS temp dirs do), and comparing a resolved target against an
    // unresolved root would reject every legitimate read.
    let canonical_root = std::fs::canonicalize(root).ok()?;
    let canonical = std::fs::canonicalize(root.join(rel)).ok()?;
    if !canonical.starts_with(&canonical_root) {
        return None;
    }
    std::fs::read_to_string(&canonical).ok()
}

/// Slice the 1-based inclusive `[start, end]` line span out of `text`;
/// `None` for a missing/invalid start or a span past the end of the file.
fn slice_span(text: &str, start: Option<i64>, end: Option<i64>) -> Option<String> {
    let start = usize::try_from(start?).ok().filter(|&l| l > 0)?;
    let end = end
        .and_then(|l| usize::try_from(l).ok())
        .unwrap_or(start)
        .max(start);
    let lines: Vec<&str> = text.lines().skip(start - 1).take(end - start + 1).collect();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Size and mtime (unix seconds) of a file, if it exists.
fn file_facts(path: &Path) -> (u64, Option<u64>) {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            (meta.len(), mtime)
        }
        Err(_) => (0, None),
    }
}
