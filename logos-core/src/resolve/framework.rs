//! The framework-promotion pass — Pass 2½ of the pipeline ([resolution-engine],
//! S-012, [FR-FW-01]..[FR-FW-04]).
//!
//! Runs **after** resolution on every index/sync and promotes matches of the
//! ratified v1 framework set ([FR-FW-03], DL-02) — Rust Axum/Actix-web,
//! Python FastAPI/Django, TypeScript/JS Express/Next.js, Go net/http + Gin,
//! Java Spring (S-015) — to first-class graph nodes:
//!
//! - a route registration becomes a [`NodeKind::Route`] node named
//!   `"METHOD /path"`, linked to its handler function via
//!   [`EdgeKind::RoutesTo`] ([FR-FW-01]);
//! - a component declaration (a Rust shared-state extractor type, a Next.js
//!   component function, a Spring stereotype class, a Django model) becomes a
//!   [`NodeKind::Component`] node linked to the underlying declaration via
//!   [`EdgeKind::References`] ([FR-FW-02] — the wired application building
//!   block).
//!
//! Per-language shape rules live in each plugin's `frameworks.scm` +
//! descriptor data (`framework_detectors`, `[framework_methods]`) under the
//! declarative capture contract interpreted by [`generic_match`] — a new
//! language's framework extraction is pure data ([NFR-MA-01]). The Rust
//! grammar alone keeps structural walkers behind its legacy capture names
//! (see [`scan_source`]).
//!
//! # Ledger-gated candidacy ([FR-FW-04])
//!
//! A file is a candidate iff its reference ledger names a framework its own
//! language's descriptor declares (`framework_detectors`: `axum…`,
//! `fastapi…`, `org::springframework…` path prefixes). External-package
//! references never resolve — the indexed graph holds no axum/fastapi nodes —
//! so the surviving ledger doubles as a free framework fingerprint. A plain
//! library has no such refs, so the pass parses **zero** files and can never
//! emit a spurious route.
//!
//! # Never fabricate ([NFR-RA-05])
//!
//! Handler and state-type texts are bound through the same [`binder`] the
//! resolution pass uses — the *exactly-one-or-nothing* acceptance rule
//! included. A route whose handler cannot be proven keeps its node but gets
//! **no** edge; a state type that does not bind to an indexed type is not
//! promoted at all.
//!
//! # Reconcile, don't accumulate
//!
//! Each run recomputes the full desired route/component set from the current
//! candidates and diffs it against the promoted nodes already in the graph:
//! missing ones are inserted, stale ones deleted, surviving ones keep their
//! node ids (and their edges are re-proven). The pass is therefore idempotent
//! and self-healing across syncs, exactly like the resolution ledger retry.
//!
//! # Budget honesty ([NFR-PE-02], OQ-07)
//!
//! The pass times itself and surfaces [`FrameworkStats`] on every
//! index/sync result, so the ≤30s-budget question is answered with measured
//! wall-clock, not guesses.
//!
//! [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
//! [FR-FW-01]: ../../../docs/specs/requirements/FR-FW-01.md
//! [FR-FW-02]: ../../../docs/specs/requirements/FR-FW-02.md
//! [FR-FW-03]: ../../../docs/specs/requirements/FR-FW-03.md
//! [FR-FW-04]: ../../../docs/specs/requirements/FR-FW-04.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-PE-02]: ../../../docs/specs/requirements/NFR-PE-02.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
//! [`NodeKind::Route`]: crate::model::NodeKind::Route
//! [`NodeKind::Component`]: crate::model::NodeKind::Component
//! [`EdgeKind::RoutesTo`]: crate::model::EdgeKind::RoutesTo
//! [`EdgeKind::References`]: crate::model::EdgeKind::References

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use rayon::prelude::*;
use tree_sitter::{Node, Parser, QueryCursor, StreamingIterator};

use crate::config::BindingPolicy;
use crate::extract::symbol::{build_symbol, descriptor_for, path_segments, SymbolContext};
use crate::graph_store::{NewNode, NodeRow, UnresolvedRefRow};
use crate::model::{EdgeKind, NodeId, NodeKind, RefForm};
use crate::models::pipeline::FrameworkStats;
use crate::plugin::{LanguagePlugin, LanguageRegistry};
use crate::runtime::Runtime;

use super::binder;

/// Method-router / method-attribute names recognised as HTTP registrations
/// (the union of Axum's `routing::*` constructors and Actix's method macros).
const HTTP_METHODS: [&str; 10] = [
    "get", "post", "put", "delete", "patch", "head", "options", "trace", "connect", "any",
];

/// Generic heads whose first type argument is a shared-state component
/// candidate: `axum::extract::State<T>` and `actix_web::web::Data<T>`.
const STATE_EXTRACTORS: [&str; 2] = ["State", "Data"];

/// Smart-pointer heads transparently unwrapped when reading a state type
/// (`State<Arc<AppState>>` promotes `AppState`).
const TRANSPARENT_WRAPPERS: [&str; 3] = ["Arc", "Rc", "Box"];

/// One matched route registration in one file (pre-binding).
#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteMatch {
    /// The registered URL path, verbatim (`"/users"`).
    path: String,
    /// The upper-cased HTTP method (`GET`, …, `ANY`).
    method: String,
    /// The handler expression text (a Rust path), when one was syntactically
    /// recognisable; `None` for closures and other non-path handlers.
    handler: Option<String>,
    /// 1-based first line of the registration site.
    start_line: u32,
    /// 1-based last line of the registration site.
    end_line: u32,
}

/// One matched shared-state extractor in one file (pre-binding).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ComponentMatch {
    /// The state type's written path text (`AppState`, `state::AppState`).
    type_path: String,
}

/// Everything the scanner found in one candidate file.
#[derive(Debug, Default, PartialEq, Eq)]
struct FileMatches {
    routes: Vec<RouteMatch>,
    components: Vec<ComponentMatch>,
}

/// Run the framework-promotion pass. See the module docs for the shape.
///
/// # Errors
/// Returns an error if the snapshot read or the commit batch fails (the batch
/// rolls back wholesale, NFR-RA-07).
pub fn run(
    runtime: &Runtime,
    registry: &LanguageRegistry,
    root: &Path,
    policy: BindingPolicy,
    delta: Option<&super::Delta>,
) -> Result<(FrameworkStats, Vec<String>)> {
    let started = Instant::now();

    // Incremental gate (S-024-HF, mirroring the CR-015 resolve delta): on a
    // `sync`/`reconcile` (`delta` is `Some`), skip the whole-graph snapshot when
    // the post-resolve graph has **no framework footprint** — no promoted
    // route/component node *and* no ledger ref naming a detector. This pass is a
    // pure, idempotent function of the candidate files (those whose ledger names a
    // detector) and the bound graph (see "Reconcile, don't accumulate" above); a
    // footprint-free graph has no candidate anywhere and nothing promoted, so a
    // from-scratch index would promote nothing either — the reconcile is provably
    // a no-op and is skipped, the framework-free fast path the [NFR-PE-03]
    // single-file-sync budget needs. A full `index` (`delta` is `None`) always
    // runs the whole-graph pass below. The footprint probe is one cheap EXISTS
    // read, not the ~O(graph) `all_nodes`/`all_edges`/`unresolved_refs`
    // materialisation the full pass pays.
    //
    // [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
    if delta.is_some() {
        let prefixes = detector_prefixes(registry);
        let has_footprint =
            runtime.submit_read(move |store| store.has_framework_footprint(&prefixes))?;
        if !has_footprint {
            return Ok((
                FrameworkStats {
                    duration_ms: elapsed_ms(started),
                    ..FrameworkStats::default()
                },
                Vec::new(),
            ));
        }
    }

    // Snapshot (one reader-pool read): the same consistent basis the
    // resolution pass binds against.
    let (files, nodes, edges, refs) = runtime.submit_read(|store| {
        Ok((
            store.indexed_files()?,
            store.all_nodes()?,
            store.all_edges()?,
            store.unresolved_refs()?,
        ))
    })?;

    // Promoted nodes already in the graph (the reconcile baseline).
    let existing: Vec<&NodeRow> = nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Route | NodeKind::Component))
        .collect();

    // Ledger-gated candidacy (FR-FW-04): which files name a framework their
    // own language's descriptor declares? Declarative since S-015 — each
    // plugin.toml lists its `framework_detectors` (canonical `::`-prefix form:
    // `axum`, `fastapi`, `org::springframework`, …) and the pass reads them
    // here instead of a hardcoded crate list.
    let detectors_by_file: HashMap<i64, &[String]> = files
        .iter()
        .filter_map(|f| {
            let ext = Path::new(&f.path).extension()?.to_str()?;
            let plugin = registry.for_extension(ext)?;
            let detectors = plugin.semantics().framework_detectors.as_slice();
            (!detectors.is_empty()).then_some((f.id, detectors))
        })
        .collect();
    let candidate_ids: HashSet<i64> = refs
        .iter()
        .filter_map(|r| {
            let file_id = r.file_id?;
            let detectors = detectors_by_file.get(&file_id)?;
            detectors
                .iter()
                .any(|d| matches_detector(&r.target, d))
                .then_some(file_id)
        })
        .collect();

    // The fast path every plain library takes: nothing to promote, nothing
    // promoted before — the pass costs one in-memory scan and exits.
    if candidate_ids.is_empty() && existing.is_empty() {
        return Ok((
            FrameworkStats {
                duration_ms: elapsed_ms(started),
                ..FrameworkStats::default()
            },
            Vec::new(),
        ));
    }

    let path_by_id: HashMap<i64, &str> = files.iter().map(|f| (f.id, f.path.as_str())).collect();
    let mut candidates: Vec<(i64, &str)> = candidate_ids
        .iter()
        .filter_map(|id| path_by_id.get(id).map(|p| (*id, *p)))
        .collect();
    candidates.sort(); // deterministic scan order (NFR-RA-06)

    // Parse the candidates on the shared worker pool, one Parser per worker
    // (the extraction pattern, AR-05). A file that vanished from disk or no
    // longer parses simply contributes no matches — best-effort (FR-FW-04).
    let scanned: Vec<(i64, &str, FileMatches)> = runtime.worker_pool().install(|| {
        candidates
            .par_iter()
            .map_init(Parser::new, |parser, (file_id, rel)| {
                let matches = scan_path(parser, registry, root, rel);
                (*file_id, *rel, matches)
            })
            .collect()
    });

    let index = binder::Index::build(&nodes, &edges, &refs);
    let desired = desired_set(&scanned, &nodes, &edges, &files, &index, policy);

    let routes = desired
        .values()
        .filter(|d| d.kind == NodeKind::Route)
        .count() as u64;
    let components = desired
        .values()
        .filter(|d| d.kind == NodeKind::Component)
        .count() as u64;

    // CR-017 / S-080: the names of route/component nodes this run *newly* promotes
    // (in the desired set, absent from the reconcile baseline). The pipeline
    // re-resolves the cross-artifact references that target them — an OpenAPI
    // `ApiOperation`'s route reference, captured at extraction but unbindable
    // until its route node existed — on the same run, because this pass creates
    // those nodes *after* the resolution pass already ran ([FR-CG-09], [FR-RS-03]).
    let existing_symbols: HashSet<&str> = existing.iter().map(|n| n.symbol.as_str()).collect();
    let newly_promoted: Vec<String> = desired
        .iter()
        .filter(|(symbol, _)| !existing_symbols.contains(symbol.as_str()))
        .map(|(_, node)| node.name.clone())
        .collect();

    commit(runtime, &existing, &edges, desired)?;
    Ok((
        FrameworkStats {
            files_scanned: candidates.len() as u64,
            routes,
            components,
            duration_ms: elapsed_ms(started),
        },
        newly_promoted,
    ))
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

/// The deduplicated union of every loaded plugin's `framework_detectors` — the
/// canonical `::`-joined detector prefixes the incremental gate
/// ([`crate::graph_store::GraphStore::has_framework_footprint`]) tests the ledger
/// against. Empty when no loaded grammar declares a framework (a plain repo), in
/// which case the gate falls back to the promoted-node check alone.
fn detector_prefixes(registry: &LanguageRegistry) -> Vec<String> {
    let mut prefixes: Vec<String> = registry
        .iter()
        .flat_map(|plugin| plugin.semantics().framework_detectors.iter().cloned())
        .collect();
    prefixes.sort();
    prefixes.dedup();
    prefixes
}

/// `true` when a ledger `target` (canonical `::`-joined) falls under a
/// descriptor detector prefix: the target *is* the detector, or extends it by
/// whole segments (`axum::routing::get` under `axum`; never `axumish` under
/// `axum`).
fn matches_detector(target: &str, detector: &str) -> bool {
    target
        .strip_prefix(detector)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
}

// ── Scanning (pure tree-sitter, per file) ────────────────────────────────────

/// Read and scan one candidate file from disk; empty matches on any miss.
fn scan_path(
    parser: &mut Parser,
    registry: &LanguageRegistry,
    root: &Path,
    rel: &str,
) -> FileMatches {
    // Defence-in-depth (NFR-SE-04): stored paths are validated relative at
    // insert time, but `Path::join` would silently *replace* the root with an
    // absolute `rel` — refuse to read outside the engine root even from a
    // corrupted store.
    if Path::new(rel).is_absolute() {
        return FileMatches::default();
    }
    let Some(plugin) = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|ext| registry.for_extension(ext))
    else {
        return FileMatches::default();
    };
    let Ok(source) = fs::read_to_string(root.join(rel)) else {
        return FileMatches::default(); // gone or unreadable — best-effort
    };
    scan_source(parser, plugin, &source)
}

/// Scan one file's source for framework anchors. Pure — no store, no disk —
/// and therefore the unit-testable core of the pass.
///
/// Two capture dialects coexist (S-015):
///
/// - the **declarative contract** — a pattern that captures the registration
///   *parts* directly (`@fw.route.path`, `@fw.route.method`, optionally
///   `@fw.route.handler`; `@fw.component.name`) is interpreted per-match,
///   generically: the captured method text maps through the descriptor's
///   `[framework_methods]` table, the path literal is unquoted, the handler
///   text is canonicalised. This is the dialect every S-015 language uses, and
///   what makes a new language's framework rules pure data ([NFR-MA-01]);
/// - the **legacy Rust anchors** (`@fw.route`, `@fw.attr`, `@fw.param`) — kept
///   for the Rust query because Axum's recursive method-router chains and
///   Actix's attribute/builder shapes need a structural walk no per-part
///   capture can express.
///
/// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
fn scan_source(parser: &mut Parser, plugin: &dyn LanguagePlugin, source: &str) -> FileMatches {
    let mut out = FileMatches::default();
    let Some(query) = plugin.query("frameworks") else {
        return out; // a grammar without the capability promotes nothing
    };
    if parser.set_language(plugin.language()).is_err() {
        return out;
    }
    let Some(tree) = parser.parse(source, None) else {
        return out;
    };

    let src = source.as_bytes();
    let methods = &plugin.semantics().framework_methods;
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), src);
    // A node can anchor at most one legacy promotion; guard against
    // overlapping query patterns capturing it twice.
    let mut seen: HashSet<usize> = HashSet::new();
    while let Some(m) = matches.next() {
        if generic_match(m.captures, capture_names, src, methods, &mut out) {
            continue; // consumed by the declarative contract
        }
        for cap in m.captures {
            if !seen.insert(cap.node.id()) {
                continue;
            }
            match capture_names[cap.index as usize] {
                "fw.route" => out.routes.extend(route_registrations(cap.node, src)),
                "fw.attr" => out.routes.extend(attribute_route(cap.node, src)),
                "fw.param" => out.components.extend(state_component(cap.node, src)),
                _ => {}
            }
        }
    }
    // Overlapping declarative patterns (e.g. a handler-bearing and a
    // handler-less variant of the same registration shape) may both match one
    // site: collapse to one match per (method, path), preferring the one that
    // names a handler — deterministically, whatever the pattern order.
    dedup_routes(&mut out.routes);
    out
}

/// Interpret one query match under the declarative capture contract; `true`
/// when the match used it (even if nothing was promoted — a method text the
/// descriptor does not map is a dropped candidate, not a legacy anchor).
fn generic_match(
    captures: &[tree_sitter::QueryCapture<'_>],
    capture_names: &[&str],
    src: &[u8],
    methods: &std::collections::BTreeMap<String, String>,
    out: &mut FileMatches,
) -> bool {
    let mut path_node: Option<Node<'_>> = None;
    let mut method_node: Option<Node<'_>> = None;
    let mut handler_node: Option<Node<'_>> = None;
    let mut component_node: Option<Node<'_>> = None;
    let (mut start_line, mut end_line) = (u32::MAX, 0u32);
    let mut generic = false;

    for cap in captures {
        let name = capture_names[cap.index as usize];
        if !name.starts_with("fw.route.") && !name.starts_with("fw.component.") {
            continue;
        }
        generic = true;
        start_line = start_line.min(cap.node.start_position().row as u32 + 1);
        end_line = end_line.max(cap.node.end_position().row as u32 + 1);
        let slot = match name {
            "fw.route.path" => &mut path_node,
            "fw.route.method" => &mut method_node,
            "fw.route.handler" => &mut handler_node,
            "fw.component.name" => &mut component_node,
            // Auxiliary captures (`fw.component.base`, …) exist only for the
            // query's own `#match?`/`#any-of?` predicates.
            _ => continue,
        };
        slot.get_or_insert(cap.node);
    }
    if !generic {
        return false;
    }

    if let Some(name) = component_node {
        out.components.push(ComponentMatch {
            type_path: text(name, src).trim().to_string(),
        });
    }
    if let (Some(path), Some(method)) = (path_node, method_node) {
        // The descriptor's [framework_methods] table is both the method
        // normaliser and the recognised-registration filter: unmapped text
        // (`app.set(…)`, an unknown annotation) promotes nothing (FR-FW-04).
        if let Some(mapped) = methods.get(text(method, src).trim()) {
            // The handler must be a plain (possibly qualified) name to bind;
            // canonicalise its separators the same way extraction does.
            let handler = handler_node.and_then(|h| {
                let segments = crate::extract::refs::split_path_text(text(h, src));
                (!segments.is_empty()).then(|| segments.join("::"))
            });
            out.routes.push(RouteMatch {
                path: crate::extract::refs::unquote(text(path, src)).to_string(),
                method: mapped.clone(),
                handler,
                start_line,
                end_line,
            });
        }
    }
    true
}

/// Collapse duplicate `(method, path)` route matches within one file, keeping
/// the first — upgraded to the first *handler-bearing* one when a later
/// overlapping pattern proves a handler the earlier could not.
fn dedup_routes(routes: &mut Vec<RouteMatch>) {
    let mut index_of: HashMap<(String, String), usize> = HashMap::new();
    let mut kept: Vec<RouteMatch> = Vec::new();
    for route in routes.drain(..) {
        match index_of.entry((route.method.clone(), route.path.clone())) {
            std::collections::hash_map::Entry::Occupied(slot) => {
                let existing = &mut kept[*slot.get()];
                if existing.handler.is_none() && route.handler.is_some() {
                    *existing = route;
                }
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(kept.len());
                kept.push(route);
            }
        }
    }
    *routes = kept;
}

/// The text of a tree-sitter node, or `""` on a non-UTF-8 slice.
fn text<'s>(node: Node<'_>, src: &'s [u8]) -> &'s str {
    node.utf8_text(src).unwrap_or("")
}

/// `true` for both Rust string-literal node kinds (`"…"` and `r"…"`).
fn is_string_literal(node: Node<'_>) -> bool {
    matches!(node.kind(), "string_literal" | "raw_string_literal")
}

/// The content of a (raw) string-literal node (quotes and raw-string hashes
/// stripped) — the `string_content` child, or `""` for an empty literal.
fn string_text<'s>(node: Node<'_>, src: &'s [u8]) -> &'s str {
    let mut walk = node.walk();
    let content = node
        .named_children(&mut walk)
        .find(|c| c.kind() == "string_content");
    content.map(|c| text(c, src)).unwrap_or("")
}

/// The last `::`-segment of a path-ish text (`web::get` → `get`).
fn last_segment(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path).trim()
}

/// Matches from one `.route("…", <expr>)` anchor: the field name must be
/// `route`, the first argument is the path, and the second argument is walked
/// as a method-router chain (Axum) or a `web::method().to(h)` chain (Actix).
fn route_registrations(call: Node<'_>, src: &[u8]) -> Vec<RouteMatch> {
    let Some(function) = call.child_by_field_name("function") else {
        return Vec::new();
    };
    let field = function
        .child_by_field_name("field")
        .map(|f| text(f, src))
        .unwrap_or("");
    if field != "route" {
        return Vec::new();
    }
    let Some(args) = call.child_by_field_name("arguments") else {
        return Vec::new();
    };
    let mut walk = args.walk();
    let named: Vec<Node<'_>> = args.named_children(&mut walk).collect();
    let (Some(path_node), Some(router)) = (named.first(), named.get(1)) else {
        return Vec::new();
    };
    if !is_string_literal(*path_node) {
        return Vec::new();
    }
    let path = string_text(*path_node, src);

    router_targets(*router, src)
        .into_iter()
        .map(|(method, handler)| RouteMatch {
            path: path.to_string(),
            method,
            handler,
            start_line: call.start_position().row as u32 + 1,
            end_line: call.end_position().row as u32 + 1,
        })
        .collect()
}

/// Walk a method-router expression, collecting `(METHOD, handler)` pairs.
///
/// Recognised shapes (recursing left through the chain):
/// - `get(h)` / `axum::routing::get(h)` — an Axum method router;
/// - `get(a).post(b)` — a chained Axum router (both pairs);
/// - `web::get().to(h)` — an Actix route builder.
///
/// Anything else contributes nothing — heuristic and best-effort (FR-FW-04).
fn router_targets(node: Node<'_>, src: &[u8]) -> Vec<(String, Option<String>)> {
    if node.kind() != "call_expression" {
        return Vec::new();
    }
    let (Some(function), Some(args)) = (
        node.child_by_field_name("function"),
        node.child_by_field_name("arguments"),
    ) else {
        return Vec::new();
    };

    match function.kind() {
        // The chain head: `get(h)` or `axum::routing::get(h)`.
        "identifier" | "scoped_identifier" => {
            let name = last_segment(text(function, src));
            if HTTP_METHODS.contains(&name) {
                vec![(name.to_uppercase(), first_path_arg(args, src))]
            } else {
                Vec::new()
            }
        }
        // A chained link: `<receiver>.post(b)` or `<receiver>.to(h)`.
        "field_expression" => {
            let field = function
                .child_by_field_name("field")
                .map(|f| text(f, src))
                .unwrap_or("");
            let Some(receiver) = function.child_by_field_name("value") else {
                return Vec::new();
            };
            if HTTP_METHODS.contains(&field) {
                let mut targets = router_targets(receiver, src);
                targets.push((field.to_uppercase(), first_path_arg(args, src)));
                targets
            } else if field == "to" {
                // Actix: the method comes from the receiver (`web::get()`).
                match chain_method(receiver, src) {
                    Some(method) => vec![(method, first_path_arg(args, src))],
                    None => Vec::new(),
                }
            } else {
                // An unknown link (`.fallback(x)`, …): salvage the chain left
                // of it, drop the link itself.
                router_targets(receiver, src)
            }
        }
        _ => Vec::new(),
    }
}

/// The upper-cased method of an Actix builder head (`web::get()` → `GET`).
fn chain_method(node: Node<'_>, src: &[u8]) -> Option<String> {
    if node.kind() != "call_expression" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    if !matches!(function.kind(), "identifier" | "scoped_identifier") {
        return None;
    }
    let name = last_segment(text(function, src));
    HTTP_METHODS.contains(&name).then(|| name.to_uppercase())
}

/// The first argument when it is a plain Rust path (`handler`,
/// `handlers::list`); `None` for closures and other expressions.
fn first_path_arg(args: Node<'_>, src: &[u8]) -> Option<String> {
    let mut walk = args.walk();
    let first = args.named_children(&mut walk).next()?;
    matches!(first.kind(), "identifier" | "scoped_identifier").then(|| text(first, src).to_string())
}

/// A route from one Actix method-attribute anchor (`#[get("/p")]` on a
/// following `fn`): the attribute name must be an HTTP method and the
/// attributed item a `function_item` — whose name is the handler.
fn attribute_route(attr_item: Node<'_>, src: &[u8]) -> Option<RouteMatch> {
    let mut walk = attr_item.walk();
    let attribute = attr_item
        .named_children(&mut walk)
        .find(|c| c.kind() == "attribute")?;
    let method = {
        let mut aw = attribute.walk();
        let ident = attribute
            .named_children(&mut aw)
            .find(|c| c.kind() == "identifier")?;
        let name = text(ident, src);
        if !HTTP_METHODS.contains(&name) {
            return None;
        }
        name.to_uppercase()
    };
    let path = {
        let token_tree = attribute.child_by_field_name("arguments")?;
        let mut tw = token_tree.walk();
        let lit = token_tree
            .named_children(&mut tw)
            .find(|c| is_string_literal(*c))?;
        string_text(lit, src).to_string()
    };

    // The attributed function: the next named sibling past any further
    // attributes or doc comments.
    let mut sibling = attr_item.next_named_sibling();
    while let Some(n) = sibling {
        match n.kind() {
            "attribute_item" | "line_comment" | "block_comment" => {
                sibling = n.next_named_sibling();
            }
            "function_item" => {
                let handler = n
                    .child_by_field_name("name")
                    .map(|name| text(name, src).to_string())?;
                return Some(RouteMatch {
                    path,
                    method,
                    handler: Some(handler),
                    start_line: attr_item.start_position().row as u32 + 1,
                    end_line: n.end_position().row as u32 + 1,
                });
            }
            _ => return None, // attributed item is not a function
        }
    }
    None
}

/// A component candidate from one generic-typed parameter anchor: the generic
/// head must be a state extractor (`State`/`Data`), and the first type
/// argument (transparently unwrapping `Arc`/`Rc`/`Box`) is the state type.
fn state_component(param: Node<'_>, src: &[u8]) -> Option<ComponentMatch> {
    let generic = param.child_by_field_name("type")?;
    if generic.kind() != "generic_type" {
        return None;
    }
    let head = generic.child_by_field_name("type")?;
    let head_name = last_segment(text(head, src));
    if !STATE_EXTRACTORS.contains(&head_name) {
        return None;
    }
    let inner = unwrap_state_type(generic, src)?;
    Some(ComponentMatch {
        type_path: text(inner, src).to_string(),
    })
}

/// The first type argument of `generic`, unwrapping transparent smart-pointer
/// layers, when it is a plain (possibly scoped) type path.
fn unwrap_state_type<'t>(generic: Node<'t>, src: &[u8]) -> Option<Node<'t>> {
    let mut current = first_type_argument(generic)?;
    // Bounded by nesting depth; 4 covers `State<Arc<Box<Rc<T>>>>` and beyond
    // lies nothing a v1 heuristic should chase.
    for _ in 0..4 {
        match current.kind() {
            "type_identifier" | "scoped_type_identifier" => return Some(current),
            "generic_type" => {
                // Only smart-pointer layers are transparent: `State<Arc<T>>`
                // promotes `T`, but `State<Option<T>>` (or any other generic)
                // is not a state type this heuristic understands.
                let head = current.child_by_field_name("type")?;
                if !TRANSPARENT_WRAPPERS.contains(&last_segment(text(head, src))) {
                    return None;
                }
                current = first_type_argument(current)?;
            }
            _ => return None,
        }
    }
    None
}

/// The first named node inside `generic`'s `type_arguments`, if any.
fn first_type_argument(generic: Node<'_>) -> Option<Node<'_>> {
    let args = generic.child_by_field_name("type_arguments")?;
    let mut walk = args.walk();
    let first = args.named_children(&mut walk).next();
    first
}

// ── Desired-set assembly (binding + symbols) ─────────────────────────────────

/// One promoted node the graph *should* contain after this run, keyed by its
/// canonical symbol string.
#[derive(Debug)]
struct DesiredNode {
    symbol: crate::model::LogosSymbol,
    kind: NodeKind,
    name: String,
    file_id: i64,
    start_line: Option<i64>,
    end_line: Option<i64>,
    /// Edges this node must carry, as `(source-is-self?, other endpoint, kind)`
    /// — resolved to ids at commit time.
    edges: Vec<DesiredEdge>,
}

/// One edge a promoted node must carry after this run.
#[derive(Debug, Clone, Copy)]
enum DesiredEdge {
    /// `file-module --Contains--> promoted` (scope anchoring).
    ContainedBy(NodeId),
    /// `promoted --RoutesTo--> handler` ([FR-FW-01]).
    RoutesTo(NodeId),
    /// `promoted --References--> state type` ([FR-FW-02]).
    References(NodeId),
}

/// Assemble the desired promoted set from the scanned matches: bind handler
/// and state-type texts through the [`binder`] (exactly-one-or-nothing,
/// [NFR-RA-05]) and build each promoted node's canonical symbol (ADR-07).
fn desired_set(
    scanned: &[(i64, &str, FileMatches)],
    nodes: &[NodeRow],
    edges: &[crate::graph_store::EdgeRow],
    files: &[crate::graph_store::FileRecord],
    index: &binder::Index,
    policy: BindingPolicy,
) -> HashMap<String, DesiredNode> {
    let ctx = SymbolContext::default();

    // Same-file declarations by name — the file-local binding fallback's
    // candidate index (see [`bind_text`]).
    let mut by_file_name: HashMap<(&str, &str), Vec<(NodeId, NodeKind)>> = HashMap::new();
    for n in nodes {
        if let Some(path) = n.file_path.as_deref() {
            by_file_name
                .entry((path, n.name.as_str()))
                .or_default()
                .push((n.id, n.kind));
        }
    }

    // File-module nodes: parentless Module rows, keyed by their file path.
    let contained: HashSet<NodeId> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Contains)
        .map(|e| e.target)
        .collect();
    let module_by_path: HashMap<&str, &NodeRow> = nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Module && !contained.contains(&n.id))
        .filter_map(|n| n.file_path.as_deref().map(|p| (p, n)))
        .collect();
    let node_by_id: HashMap<NodeId, &NodeRow> = nodes.iter().map(|n| (n.id, n)).collect();
    let file_id_by_path: HashMap<&str, i64> =
        files.iter().map(|f| (f.path.as_str(), f.id)).collect();

    let mut desired: HashMap<String, DesiredNode> = HashMap::new();

    for (file_id, rel, matches) in scanned {
        let Some(module) = module_by_path.get(rel) else {
            continue; // no file-module anchor (symbol build failed at extract)
        };
        // The same segment rule extraction uses — symbol namespaces must
        // never drift between the two passes (ADR-07).
        let segments = path_segments(rel);

        for route in &matches.routes {
            let name = format!("{} {}", route.method, route.path);
            let chain = [
                descriptor_for(NodeKind::Module, "route", 0),
                descriptor_for(NodeKind::Route, &name, 0),
            ];
            let Ok(symbol) = build_symbol(&ctx, &segments, &chain) else {
                continue;
            };
            let mut node_edges = vec![DesiredEdge::ContainedBy(module.id)];
            if let Some(handler) = &route.handler {
                if let Some(target) = bind_text(
                    index,
                    module,
                    *file_id,
                    handler,
                    EdgeKind::Calls,
                    policy,
                    rel,
                    &by_file_name,
                ) {
                    node_edges.push(DesiredEdge::RoutesTo(target));
                }
            }
            // One route node per (file, METHOD, path) symbol: a duplicate
            // registration deduplicates first-wins, deterministically (the
            // scan order is sorted and per-file matches follow AST order).
            desired
                .entry(symbol.as_str().to_string())
                .or_insert_with(|| DesiredNode {
                    symbol,
                    kind: NodeKind::Route,
                    name,
                    file_id: *file_id,
                    start_line: Some(i64::from(route.start_line)),
                    end_line: Some(i64::from(route.end_line)),
                    edges: node_edges,
                });
        }

        for component in &matches.components {
            // Bind the written state type to an indexed type node — or skip:
            // a component is only promoted over a type we actually know
            // (NFR-RA-05; FR-FW-04 keeps externals out).
            let Some(target) = bind_text(
                index,
                module,
                *file_id,
                &component.type_path,
                EdgeKind::References,
                policy,
                rel,
                &by_file_name,
            ) else {
                continue;
            };
            let Some(type_node) = node_by_id.get(&target) else {
                continue;
            };
            if !is_component_target(type_node.kind) {
                continue;
            }
            // The component anchors at the *type's* file, so two usage sites
            // promote one component and re-extraction of the type's file
            // invalidates it naturally.
            let Some(type_path) = type_node.file_path.as_deref() else {
                continue;
            };
            let Some(&type_file_id) = file_id_by_path.get(type_path) else {
                continue;
            };
            let type_segments: Vec<&str> = type_path
                .split('/')
                .filter(|s| !s.is_empty() && *s != ".")
                .collect();
            let chain = [
                descriptor_for(NodeKind::Module, "component", 0),
                descriptor_for(NodeKind::Component, &type_node.name, 0),
            ];
            let Ok(symbol) = build_symbol(&ctx, &type_segments, &chain) else {
                continue;
            };
            let mut node_edges = vec![DesiredEdge::References(target)];
            if let Some(type_module) = module_by_path.get(type_path) {
                node_edges.push(DesiredEdge::ContainedBy(type_module.id));
            }
            desired
                .entry(symbol.as_str().to_string())
                .or_insert_with(|| DesiredNode {
                    symbol,
                    kind: NodeKind::Component,
                    name: type_node.name.clone(),
                    file_id: type_file_id,
                    start_line: None,
                    end_line: None,
                    edges: node_edges,
                });
        }
    }
    desired
}

/// Node kinds a component promotion may target: the type-like kinds (a Rust
/// `State<T>`/`Data<T>` state type, a Spring stereotype class, a Django
/// model) plus the callable kinds, because a Next.js/React component *is* an
/// exported function ([FR-FW-02], S-015).
///
/// [FR-FW-02]: ../../../docs/specs/requirements/FR-FW-02.md
fn is_component_target(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Struct
            | NodeKind::Enum
            | NodeKind::Class
            | NodeKind::TypeAlias
            | NodeKind::Interface
            | NodeKind::Function
            | NodeKind::Method
    )
}

/// Bind a written path text against the snapshot index from a file-module
/// scope, via the same [`binder`] the resolution pass uses. `kind` selects the
/// binder's target filter (`Calls` → callables, anything else → any node).
///
/// When the scope hierarchy cannot see the target, a **file-local fallback**
/// (S-015) applies: a class-nested handler (a Java `@GetMapping` method, a TS
/// class method) is invisible from the file-module scope a synthetic ref binds
/// from, because bare names do not collapse class members to module scope.
/// The registering file is where such a handler lives, so the fallback accepts
/// exactly one same-named, kind-admissible declaration in `rel` — the
/// [NFR-RA-05] exactly-one-or-nothing rule, narrowed to one file. Dotted paths
/// had their full scope-hierarchy chance and get no second one.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[allow(clippy::too_many_arguments)]
fn bind_text(
    index: &binder::Index,
    module: &NodeRow,
    file_id: i64,
    target: &str,
    kind: EdgeKind,
    policy: BindingPolicy,
    rel: &str,
    by_file_name: &HashMap<(&str, &str), Vec<(NodeId, NodeKind)>>,
) -> Option<NodeId> {
    let synthetic = UnresolvedRefRow {
        id: 0,
        file_id: Some(file_id),
        source_symbol: module.symbol.as_str().to_string(),
        target: target.to_string(),
        alias: None,
        form: RefForm::Path,
        kind,
        line: None,
        resolved: false,
        payload: None,
    };
    match binder::bind(&synthetic, index, policy) {
        binder::Outcome::Bound { target, .. } => Some(target),
        // Framework refs are never cross-artifact module calls, so the multi-target
        // outcome cannot arise here; treat it as no single target defensively.
        binder::Outcome::BoundMany { .. } => None,
        binder::Outcome::Unbound => {
            if target.contains("::") {
                return None;
            }
            let admits = |k: NodeKind| {
                if kind == EdgeKind::Calls {
                    matches!(k, NodeKind::Function | NodeKind::Method)
                } else {
                    is_component_target(k)
                }
            };
            let candidates: Vec<NodeId> = by_file_name
                .get(&(rel, target))
                .map(|nodes| {
                    nodes
                        .iter()
                        .filter(|(_, k)| admits(*k))
                        .map(|(id, _)| *id)
                        .collect()
                })
                .unwrap_or_default();
            match candidates.as_slice() {
                [one] => Some(*one),
                _ => None, // zero or ambiguous — never fabricate
            }
        }
    }
}

// ── Commit (serial, one writer batch) ────────────────────────────────────────

/// `true` for an edge kind the framework-promotion pass *owns* and therefore
/// reconciles around its promoted nodes: the `Contains` anchoring, the
/// [`EdgeKind::RoutesTo`] handler link ([FR-FW-01]), and the
/// [`EdgeKind::References`] component link ([FR-FW-02]). Every other kind incident
/// to a promoted node is owned by a different pass — the resolution engine's
/// `ArtifactBinding`/`ArtifactRef` (CR-011), say — and must be left untouched, so
/// this pass never deletes an edge it did not create.
///
/// [FR-FW-01]: ../../../docs/specs/requirements/FR-FW-01.md
/// [FR-FW-02]: ../../../docs/specs/requirements/FR-FW-02.md
fn is_framework_owned(kind: EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::Contains | EdgeKind::RoutesTo | EdgeKind::References
    )
}

/// Reconcile the graph's promoted nodes to `desired` in one writer batch:
/// delete stale nodes, insert missing ones (id-stable for survivors), and
/// re-prove every promoted edge.
fn commit(
    runtime: &Runtime,
    existing: &[&NodeRow],
    edges: &[crate::graph_store::EdgeRow],
    desired: HashMap<String, DesiredNode>,
) -> Result<()> {
    let existing_by_symbol: HashMap<&str, NodeId> =
        existing.iter().map(|n| (n.symbol.as_str(), n.id)).collect();

    // Stale promoted nodes: in the graph, not in the desired set.
    let stale: Vec<NodeId> = existing
        .iter()
        .filter(|n| !desired.contains_key(n.symbol.as_str()))
        .map(|n| n.id)
        .collect();

    // Edges currently incident to *surviving* promoted nodes — candidates for
    // edge-level reconciliation. (Edges incident to stale nodes are cascaded
    // away by the node delete in step 1; any of them also caught here merely
    // produce a harmless 0-row delete.)
    //
    // Restricted to the edge kinds this pass *owns* ([`is_framework_owned`]):
    // `Contains`/`RoutesTo`/`References`. A foreign edge another pass created and
    // owns — notably the resolution engine's `ArtifactBinding` from an OpenAPI
    // `ApiOperation` to a route handler (S-069, CR-011) — is incident to a route
    // node but is **not** in this pass's desired `want` set, so reconciling over
    // it would delete the binding the resolution pass just proved. Scoping the
    // candidate set to the framework's own kinds leaves foreign edges to their
    // owning pass (resolution flips them via the ledger; a node delete cascades
    // them) ([NFR-RA-05], the never-clobber companion of never-fabricate).
    let surviving: HashSet<NodeId> = existing
        .iter()
        .filter(|n| desired.contains_key(n.symbol.as_str()))
        .map(|n| n.id)
        .collect();
    let current_edges: Vec<(NodeId, NodeId, EdgeKind)> = edges
        .iter()
        .filter(|e| is_framework_owned(e.kind))
        .filter(|e| surviving.contains(&e.source) || surviving.contains(&e.target))
        .map(|e| (e.source, e.target, e.kind))
        .collect();

    // The owned work list moved into the writer closure (it runs on the
    // writer thread), with each entry's existing id resolved up front; sorted
    // on the symbol string so the commit order is deterministic (NFR-RA-06).
    let mut plan: Vec<(Option<NodeId>, DesiredNode)> = desired
        .into_values()
        .map(|d| (existing_by_symbol.get(d.symbol.as_str()).copied(), d))
        .collect();
    plan.sort_by(|(_, a), (_, b)| a.symbol.as_str().cmp(b.symbol.as_str()));

    runtime.submit_write(move |w| {
        // 1) Retire stale promoted nodes (their edges cascade).
        for id in &stale {
            w.delete_node(*id)?;
        }

        // 2) Ensure every desired node exists, collecting its id.
        let mut id_of: Vec<NodeId> = Vec::with_capacity(plan.len());
        for (existing_id, item) in &plan {
            let id = match existing_id {
                Some(id) => *id,
                None => {
                    let symbol_id = w.upsert_symbol(&item.symbol)?;
                    w.insert_node(&NewNode {
                        file_id: Some(item.file_id),
                        start_line: item.start_line,
                        end_line: item.end_line,
                        ..NewNode::plain(symbol_id, item.kind, &item.name)
                    })?
                }
            };
            id_of.push(id);
        }

        // 3) Edge reconciliation: the full desired edge set, with self ids
        //    resolved.
        let mut want: HashSet<(NodeId, NodeId, EdgeKind)> = HashSet::new();
        for ((_, item), self_id) in plan.iter().zip(&id_of) {
            for e in &item.edges {
                want.insert(match e {
                    DesiredEdge::ContainedBy(module) => (*module, *self_id, EdgeKind::Contains),
                    DesiredEdge::RoutesTo(handler) => (*self_id, *handler, EdgeKind::RoutesTo),
                    DesiredEdge::References(ty) => (*self_id, *ty, EdgeKind::References),
                });
            }
        }
        // Stale edges on surviving promoted nodes (e.g. a handler binding
        // that is no longer provable, NFR-RA-05).
        for (source, target, kind) in &current_edges {
            if !want.contains(&(*source, *target, *kind)) {
                w.delete_edge(*source, *target, *kind)?;
            }
        }
        // Missing desired edges (idempotent for the survivors').
        let mut ordered: Vec<&(NodeId, NodeId, EdgeKind)> = want.iter().collect();
        ordered.sort_by_key(|(s, t, k)| (*s, *t, k.as_i32()));
        for (source, target, kind) in ordered {
            w.insert_edge_if_absent(*source, *target, *kind)?;
        }
        Ok(())
    })
}

#[cfg(test)]
#[cfg(feature = "lang-rust")]
mod tests;
