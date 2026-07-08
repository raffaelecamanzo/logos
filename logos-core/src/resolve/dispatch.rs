//! The framework-dispatch live-rooting pass — Pass 2¾ of the pipeline
//! ([resolution-engine], [CR-043], [ADR-39], [FR-RS-03]).
//!
//! Runs **after** resolution and the framework-promotion pass on every
//! index/sync. It recognises the **indirect call forms whose callers the binder
//! cannot bind** — methods dispatched not by a source-visible call edge but by
//! an external framework — and live-roots them so the dead-code reachability
//! pass ([FR-AN-01], [annotation-engine]) no longer mis-reports them dead.
//!
//! Three Rust dispatch shapes are recognised (Phases 1 & 3, [CR-043] §3.2 B,
//! [CR-068] §3.2):
//!
//! - **framework trait-impl dispatch** — a method declared in an
//!   `impl Trait for Type` block. The framework (`tracing`'s `Layer`/`Visit`,
//!   rmcp's `ServerHandler`, `Drop`, …) invokes it through the trait object /
//!   vtable; there is no source-visible call site, so the method falls out of
//!   the reachable set and is reported dead (`on_event`/`record_str` in
//!   `observability/layer.rs`).
//! - **closure-argument tool dispatch** — a method carrying a dispatch
//!   attribute ([`RUST_DISPATCH_ATTRS`], rmcp's `#[tool]`). The attribute macro
//!   generates the router that dispatches it; the body is a
//!   `self.run_result("x", |e| e.x())` closure delegator, and the method itself
//!   has no source-visible caller (`session_end`/`coverage_ingest`/
//!   `wiki_status`/`rescan` in `mcp/src/server.rs`).
//! - **function-pointer handoff** (S-276, [CR-068]) — a callable handed to an
//!   axum router/middleware builder **by value**, never at a source-visible call
//!   site: the ratified Phase-3 set is `.fallback(h)`, `middleware::from_fn(h)`,
//!   `from_fn_with_state(state, h)`, and every method-router handler inside a
//!   `route(path, get(h)|post(h)|…)` registration, including chained setters
//!   (`get(a).post(b)` roots both). The handler is referenced only as a
//!   function pointer, so the binder proves no `Calls` edge and the reachability
//!   pass mis-reports it dead — the `spa_fallback`/`intent_guard`/`method_guard`/
//!   `host_guard`/`csp_headers` shape in `web/src/lib.rs`. Recognition resolves
//!   the handler name to the **one same-file callable** carrying it and live-roots
//!   *that* node. The registering file is where a wired guard/fallback lives, so a
//!   same-file exactly-one-or-nothing bind ([NFR-RA-05]) keeps recognition
//!   per-file pure (the wiring site and the declaration are always in the one
//!   re-scanned file) and never fabricates: a handler that is cross-file
//!   (`get(api_v1::overview)`) or ambiguous within the file is left to the
//!   framework-promotion pass ([`super::framework`]) / ordinary binding.
//!
//! # The live-root marker — a self-[`RoutesTo`](EdgeKind::RoutesTo) edge
//!
//! For each recognised method the pass persists a single **`RoutesTo`
//! self-edge** (`method --RoutesTo--> method`). A self-edge is a shape no
//! genuine framework route ever produces (a [`NodeKind::Route`] node routes to
//! a *distinct* handler), so it is an unambiguous, surface-free marker the
//! dead-code pass reads as "this callable is a live framework entry point"
//! ([`crate::annotate`] `live_set`). The marker is a plain (non-derived) edge
//! deliberately: the annotation pass clears *derived* edges every run
//! (`clear_derived`) immediately after `live_set` has consumed them, so a
//! derived marker would not survive to the next sync of an unrelated file
//! (whose own dispatch scan does not re-add it); a plain edge persists and is
//! reconciled by this pass alone. It pollutes neither the route surface
//! ([`NodeKind::Route`] listings) nor the `ApiOperation`→route binding key map,
//! and adds no node and no schema column.
//!
//! # Never fabricate, false-live biased ([NFR-RA-05], [AR-05])
//!
//! The marker roots a **real, indexed** method node; no edge between two
//! distinct nodes is invented, so the never-fabricate rule holds. Recognition is
//! deliberately a *superset* of "external frameworks" — every trait-impl method
//! is rooted, not only those of external traits — keeping the failure mode
//! biased toward **false-live** (a missed dead flag), never false-dead, exactly
//! the [AR-05] honesty posture. A locally-dispatched trait method that is also
//! genuinely dead is the accepted cost of that bias.
//!
//! # Per-file, reconciled, incremental-safe ([NFR-RA-06], [NFR-PE-03])
//!
//! A method's marker is a pure function of **its own file's syntax** — whether
//! it sits in a trait impl, or carries a dispatch attribute — with no
//! dependency on the rest of the graph. A function-pointer handoff marker is
//! likewise per-file pure: it is planted on the handler only from a **same-file**
//! handoff site that resolves to it uniquely, so the wiring site and the
//! declaration are always in the one re-scanned file. So a full index scans every `.rs` file
//! and reconciles the whole marker set; an incremental sync scans only the
//! changed `.rs` files and reconciles only *their* nodes' markers, and the two
//! produce a byte-identical marker set (every untouched file's markers provably
//! cannot have changed, and persist because the marker is non-derived). A sync
//! that changed no Rust file does no work at all.
//!
//! [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
//! [annotation-engine]: ../../../docs/specs/architecture/components/annotation-engine.md
//! [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md
//! [CR-068]: ../../../docs/requests/CR-068-reachability-binding-precision.md
//! [ADR-39]: ../../../docs/specs/architecture/decisions/ADR-39.md
//! [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
//! [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
//! [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
//! [AR-05]: ../../../docs/specs/architecture.md#13-risk-register

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use rayon::prelude::*;
use tree_sitter::{Node, Parser};

use crate::model::{EdgeKind, NodeId, NodeKind};
use crate::models::pipeline::DispatchStats;
use crate::plugin::LanguageRegistry;
use crate::runtime::Runtime;

/// The file extension whose dispatch shapes this pass recognises. Phase 1 is
/// Rust-only ([CR-043]; other languages render `is_dead = NULL` until their
/// binder coverage is proven, [ADR-39]/S-159); the structural walk below is
/// written against the Rust grammar's node kinds.
const RUST_EXT: &str = "rs";

/// Attribute names whose last path segment marks a method as a framework
/// dispatch entry point. The ratified Phase 1 set is rmcp's `#[tool]` — the
/// attribute macro generates the tool router that invokes the method, so it has
/// no source-visible caller. Kept as a small Rust-specific constant beside the
/// structural trait-impl walk, mirroring the legacy Rust framework anchors in
/// [`super::framework`] (`HTTP_METHODS`, `STATE_EXTRACTORS`); a later increment
/// can lift it into the plugin descriptor like `framework_detectors`.
const RUST_DISPATCH_ATTRS: [&str; 1] = ["tool"];

/// The axum method-router constructor heads whose first argument is a handler
/// handed over by value — recognised **only inside a `route(path, …)` second
/// argument** (see [`collect_method_router_handlers`]), so a bare same-named call
/// elsewhere (`map.get(k)`, a local `head(x)`) is never mistaken for a route
/// handler. Mirrors the HTTP-method set the framework-promotion pass recognises
/// ([`super::framework`]); kept as a small Rust-specific constant beside the
/// handoff walk (S-276, [CR-068]).
const ROUTER_METHOD_HEADS: [&str; 10] = [
    "get", "post", "put", "delete", "patch", "head", "options", "trace", "connect", "any",
];

/// The axum `Router::route(path, method_router)` method whose second argument is
/// a (possibly chained) method-router expression carrying the handlers (S-276,
/// [CR-068]).
const ROUTER_REGISTER: &str = "route";

/// The axum middleware constructors that take a handler function pointer: the
/// first argument is the handler for `from_fn`, the **second** for
/// `from_fn_with_state` (its first argument is the shared state) (S-276,
/// [CR-068]). Distinctive names, so recognised wherever they appear.
const MIDDLEWARE_FROM_FN: &str = "from_fn";
const MIDDLEWARE_FROM_FN_WITH_STATE: &str = "from_fn_with_state";

/// The axum `Router::fallback(handler)` method whose sole argument is a handler
/// handed over by value (S-276, [CR-068]).
const ROUTER_FALLBACK: &str = "fallback";

/// Run the dispatch live-rooting pass: mark every framework-dispatched method
/// with its live-root marker edge, reconciling against the markers already in
/// the graph.
///
/// `delta` is `None` for a full [`index`](crate::pipeline::index) (scan and
/// reconcile every `.rs` file) and `Some` for an incremental
/// [`sync`](crate::pipeline::sync) (scan and reconcile only the changed `.rs`
/// files). A sync that touched no Rust file returns immediately.
///
/// # Errors
/// Returns an error if the snapshot read or the reconcile write fails (the write
/// batch rolls back wholesale, [NFR-RA-07]).
///
/// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
pub fn run(
    runtime: &Runtime,
    registry: &LanguageRegistry,
    root: &Path,
    delta: Option<&super::Delta>,
) -> Result<DispatchStats> {
    let started = Instant::now();

    // Candidacy: a full index considers every indexed `.rs` file; an incremental
    // sync only the `.rs` files the change-set re-extracted or removed. A sync
    // with no Rust change has nothing to reconcile and reads nothing.
    let changed_rs: Option<HashSet<String>> = delta.map(|d| {
        d.changed_paths
            .iter()
            .filter(|p| is_rust(p))
            .cloned()
            .collect()
    });
    if matches!(&changed_rs, Some(set) if set.is_empty()) {
        return Ok(DispatchStats {
            duration_ms: elapsed_ms(started),
            ..DispatchStats::default()
        });
    }

    // Snapshot the basis we reconcile against, scoped so the pass stays
    // change-proportional on the sync hot path ([NFR-PE-03]): the callable nodes
    // to map a recognised `(file, line)` to its node id — every node on a full
    // index, only the changed files' nodes on a sync — plus the existing dispatch
    // markers (a targeted read, not the whole-graph edge scan).
    let (nodes, markers) = match &changed_rs {
        // Full index: every node, and a whole-graph marker scan (within the cold
        // index budget, not the sync hot path).
        None => runtime.submit_read(|store| Ok((store.all_nodes()?, store.dispatch_markers()?)))?,
        // Sync: only the changed files' callable nodes, and only the markers on
        // *those* nodes (index-served), so the read is O(changed) ([NFR-PE-03]).
        Some(changed) => {
            let paths: Vec<String> = changed.iter().cloned().collect();
            runtime.submit_read(move |store| {
                let nodes = store.callable_nodes_in_files(&paths)?;
                let ids: Vec<NodeId> = nodes.iter().map(|n| n.id).collect();
                let markers = store.markers_for_nodes(&ids)?;
                Ok((nodes, markers))
            })?
        }
    };

    // The `.rs` files to (re)scan, project-relative and deterministically
    // ordered ([NFR-RA-06]): the changed `.rs` set on a sync, every `.rs` file in
    // the graph on a full index.
    let mut candidate_paths: Vec<String> = match &changed_rs {
        Some(changed) => changed.iter().cloned().collect(),
        None => nodes
            .iter()
            .filter_map(|n| n.file_path.as_deref())
            .filter(|p| is_rust(p))
            .map(str::to_string)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect(),
    };
    candidate_paths.sort();

    let candidate_set: HashSet<&str> = candidate_paths.iter().map(String::as_str).collect();

    // (file_path, start_line) → node id, restricted to the callable nodes in the
    // candidate files — the universe a recognised method binds to.
    let node_by_loc: HashMap<(&str, i64), NodeId> = nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
        .filter_map(|n| {
            let path = n.file_path.as_deref()?;
            if !candidate_set.contains(path) {
                return None;
            }
            Some(((path, n.start_line?), n.id))
        })
        .collect();

    // (file_path, callable name) → its node ids, restricted to the callable nodes
    // in the candidate files — the universe a function-pointer handoff resolves a
    // handler name against (same file only, exactly-one-or-nothing, [NFR-RA-05],
    // S-276). A name shared by two callables in one file is left ambiguous and
    // never bound.
    let mut callable_by_name: HashMap<(&str, &str), Vec<NodeId>> = HashMap::new();
    for n in &nodes {
        if !matches!(n.kind, NodeKind::Function | NodeKind::Method) {
            continue;
        }
        let Some(path) = n.file_path.as_deref() else {
            continue;
        };
        if !candidate_set.contains(path) {
            continue;
        }
        callable_by_name
            .entry((path, n.name.as_str()))
            .or_default()
            .push(n.id);
    }

    // Parse the candidate files on the shared worker pool, one parser per worker
    // (the extraction/framework pattern). A vanished or unparsable file simply
    // contributes no entries — best-effort.
    let Some(plugin) = registry.for_extension(RUST_EXT) else {
        // No Rust grammar loaded: nothing this pass can recognise.
        return Ok(DispatchStats {
            duration_ms: elapsed_ms(started),
            ..DispatchStats::default()
        });
    };
    let language = plugin.language();
    let scanned: Vec<(String, FileScan)> = runtime.worker_pool().install(|| {
        candidate_paths
            .par_iter()
            .map_init(Parser::new, |parser, rel| {
                let scan = scan_path(parser, language, root, rel);
                (rel.clone(), scan)
            })
            .collect()
    });

    // The desired marker set: the node id of every recognised dispatch method or
    // function-pointer handoff handler in a candidate file. A dispatch method
    // whose `(file, line)` does not map to an indexed callable — a parse drift or
    // a removed node — is silently dropped, never fabricated; a handoff handler
    // binds only to the **one** same-file callable of that name, so a cross-file
    // or in-file-ambiguous name is likewise dropped ([NFR-RA-05], S-276).
    let mut desired: HashSet<NodeId> = HashSet::new();
    for (rel, scan) in &scanned {
        for entry in &scan.roots {
            if let Some(&id) = node_by_loc.get(&(rel.as_str(), entry.start_line)) {
                desired.insert(id);
            }
        }
        for handler in &scan.handoffs {
            if let Some([only]) = callable_by_name
                .get(&(rel.as_str(), handler.as_str()))
                .map(Vec::as_slice)
            {
                desired.insert(*only);
            }
        }
    }

    // The markers already in the graph, restricted to nodes in the candidate
    // files — the only ones this run is allowed to reconcile (a sync must leave
    // every untouched file's markers exactly as they are).
    let node_file: HashMap<NodeId, &str> = nodes
        .iter()
        .filter_map(|n| Some((n.id, n.file_path.as_deref()?)))
        .collect();
    let existing: HashSet<NodeId> = markers
        .into_iter()
        .filter(|id| node_file.get(id).is_some_and(|p| candidate_set.contains(p)))
        .collect();

    // Reconcile: add the new markers, retire the stale ones. Sorted for a
    // deterministic write order ([NFR-RA-06]).
    let mut to_add: Vec<NodeId> = desired.difference(&existing).copied().collect();
    let mut to_remove: Vec<NodeId> = existing.difference(&desired).copied().collect();
    to_add.sort();
    to_remove.sort();
    let added = to_add.len() as u64;
    let removed = to_remove.len() as u64;

    if !to_add.is_empty() || !to_remove.is_empty() {
        runtime.submit_write(move |w| {
            for id in &to_remove {
                w.delete_edge(*id, *id, EdgeKind::RoutesTo)?;
            }
            for id in &to_add {
                // A plain (non-derived) edge: the annotation pass's `clear_derived`
                // must not wipe it (see the module docs), and this pass is its sole
                // owner — reconciled by the `RoutesTo` self-edge shape, not a flag.
                w.insert_edge_if_absent(*id, *id, EdgeKind::RoutesTo)?;
            }
            Ok(())
        })?;
    }

    Ok(DispatchStats {
        files_scanned: candidate_paths.len() as u64,
        entries: desired.len() as u64,
        markers_added: added,
        markers_removed: removed,
        duration_ms: elapsed_ms(started),
    })
}

/// `true` if `path` is a Rust source file.
fn is_rust(path: &str) -> bool {
    Path::new(path).extension().and_then(|e| e.to_str()) == Some(RUST_EXT)
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

/// One recognised dispatch method in one file (pre node-id mapping): the
/// declaration's 1-based start line, matched against the node's `start_line` the
/// same way extraction records it (the `function_item` row, attributes excluded
/// because Rust models outer attributes as *preceding siblings*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DispatchEntry {
    start_line: i64,
}

/// Everything the scanner recognised in one file (pre node-id mapping): the
/// declaration lines of framework-dispatched methods, plus the handler names
/// handed to an axum router/middleware by value (function-pointer handoffs,
/// S-276). Both are pure functions of this file's syntax.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct FileScan {
    /// Declaration start-lines of trait-impl / `#[tool]` dispatch methods,
    /// deduplicated and sorted by line.
    roots: Vec<DispatchEntry>,
    /// Bare handler names handed to a router/middleware by value, deduplicated
    /// and sorted — each resolved to the one same-file callable of that name in
    /// [`run`] ([NFR-RA-05]).
    handoffs: Vec<String>,
}

/// Read and scan one file for dispatch entries; a path that is absolute, not
/// Rust, or unreadable yields none (best-effort, defence-in-depth against an
/// absolute `rel` escaping the engine root, [NFR-SE-04]).
fn scan_path(
    parser: &mut Parser,
    language: &tree_sitter::Language,
    root: &Path,
    rel: &str,
) -> FileScan {
    if Path::new(rel).is_absolute() || !is_rust(rel) {
        return FileScan::default();
    }
    let Ok(source) = fs::read_to_string(root.join(rel)) else {
        return FileScan::default();
    };
    scan_source(parser, language, &source)
}

/// Scan one Rust file's source for dispatch entries. Pure — no store, no disk —
/// and therefore the unit-testable core of the pass.
///
/// A method is a **root** entry if it is declared in an `impl Trait for Type`
/// block (framework trait-impl dispatch) **or** it carries a dispatch attribute
/// ([`RUST_DISPATCH_ATTRS`]). A **handoff** entry is a handler name handed to an
/// axum router/middleware by value ([`handoff_handler`]). Every signal is read
/// from this file's own syntax, so the result depends on nothing outside it.
fn scan_source(
    parser: &mut Parser,
    language: &tree_sitter::Language,
    source: &str,
) -> FileScan {
    if parser.set_language(language).is_err() {
        return FileScan::default();
    }
    let Some(tree) = parser.parse(source, None) else {
        return FileScan::default();
    };
    let src = source.as_bytes();
    // BTreeSets so both lists come out de-duplicated AND already sorted.
    let mut roots: BTreeSet<i64> = BTreeSet::new();
    let mut handoffs: BTreeSet<String> = BTreeSet::new();

    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "impl_item" {
            let is_trait_impl = node.child_by_field_name("trait").is_some();
            if let Some(body) = node.child_by_field_name("body") {
                let mut cursor = body.walk();
                for method in body.children(&mut cursor) {
                    if method.kind() != "function_item" {
                        continue;
                    }
                    if is_trait_impl || has_dispatch_attribute(method, src) {
                        roots.insert(method.start_position().row as i64 + 1);
                    }
                }
            }
        }
        if node.kind() == "call_expression" {
            collect_handoffs(node, src, &mut handoffs);
        }
        for i in (0..node.child_count()).rev() {
            if let Some(child) = node.child(i) {
                stack.push(child);
            }
        }
    }

    FileScan {
        roots: roots
            .into_iter()
            .map(|start_line| DispatchEntry { start_line })
            .collect(),
        handoffs: handoffs.into_iter().collect(),
    }
}

/// Collect the handler names a `call_expression` hands to an axum
/// router/middleware **by value** into `out` (S-276, [CR-068] §3.2).
///
/// The ratified Phase-3 shapes:
///
/// - **`.route(path, get(h)|post(h)|…)`** — a `.route(...)` method call whose
///   second argument is a method-router; every handler in the (possibly chained)
///   method-router expression is collected ([`collect_method_router_handlers`]),
///   so `get(a).post(b)` yields both `a` and `b`. Method-router constructors are
///   recognised **only inside a `route(...)` argument**, never as a bare `get(h)`
///   anywhere, so an ordinary same-named call (a local `head(x)`, `map.get(k)`)
///   is not mistaken for a route handler;
/// - middleware `from_fn(h)` / `middleware::from_fn(h)` — handler is argument 0;
/// - middleware `from_fn_with_state(state, h)` — handler is argument **1** (the
///   first argument is the shared state);
/// - `Router::fallback(h)` — a `.fallback(h)` method call; handler is argument 0.
///
/// Each handler must be an `identifier`/`scoped_identifier`; the collected name
/// is its last `::` segment (a closure, block, or other expression is skipped).
/// Resolution to a node is the caller's job and is same-file
/// exactly-one-or-nothing, so a name that resolves cross-file or ambiguously
/// binds nothing ([NFR-RA-05]).
fn collect_handoffs(call: Node<'_>, src: &[u8], out: &mut BTreeSet<String>) {
    let Some(function) = call.child_by_field_name("function") else {
        return;
    };
    let Some(args) = call.child_by_field_name("arguments") else {
        return;
    };
    match function.kind() {
        // `from_fn(h)` / `middleware::from_fn(h)` / `from_fn_with_state(state, h)`
        // — distinctive axum names, recognised wherever they appear.
        "identifier" | "scoped_identifier" => {
            let head = last_path_segment(node_text(function, src));
            let slot = if head == MIDDLEWARE_FROM_FN {
                0
            } else if head == MIDDLEWARE_FROM_FN_WITH_STATE {
                1
            } else {
                return;
            };
            if let Some(name) = ident_arg_name(args, slot, src) {
                out.insert(name);
            }
        }
        "field_expression" => {
            let Some(field) = function.child_by_field_name("field") else {
                return;
            };
            let field = node_text(field, src);
            if field == ROUTER_FALLBACK {
                // `.fallback(h)` — handler is argument 0.
                if let Some(name) = ident_arg_name(args, 0, src) {
                    out.insert(name);
                }
            } else if field == ROUTER_REGISTER {
                // `.route(path, <method-router>)` — walk the second argument, the
                // (possibly chained) method-router carrying the handlers.
                if let Some(router) = args.named_child(1) {
                    collect_method_router_handlers(router, src, out);
                }
            }
        }
        _ => {}
    }
}

/// Walk a method-router expression, collecting each handler handed over by value
/// (S-276). Recognised shapes, recursing left through the chain:
///
/// - `get(h)` / `axum::routing::get(h)` / `get::<T>(h)` — a method-router
///   constructor whose head is in [`ROUTER_METHOD_HEADS`]; handler is argument 0;
/// - `<receiver>.post(h)` — a chained setter (`get(a).post(b)`): the receiver is
///   walked first, then this link's argument-0 handler is taken;
/// - any other link is a no-op on itself, but its receiver is still walked so a
///   handler left of an unrecognised link is not lost.
///
/// Mirrors the framework-promotion pass's `router_targets` walk, minus the
/// method/path bookkeeping ([`super::framework`]).
fn collect_method_router_handlers(node: Node<'_>, src: &[u8], out: &mut BTreeSet<String>) {
    if node.kind() != "call_expression" {
        return;
    }
    let Some(function) = node.child_by_field_name("function") else {
        return;
    };
    let args = node.child_by_field_name("arguments");
    match function.kind() {
        // The chain head: `get(h)` / `axum::routing::get(h)`.
        "identifier" | "scoped_identifier" => {
            if ROUTER_METHOD_HEADS.contains(&last_path_segment(node_text(function, src))) {
                if let Some(name) = args.and_then(|a| ident_arg_name(a, 0, src)) {
                    out.insert(name);
                }
            }
        }
        // A turbofished head: `get::<T>(h)`.
        "generic_function" => {
            if let Some(inner) = function.child_by_field_name("function") {
                if matches!(inner.kind(), "identifier" | "scoped_identifier")
                    && ROUTER_METHOD_HEADS.contains(&last_path_segment(node_text(inner, src)))
                {
                    if let Some(name) = args.and_then(|a| ident_arg_name(a, 0, src)) {
                        out.insert(name);
                    }
                }
            }
        }
        // A chained link `<receiver>.method(h)`: walk the receiver, then take this
        // link's handler when its field is a method-router setter.
        "field_expression" => {
            if let Some(receiver) = function.child_by_field_name("value") {
                collect_method_router_handlers(receiver, src, out);
            }
            if let Some(field) = function.child_by_field_name("field") {
                if ROUTER_METHOD_HEADS.contains(&node_text(field, src)) {
                    if let Some(name) = args.and_then(|a| ident_arg_name(a, 0, src)) {
                        out.insert(name);
                    }
                }
            }
        }
        _ => {}
    }
}

/// The last `::` segment of the `n`-th named argument (0-based) when it is an
/// `identifier`/`scoped_identifier` — a handler handed over by value; `None` for
/// a closure, literal, or other expression. Indexed access (not a `TreeCursor`)
/// so the returned node does not borrow a local cursor.
fn ident_arg_name(args: Node<'_>, n: usize, src: &[u8]) -> Option<String> {
    let arg = args.named_child(n)?;
    matches!(arg.kind(), "identifier" | "scoped_identifier")
        .then(|| last_path_segment(node_text(arg, src)).to_string())
}

/// The last `::`-path segment of a path text (`get` for `axum::routing::get`,
/// `overview` for `api_v1::overview`), trimmed.
fn last_path_segment(text: &str) -> &str {
    text.rsplit("::").next().unwrap_or(text).trim()
}

/// The UTF-8 text of a node, or `""` on a non-UTF-8 slice.
fn node_text<'s>(node: Node<'_>, src: &'s [u8]) -> &'s str {
    node.utf8_text(src).unwrap_or("")
}

/// `true` if `item`'s preceding outer-attribute run carries a dispatch
/// attribute ([`RUST_DISPATCH_ATTRS`]).
///
/// tree-sitter-rust models an item's outer attributes as `attribute_item`
/// **preceding siblings**, not children, so the run is walked backward from the
/// item (skipping comments, which do not detach the attributes from it) until a
/// non-attribute, non-comment sibling ends it — mirroring the test-marker
/// attribute walk in [`crate::extract`].
fn has_dispatch_attribute(item: Node<'_>, src: &[u8]) -> bool {
    let mut sibling = item.prev_sibling();
    while let Some(n) = sibling {
        let kind = n.kind();
        if kind == "line_comment" || kind == "block_comment" {
            sibling = n.prev_sibling();
            continue;
        }
        if kind != "attribute_item" {
            break;
        }
        if let Some(attr) = child_of_kind(n, "attribute") {
            if attribute_last_segment(attr, src).is_some_and(|s| RUST_DISPATCH_ATTRS.contains(&s)) {
                return true;
            }
        }
        sibling = n.prev_sibling();
    }
    false
}

/// The last `::`-path segment of an `attribute`'s name (`tool` for `#[tool(…)]`,
/// `test` for `#[tokio::test]`).
fn attribute_last_segment<'s>(attr: Node<'_>, src: &'s [u8]) -> Option<&'s str> {
    // The attribute's path is its first `identifier`/`scoped_identifier` child;
    // read the whole text and take the last `::` segment. Indexed iteration
    // (not a `TreeCursor`) so the returned node does not outlive a local cursor.
    let path = (0..attr.child_count())
        .filter_map(|i| attr.child(i))
        .find(|c| matches!(c.kind(), "identifier" | "scoped_identifier"))?;
    let text = path.utf8_text(src).ok()?;
    text.rsplit("::").next()
}

/// The first direct child of `node` of kind `kind`. Indexed iteration so the
/// returned node does not borrow a local `TreeCursor`.
fn child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    (0..node.child_count())
        .filter_map(|i| node.child(i))
        .find(|c| c.kind() == kind)
}

#[cfg(test)]
#[cfg(feature = "lang-rust")]
mod tests;
