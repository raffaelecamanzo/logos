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
//!    bind flip back to retry state. One transaction, atomic rollback
//!    ([NFR-RA-07]).
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
//!
//! [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
//! [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
//! [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
//! [AQ-04]: ../../../docs/specs/architecture.md#14-open-questions
//! [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
//! [NFR-RA-11]: ../../../docs/specs/requirements/NFR-RA-11.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [FR-RS-04]: ../../../docs/specs/requirements/FR-RS-04.md
//! [UAT-RS-01]: ../../../docs/specs/requirements/UAT-RS-01.md

mod binder;
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
/// The shared positional route-template normalizer (S-069, CR-011): aligns the
/// OpenAPI `ApiOperation` path templates with framework-extracted `route` node
/// templates under one parameter-position-only comparison. See its module docs.
pub(crate) mod grpc_key;
pub(crate) mod route_template;
/// The HTTP client-call arm normalizer (S-252, CR-061, FR-WS-08): reduces a
/// captured outbound call to its `"METHOD /template"` bind target or the reason
/// it is honestly unbindable (base-url-runtime / path-not-composed). The one
/// arm-specific piece of the pluggable invocation-arm contract. See its module docs.
pub(crate) mod http_client_call;
/// The broker-topic promotion pass (S-256, CR-061, FR-WS-11, ADR-55): promotes the
/// ledger-only broker publish/subscribe references S-254 captured to first-class
/// `topic`/`producer`/`consumer` nodes joined by `publishes`/`subscribes` edges —
/// reconciled every run, and provably inert on a graph with no broker topics. See
/// its module docs.
pub mod topics;

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use rayon::prelude::*;

use crate::config::BindingPolicy;
use crate::graph_store::{EdgeRow, GraphStore, NodeRow, UnresolvedRefRow};
use crate::models::pipeline::{RelationCoverage, ResolutionStats};
use crate::runtime::Runtime;

/// The consistent graph state one resolution run binds against.
struct Snapshot {
    nodes: Vec<NodeRow>,
    edges: Vec<EdgeRow>,
    refs: Vec<UnresolvedRefRow>,
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
    /// Project-relative paths re-extracted or removed this sync.
    pub changed_paths: HashSet<String>,
    /// Tokenized names this sync added or removed (see [`tokens`]).
    pub dirty_tokens: HashSet<String>,
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
/// # Errors
/// Returns an error if the snapshot read or the commit batch fails (the
/// batch rolls back wholesale, [NFR-RA-07]).
///
/// [FR-RS-04]: ../../../docs/specs/requirements/FR-RS-04.md
/// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
pub fn run(
    runtime: &Runtime,
    policy: BindingPolicy,
    delta: Option<&Delta>,
) -> Result<ResolutionStats> {
    let want_file_paths = delta.is_some();
    let snap = runtime.submit_read(|store| {
        Ok(Snapshot {
            nodes: store.all_nodes()?,
            edges: store.all_edges()?,
            refs: store.unresolved_refs()?,
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

    let index = binder::Index::build(&snap.nodes, &snap.edges, &snap.refs);

    // A full index (no delta) re-binds the whole ledger. An incremental sync
    // re-binds only the rows whose outcome the change-set can move; every other
    // row provably keeps the edge and resolved flag the snapshot already holds,
    // so the committed graph is byte-identical to a full re-bind (the CR-015
    // equivalence invariant, guarded by `tests/indexing.rs`). Binding a handful
    // of rows instead of the entire ~40k ledger on every core is the melt fix.
    let selected: Vec<&UnresolvedRefRow> = match delta {
        None => snap.refs.iter().collect(),
        Some(d) if d.changed_paths.is_empty() && d.dirty_tokens.is_empty() => Vec::new(),
        Some(d) => snap
            .refs
            .iter()
            .filter(|&r| is_affected(r, d, &snap.file_paths, &index))
            .collect(),
    };

    // Parallel compute on the shared worker pool (AQ-04): pure binding against
    // the immutable index; `collect` preserves input order (NFR-RA-06).
    let outcomes: Vec<(i64, bool, binder::Outcome)> = runtime.worker_pool().install(|| {
        selected
            .par_iter()
            .map(|&r| (r.id, r.resolved, binder::bind(r, &index, policy)))
            .collect()
    });

    // Stats are over the WHOLE ledger, not just the re-bound subset: a row this
    // run touched uses its fresh outcome, an untouched row reads through to its
    // snapshot resolved flag (equal, by the invariant above, to what a re-bind
    // would compute). `bound_now` indexes the touched rows; `final_bound` merges.
    let bound_now: HashMap<i64, bool> =
        outcomes.iter().map(|(id, _, o)| (*id, is_bound(o))).collect();
    let final_bound =
        |r: &UnresolvedRefRow| -> bool { bound_now.get(&r.id).copied().unwrap_or(r.resolved) };

    let refs_total = snap.refs.len() as u64;
    let refs_resolved = snap.refs.iter().filter(|&r| final_bound(r)).count() as u64;

    // Per-relation-class coverage for the cross-artifact references (CR-011,
    // FR-CG-11): the relation token rides on each ledger row's payload; group by
    // it on the row's final bound state. Computed before `outcomes` moves into
    // the write batch below.
    let by_relation =
        relation_coverage(snap.refs.iter().map(|r| (r.payload.as_deref(), final_bound(r))));

    // Serial commit: one transaction through the writer actor (ADR-02).
    let edges_created = runtime.submit_write(move |w| {
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
                    if !was_resolved {
                        w.mark_ref_resolved(*ref_id, true)?;
                    }
                }
                binder::Outcome::BoundMany {
                    source,
                    targets,
                    kind,
                    payload,
                } => {
                    // A module call fans out to every admitted `.tf` in its source
                    // dir (CR-011): one edge per target, all sharing the relation
                    // payload, all idempotent. `targets` is non-empty, so the row
                    // is resolved.
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
                    if !was_resolved {
                        w.mark_ref_resolved(*ref_id, true)?;
                    }
                }
                binder::Outcome::Unbound => {
                    // A previously bound row whose target vanished flips back
                    // to retry state — the ledger never lies (NFR-CC-04).
                    if *was_resolved {
                        w.mark_ref_resolved(*ref_id, false)?;
                    }
                }
            }
        }
        Ok(created)
    })?;

    Ok(stats(refs_total, refs_resolved, edges_created, by_relation))
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
/// Three reasons force a re-bind; any one suffices:
/// 1. **A** — `r` belongs to a file re-extracted or removed this sync. Its source
///    may have moved, and capture-before-delete lands inbound cross-file edges
///    here as `Symbol` rows ([ADR-10]); both need rebinding.
/// 2. **Artifact fallback** — a cross-artifact reference (CR-011) resolves against
///    file-path/route buckets whose normalization can erase the literal a token
///    test would key on. They are a small minority, so re-bind them whenever
///    anything changed rather than reason about their normalization.
/// 3. **B** — the row's target (or a name its file's `as`-aliases expand that
///    target through) is a token this sync added or removed, so its candidate set
///    may have changed. Delegated to [`binder::Index::ref_affected`].
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
) -> bool {
    if let Some(path) = r.file_id.and_then(|id| file_paths.get(&id)) {
        if delta.changed_paths.contains(path) {
            return true;
        }
    }
    if r.kind.is_config_reference() {
        return true;
    }
    index.ref_affected(r, &delta.dirty_tokens)
}

/// Group a stream of `(relation payload, is-bound)` pairs into per-relation-class
/// [`RelationCoverage`] (CR-011, [FR-CG-11], [FR-RS-04]).
///
/// Only cross-artifact references carry a payload, so a `None` payload (every
/// code/doc/access ref) contributes nothing — the breakdown is exactly the
/// artifact wiring. The `BTreeMap` key order makes the surface deterministic
/// across runs ([NFR-RA-06]).
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
    let refs = store.unresolved_refs()?;
    let total = refs.len() as u64;
    let resolved = refs.iter().filter(|r| r.resolved).count() as u64;
    let by_relation = relation_coverage(refs.iter().map(|r| (r.payload.as_deref(), r.resolved)));
    Ok(stats(total, resolved, 0, by_relation))
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
