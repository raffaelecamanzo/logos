//! Pass 2 of the pipeline — the resolution engine ([resolution-engine],
//! S-011, [ADR-10]).
//!
//! [`run`] re-evaluates the **entire** `unresolved_refs` ledger against a
//! consistent snapshot of the graph:
//!
//! 1. **Snapshot** (one reader-pool read): all nodes, all edges, every ledger
//!    row.
//! 2. **Parallel compute** (the shared worker pool, [AQ-04]): each row is
//!    bound independently against the immutable [`binder::Index`] — pure CPU,
//!    no store access, deterministic regardless of thread count.
//! 3. **Serial commit** (one writer-actor batch, [ADR-02]): bound rows become
//!    edges (idempotently) and are flagged `resolved`; rows that no longer
//!    bind flip back to retry state, and on an incremental run the edges a
//!    re-bound source no longer produces are deleted ([FR-SY-12]). A
//!    capture-before-delete row is deleted once it is spent — bound, or its
//!    source's own rows re-bound — so the ledger stays a fresh index's
//!    ([FR-SY-10]). One transaction, atomic rollback ([NFR-RA-07]).
//!
//! Re-evaluating everything — not just the unresolved tail — is what makes
//! the pass self-healing: a deferred reference binds on the sync that indexes
//! its target ([UAT-RS-01]), and a row whose target vanished flips back to
//! unresolved instead of lying. The compute is in-memory hash lookups; on the
//! Logos dogfood it is far below the sync budget.
//!
//! # Honesty contract ([FR-RS-04], [NFR-RA-11], [NFR-CC-04])
//!
//! [`run`] and [`coverage`] surface the same [`ResolutionStats`] read-model:
//! total refs, bound refs, surviving unresolved refs, and the bound-ratio.
//! Heuristic results are never presented as ground truth — the coverage
//! number rides along wherever resolution data is consumed.
//! [`coverage_by_language`] states the same coverage per language, with its
//! denominator and a same-file/cross-file split ([FR-RS-09]), because one
//! global ratio averages a language that binds nothing across a file boundary
//! into the rest.
//!
//! [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
//! [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
//! [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
//! [AQ-04]: ../../../docs/specs/architecture.md#14-open-questions
//! [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
//! [NFR-RA-11]: ../../../docs/specs/requirements/NFR-RA-11.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [FR-RS-04]: ../../../docs/specs/requirements/FR-RS-04.md
//! [FR-RS-09]: ../../../docs/specs/requirements/FR-RS-09.md
//! [FR-SY-10]: ../../../docs/specs/requirements/FR-SY-10.md
//! [FR-SY-12]: ../../../docs/specs/requirements/FR-SY-12.md
//! [UAT-RS-01]: ../../../docs/specs/requirements/UAT-RS-01.md

mod binder;
pub(crate) use binder::{
    is_class_like, FULLY_QUALIFIED_HEAD, GLOBAL_WILDCARD_ALIAS, SELF_TYPE_HEAD, STATIC_WILDCARD_ALIAS,
    TRAIT_USE_ALIAS,
};
/// The broker topic-identity rule (S-424, CR-136, FR-WS-27, ADR-52): the ONE
/// function the intra-repo promotion pass, the federation bridge and the
/// coverage read-model all resolve a broker topic operand through, so a `Topic`
/// node and a bridge edge can never key one captured fact two ways. It lives
/// here rather than in `federation` because the promotion pass runs on every
/// single-root index, where federation is absent. See its module docs.
pub mod broker_identity;
/// Configuration-bound operand resolution (S-382, CR-121, FR-WS-19, ADR-64): a
/// placeholder, a value-annotation key or a configuration-bound accessor
/// resolved against the **committed** configuration corpus, retaining every
/// profile-tagged value where overlays disagree and carrying `config-bound`
/// provenance — the key, the defining sources and the profile set — to every
/// surface. The refusals are its boundary, not its residue. See its module docs.
pub mod binding;
/// The framework-dispatch live-rooting pass (CR-043, ADR-39): recognises
/// framework-dispatched Rust methods (trait-impl dispatch, `#[tool]` tool
/// dispatch) and live-roots them with a self-`RoutesTo` marker so the
/// dead-code pass stops mis-reporting them dead — never fabricating, false-live
/// biased, reconciled every run. See its module docs.
pub mod dispatch;
/// The framework-promotion pass (S-012): promotes Axum/Actix route and
/// shared-state matches to `route`/`component` nodes against the resolved
/// graph — ledger-gated, binder-proven, reconciled every run. See its module docs.
pub mod framework;
/// The Go modules a tree declares (S-439, CR-142 D1): the `go.mod` `module`
/// directive a Go import path is anchored on, so an intra-module path binds to
/// its package and an external one never binds to a directory that merely
/// shares its last segments. See its module docs.
pub(crate) mod go_module;
pub(crate) mod grpc_key;
/// The package-shaped module key (S-465, CR-149, FR-RS-01): the ONE derivation
/// of the package a file declares — and so of a type's fully-qualified name —
/// from its path, for a language whose descriptor declares `[package_modules]`,
/// and from the namespace each file declares, for one whose `[module_model]` is
/// `namespace` (S-518, FR-RS-13).
/// The binder keys such files by it; any later consumer asks it rather than
/// splitting a path a second way. See its module docs.
pub mod package_key;
/// The shared positional route-template normalizer (S-069, CR-011): aligns the
/// OpenAPI `ApiOperation` path templates with framework-extracted `route` node
/// templates under one parameter-position-only comparison. See its module docs.
pub mod route_template;
/// The shared wildcard-method matching rule (S-349, CR-109, FR-CG-09): decides
/// whether a provider registered under one HTTP method serves a consumer that
/// declares another (`ANY` is the wildcard), and which of the providers sharing
/// a normalized template a consumer may bind. The one rule all three
/// candidate-selection sites reduce their template bucket through. See its
/// module docs.
pub(crate) mod route_method;
/// The HTTP client-call arm normalizer (S-252, CR-061, FR-WS-08): reduces a
/// captured outbound call to its `"METHOD /template"` bind target or the reason
/// it is honestly unbindable (base-url-runtime / path-not-composed). The one
/// arm-specific piece of the pluggable invocation-arm contract. See its module docs.
pub(crate) mod http_client_call;
/// The shared promotion-commit primitive (S-292, CR-082, ADR-54): the one
/// reconcile-and-commit algorithm the `framework` and `topics` passes both run,
/// parameterised over how a desired edge names its endpoints. See its module docs.
mod promote;
/// The broker-topic promotion pass (S-256, CR-061, FR-WS-11, ADR-55): promotes the
/// ledger-only broker publish/subscribe references S-254 captured to first-class
/// `topic`/`producer`/`consumer` nodes joined by `publishes`/`subscribes` edges —
/// reconciled every run, and provably inert on a graph with no broker topics. See
/// its module docs.
pub mod topics;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use rayon::prelude::*;

use crate::config::{BindingPolicy, Resolution};
use crate::graph_store::{EdgeRow, GraphStore, NodeArity, NodeRow, RelationCounts, UnresolvedRefRow};
use crate::model::{EdgeKind, NodeId, RefForm};
use crate::models::navigation::{
    CallResidue, CallResidueReason, LanguageResolution, RelationResolution, ResidueScope,
};
use crate::models::pipeline::{RelationCoverage, ResolutionStats};
use crate::plugin::LanguageRegistry;
use crate::runtime::Runtime;

/// `true` when a ledger `target` (canonical `::`-joined) falls under a
/// descriptor detector prefix: the target *is* the detector, or extends it by
/// whole segments (`axum::routing::get` under `axum`; never `axumish` under
/// `axum`).
///
/// One rule, two ledger-gated candidacy checks — the framework pass's
/// provider-side `framework_detectors` ([FR-FW-04]) and the HTTP client-call
/// arm's consumer-side `http_client_detectors` ([FR-WS-08]). They are the same
/// question asked of the same canonical target form, so they share one body: a
/// future tightening (a trailing `::`, case folding) then lands once instead of
/// drifting between two copies.
///
/// [FR-FW-04]: ../../../docs/specs/requirements/FR-FW-04.md
/// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
pub(crate) fn matches_detector(target: &str, detector: &str) -> bool {
    target
        .strip_prefix(detector)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
}

/// The consistent graph state one resolution run binds against.
struct Snapshot {
    nodes: Vec<NodeRow>,
    edges: Vec<EdgeRow>,
    refs: Vec<UnresolvedRefRow>,
    /// Every node's recorded self type (S-493, `nodes.self_type`).
    self_types: Vec<(NodeId, String)>,
    /// Every node's recorded arity facts (S-591; read by S-604 for
    /// `nodes.takes_self`).
    arities: Vec<NodeArity>,
    /// Every file's recorded declared namespace (S-518, `files.namespace`).
    namespaces: Vec<(String, String)>,
    /// file_id → project-relative path, for an incremental run to test a row's
    /// owning file against the change-set. Empty on a full index.
    file_paths: HashMap<i64, String>,
}

/// The change-set an incremental [`run`] resolves against (the [`sync`] path).
///
/// Passing `None` to [`run`] re-binds the whole ledger — a cold [`index`], where
/// every row is new. A `Some(Delta)` re-binds only the rows the change can move:
/// `changed_paths` are the project-relative files re-extracted or removed this
/// sync (their own rows, including capture-before-delete rows, always re-bind);
/// `dirty_tokens` are the tokenized names a node in one of those files carried
/// *before or after* the sync — the binding-bucket keys that changed, so a row in
/// an untouched file that targets one of them must be reconsidered too.
///
/// [`sync`]: crate::pipeline::sync
/// [`index`]: crate::pipeline::index
#[derive(Debug, Default)]
pub struct Delta {
    /// Project-relative paths re-extracted or removed this sync — plus any Go
    /// module descriptor (`go.mod`) the sync was handed, which is never indexed
    /// but moves every Go import binding (S-439).
    pub changed_paths: HashSet<String>,
    /// Tokenized names this sync added or removed (see [`tokens`]).
    pub dirty_tokens: HashSet<String>,
    /// Whether a changed file declared a global namespace wildcard before or
    /// after the sync (S-518; a C# `global using`). Such a wildcard brings
    /// names into view in untouched files under names no dirty token spells,
    /// so every package-shaped row is re-bound.
    pub global_imports_moved: bool,
}

/// Split `s` into lowercased identifier tokens — maximal runs of ASCII
/// alphanumerics and `_`.
///
/// The shared tokenizer of the incremental change-set: [`Delta::dirty_tokens`]
/// is built by tokenizing the names a sync adds or removes, and a reference is
/// re-bound when a token of its target (or of an `as`-alias it expands through)
/// lands in that set. Names rather than canonical symbols, because every binding
/// key the binder reads is some node's human name, module name, route literal,
/// or file-path segment — all of which survive this split on both sides.
pub(crate) fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Run the resolution pass: bind every ledger row it can, persist the rest.
///
/// See the module docs for the snapshot → parallel-compute → serial-commit
/// shape. Returns the run's [`ResolutionStats`] ([FR-RS-04]).
///
/// `tree` is the registry the tree was indexed with and its root — the
/// path-specifier context (S-439, [CR-142] D1). The registry says which files
/// write their import specifiers as paths and which files each may resolve to
/// ([`LanguageRegistry::specifier_target_extensions`]); the root supplies the
/// `go.mod` above each indexed `.go` file ([`go_module`]) so a Go import path
/// binds against the module that declares it. The pipeline always passes it.
/// `None` is for a synthetic graph with no tree behind it: no `go.mod` is read,
/// no import binds by path rules, and every import takes the member-path
/// hierarchy — which reads a bare `react` as a name, so it is not what a real
/// tree may be resolved with.
///
/// [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
///
/// # Errors
/// Returns an error if the snapshot read or the commit batch fails (the
/// batch rolls back wholesale, [NFR-RA-07]).
///
/// [FR-RS-04]: ../../../docs/specs/requirements/FR-RS-04.md
/// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
pub fn run(
    runtime: &Runtime,
    tree: Option<(&LanguageRegistry, &Path)>,
    resolution: &Resolution,
    delta: Option<&Delta>,
) -> Result<ResolutionStats> {
    let policy = resolution.policy;
    let want_file_paths = delta.is_some();
    let snap = runtime.submit_read(|store| {
        Ok(Snapshot {
            nodes: store.all_nodes()?,
            edges: store.all_edges()?,
            refs: store.unresolved_refs()?,
            self_types: store.node_self_types()?,
            arities: store.node_arities()?,
            namespaces: store.file_namespaces()?,
            // The file_id → path map only an incremental run needs (to test a
            // row's owning file against the change-set); a full index skips it.
            file_paths: if want_file_paths {
                store
                    .indexed_files()?
                    .into_iter()
                    .map(|f| (f.id, f.path))
                    .collect()
            } else {
                HashMap::new()
            },
        })
    })?;

    let (specifier_targets, go_modules) =
        tree.map_or_else(Default::default, |(registry, root)| {
            let go_files: std::collections::BTreeSet<&str> = snap
                .nodes
                .iter()
                .filter_map(|n| n.file_path.as_deref())
                .filter(|p| p.ends_with(".go"))
                .collect();
            (
                registry.specifier_target_extensions(),
                go_module::discover(root, go_files),
            )
        });
    // A package-shaped language (CR-149) keys its files by their package, and a
    // declared-namespace one (S-518) by the namespace each file declares, as
    // its plugin declares; a synthetic graph with no registry keeps the default
    // module model for every file.
    let layout = tree.map_or_else(Default::default, |(registry, _)| {
        package_key::PackageLayout::from_registry(registry)
            .with_declared_namespaces(snap.namespaces.iter().cloned())
            .with_import_root_overrides(&resolution.import_roots)
    });
    let index = binder::Index::build_with_layout(&snap.nodes, &snap.edges, &snap.refs, layout)
        .with_self_types(snap.self_types)
        .with_arities(&snap.arities)
        .with_path_specifiers(specifier_targets, go_modules)
        .with_imported_bindings(&snap.refs, policy);

    // A full index (no delta) re-binds the whole ledger. An incremental sync
    // re-binds only the rows whose outcome the change-set can move; every other
    // row provably keeps the edge and resolved flag the snapshot already holds,
    // so the committed graph is byte-identical to a full re-bind (the CR-015
    // equivalence invariant, guarded by `tests/indexing.rs`). Binding a handful
    // of rows instead of the entire ~40k ledger on every core is the melt fix.
    // Decided once per run, not per row: whether the change added or removed a
    // package file that can move the detected import roots (S-519), which
    // re-keys a whole language.
    let roots_moved = delta.is_some_and(|d| d.changed_paths.iter().any(|p| index.moves_import_roots(p)));
    let mut selected: Vec<&UnresolvedRefRow> = match delta {
        None => snap.refs.iter().collect(),
        Some(d) if d.changed_paths.is_empty() && d.dirty_tokens.is_empty() => Vec::new(),
        Some(d) => {
            // …and whether it moved a type of the hierarchy a supertype walk
            // climbs.
            let moved = Moved {
                hierarchy: index.hierarchy_touched(&d.dirty_tokens),
                import_roots: roots_moved,
            };
            // …and the names a renaming import elsewhere gives a dirty name,
            // which a proven Rust receiver's type is read through (S-588).
            let renamed = index.renamed_import_tokens(&d.dirty_tokens);
            snap.refs
                .iter()
                .filter(|&r| {
                    is_affected(r, d, &snap.file_paths, &index, moved)
                        || (!renamed.is_empty()
                            && binder::is_proven_receiver_call(r)
                            && index.ref_affected(r, &renamed))
                })
                .collect()
        }
    };

    // A re-selected row that was bound may come back unbound, ambiguous or
    // bound elsewhere, and edges carry no provenance: every row of the source
    // its edge left from is re-bound, so the edges that source no longer
    // produces can be retracted below (`retract_unproduced`; S-519, S-596).
    let swept = swept_sources(
        &mut selected,
        &snap.refs,
        &snap.nodes,
        &snap.file_paths,
        &index,
        delta,
        roots_moved,
    );

    // Parallel compute on the shared worker pool (AQ-04): pure binding against
    // the immutable index; `collect` preserves input order (NFR-RA-06).
    let mut outcomes: Vec<(i64, bool, binder::Outcome)> = runtime.worker_pool().install(|| {
        selected
            .par_iter()
            .map(|&r| (r.id, r.resolved, binder::bind(r, &index, policy)))
            .collect()
    });

    // A binding can move although neither endpoint's file changed — a rival
    // arriving, a supertype dropped, a submodule taking over a re-exported
    // name — and the edge its row no longer binds would outlive it. Every row
    // of the swept sources was re-bound above, so the edges they no longer
    // produce are exactly the stale ones, and a capture row may not restore
    // one (FR-SY-12, NFR-RA-06). Before the stats, which read the amended
    // outcomes.
    let captures: HashSet<i64> = selected
        .iter()
        .filter(|r| r.form == RefForm::Symbol)
        .map(|r| r.id)
        .collect();
    let stale = retract_unproduced(&snap.nodes, &snap.edges, &swept, &captures, &mut outcomes);

    // Stats are over the whole committed ledger (less its capture-before-delete
    // rows, below), not just the re-bound subset: a row this run touched uses its
    // fresh outcome, an untouched row reads through to its snapshot resolved
    // flag (equal, by the invariant above, to what a re-bind would compute).
    // `bound_now` indexes the touched rows; `final_bound` merges.
    let bound_now: HashMap<i64, bool> =
        outcomes.iter().map(|(id, _, o)| (*id, is_bound(o))).collect();
    let final_bound =
        |r: &UnresolvedRefRow| -> bool { bound_now.get(&r.id).copied().unwrap_or(r.resolved) };

    // The capture rows this run deletes, so the committed ledger is a fresh
    // index's (CR-187, FR-SY-10) — and the stats count only what stays.
    let live: HashSet<&str> = snap.nodes.iter().map(|n| n.symbol.as_str()).collect();
    let spent = spent_captures(&snap.refs, &swept, &live, final_bound);
    // The reported population also leaves out every capture-before-delete row
    // that stays (S-598, CR-195): each duplicates a reference its source file's
    // own row records, so counting it made a synced store's ratio differ from a
    // cold reindex's. The same population `GraphStore::counts` and `coverage`
    // read. The spent captures are `Symbol` rows too, so this filter subsumes
    // the first; both are kept so each rule reads where it is decided.
    let reported = || {
        snap.refs
            .iter()
            .filter(|r| !spent.contains(&r.id) && r.form != RefForm::Symbol)
    };

    let refs_total = reported().count() as u64;
    let refs_resolved = reported().filter(|&r| final_bound(r)).count() as u64;

    // Per-relation-class coverage for the cross-artifact references (CR-011,
    // FR-CG-11): the relation token rides on each ledger row's payload; group by
    // it on the row's final bound state. Computed before `outcomes` moves into
    // the write batch below.
    let by_relation =
        relation_coverage(reported().map(|r| (r.payload.as_deref(), final_bound(r))));

    // Serial commit: one transaction through the writer actor (ADR-02).
    let edges_created = runtime.submit_write(move |w| {
        for (source, target, kind) in &stale {
            w.delete_edge(*source, *target, *kind)?;
        }
        let mut created = 0u64;
        for (ref_id, was_resolved, outcome) in &outcomes {
            match outcome {
                binder::Outcome::Bound {
                    source,
                    target,
                    kind,
                    payload,
                } => {
                    // Idempotent: a captured exact-symbol ref and the textual
                    // ref for the same call legitimately bind the same edge. An
                    // artifact bind carries its relation class onto the edge
                    // (CR-011); code/doc/access binds pass `None`.
                    if w.insert_edge_with_payload_if_absent(
                        *source,
                        *target,
                        *kind,
                        payload.as_deref(),
                    )? {
                        created += 1;
                    }
                    if !was_resolved && !spent.contains(ref_id) {
                        w.mark_ref_resolved(*ref_id, true)?;
                    }
                }
                binder::Outcome::BoundMany {
                    source,
                    targets,
                    kind,
                    payload,
                } => {
                    // One reference naming a set: a Terraform module call's
                    // `.tf` files (CR-011), a `dyn T` call's impls (S-281), a Go
                    // import's package files (S-439). One edge per target, all
                    // sharing the payload, all idempotent. `targets` is
                    // non-empty, so the row is resolved.
                    for target in targets {
                        if w.insert_edge_with_payload_if_absent(
                            *source,
                            *target,
                            *kind,
                            payload.as_deref(),
                        )? {
                            created += 1;
                        }
                    }
                    if !was_resolved && !spent.contains(ref_id) {
                        w.mark_ref_resolved(*ref_id, true)?;
                    }
                }
                binder::Outcome::Unbound => {
                    // A previously bound row whose target vanished flips back
                    // to retry state — the ledger never lies (NFR-CC-04).
                    if *was_resolved && !spent.contains(ref_id) {
                        w.mark_ref_resolved(*ref_id, false)?;
                    }
                }
            }
        }
        // After the edges they restored are written, in the same transaction
        // (a spent capture's edge is never lost with its row).
        for ref_id in &spent {
            w.delete_unresolved_ref(*ref_id)?;
        }
        Ok(created)
    })?;

    Ok(stats(refs_total, refs_resolved, edges_created, by_relation))
}

/// The sources an incremental run re-binds **whole** (S-519, S-596), with every
/// row of each added to `selected`: the source of each selected row that was
/// bound (its binding may move, and its old edge must not outlive it); of each
/// selected capture-before-delete row whose source lies in a file this sync
/// re-extracted (the source's fresh rows, all selected and none bound yet,
/// decide its edges, so the capture must not restore one they dropped); and of
/// every row of an import-root file when the run moved the detected roots
/// (`is_affected` reason 7 selected those rows already). Empty for a full
/// index, which re-binds the whole ledger over a graph whose files were all
/// re-extracted.
///
/// The unit is the source symbol, not its file. An edge is keyed by the node
/// it leaves from, and every row that can produce one names that node as its
/// `source_symbol` — a capture-before-delete row ([ADR-10]) too, although it
/// lives in its target's file — so re-binding a source's rows is all a
/// retraction needs. The file's other sources keep their snapshot bindings:
/// sweeping whole files re-bound about 40% of this repository's ledger on
/// every one-file sync (S-596 implementation notes).
///
/// [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
fn swept_sources<'s>(
    selected: &mut Vec<&'s UnresolvedRefRow>,
    refs: &'s [UnresolvedRefRow],
    nodes: &[NodeRow],
    file_paths: &HashMap<i64, String>,
    index: &binder::Index,
    delta: Option<&Delta>,
    roots_moved: bool,
) -> HashSet<&'s str> {
    let Some(delta) = delta else {
        return HashSet::new();
    };
    let mut swept: HashSet<&'s str> = selected
        .iter()
        .filter(|r| r.resolved)
        .map(|r| r.source_symbol.as_str())
        .collect();
    let captured: HashSet<&'s str> = selected
        .iter()
        .filter(|r| r.form == RefForm::Symbol)
        .map(|r| r.source_symbol.as_str())
        .collect();
    if !captured.is_empty() {
        swept.extend(
            nodes
                .iter()
                .filter(|n| n.file_path.as_ref().is_some_and(|p| delta.changed_paths.contains(p)))
                .filter_map(|n| captured.get(n.symbol.as_str()).copied()),
        );
    }
    if roots_moved {
        swept.extend(
            refs.iter()
                .filter(|r| {
                    r.file_id
                        .and_then(|id| file_paths.get(&id))
                        .is_some_and(|p| index.has_import_roots(p))
                })
                .map(|r| r.source_symbol.as_str()),
        );
    }
    if swept.is_empty() {
        return swept;
    }
    let chosen: HashSet<i64> = selected.iter().map(|r| r.id).collect();
    selected.extend(
        refs.iter()
            .filter(|r| !chosen.contains(&r.id) && swept.contains(r.source_symbol.as_str())),
    );
    swept
}

/// The reference-bound edges out of the `swept` sources (S-519, S-596,
/// [FR-SY-12]) that no re-bound row of theirs produces any more — what a moved
/// binding leaves stale — with `outcomes` amended so that a capture row
/// (`captures`) never restores one.
///
/// Every row of those sources was re-bound ([`swept_sources`]), so an edge none
/// of them names has lost the row that bound it. A capture row stands in for
/// its source's own row only while that row keeps its snapshot binding; once
/// the row is re-bound it decides alone, as on a cold index, which holds no
/// capture rows. Only the kinds a ledger row binds are considered: containment
/// and the framework pass's edges are other passes'.
///
/// [FR-SY-12]: ../../../docs/specs/requirements/FR-SY-12.md
fn retract_unproduced(
    nodes: &[NodeRow],
    edges: &[EdgeRow],
    swept: &HashSet<&str>,
    captures: &HashSet<i64>,
    outcomes: &mut [(i64, bool, binder::Outcome)],
) -> Vec<(NodeId, NodeId, EdgeKind)> {
    if swept.is_empty() {
        return Vec::new();
    }
    let produced: HashSet<(NodeId, NodeId, EdgeKind)> = outcomes
        .iter()
        .filter(|(id, _, _)| !captures.contains(id))
        .flat_map(|(_, _, outcome)| bound_edges(outcome))
        .collect();
    let swept_node: HashSet<NodeId> = nodes
        .iter()
        .filter(|n| swept.contains(n.symbol.as_str()))
        .map(|n| n.id)
        .collect();
    let stale = |edge: &(NodeId, NodeId, EdgeKind)| {
        swept_node.contains(&edge.0) && !produced.contains(edge)
    };
    for (id, _, outcome) in outcomes.iter_mut() {
        if captures.contains(id) && bound_edges(outcome).iter().any(stale) {
            *outcome = binder::Outcome::Unbound;
        }
    }
    edges
        .iter()
        .map(|e| (e.source, e.target, e.kind))
        .filter(|edge| {
            matches!(
                edge.2,
                EdgeKind::Calls
                    | EdgeKind::Imports
                    | EdgeKind::Accesses
                    | EdgeKind::Extends
                    | EdgeKind::Implements
                    | EdgeKind::Instantiates
                    | EdgeKind::TypeUses
            ) && stale(edge)
        })
        .collect()
}

/// The capture-before-delete rows ([ADR-10]) a run deletes — every one that is
/// **spent** (CR-187, [FR-SY-10]):
///
/// - it binds (`final_bound`): the edge it carried across the delete is
///   restored, or already present, and a fresh index never holds the row.
///   That includes a resolved capture an earlier version kept in the ledger,
///   so a store synced before this rule heals on its next run;
/// - or its source is `swept`: every row of that source was re-bound this run
///   and decides its edges alone, as on a fresh index ([`retract_unproduced`]
///   has already turned the capture `Unbound` if it would restore an edge
///   they no longer produce);
/// - or its source is no `live` node: the sync deleted or renamed it (its
///   file removed, or re-extracted without it), so no edge leaves it, and a
///   source that returns brings fresh rows of its own.
///
/// A capture that cannot bind, from a live source that was not re-bound,
/// stays unresolved: nothing else in the ledger may carry its edge, and it
/// binds again if its target returns — never invented ([NFR-RA-05]).
///
/// A capture is told by its form: only capture-before-delete writes
/// [`RefForm::Symbol`].
///
/// [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
/// [FR-SY-10]: ../../../docs/specs/requirements/FR-SY-10.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn spent_captures(
    refs: &[UnresolvedRefRow],
    swept: &HashSet<&str>,
    live: &HashSet<&str>,
    final_bound: impl Fn(&UnresolvedRefRow) -> bool,
) -> HashSet<i64> {
    refs.iter()
        .filter(|r| {
            let source = r.source_symbol.as_str();
            r.form == RefForm::Symbol
                && (final_bound(r) || swept.contains(source) || !live.contains(source))
        })
        .map(|r| r.id)
        .collect()
}

/// The edges a bind [`Outcome`](binder::Outcome) produces: none, one, or one
/// per target of a set.
fn bound_edges(o: &binder::Outcome) -> Vec<(NodeId, NodeId, EdgeKind)> {
    match o {
        binder::Outcome::Bound {
            source, target, kind, ..
        } => vec![(*source, *target, *kind)],
        binder::Outcome::BoundMany {
            source,
            targets,
            kind,
            ..
        } => targets.iter().map(|t| (*source, *t, *kind)).collect(),
        binder::Outcome::Unbound => Vec::new(),
    }
}

/// `true` when a bind [`Outcome`](binder::Outcome) produced at least one edge.
fn is_bound(o: &binder::Outcome) -> bool {
    matches!(
        o,
        binder::Outcome::Bound { .. } | binder::Outcome::BoundMany { .. }
    )
}

/// Whether the incremental run must re-bind row `r` given `delta`.
///
/// Seven reasons force a re-bind; any one suffices:
/// 1. **A** — `r` belongs to a file re-extracted or removed this sync. Its source
///    may have moved, and capture-before-delete lands inbound cross-file edges
///    here as `Symbol` rows ([ADR-10]); both need rebinding.
/// 2. **Artifact fallback** — a cross-artifact reference (CR-011) resolves against
///    file-path/route buckets whose normalization can erase the literal a token
///    test would key on. They are a small minority, so re-bind them whenever
///    anything changed rather than reason about their normalization.
/// 3. **Path-specifier fallback** (S-439) — an import written as a path binds
///    against the file tree and the `go.mod` module declarations, and neither is
///    a name a token test sees: `'.'` spells no token at all, a file added to a
///    Go module's root package shares none with the module path, and a `go.mod`
///    carries no node. So every `Imports` row from a path-grammar file — and
///    every call recorded through one of its imports (S-440), which binds within
///    that import's targets ([`is_import_scoped`]) — is re-bound whenever
///    anything changed: the same stance as the artifact fallback, for the same
///    reason.
/// 4. **B** — the row's target (or a name its file's `as`-aliases expand that
///    target through) is a token this sync added or removed, so its candidate set
///    may have changed. Delegated to [`binder::Index::ref_affected`].
/// 5. **Hierarchy** (S-468; S-522) — `moved.hierarchy`: the sync dirtied a type
///    of the `Extends` hierarchy, and `r` is a call from a file that can walk
///    it — package-shaped, or of a language whose types record a hierarchy
///    row ([`binder::Index::walks_hierarchy`]). Such a call may bind through a
///    supertype walk that crosses the moved type while spelling none of its
///    names — `Leaf::start` reaching `Base.start` through a `Mid` that just
///    gained `extends Base`, or a Python `self.start()` through a base that
///    just gained `(Base)`. A language that records no supertype — Rust —
///    keeps its selection.
/// 6. **Global wildcard** (S-518) — `delta.global_imports_moved`: a changed file
///    declared a C# `global using` before or after the sync, and `r` is from a
///    package-shaped file. The wildcard brings names into view in files the sync
///    never touched, under names no dirty token spells.
/// 7. **Import roots** (S-519) — `moved.import_roots`: the sync added or removed
///    a package file beneath a candidate import root (`src/pkg/__init__.py`), so
///    the detected roots — and with them every key of that language — may have
///    moved, and `r` is from a file keyed under import roots.
///
/// Every other row provably keeps its binding (its source is in an untouched file
/// and no key it reads changed), so it is skipped — that is where the work goes.
///
/// [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
fn is_affected(
    r: &UnresolvedRefRow,
    delta: &Delta,
    file_paths: &HashMap<i64, String>,
    index: &binder::Index,
    moved: Moved,
) -> bool {
    if let Some(path) = r.file_id.and_then(|id| file_paths.get(&id)) {
        if delta.changed_paths.contains(path) {
            return true;
        }
        if is_import_scoped(r) && index.is_path_specifier_file(path) {
            return true;
        }
        if moved.hierarchy && r.kind == EdgeKind::Calls && index.walks_hierarchy(path) {
            return true;
        }
        if delta.global_imports_moved && index.is_package_shaped(path) {
            return true;
        }
        if moved.import_roots && index.has_import_roots(path) {
            return true;
        }
    }
    if r.kind.is_config_reference() {
        return true;
    }
    index.ref_affected(r, &delta.dirty_tokens)
}

/// What an incremental run decided once, before selecting rows: whether the
/// sync moved a type of the package-shaped `Extends` hierarchy (S-468), and
/// whether it moved the detected import roots (S-519).
#[derive(Debug, Clone, Copy)]
struct Moved {
    hierarchy: bool,
    import_roots: bool,
}

/// Whether `r` binds against a path-grammar import's target — an `Imports` row
/// itself (S-439), or a call recorded through one, `<import target>::<name>`
/// (S-440). Such a call reads the import's binding, which moves with the file
/// tree and the `go.mod` declarations exactly as the import does, so it is
/// re-selected on the same terms. A single-segment call names no import.
fn is_import_scoped(r: &UnresolvedRefRow) -> bool {
    r.kind == EdgeKind::Imports || (r.kind == EdgeKind::Calls && r.target.contains("::"))
}

/// Group a stream of `(relation payload, is-bound)` pairs into per-relation-class
/// [`RelationCoverage`] (CR-011, [FR-CG-11], [FR-RS-04]).
///
/// Only cross-artifact references carry a payload, so a `None` payload (every
/// code/doc/access ref) contributes nothing — the breakdown is exactly the
/// artifact wiring. The `BTreeMap` key order makes the surface deterministic
/// across runs ([NFR-RA-06]).
///
/// # An invocation arm's recorded refusals land in `unresolved`, deliberately
///
/// This function buckets on `is-bound` alone, and a keyless refusal row — the
/// broker arm's since [CR-107], the HTTP client-call arm's since [CR-120] — is
/// permanently unbound: no key, so nothing to bind to. Those rows therefore
/// raise `unresolved` for their relation class here, and with it
/// `ResolutionStats::refs_unresolved`, the `refs_resolved / refs_total` ratio
/// `navigate::status` reports as `resolution_coverage`, and the `logos stats`
/// per-relation breakdown.
///
/// That is the honest reading, not a leak: the sites were always there and were
/// previously not counted at all, which is the sparsity-as-absence dishonesty
/// [NFR-CC-04] forbids and [CR-120] corrects. What must not happen is the
/// figures moving without the reason being available, so it is stated at the two
/// places a reader meets the number — here and on
/// [`RelationCoverage::unresolved`](crate::models::RelationCoverage::unresolved).
/// Filtering them out is the alternative and is deliberately not taken: it would
/// make this breakdown disagree with the ledger it is a breakdown *of*, and the
/// reason each refusal carries is already reported by the [FR-WS-05] tier.
///
/// None of these figures is a gate input ([ADR-53], [NFR-CC-04]) — they are
/// read-model and freshness-line output — so a rising refusal count cannot move
/// `logos check` or the gated signal.
///
/// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
/// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
///
/// [FR-CG-11]: ../../../docs/specs/requirements/FR-CG-11.md
/// [FR-RS-04]: ../../../docs/specs/requirements/FR-RS-04.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(crate) fn relation_coverage<'a>(
    rows: impl IntoIterator<Item = (Option<&'a str>, bool)>,
) -> BTreeMap<String, RelationCoverage> {
    let mut map: BTreeMap<String, RelationCoverage> = BTreeMap::new();
    for (payload, bound) in rows {
        if let Some(relation) = payload {
            let entry = map.entry(relation.to_string()).or_default();
            if bound {
                entry.bound += 1;
            } else {
                entry.unresolved += 1;
            }
        }
    }
    map
}

/// The current coverage/confidence read-model straight from the ledger
/// ([FR-RS-04]) — the `coverage()` interface of the [resolution-engine]
/// component, consumed by `status`/governance surfaces (S-013+).
///
/// `edges_created` is a per-run counter and is always `0` here.
///
/// # Errors
/// Returns an error if the ledger cannot be read.
///
/// [FR-RS-04]: ../../../docs/specs/requirements/FR-RS-04.md
/// [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
pub fn coverage(store: &dyn GraphStore) -> Result<ResolutionStats> {
    // Capture-before-delete (`Symbol`-form) rows are no reference of the
    // readout — each duplicates its source file's own row (S-598, CR-195) — so
    // this reads the population `GraphStore::counts` does.
    let refs: Vec<_> = store
        .unresolved_refs()?
        .into_iter()
        .filter(|r| r.form != RefForm::Symbol)
        .collect();
    let total = refs.len() as u64;
    let resolved = refs.iter().filter(|r| r.resolved).count() as u64;
    let by_relation = relation_coverage(refs.iter().map(|r| (r.payload.as_deref(), r.resolved)));
    Ok(stats(total, resolved, 0, by_relation))
}

/// Resolution coverage **per language, with its denominator** ([FR-RS-09],
/// [S-441], [CR-142] D3) — the per-language half of the `coverage()` interface
/// of the [resolution-engine], read straight from the graph.
///
/// [`coverage`] answers "how much of the ledger is bound?" once for the whole
/// graph, and that single ratio is what let a language binding nothing across
/// a file boundary hide inside a Rust-dominated aggregate for 74 sprints. This
/// answers it per language and per relation class, and splits the resolved
/// edges on whether they cross a file boundary, so *resolves only same-file
/// references* and *resolves none* are two different rows. Each class's
/// cross-file figure passes through
/// [`RelationResolution::measured`](crate::models::RelationResolution::measured),
/// so a zero is always a named state and never a count.
///
/// Computed once and consumed twice ([CR-143] §3.5): `status` renders it, and
/// the relational answers attach the row for the language they were computed
/// over ([S-442]).
///
/// A pure read that persists nothing ([ADR-28]).
///
/// # Errors
/// Returns an error if the graph cannot be read.
///
/// [FR-RS-09]: ../../../docs/specs/requirements/FR-RS-09.md
/// [S-441]: ../../../docs/planning/journal.md#s-441-resolution-coverage-is-reported-per-language-with-its-denominator
/// [S-442]: ../../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
/// [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
/// [CR-143]: ../../../docs/requests/CR-143-a-relational-answer-states-its-resolution-denominator.md
/// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
/// [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
pub fn coverage_by_language(store: &dyn GraphStore) -> Result<Vec<LanguageResolution>> {
    Ok(store
        .resolution_by_language()?
        .into_iter()
        .map(|row| LanguageResolution {
            language: row.language,
            files: row.files,
            calls: measured(row.calls),
            imports: measured(row.imports),
            call_residue: None,
        })
        .collect())
}

/// Why each package-shaped language's unbound `Calls` rows stay unbound, by
/// reason ([FR-RS-10], [S-468], [CR-150] §3.2 C) — and Rust's ([FR-RS-42],
/// S-589) — the `call_residue` of the `status` row
/// ([`LanguageResolution::call_residue`]), keyed by `files.language`.
///
/// Each unbound `Calls` row of a package-shaped file, or of a file whose
/// language proves receivers through peeled wrappers
/// ([`PackageLayout::peels_receivers`]: Rust), is re-walked by the binder
/// under `policy` and counted under the reason the walk gave up with
/// ([`binder::residue`]), so a reason can never describe a path the bind did not
/// take. A Rust row the walk records no reason for — anything but a receiver
/// call, a `Self::m` call inside an `impl` or a proven `T::m` call — is
/// `unclassified`.
/// The index is [`run`]'s, built with the same package layout, minus the
/// path-specifier and imported-binding scopes `run` chains on: those are read
/// only for a path-grammar (TypeScript, JavaScript, Go) file, never for a
/// package-shaped row. The population is
/// the per-language ledger's: rows of a file that records a language, less the
/// capture-before-delete (`Symbol`-form) rows ([ADR-10], S-598), so `unbound`
/// equals the row's `calls.references − calls.bound`.
///
/// [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
///
/// Empty — and nothing is read beyond the file listing and its declared
/// namespaces — when no file is package-shaped or Rust, so a graph of
/// path-grammar languages alone pays for none of it. A pure read that persists
/// nothing ([ADR-28]); it is `status`'s alone, because the relational answers
/// attach their rows from the aggregate reads and must stay cheap.
///
/// The split between `external-type` and `type-in-another-member` needs the
/// other members' declared types, so it is decided by the workspace
/// ([`CallResidue::split_by_workspace`]); here every such row is
/// `external-type`, under `scope: "repository"`.
///
/// # Errors
/// Returns an error if the graph cannot be read.
///
/// [FR-RS-10]: ../../../docs/specs/requirements/FR-RS-10.md
/// [FR-RS-42]: ../../../docs/specs/requirements/FR-RS-42.md
/// [S-468]: ../../../docs/planning/journal.md#s-468-a-type-qualified-java-call-binds-among-its-types-members-and-in-repo-supertypes
/// [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
/// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
/// [`LanguageResolution::call_residue`]: crate::models::LanguageResolution::call_residue
/// [`CallResidue::split_by_workspace`]: crate::models::CallResidue::split_by_workspace
/// [`PackageLayout::peels_receivers`]: package_key::PackageLayout::peels_receivers
pub(crate) fn call_residue_by_language(
    store: &dyn GraphStore,
    registry: &LanguageRegistry,
    policy: BindingPolicy,
) -> Result<BTreeMap<String, CallResidue>> {
    let layout = package_key::PackageLayout::from_registry(registry)
        .with_declared_namespaces(store.file_namespaces()?);
    let files: HashMap<i64, String> = store
        .indexed_files()?
        .into_iter()
        .filter(|f| layout.is_package_shaped(&f.path) || layout.peels_receivers(&f.path))
        .map(|f| (f.id, f.path))
        .collect();
    if files.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut paths: Vec<String> = files.values().cloned().collect();
    paths.sort();
    let mut language_of: HashMap<String, String> = HashMap::new();
    // Chunked: the reader binds one parameter per path.
    for chunk in paths.chunks(500) {
        for (path, language) in store.file_languages(chunk)? {
            if let Some(language) = language {
                language_of.insert(path, language);
            }
        }
    }

    let (nodes, edges, refs) = (store.all_nodes()?, store.all_edges()?, store.unresolved_refs()?);
    let index = binder::Index::build_with_layout(&nodes, &edges, &refs, layout)
        .with_self_types(store.node_self_types()?)
        .with_arities(&store.node_arities()?);
    let declared = index.declared_type_names();

    let mut out: BTreeMap<String, CallResidue> = BTreeMap::new();
    for language in language_of.values() {
        out.entry(language.clone()).or_insert_with(|| CallResidue {
            unbound: 0,
            reasons: CallResidue::REPOSITORY_REASONS
                .iter()
                .map(|reason| (*reason, 0))
                .collect(),
            unclassified: 0,
            scope: ResidueScope::Repository,
            external_candidates: BTreeMap::new(),
            declared_types: declared.clone(),
        });
    }
    for r in &refs {
        // A capture-before-delete row sits under its target file and duplicates
        // a call its source's own row records: it is no call site, so it is
        // neither unbound nor unclassified here (S-598, CR-195) — the same
        // population `resolution_by_language` counts.
        if r.kind != EdgeKind::Calls || r.resolved || r.form == RefForm::Symbol {
            continue;
        }
        let Some(language) = r
            .file_id
            .and_then(|id| files.get(&id))
            .and_then(|path| language_of.get(path))
        else {
            continue;
        };
        let Some(residue) = out.get_mut(language) else {
            continue;
        };
        residue.unbound += 1;
        let reason = match binder::residue(r, &index, policy) {
            None => {
                residue.unclassified += 1;
                continue;
            }
            Some(binder::Residue::NoReceiverEvidence) => CallResidueReason::NoReceiverEvidence,
            Some(binder::Residue::ExternalType { candidates }) => {
                *residue.external_candidates.entry(candidates).or_default() += 1;
                CallResidueReason::ExternalType
            }
            Some(binder::Residue::OverloadAmbiguous) => CallResidueReason::OverloadAmbiguous,
            Some(binder::Residue::TypeAmbiguous) => CallResidueReason::TypeAmbiguous,
            Some(binder::Residue::SupertypeUnreached) => CallResidueReason::SupertypeUnreached,
        };
        *residue.reasons.entry(reason).or_default() += 1;
    }
    Ok(out)
}

/// One class's raw counts, classified.
fn measured(counts: RelationCounts) -> RelationResolution {
    RelationResolution::measured(
        counts.references,
        counts.bound,
        counts.same_file_edges,
        counts.cross_file_edges,
    )
}

/// Assemble a [`ResolutionStats`], deriving the unresolved count and the
/// bound-ratio (`1.0` for an empty ledger — nothing to resolve is full
/// coverage, honestly).
fn stats(
    refs_total: u64,
    refs_resolved: u64,
    edges_created: u64,
    by_relation: BTreeMap<String, RelationCoverage>,
) -> ResolutionStats {
    let coverage = if refs_total == 0 {
        1.0
    } else {
        refs_resolved as f64 / refs_total as f64
    };
    ResolutionStats {
        refs_total,
        refs_resolved,
        refs_unresolved: refs_total - refs_resolved,
        edges_created,
        coverage,
        by_relation,
    }
}

#[cfg(test)]
mod tests;
#[cfg(all(test, feature = "lang-java"))]
mod package_rung_tests;
#[cfg(test)]
mod path_module_tests;
#[cfg(test)]
mod call_target_tests;
#[cfg(test)]
mod mod_declaration_tests;
#[cfg(test)]
mod type_relation_tests;
