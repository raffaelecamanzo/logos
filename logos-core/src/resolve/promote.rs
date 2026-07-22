//! The shared promotion-commit primitive — one reconcile-and-commit algorithm
//! the framework ([framework], S-012) and broker-topic ([topics], S-256)
//! passes both run (CR-082, [ADR-54]).
//!
//! Both passes recompute a full *desired* set of promoted nodes each run and
//! reconcile the graph to it in one writer batch: retire stale nodes, insert
//! the missing ones (id-stable for survivors), and re-prove every owned edge.
//! The two passes differ only in how a desired edge names its endpoints — the
//! framework pass carries already-bound [`NodeId`] targets, the topics pass
//! names its broker-edge target by **symbol** because the target [`Topic`] may
//! be created in the very same batch. That difference is the one axis
//! [`reconcile`] is parameterised over: the caller supplies a `resolve_edge`
//! closure mapping its own edge descriptor to a [`PromotedEdge`] against the
//! batch's symbol→id map. Everything else — the stale/surviving partition, the
//! owned-edge candidate scoping, the symbol-sorted commit order ([NFR-RA-06]),
//! the idempotent `insert_edge_if_absent` — is shared verbatim, so both passes'
//! promoted node/edge identity, ordering, and idempotency are unchanged.
//!
//! [framework]: super::framework
//! [topics]: super::topics
//! [`Topic`]: crate::model::NodeKind::Topic
//! [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::graph_store::{EdgeRow, NewNode, NodeRow};
use crate::model::{EdgeKind, LogosSymbol, NodeId, NodeKind};
use crate::runtime::Runtime;

/// One node a promotion pass wants in the graph after this run, keyed by its
/// canonical symbol. Generic over the pass's own edge descriptor `E` so each
/// pass keeps its edge vocabulary while sharing the reconcile.
///
/// `file_id` is `Option` because a repo-scoped identity node (a broker
/// [`Topic`](crate::model::NodeKind::Topic)) has no anchoring file; the
/// framework pass always sets `Some`.
pub(super) struct Promoted<E> {
    pub symbol: LogosSymbol,
    pub kind: NodeKind,
    pub name: String,
    pub file_id: Option<i64>,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    /// Edges this node must carry after the run, in the pass's own vocabulary;
    /// resolved to concrete endpoints by the caller's `resolve_edge`.
    pub edges: Vec<E>,
}

/// A promoted edge with both endpoints resolved to graph ids — the output of a
/// pass's `resolve_edge`, ready for the shared `want` set.
pub(super) struct PromotedEdge {
    pub source: NodeId,
    pub target: NodeId,
    pub kind: EdgeKind,
}

impl PromotedEdge {
    /// A resolved `source --kind--> target` promoted edge.
    pub(super) fn new(source: NodeId, target: NodeId, kind: EdgeKind) -> Self {
        Self {
            source,
            target,
            kind,
        }
    }
}

/// Reconcile the graph's promoted nodes to `desired` in one writer batch:
/// delete stale nodes, insert missing ones (id-stable for survivors), and
/// re-prove every promoted edge.
///
/// `is_owned` reports the edge kinds this pass **owns** around its promoted
/// nodes; only those are candidates for edge-level reconciliation. A foreign
/// edge another pass created and owns — the resolution engine's
/// `ArtifactBinding` from an OpenAPI `ApiOperation` to a route handler (S-069,
/// CR-011), say — is left untouched, so a pass can never delete an edge it did
/// not create (the never-clobber companion of never-fabricate, [NFR-RA-05]).
///
/// `resolve_edge` maps one desired edge descriptor to a concrete
/// [`PromotedEdge`] given the promoting node's id and the batch's symbol→id
/// map; it returns `None` to drop an edge whose target cannot be resolved
/// rather than point it at a fabricated node.
///
/// The commit order is the desired set's symbol order ([NFR-RA-06]): `desired`
/// is sorted by symbol here, so the caller need not pre-sort.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(super) fn reconcile<E, R>(
    runtime: &Runtime,
    existing: &[&NodeRow],
    edges: &[EdgeRow],
    desired: Vec<Promoted<E>>,
    is_owned: fn(EdgeKind) -> bool,
    resolve_edge: R,
) -> Result<()>
where
    E: Send + 'static,
    R: Fn(NodeId, &E, &HashMap<String, NodeId>) -> Option<PromotedEdge> + Send + 'static,
{
    let desired_symbols: HashSet<&str> = desired.iter().map(|d| d.symbol.as_str()).collect();
    let existing_by_symbol: HashMap<&str, NodeId> =
        existing.iter().map(|n| (n.symbol.as_str(), n.id)).collect();

    // Stale promoted nodes: in the graph, not in the desired set.
    let stale: Vec<NodeId> = existing
        .iter()
        .filter(|n| !desired_symbols.contains(n.symbol.as_str()))
        .map(|n| n.id)
        .collect();

    // Edges currently incident to *surviving* promoted nodes, restricted to the
    // kinds this pass owns — the candidates for edge-level reconciliation.
    // (Edges on stale nodes cascade away with the node delete below; any also
    // caught here merely produce a harmless 0-row delete.)
    let surviving: HashSet<NodeId> = existing
        .iter()
        .filter(|n| desired_symbols.contains(n.symbol.as_str()))
        .map(|n| n.id)
        .collect();
    let current_edges: Vec<(NodeId, NodeId, EdgeKind)> = edges
        .iter()
        .filter(|e| is_owned(e.kind))
        .filter(|e| surviving.contains(&e.source) || surviving.contains(&e.target))
        .map(|e| (e.source, e.target, e.kind))
        .collect();

    // The work list, moved into the writer closure, with each entry's existing
    // id resolved up front; sorted on the symbol string so the commit order is
    // deterministic ([NFR-RA-06]).
    let mut plan: Vec<(Option<NodeId>, Promoted<E>)> = desired
        .into_iter()
        .map(|d| (existing_by_symbol.get(d.symbol.as_str()).copied(), d))
        .collect();
    plan.sort_by(|(_, a), (_, b)| a.symbol.as_str().cmp(b.symbol.as_str()));

    runtime.submit_write(move |w| {
        // 1) Retire stale promoted nodes (their edges cascade).
        for id in &stale {
            w.delete_node(*id)?;
        }

        // 2) Ensure every desired node exists, remembering its id **by symbol** —
        //    a symbol-keyed edge names a node that may have been created moments
        //    ago in this very batch.
        let mut id_by_symbol: HashMap<String, NodeId> = HashMap::with_capacity(plan.len());
        for (existing_id, item) in &plan {
            let id = match existing_id {
                Some(id) => *id,
                None => {
                    let symbol_id = w.upsert_symbol(&item.symbol)?;
                    w.insert_node(&NewNode {
                        file_id: item.file_id,
                        start_line: item.start_line,
                        end_line: item.end_line,
                        ..NewNode::plain(symbol_id, item.kind, &item.name)
                    })?
                }
            };
            id_by_symbol.insert(item.symbol.as_str().to_string(), id);
        }

        // 3) Edge reconciliation: the full desired edge set, each descriptor
        //    resolved to concrete endpoints (a target that cannot be resolved is
        //    dropped rather than fabricated).
        let mut want: HashSet<(NodeId, NodeId, EdgeKind)> = HashSet::new();
        for (_, item) in &plan {
            let Some(&self_id) = id_by_symbol.get(item.symbol.as_str()) else {
                continue;
            };
            for e in &item.edges {
                if let Some(edge) = resolve_edge(self_id, e, &id_by_symbol) {
                    want.insert((edge.source, edge.target, edge.kind));
                }
            }
        }
        // Stale edges on surviving promoted nodes (an owned link no longer
        // provable, NFR-RA-05).
        for (source, target, kind) in &current_edges {
            if !want.contains(&(*source, *target, *kind)) {
                w.delete_edge(*source, *target, *kind)?;
            }
        }
        // Missing desired edges (idempotent for the survivors'). Sorted so the
        // insert order is deterministic ([NFR-RA-06]).
        let mut ordered: Vec<&(NodeId, NodeId, EdgeKind)> = want.iter().collect();
        ordered.sort_by_key(|(s, t, k)| (*s, *t, k.as_i32()));
        for (source, target, kind) in ordered {
            w.insert_edge_if_absent(*source, *target, *kind)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests;
