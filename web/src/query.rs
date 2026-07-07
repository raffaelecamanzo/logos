//! The read-only structured + relational graph query surface (S-120 / FR-UI-14,
//! [ADR-35]).
//!
//! Gives the interactive canvas a genuine **whole-graph** query in place of the
//! old visible-only substring locate. It is **pure composition** of existing
//! read-only `Engine` read-models — FTS5 symbol search ([FR-NV-01]) for the
//! field-filter form, and the `callers`/`callees`/`impact` navigation primitives
//! ([FR-NV-10]) for the relational form — reached through the façade's read-only
//! accessors ([ADR-28]). It adds **no engine primitive, no store, and no write
//! path**: a query mutates nothing. There is no query grammar/parser — the
//! structured form (typed fields + a verb selector) is the whole contract
//! ([ADR-35]), so a complex compound query is deliberately out of scope.
//!
//! The query runs over the **whole graph** (the indexed FTS table / navigation
//! read-model), not the canvas's currently-rendered set, so it surfaces nodes
//! that are not currently visible ([FR-UI-14]). Results are presentation-shaped
//! [`QueryHit`]s whose `id` is the canonical symbol — the canvas node id — so a
//! clicked hit can be centered and locked by the merged S-119 locked-selection
//! mechanism. An empty result is an honest "no matches" state, never an error
//! ([NFR-CC-04], [FR-NV-09]).
//!
//! [ADR-35]: ../../../docs/specs/architecture/decisions/ADR-35.md
//! [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
//! [FR-UI-14]: ../../../docs/specs/requirements/FR-UI-14.md
//! [FR-NV-01]: ../../../docs/specs/requirements/FR-NV-01.md
//! [FR-NV-10]: ../../../docs/specs/requirements/FR-NV-10.md
//! [FR-NV-09]: ../../../docs/specs/requirements/FR-NV-09.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md

use std::collections::{HashMap, HashSet};

use logos_core::model::NodeKind;
use logos_core::models::navigation::{GraphLayer, ImpactResult, SymbolRef};
use logos_core::Engine;
use serde::Serialize;

/// The ranked-result display cap (the sprint risk-table "cap ranked results"
/// bound). Shared as the `callers`/`callees` `limit` for the relational verbs.
const QUERY_LIMIT: usize = 50;

/// The wider search pool a filter query draws from **before** the layer/file
/// post-filter, so refining by a derived layer or a file path still leaves a full
/// ranked top-`QUERY_LIMIT` to return (the `kind` filter is pushed into `search`
/// itself, so it needs no widening). Bounded so the whole-graph query stays a
/// fast indexed point query (reuse the indexed search, cap the results).
const FILTER_POOL: usize = 200;

/// A relational verb over the navigation read-model ([FR-NV-10]). The structured
/// query's only verbs — no grammar, no free-text DSL ([ADR-35]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    /// Direct callers of the target symbol (`Engine::callers`).
    Callers,
    /// Direct callees of the target symbol (`Engine::callees`).
    Callees,
    /// Transitive impact of the target symbol, both directions (`Engine::impact`).
    Impact,
}

impl Verb {
    /// Parse the wire verb token, or `None` for an unrecognised one.
    fn from_wire(s: &str) -> Option<Verb> {
        match s {
            "callers-of" => Some(Verb::Callers),
            "callees-of" => Some(Verb::Callees),
            "impact-of" => Some(Verb::Impact),
            _ => None,
        }
    }

    /// The human noun phrase for echoes and the honest empty-state note
    /// ("No callers of foo.").
    fn noun(self) -> &'static str {
        match self {
            Verb::Callers => "callers of",
            Verb::Callees => "callees of",
            Verb::Impact => "impact for",
        }
    }
}

/// One ranked result row — a presentation-shaped graph node the canvas lists and
/// can center/lock. `id` is the canonical symbol string (the canvas node id), so
/// selecting a row round-trips into the locked-selection mechanism (S-119).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct QueryHit {
    /// Stable identity — the canonical SCIP symbol = the canvas node id.
    pub id: String,
    /// Human-facing label (the node name).
    pub label: String,
    /// The node ontology kind (wire form: lower-case snake_case).
    pub kind: NodeKind,
    /// The presentation layer this node renders in, derived from the kind through
    /// the canonical [`GraphLayer`] classification (never fabricated, [NFR-RA-05]).
    pub layer: GraphLayer,
    /// Project-relative defining file, when bound.
    pub file: Option<String>,
    /// 1-based start line of the declaration, when recorded.
    pub line: Option<u32>,
    /// 1-based rank within the ranked result set.
    pub rank: u32,
}

impl QueryHit {
    /// Shape a navigation [`SymbolRef`] into a ranked hit, deriving the layer from
    /// the kind through the single source of truth shared with the canvas (S-121).
    fn from_symbol(s: &SymbolRef, rank: u32) -> QueryHit {
        QueryHit {
            id: s.symbol.clone(),
            label: s.name.clone(),
            kind: s.kind,
            layer: GraphLayer::from(s.kind),
            file: s.file.clone(),
            line: s.line,
            rank,
        }
    }
}

/// The read-only query result the canvas consumes. Presentation-shaped: it
/// carries the interpreted query echo, the ranked hits, the pre-cap `total` (so
/// the UI can say "showing N of M"), and an honest `note` for the no-matches /
/// guidance states — never an error status ([NFR-CC-04], [FR-NV-09]).
#[derive(Debug, Default, Serialize)]
pub(crate) struct QueryResponse {
    /// `"filter"`, `"relation"`, or `"empty"` — which form was interpreted.
    pub mode: &'static str,
    /// A human echo of the interpreted query (the search term + active filters, or
    /// the verb + target).
    pub query: String,
    /// The ranked hits, best-first, at most [`QUERY_LIMIT`].
    pub hits: Vec<QueryHit>,
    /// How many matches existed before the display cap — the honesty denominator.
    pub total: u32,
    /// An honest note: the opening prompt, input guidance, or the "no matches"
    /// state. `None` when there are hits to show.
    pub note: Option<String>,
    /// "Did you mean" names passed through from the underlying read-model
    /// ([FR-NV-09]).
    pub suggestions: Vec<String>,
    /// Degradation channel (ADR-14): a failed underlying read is reported, not
    /// surfaced as an HTTP error.
    pub warnings: Vec<String>,
}

impl QueryResponse {
    /// No usable input — the honest opening prompt, not an error.
    fn empty() -> QueryResponse {
        QueryResponse { mode: "empty", ..QueryResponse::default() }
    }

    /// An honest guidance/no-input note for `mode` (e.g. "enter a search term",
    /// "unknown kind") — a 200 response with no hits, never an error.
    fn guidance(mode: &'static str, note: String) -> QueryResponse {
        QueryResponse { mode, note: Some(note), ..QueryResponse::default() }
    }
}

/// Run a structured query over the read-only engine read-models, dispatching on
/// the presence of a relational `verb`. The single composition entry point the
/// [`crate`] handler bridges to; a pure reader over `Engine` (no store mutation,
/// [ADR-28]).
pub(crate) fn run(engine: &Engine, params: &HashMap<String, String>) -> QueryResponse {
    match get(params, "verb") {
        Some(verb) => run_relation(
            engine,
            verb,
            get(params, "target").or_else(|| get(params, "symbol")).unwrap_or_default(),
        ),
        None => run_filter(engine, params),
    }
}

/// A trimmed, non-empty query parameter, or `None`.
fn get<'a>(params: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    params.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
}

/// The field-filter form: a whole-graph FTS `search` over a term, refined by an
/// optional `kind` (pushed into `search`), `layer` (derived from kind), and
/// `file` (path-substring) filter. Ranked by search relevance ([ADR-35]).
fn run_filter(engine: &Engine, params: &HashMap<String, String>) -> QueryResponse {
    let text = get(params, "q").unwrap_or_default();
    let kind_raw = get(params, "kind");
    let layer_raw = get(params, "layer");
    let file = get(params, "file");

    // No input at all → the honest opening prompt (not an error, FR-NV-09).
    if text.is_empty() && kind_raw.is_none() && layer_raw.is_none() && file.is_none() {
        return QueryResponse::empty();
    }
    // A filter query composes whole-graph symbol search ([ADR-35]); it needs a
    // term to rank by relevance — the field filters refine a search, they do not
    // replace it (there is no whole-graph node enumeration that carries `file`).
    if text.is_empty() {
        return QueryResponse::guidance(
            "filter",
            "Enter a search term — the kind, layer, and file filters refine a search.".to_string(),
        );
    }
    // An unrecognised kind/layer is reported honestly rather than silently ignored
    // (it would otherwise read as "no matches" for a valid-looking query).
    let kind = match kind_raw {
        Some(k) => match NodeKind::from_wire(k) {
            Some(nk) => Some(nk),
            None => return QueryResponse::guidance("filter", format!("Unknown kind “{k}”.")),
        },
        None => None,
    };
    let layer = match layer_raw {
        Some(l) => match parse_layer(l) {
            Some(gl) => Some(gl),
            None => {
                return QueryResponse::guidance(
                    "filter",
                    format!("Unknown layer “{l}” (expected code, doc, or artifact)."),
                )
            }
        },
        None => None,
    };

    // Widen the search pool only when a post-filter (layer/file) is active, so
    // refining does not starve the ranked top-`QUERY_LIMIT`; the `kind` filter is
    // native to `search`, so it needs no widening.
    let limit = if layer.is_some() || file.is_some() { FILTER_POOL } else { QUERY_LIMIT };
    let result = engine.search(text, kind, Some(limit));
    let matched: Vec<&SymbolRef> = result
        .hits
        .iter()
        .filter(|s| layer.is_none_or(|l| GraphLayer::from(s.kind) == l))
        .filter(|s| file.is_none_or(|f| s.file.as_deref().is_some_and(|p| p.contains(f))))
        .collect();
    let total = matched.len() as u32;
    let hits = rank_hits(matched);

    let echo = filter_echo(text, kind, layer, file);
    let note = hits.is_empty().then(|| format!("No matches for {echo}."));
    QueryResponse {
        mode: "filter",
        query: echo,
        hits,
        total,
        note,
        suggestions: result.suggestions,
        warnings: result.warnings,
    }
}

/// The relational form: a verb applied to a target symbol, resolved by delegating
/// to the existing `callers`/`callees`/`impact` navigation read-model ([FR-NV-10],
/// [ADR-35]). Results are in the navigation read-model's natural order
/// (nearest-first). An unresolved target or an empty relation is the honest
/// "no matches" state, not an error ([FR-NV-09]).
fn run_relation(engine: &Engine, raw_verb: &str, target: &str) -> QueryResponse {
    let Some(verb) = Verb::from_wire(raw_verb) else {
        return QueryResponse::guidance("relation", format!("Unknown relation “{raw_verb}”."));
    };
    if target.is_empty() {
        return QueryResponse::guidance(
            "relation",
            format!("Enter a target symbol for “{}”.", verb.noun()),
        );
    }
    let echo = format!("{} {target}", verb.noun());
    // Each arm clones what it needs out of the read-model (hits + resolved name)
    // before moving its `suggestions`/`warnings`, so no borrow of the read-model
    // outlives it.
    let (resolved, hits, total, suggestions, warnings) = match verb {
        Verb::Callers => {
            let r = engine.callers(target, Some(QUERY_LIMIT));
            (resolved_name(r.resolved.as_ref()), rank_hits(&r.callers), r.total, r.suggestions, r.warnings)
        }
        Verb::Callees => {
            let r = engine.callees(target, Some(QUERY_LIMIT));
            (resolved_name(r.resolved.as_ref()), rank_hits(&r.callees), r.total, r.suggestions, r.warnings)
        }
        Verb::Impact => {
            let r = engine.impact(target, None);
            let nodes = impact_nodes(&r);
            let total = nodes.len() as u32;
            let hits = rank_hits(nodes);
            (resolved_name(r.resolved.as_ref()), hits, total, r.suggestions, r.warnings)
        }
    };
    let note = if resolved.is_none() {
        Some(format!("“{target}” did not resolve to a symbol."))
    } else if hits.is_empty() {
        // An empty relation is a fact about a real, resolved node, not an error.
        Some(format!("No {} {}.", verb.noun(), resolved.as_deref().unwrap_or(target)))
    } else {
        None
    };
    QueryResponse { mode: "relation", query: echo, hits, total, note, suggestions, warnings }
}

/// The resolved target's human name, cloned out so the read-model can be consumed.
fn resolved_name(resolved: Option<&SymbolRef>) -> Option<String> {
    resolved.map(|s| s.name.clone())
}

/// Shape an ordered set of symbols into ranked hits, capped at [`QUERY_LIMIT`].
/// Generic over any iterator of `&SymbolRef` so it serves the filter pool
/// (`Vec<&SymbolRef>`), the adjacency results (`&[SymbolRef]`), and the impact set.
fn rank_hits<'a>(refs: impl IntoIterator<Item = &'a SymbolRef>) -> Vec<QueryHit> {
    refs.into_iter()
        .take(QUERY_LIMIT)
        .enumerate()
        .map(|(i, s)| QueryHit::from_symbol(s, i as u32 + 1))
        .collect()
}

/// The impact nodes in natural traversal order — upstream ("breaks if changed")
/// then downstream ("depends on"), each nearest-first as the read-model returns
/// them, deduped by symbol so a node reachable both ways appears once.
fn impact_nodes(r: &ImpactResult) -> Vec<&SymbolRef> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<&SymbolRef> = Vec::new();
    for entry in r.upstream.iter().chain(r.downstream.iter()) {
        if seen.insert(entry.symbol.symbol.as_str()) {
            out.push(&entry.symbol);
        }
    }
    out
}

/// Parse the `layer` filter token into a [`GraphLayer`], or `None` for an
/// unrecognised one.
fn parse_layer(s: &str) -> Option<GraphLayer> {
    // The single source of truth for the layer wire form lives in logos-core,
    // shared with the canvas's server-side `layers` re-budgeting filter (S-122).
    GraphLayer::from_wire(s)
}

/// A readable echo of the interpreted filter query — the search term plus any
/// active field filters — for the response and the canvas's result summary.
fn filter_echo(
    text: &str,
    kind: Option<NodeKind>,
    layer: Option<GraphLayer>,
    file: Option<&str>,
) -> String {
    let mut echo = format!("“{text}”");
    if let Some(k) = kind {
        echo.push_str(&format!(" kind:{}", k.as_str()));
    }
    if let Some(l) = layer {
        echo.push_str(&format!(" layer:{}", layer_token(l)));
    }
    if let Some(f) = file {
        echo.push_str(&format!(" file:{f}"));
    }
    echo
}

/// The wire token for a layer (the inverse of [`parse_layer`]).
fn layer_token(layer: GraphLayer) -> &'static str {
    match layer {
        GraphLayer::Code => "code",
        GraphLayer::Doc => "doc",
        GraphLayer::Artifact => "artifact",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use logos_core::models::navigation::ImpactEntry;

    fn sym(symbol: &str, name: &str, kind: NodeKind, file: Option<&str>) -> SymbolRef {
        SymbolRef {
            symbol: symbol.to_string(),
            name: name.to_string(),
            kind,
            file: file.map(str::to_string),
            line: Some(1),
        }
    }

    #[test]
    fn verb_parsing_round_trips_the_three_verbs_and_rejects_others() {
        assert_eq!(Verb::from_wire("callers-of"), Some(Verb::Callers));
        assert_eq!(Verb::from_wire("callees-of"), Some(Verb::Callees));
        assert_eq!(Verb::from_wire("impact-of"), Some(Verb::Impact));
        assert_eq!(Verb::from_wire("calls"), None);
        assert_eq!(Verb::from_wire(""), None);
    }

    #[test]
    fn layer_parsing_accepts_the_three_layers_and_rejects_others() {
        assert_eq!(parse_layer("code"), Some(GraphLayer::Code));
        assert_eq!(parse_layer("doc"), Some(GraphLayer::Doc));
        assert_eq!(parse_layer("artifact"), Some(GraphLayer::Artifact));
        assert_eq!(parse_layer("Code"), None);
        assert_eq!(parse_layer("widget"), None);
    }

    #[test]
    fn a_hit_derives_its_layer_from_the_kind_and_keeps_the_canonical_id() {
        // The layer is the canonical GraphLayer::from(kind) (S-121), never
        // fabricated; the id is the canonical symbol so it round-trips into the
        // canvas lock target.
        let doc = QueryHit::from_symbol(&sym("scip::FR", "FR-UI-14", NodeKind::Requirement, None), 3);
        assert_eq!(doc.layer, GraphLayer::Doc);
        assert_eq!(doc.id, "scip::FR");
        assert_eq!(doc.label, "FR-UI-14");
        assert_eq!(doc.rank, 3);
        let code = QueryHit::from_symbol(&sym("scip::f", "f", NodeKind::Function, Some("a.rs")), 1);
        assert_eq!(code.layer, GraphLayer::Code);
        assert_eq!(code.file.as_deref(), Some("a.rs"));
    }

    #[test]
    fn impact_nodes_are_upstream_then_downstream_nearest_first_deduped() {
        let r = ImpactResult {
            upstream: vec![
                ImpactEntry { symbol: sym("scip::a", "a", NodeKind::Function, None), distance: 1 },
                ImpactEntry { symbol: sym("scip::b", "b", NodeKind::Function, None), distance: 2 },
            ],
            downstream: vec![
                // `a` is reachable both ways → appears once (the upstream slot).
                ImpactEntry { symbol: sym("scip::a", "a", NodeKind::Function, None), distance: 1 },
                ImpactEntry { symbol: sym("scip::c", "c", NodeKind::Function, None), distance: 1 },
            ],
            ..Default::default()
        };
        let ids: Vec<&str> = impact_nodes(&r).iter().map(|s| s.symbol.as_str()).collect();
        assert_eq!(ids, vec!["scip::a", "scip::b", "scip::c"]);
    }

    #[test]
    fn filter_echo_names_the_term_and_the_active_filters() {
        let echo = filter_echo("Engine", Some(NodeKind::Function), Some(GraphLayer::Code), Some("src/"));
        assert_eq!(echo, "“Engine” kind:function layer:code file:src/");
        assert_eq!(filter_echo("x", None, None, None), "“x”");
    }
}
