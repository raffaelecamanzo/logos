//! A finding in graph context ([FR-SC-13], S-669): the symbol that encloses
//! it, whether a `route` reaches it over call edges, and how much depends on
//! it — each claim with its confidence.
//!
//! [`contexts`] is computed by the engine from logos.db and handed into
//! [`findings`](super::findings), which reads no graph itself.
//!
//! - **Enclosing symbol.** The innermost code symbol whose span holds the
//!   finding's start line, by the one binder a git hunk is attributed with
//!   ([`innermost`]). Route nodes and documentation or config nodes are not
//!   candidates: a finding sits in code, and a route's span is its
//!   registration, not the handler it names.
//! - **Reachability.** Whether any `route` node reaches the symbol over
//!   `RoutesTo` and `Calls` edges, at any distance; the nearest route is named
//!   with its hop count.
//! - **Blast radius.** The sizes of [`impact`](crate::navigate::impact) on the
//!   same symbol — the same traversal `logos impact <symbol>` runs
//!   ([`impact_sets`](crate::navigate::impact_sets)), over the view the listing
//!   already hydrated, so the two never disagree. The lists themselves are that
//!   command's answer.
//!
//! # Confidence ([NFR-CC-04], [NFR-RA-05])
//! Each claim is `resolved`, `unresolved` or `unbound`:
//!
//! - `unbound` — there is no enclosing symbol: the finding has no location,
//!   its path is not an indexed file (a lockfile, excluded by default, keeps
//!   its manifest line in [`line`](FindingContext::line)), its file changed
//!   since the scan, its line is in no code symbol, or it is a secret found
//!   only in history (`historical`, anchored to its commit). A stale finding
//!   is never bound: its line may now point at a different symbol.
//! - `resolved` — the per-language resolution denominator has a cross-file
//!   `calls` figure for the enclosing file's language: the graph binds calls
//!   across files there.
//! - `unresolved` — the language has no such figure (same-file-only, no
//!   resolved edges, nothing recorded, or no row). A route that *is* found is
//!   still `reachable`; one that is not is `unknown`, never `unreachable`.
//!
//! A route not found is `unreachable` only when the graph could have seen
//! every way in. Otherwise the claim is `unresolved` and `unknown`, naming the
//! way in it could not see ([`OpenPath`]): the graph holds no route, a route
//! names no proven handler, the symbol sits behind a supertype's dispatch, or
//! it is referenced other than by a call. The blast radius of a symbol behind
//! dispatch is `unresolved` too: its callers through the supertype are
//! counted against the supertype's member.
//!
//! The denominator row the label was read from rides on the payload
//! ([`resolution`](FindingContext::resolution)), so a reader can see how much
//! of the language's calls bind.
//!
//! [FR-SC-13]: ../../../docs/specs/requirements/FR-SC-13.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use anyhow::Result;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use serde::Serialize;

use crate::engine::Engine;
use crate::graph_store::NodeRow;
use crate::history::security::CodeScope;
use crate::history::{FindingBinding, SecurityFinding, FRESHNESS_STALE};
use crate::hydrate::{Granularity, GraphView};
use crate::model::{EdgeKind, NodeId, NodeKind};
use crate::models::navigation::{RelationResolution, SymbolRef};
use crate::navigate::branch::innermost;
use crate::navigate::symbol_ref;

/// How far a claim can be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// The graph resolves the edges the claim rests on.
    Resolved,
    /// The language or file has unresolved references on the path.
    Unresolved,
    /// There is no enclosing symbol to make the claim about.
    Unbound,
}

/// Why a finding has no enclosing symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnboundBecause {
    /// The result names no file or no line.
    NoLocation,
    /// Its path is not exactly one indexed file — a lockfile is excluded from
    /// the index by default.
    NotIndexed,
    /// Its file changed since the scan, so its line may point elsewhere.
    Stale,
    /// Its line lies in no code symbol's span.
    NoSymbol,
    /// A secret found only in history: its file is gone, or its line holds
    /// other text now, so it is anchored to its commit and no symbol of
    /// today's tree is claimed for it (S-767).
    Historical,
}

/// Why the graph cannot rule out a route reaching a symbol: a way in that no
/// walk over resolved `RoutesTo` and `Calls` edges would see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenPath {
    /// The file's language binds no call across files.
    Language,
    /// The graph holds no route at all — a framework it does not promote has
    /// none to walk from.
    NoRoutes,
    /// A route names no handler the graph proved (a closure, an unresolved
    /// path), so it could reach anything.
    UnprovenRoute,
    /// The symbol is a member of a type that implements or extends another,
    /// or implements one itself: a call can reach it through dispatch on the
    /// supertype, which binds to the supertype's member.
    Dispatch,
    /// The symbol is referenced other than by a call — passed as a value,
    /// handed off as a function pointer — and the walk follows calls only.
    ValueReference,
    /// The symbol is not in the hydrated graph yet (it landed after the
    /// view's snapshot), so it was not looked at.
    NotInGraph,
}

/// Whether a route reaches the enclosing symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// A `route` node reaches it.
    Reachable,
    /// None does, and the graph could have seen every way in: its language
    /// binds calls across files and no [`OpenPath`] applies.
    Unreachable,
    /// None was found, and the graph could not have found one: the claim is
    /// unresolved or unbound, or the symbol is not in the hydrated graph yet.
    Unknown,
}

/// One finding's graph context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FindingContext {
    /// The line the finding is bound at — its start line. An unbound finding
    /// keeps it: a lockfile or manifest finding names its line here.
    pub line: Option<i64>,
    pub enclosing: Enclosing,
    pub reachability: Reachability,
    pub blast_radius: BlastRadius,
    /// The `calls` row of the enclosing file's language, from the
    /// per-language resolution denominator the reachability and blast-radius
    /// labels are read from. `None` when unbound or when the language has no
    /// row.
    pub resolution: Option<CallResolution>,
}

/// The symbol that encloses the finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Enclosing {
    /// `resolved` when a symbol is found — a span lookup does not depend on
    /// call resolution — else `unbound`.
    pub confidence: Confidence,
    pub symbol: Option<SymbolRef>,
    /// Why there is none; `None` exactly when there is a symbol.
    pub unbound_because: Option<UnboundBecause>,
}

/// Whether a route reaches the enclosing symbol over `RoutesTo` and `Calls`
/// edges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reachability {
    pub confidence: Confidence,
    pub verdict: Verdict,
    /// The nearest route that reaches it; `Some` exactly when `reachable`.
    pub route: Option<RouteHit>,
    /// Why the claim is `unresolved`; `Some` exactly then.
    pub unresolved_because: Option<OpenPath>,
}

/// The route nearest to a reachable symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RouteHit {
    /// The route node's canonical symbol.
    pub symbol: String,
    /// Its name: method and path, `GET /users`.
    pub name: String,
    /// Edges from the route to the symbol: `1` for its handler.
    pub hops: u32,
}

/// How much depends on the enclosing symbol: the sizes of `impact` on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlastRadius {
    pub confidence: Confidence,
    /// `None` when unbound.
    #[serde(flatten)]
    pub size: Option<ImpactSize>,
}

/// The sizes of one `impact` answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ImpactSize {
    /// The depth bound both directions were walked to.
    pub depth: u32,
    /// Symbols that break if it changes.
    pub upstream: usize,
    /// Symbols it depends on.
    pub downstream: usize,
    /// Documentation sections that reference it.
    pub docs: usize,
}

/// One language's `calls` resolution, as the denominator states it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallResolution {
    pub language: String,
    pub calls: RelationResolution,
}

impl FindingContext {
    /// The context of a finding with no enclosing symbol.
    #[must_use]
    pub fn unbound(line: Option<i64>, because: UnboundBecause) -> Self {
        Self {
            line,
            enclosing: Enclosing {
                confidence: Confidence::Unbound,
                symbol: None,
                unbound_because: Some(because),
            },
            reachability: Reachability {
                confidence: Confidence::Unbound,
                verdict: Verdict::Unknown,
                route: None,
                unresolved_because: None,
            },
            blast_radius: BlastRadius {
                confidence: Confidence::Unbound,
                size: None,
            },
            resolution: None,
        }
    }
}

/// The graph context of each finding, in order ([module docs](self)).
///
/// # Errors
/// A store or hydration failure.
pub(crate) fn contexts(engine: &Engine, findings: &[&SecurityFinding]) -> Result<Vec<FindingContext>> {
    let anchors: Vec<Result<(&str, u32), UnboundBecause>> = findings.iter().map(|f| anchor(f)).collect();
    let paths = anchored_paths(&anchors);
    if paths.is_empty() {
        return Ok(findings
            .iter()
            .zip(&anchors)
            .map(|(f, a)| FindingContext::unbound(f.start_line, a.err().unwrap_or(UnboundBecause::NoSymbol)))
            .collect());
    }

    let runtime = engine.nav_runtime()?;
    let (spans, languages, rows) = runtime.submit_read(move |store| {
        Ok((
            store.span_nodes_in_files(&paths)?,
            store.file_languages(&paths)?,
            crate::resolve::coverage_by_language(store)?,
        ))
    })?;
    let bound = bind(&spans, &anchors);

    let view = engine.hydrate(Granularity::ExcludeContains)?;
    // The full symbol view keeps `Contains` and the structural facts
    // (`Implements`, `Extends`) the dependency view fences out.
    let symbols = engine.hydrate(Granularity::Symbol)?;
    let routes = route_reach(&view);
    // Every bound symbol's documentation links, in one read.
    let ids: Vec<NodeId> = bound
        .iter()
        .filter_map(|b| b.as_ref().ok().map(|row| row.id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let docs: HashMap<NodeId, usize> = runtime.submit_read(move |store| {
        ids.into_iter()
            .map(|id| Ok((id, crate::navigate::impact_docs(store, id)?.len())))
            .collect()
    })?;
    let mut sizes: HashMap<&str, ImpactSize> = HashMap::new();
    let mut out = Vec::with_capacity(findings.len());
    for (finding, bound) in findings.iter().zip(bound) {
        let row = match bound {
            Ok(row) => row,
            Err(because) => {
                out.push(FindingContext::unbound(finding.start_line, because));
                continue;
            }
        };
        let resolution = row
            .file_path
            .as_deref()
            .and_then(|path| languages.get(path).cloned().flatten())
            .and_then(|language| rows.iter().find(|r| r.language == language))
            .map(|r| CallResolution { language: r.language.clone(), calls: r.calls.clone() });
        let language = match &resolution {
            Some(r) if r.calls.cross_file_edges.is_some() => Confidence::Resolved,
            _ => Confidence::Unresolved,
        };
        // A caller through the supertype is counted against the supertype's
        // member, so the upstream set can be short.
        let blast = if dispatched(&symbols, row.symbol.as_str()) {
            Confidence::Unresolved
        } else {
            language
        };
        let size = match sizes.get(row.symbol.as_str()) {
            Some(size) => *size,
            None => {
                let size = impact_size(engine, &view, row, docs.get(&row.id).copied().unwrap_or(0))?;
                sizes.insert(row.symbol.as_str(), size);
                size
            }
        };
        out.push(FindingContext {
            line: finding.start_line,
            enclosing: Enclosing {
                confidence: Confidence::Resolved,
                symbol: Some(symbol_ref(row)),
                unbound_because: None,
            },
            reachability: reachability(&view, &symbols, &routes, row.symbol.as_str(), language),
            blast_radius: BlastRadius { confidence: blast, size: Some(size) },
            resolution,
        });
    }
    Ok(out)
}

/// Whether each finding's enclosing code node is test code ([FR-AN-05]), in
/// order: the graph's half of its code scope ([`super::scope`], [FR-SC-28]).
/// A finding binds exactly as [`contexts`] binds it, so a historical, unbound
/// or stale one has no enclosing node and reads `false`, as does one whose
/// line lies in no code symbol. One span read and one read of the test-marked
/// ids, with no hydration: the counting surfaces ask it of every stored
/// finding. It reads the graph as it stands and never indexes first; an engine
/// with no graph reader (a transient [`Engine::open`]) answers `false` for
/// every finding, so the path rule alone classifies them.
///
/// # Errors
/// A store failure.
///
/// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
/// [FR-SC-28]: ../../../docs/specs/requirements/FR-SC-28.md
pub(crate) fn in_test_symbols(engine: &Engine, findings: &[&SecurityFinding]) -> Result<Vec<bool>> {
    let anchors: Vec<Result<(&str, u32), UnboundBecause>> = findings.iter().map(|f| anchor(f)).collect();
    let paths = anchored_paths(&anchors);
    let Some(runtime) = engine.nav_runtime_no_prologue().ok().filter(|_| !paths.is_empty()) else {
        return Ok(vec![false; findings.len()]);
    };
    let (spans, tests) = runtime.submit_read(move |store| Ok((store.span_nodes_in_files(&paths)?, store.test_node_ids()?)))?;
    let tests: HashSet<NodeId> = tests.into_iter().collect();
    Ok(bind(&spans, &anchors)
        .into_iter()
        .map(|row| row.is_ok_and(|row| tests.contains(&row.id)))
        .collect())
}

/// Each finding's code scope by this engine's graph ([`super::scope::scopes`]
/// over [`in_test_symbols`]): what the Files & Risk and hotspot findings cell
/// counts by. Here, beside the graph read, so the engine names one module for
/// both.
///
/// # Errors
/// A store failure.
pub(crate) fn code_scopes(engine: &Engine, findings: &[&SecurityFinding]) -> Result<Vec<CodeScope>> {
    let scopes = super::scope::scopes(engine.root(), findings, |f| in_test_symbols(engine, f))?;
    Ok(scopes.into_iter().map(|s| s.code_scope).collect())
}

/// The distinct paths `anchors` bind in, sorted: what one span read covers.
fn anchored_paths(anchors: &[Result<(&str, u32), UnboundBecause>]) -> Vec<String> {
    anchors
        .iter()
        .filter_map(|a| a.as_ref().ok().map(|(path, _)| (*path).to_string()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Each anchor's enclosing code node among `spans`, or why it has none: the
/// one binding [`contexts`] and [`in_test_symbols`] share.
fn bind<'a>(
    spans: &'a [NodeRow],
    anchors: &[Result<(&str, u32), UnboundBecause>],
) -> Vec<Result<&'a NodeRow, UnboundBecause>> {
    let mut by_path: BTreeMap<&str, Vec<&NodeRow>> = BTreeMap::new();
    for row in spans.iter().filter(|row| encloses_code(row.kind)) {
        if let Some(path) = row.file_path.as_deref() {
            by_path.entry(path).or_default().push(row);
        }
    }
    anchors
        .iter()
        .map(|anchor| {
            let (path, line) = (*anchor)?;
            by_path.get(path).and_then(|nodes| enclosing(nodes, line)).ok_or(UnboundBecause::NoSymbol)
        })
        .collect()
}

/// The indexed path and line a finding binds at, or why it binds at none.
fn anchor(f: &SecurityFinding) -> Result<(&str, u32), UnboundBecause> {
    let line = f
        .start_line
        .and_then(|l| u32::try_from(l).ok())
        .filter(|&l| l > 0);
    match (f.binding, f.path.as_deref(), line) {
        (FindingBinding::Historical, _, _) => Err(UnboundBecause::Historical),
        (FindingBinding::Unbound, _, _) | (_, _, None) => Err(UnboundBecause::NoLocation),
        (FindingBinding::Unmatched | FindingBinding::Ambiguous, _, _) | (_, None, _) => {
            Err(UnboundBecause::NotIndexed)
        }
        _ if f.freshness == FRESHNESS_STALE => Err(UnboundBecause::Stale),
        (FindingBinding::Bound, Some(path), Some(line)) => Ok((path, line)),
    }
}

/// Whether a node of `kind` can enclose a finding: a code symbol, never a
/// route registration or a documentation or config node.
const fn encloses_code(kind: NodeKind) -> bool {
    !matches!(kind, NodeKind::Route) && !kind.is_non_code()
}

/// The one symbol `line` sits innermost in. Of nodes with the same span, a
/// declaration wins over the file's module — a file holding one function
/// spans exactly what the function does, and the module's symbol is a prefix
/// of the function's, so the symbol order alone would pick the module — and
/// any remaining tie goes to the lower canonical symbol, so the answer is
/// deterministic.
fn enclosing<'a>(nodes: &[&'a NodeRow], line: u32) -> Option<&'a NodeRow> {
    innermost(nodes, (line, line)).into_iter().min_by(|a, b| {
        let width = |row: &NodeRow| {
            let start = row.start_line.unwrap_or(0);
            row.end_line.unwrap_or(0).max(start) - start
        };
        width(a)
            .cmp(&width(b))
            .then_with(|| (a.kind == NodeKind::Module).cmp(&(b.kind == NodeKind::Module)))
            .then_with(|| a.symbol.as_str().cmp(b.symbol.as_str()))
    })
}

/// What one walk from every route found.
struct RouteReach {
    /// For every vertex a route reaches, the nearest such route and its
    /// distance.
    reached: HashMap<NodeIndex, (NodeIndex, u32)>,
    /// Why no symbol can be called unreachable, whichever it is.
    open: Option<OpenPath>,
}

/// One breadth-first walk over `RoutesTo` and `Calls` edges from every route
/// at once, seeded in symbol order so a tie is broken the same way on every
/// run.
fn route_reach(view: &GraphView) -> RouteReach {
    let graph = view.graph();
    let mut roots: Vec<NodeIndex> = graph
        .node_indices()
        .filter(|&i| graph[i].kind == Some(NodeKind::Route))
        .collect();
    roots.sort_by(|a, b| graph[*a].key.cmp(&graph[*b].key));
    let proven = |route: NodeIndex| {
        graph
            .edges_directed(route, Direction::Outgoing)
            .any(|e| e.weight().kind == Some(EdgeKind::RoutesTo) && e.target() != route)
    };
    let open = if roots.is_empty() {
        Some(OpenPath::NoRoutes)
    } else if !roots.iter().all(|&r| proven(r)) {
        Some(OpenPath::UnprovenRoute)
    } else {
        None
    };
    let mut reached: HashMap<NodeIndex, (NodeIndex, u32)> = roots.iter().map(|&r| (r, (r, 0))).collect();
    let mut queue: VecDeque<NodeIndex> = roots.into_iter().collect();
    while let Some(current) = queue.pop_front() {
        let (route, distance) = reached[&current];
        for edge in graph.edges_directed(current, Direction::Outgoing) {
            if !matches!(edge.weight().kind, Some(EdgeKind::RoutesTo | EdgeKind::Calls)) {
                continue;
            }
            if let std::collections::hash_map::Entry::Vacant(slot) = reached.entry(edge.target()) {
                slot.insert((route, distance + 1));
                queue.push_back(edge.target());
            }
        }
    }
    RouteReach { reached, open }
}

/// The reachability claim for `symbol`, whose language is labelled
/// `language`. A route found is `reachable` whatever the label; none found is
/// `unreachable` only when no [`OpenPath`] applies.
fn reachability(
    view: &GraphView,
    symbols: &GraphView,
    routes: &RouteReach,
    symbol: &str,
    language: Confidence,
) -> Reachability {
    let graph = view.graph();
    let at = view.index_of(symbol);
    let by_language = (language != Confidence::Resolved).then_some(OpenPath::Language);
    if let Some(&(route, hops)) = at.and_then(|i| routes.reached.get(&i)).filter(|(_, hops)| *hops > 0) {
        return Reachability {
            confidence: language,
            verdict: Verdict::Reachable,
            route: Some(RouteHit {
                symbol: graph[route].key.clone(),
                name: graph[route].label.clone(),
                hops,
            }),
            unresolved_because: by_language,
        };
    }
    let open = by_language
        .or_else(|| at.is_none().then_some(OpenPath::NotInGraph))
        .or(routes.open)
        .or_else(|| dispatched(symbols, symbol).then_some(OpenPath::Dispatch))
        .or_else(|| value_referenced(symbols, symbol).then_some(OpenPath::ValueReference));
    match open {
        None => Reachability {
            confidence: Confidence::Resolved,
            verdict: Verdict::Unreachable,
            route: None,
            unresolved_because: None,
        },
        Some(open) => Reachability {
            confidence: Confidence::Unresolved,
            verdict: Verdict::Unknown,
            route: None,
            unresolved_because: Some(open),
        },
    }
}

/// `true` when `symbol` implements or extends something, is implemented or
/// extended, or is a member of a type that implements or extends something —
/// in the full symbol view, which keeps `Contains` and those facts.
fn dispatched(symbols: &GraphView, symbol: &str) -> bool {
    let Some(at) = symbols.index_of(symbol) else {
        return false;
    };
    let graph = symbols.graph();
    let supertyped = |i: NodeIndex| {
        graph
            .edges_directed(i, Direction::Outgoing)
            .any(|e| matches!(e.weight().kind, Some(EdgeKind::Implements | EdgeKind::Extends)))
    };
    let mut incoming = graph.edges_directed(at, Direction::Incoming);
    supertyped(at)
        || incoming.any(|e| match e.weight().kind {
            Some(EdgeKind::Implements | EdgeKind::Extends) => true,
            Some(EdgeKind::Contains) => supertyped(e.source()),
            _ => false,
        })
}

/// `true` when anything reaches `symbol` other than a call or its lexical
/// container: a reference, a handoff marker, an instantiation, a type use.
fn value_referenced(symbols: &GraphView, symbol: &str) -> bool {
    symbols.index_of(symbol).is_some_and(|at| {
        symbols
            .graph()
            .edges_directed(at, Direction::Incoming)
            .any(|e| !matches!(e.weight().kind, Some(EdgeKind::Calls | EdgeKind::Contains)))
    })
}

/// The sizes of [`impact`](crate::navigate::impact) on `row` at its default
/// depth — the answer `logos impact <symbol>` gives, from the same traversal
/// ([`impact_sets`](crate::navigate::impact_sets)) over the view this listing
/// already hydrated, with its `docs` links counted by the caller.
fn impact_size(engine: &Engine, view: &GraphView, row: &NodeRow, docs: usize) -> Result<ImpactSize> {
    let depth = crate::navigate::DEFAULT_IMPACT_DEPTH;
    let (upstream, downstream) = crate::navigate::impact_sets(engine, view, row, depth)?;
    Ok(ImpactSize {
        depth: depth as u32,
        upstream: upstream.len(),
        downstream: downstream.len(),
        docs,
    })
}
