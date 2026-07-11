//! The **broker-topic promotion pass** — Pass 2¾ of the pipeline
//! ([resolution-engine], S-256, [FR-WS-11], [ADR-55]).
//!
//! Runs after the framework-promotion pass on every index/sync and promotes the
//! **ledger-only** broker references S-254 captured ([FR-WS-10]) to the
//! first-class graph entities migration 17 admitted (S-255, [FR-WS-11]):
//!
//! - each distinct topic key becomes one [`NodeKind::Topic`] node;
//! - each `(publishing declaration, topic)` becomes a [`NodeKind::Producer`]
//!   node linked to its topic by [`EdgeKind::Publishes`];
//! - each `(subscribing declaration, topic)` becomes a [`NodeKind::Consumer`]
//!   node linked to its topic by [`EdgeKind::Subscribes`].
//!
//! # The ledger is the input — no second capture
//! The pass **re-reads nothing from disk**. S-254's `brokers.scm` interpreter
//! ([`crate::extract::broker`]) already normalized every static-topic
//! publish/subscribe site into an `unresolved_refs` row tagged
//! [`BrokerPublish`](ArtifactRelation::BrokerPublish) /
//! [`BrokerSubscribe`](ArtifactRelation::BrokerSubscribe), so promotion is a pure
//! function of that ledger and the bound graph. A dynamically-composed topic was
//! already refused at capture ([NFR-RA-05]) and can never reach this pass, so no
//! topic is ever fabricated here.
//!
//! # Identity: a topic is repo-scoped, a producer/consumer is site-scoped
//! A [`Topic`](NodeKind::Topic) is the *shared* identity two sides meet on, so its
//! symbol carries **no file path** (`logos . . . topic/orders#`): two files
//! publishing `orders` share one topic node, which is precisely what makes a
//! per-repo topic graph visible *before* any cross-repo match exists
//! ([FR-WS-11], [ADR-55]). It is also the only promoted node with no `file_id` —
//! a topic is not declared at a line, and anchoring it to an arbitrary one of its
//! call sites would fabricate a location.
//!
//! A [`Producer`](NodeKind::Producer)/[`Consumer`](NodeKind::Consumer) *is* a code
//! site, so it hangs off its enclosing declaration's symbol
//! (`…/OrderService#publish().orders#`) — unique per `(declaration, topic)`, and
//! file-anchored, so re-extracting the file naturally invalidates it.
//!
//! # Reconcile, don't accumulate
//! Like the framework pass, each run recomputes the full desired set from the
//! current ledger and diffs it against what is promoted: missing nodes are
//! inserted, stale ones deleted, survivors keep their ids. The pass is therefore
//! idempotent and self-healing across syncs, and a graph whose last topic
//! disappeared is demoted cleanly.
//!
//! # A no-topic graph is byte-for-byte unaffected ([NFR-RA-06], [FR-WS-11])
//! An incremental `sync` first asks the store for a **broker footprint** — any
//! promoted broker node, or any broker-arm ledger row. A repo with neither (every
//! repo that indexes no broker topics) skips the whole-graph snapshot and writes
//! nothing at all, so its store is bit-identical with and without this pass.
//!
//! # The cross-member bind is *not* here
//! Promotion is per-repo. Binding a producer in one member to a consumer in
//! another rides the same topic identity through the workspace bridge
//! ([`crate::federation::broker`]) — see that module. The two are projections of
//! one captured fact, keyed identically, which is why they cannot disagree.
//!
//! [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
//! [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
//! [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
//! [ADR-55]: ../../../docs/specs/architecture/decisions/ADR-55.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use anyhow::Result;

use crate::extract::symbol::{descriptor_for, SymbolContext};
use crate::graph_store::{NewNode, NodeRow, UnresolvedRefRow};
use crate::model::{ArtifactRelation, EdgeKind, LogosSymbol, NodeId, NodeKind};
use crate::runtime::Runtime;

/// What one promotion run did — surfaced for tracing, not for the gated signal.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TopicStats {
    /// Distinct topic nodes in the desired set.
    pub topics: u64,
    /// Producer (publish-site) nodes in the desired set.
    pub producers: u64,
    /// Consumer (subscribe-site) nodes in the desired set.
    pub consumers: u64,
    /// Wall-clock of the pass.
    pub duration_ms: u64,
}

/// The node kinds this pass owns and reconciles.
const PROMOTED: [NodeKind; 3] = [NodeKind::Topic, NodeKind::Producer, NodeKind::Consumer];

/// Run the broker-topic promotion pass. See the module docs for the shape.
///
/// # Errors
/// Returns an error if the snapshot read or the commit batch fails (the batch
/// rolls back wholesale, [NFR-RA-07]).
///
/// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
pub fn run(runtime: &Runtime, delta: Option<&super::Delta>) -> Result<TopicStats> {
    let started = Instant::now();

    // The no-topic fast path (see the module docs): on a `sync`, a graph with no
    // broker footprint can promote nothing and has nothing to demote, so the
    // whole-graph snapshot below is provably a no-op and is skipped. One cheap
    // EXISTS read instead of the O(graph) materialisation. A full `index`
    // (`delta` is `None`) always runs the pass — its own read is the snapshot it
    // would otherwise need anyway.
    if delta.is_some() {
        let has_footprint = runtime.submit_read(|store| store.has_broker_footprint())?;
        if !has_footprint {
            return Ok(TopicStats {
                duration_ms: elapsed_ms(started),
                ..TopicStats::default()
            });
        }
    }

    // One consistent snapshot, the same basis the framework pass reconciles from.
    let (files, nodes, edges, refs) = runtime.submit_read(|store| {
        Ok((
            store.indexed_files()?,
            store.all_nodes()?,
            store.all_edges()?,
            store.unresolved_refs()?,
        ))
    })?;

    let existing: Vec<&NodeRow> = nodes
        .iter()
        .filter(|n| PROMOTED.contains(&n.kind))
        .collect();

    let file_id_by_path: HashMap<&str, i64> =
        files.iter().map(|f| (f.path.as_str(), f.id)).collect();
    let desired = desired_set(&refs, &nodes, &file_id_by_path);

    // A graph with neither a broker ref nor a promoted broker node is untouched:
    // no writer batch is opened at all, so its store stays byte-identical
    // ([FR-WS-11], [NFR-RA-06]).
    if desired.is_empty() && existing.is_empty() {
        return Ok(TopicStats {
            duration_ms: elapsed_ms(started),
            ..TopicStats::default()
        });
    }

    let count = |kind: NodeKind| desired.values().filter(|d| d.kind == kind).count() as u64;
    let stats = TopicStats {
        topics: count(NodeKind::Topic),
        producers: count(NodeKind::Producer),
        consumers: count(NodeKind::Consumer),
        duration_ms: 0, // stamped after the commit, below
    };

    commit(runtime, &existing, &edges, desired)?;
    Ok(TopicStats {
        duration_ms: elapsed_ms(started),
        ..stats
    })
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

/// One node this run wants promoted, keyed in the desired map by its symbol
/// string.
#[derive(Debug, Clone)]
struct DesiredNode {
    symbol: LogosSymbol,
    kind: NodeKind,
    name: String,
    /// The anchoring file — `None` for a [`Topic`](NodeKind::Topic), which is a
    /// repo-scoped identity rather than a declaration at a line.
    file_id: Option<i64>,
    start_line: Option<i64>,
    end_line: Option<i64>,
    edges: Vec<DesiredEdge>,
}

/// One edge a promoted node must carry after this run.
///
/// The two broker edges point at a [`Topic`](NodeKind::Topic) that may itself be
/// created in the *same* commit, so they name their target by **symbol** (resolved
/// to an id inside the writer batch) rather than by [`NodeId`] the way the
/// framework pass's already-bound targets can.
#[derive(Debug, Clone)]
enum DesiredEdge {
    /// `enclosing declaration --Contains--> promoted` (scope anchoring).
    ContainedBy(NodeId),
    /// `producer --Publishes--> topic` ([FR-WS-11]).
    Publishes(String),
    /// `consumer --Subscribes--> topic` ([FR-WS-11]).
    Subscribes(String),
}

/// The topic key, kind, and enclosing declaration one broker ledger row promotes.
struct BrokerRef<'a> {
    /// `Producer` for a publish, `Consumer` for a subscribe.
    kind: NodeKind,
    /// The arm-normalized topic key (`orders`, `orders#com.acme.OrderCreated`).
    topic: &'a str,
    /// The declaration the capture attributed the site to.
    enclosing: &'a NodeRow,
    /// 1-based line of the publish/subscribe site.
    line: Option<i64>,
}

/// Project the ledger onto the broker rows this pass promotes.
///
/// A row qualifies iff its `payload` names a broker arm relation **and** its
/// `source_symbol` resolves to a node actually in the graph. A row whose
/// enclosing declaration is unknown (its file was deleted, or the symbol never
/// bound) promotes nothing rather than hanging a producer off a fabricated
/// parent ([NFR-RA-05]).
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn broker_refs<'a>(
    refs: &'a [UnresolvedRefRow],
    node_by_symbol: &'a HashMap<&'a str, &'a NodeRow>,
) -> Vec<BrokerRef<'a>> {
    refs.iter()
        .filter_map(|row| {
            let kind = match row.payload.as_deref().and_then(ArtifactRelation::from_wire) {
                Some(ArtifactRelation::BrokerPublish) => NodeKind::Producer,
                Some(ArtifactRelation::BrokerSubscribe) => NodeKind::Consumer,
                _ => return None,
            };
            let topic = row.target.trim();
            if topic.is_empty() {
                return None; // a keyless row is not a topic — never fabricate one
            }
            let enclosing = node_by_symbol.get(row.source_symbol.as_str())?;
            Some(BrokerRef {
                kind,
                topic,
                enclosing,
                line: row.line,
            })
        })
        .collect()
}

/// The repo-scoped symbol of a topic: no path segments, so every member of the
/// repo that names `key` meets on **one** node ([FR-WS-11]).
fn topic_symbol(ctx: &SymbolContext, key: &str) -> Result<LogosSymbol> {
    crate::extract::symbol::build_symbol(
        ctx,
        &[],
        &[
            descriptor_for(NodeKind::Module, "topic", 0),
            descriptor_for(NodeKind::Topic, key, 0),
        ],
    )
}

/// The symbol of a publish/subscribe **site**: the topic descriptor hung off the
/// enclosing declaration's own symbol under a **role namespace**, so it is unique
/// per `(declaration, role, topic)` and lives in the file that declares it.
///
/// The role namespace (`producer/` | `consumer/`) is load-bearing, not decoration:
/// [`descriptor_for`] renders both roles as the same `name#` type descriptor, so
/// without it a **relay** — one declaration that subscribes to a topic and
/// re-publishes on it, the shape S-254 explicitly preserves through the ledger
/// dedup — would collide its producer and its consumer onto one symbol and
/// silently lose a real broker fact. Mirrors the framework pass's `route/` /
/// `component/` pseudo-namespace convention.
fn site_symbol(enclosing: &LogosSymbol, kind: NodeKind, key: &str) -> Result<LogosSymbol> {
    let role = match kind {
        NodeKind::Producer => "producer",
        _ => "consumer",
    };
    LogosSymbol::parse(&format!(
        "{}{}{}",
        enclosing.as_str(),
        descriptor_for(NodeKind::Module, role, 0),
        descriptor_for(kind, key, 0)
    ))
}

/// Assemble the desired promoted set from the broker ledger rows.
///
/// Deterministic ([NFR-RA-06]): rows are folded into a [`BTreeMap`] keyed by
/// symbol, and a repeated site (the same declaration publishing one topic twice)
/// collapses to one node carrying the **first** line — one producer of a topic per
/// declaration, never a duplicate per call.
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn desired_set(
    refs: &[UnresolvedRefRow],
    nodes: &[NodeRow],
    file_id_by_path: &HashMap<&str, i64>,
) -> BTreeMap<String, DesiredNode> {
    let ctx = SymbolContext::default();
    let node_by_symbol: HashMap<&str, &NodeRow> =
        nodes.iter().map(|n| (n.symbol.as_str(), n)).collect();

    let mut desired: BTreeMap<String, DesiredNode> = BTreeMap::new();

    for r in broker_refs(refs, &node_by_symbol) {
        let Ok(topic) = topic_symbol(&ctx, r.topic) else {
            continue; // a key that cannot be encoded as a symbol is refused, not coerced
        };
        let Ok(site) = site_symbol(&r.enclosing.symbol, r.kind, r.topic) else {
            continue;
        };

        // The shared topic node. Repo-scoped: the first row to name it creates it,
        // every later row on the same key finds it.
        desired
            .entry(topic.as_str().to_string())
            .or_insert_with(|| DesiredNode {
                symbol: topic.clone(),
                kind: NodeKind::Topic,
                name: r.topic.to_string(),
                file_id: None,
                start_line: None,
                end_line: None,
                edges: Vec::new(),
            });

        // The site node, anchored under the declaration that publishes/subscribes.
        let file_id = r
            .enclosing
            .file_path
            .as_deref()
            .and_then(|p| file_id_by_path.get(p).copied());
        let broker_edge = match r.kind {
            NodeKind::Producer => DesiredEdge::Publishes(topic.as_str().to_string()),
            _ => DesiredEdge::Subscribes(topic.as_str().to_string()),
        };
        let entry = desired
            .entry(site.as_str().to_string())
            .or_insert_with(|| DesiredNode {
                symbol: site,
                kind: r.kind,
                name: r.topic.to_string(),
                file_id,
                start_line: r.line,
                end_line: r.line,
                edges: vec![DesiredEdge::ContainedBy(r.enclosing.id), broker_edge],
            });
        // A second capture of the same site keeps the earliest line — deterministic
        // regardless of ledger order.
        if let (Some(existing), Some(line)) = (entry.start_line, r.line) {
            if line < existing {
                entry.start_line = Some(line);
                entry.end_line = Some(line);
            }
        }
    }

    desired
}

/// `true` for an edge kind this pass **owns** around its promoted nodes: the
/// `Contains` anchoring and the two broker edges. Every other kind incident to a
/// promoted node belongs to another pass and is left untouched, so this pass can
/// never delete an edge it did not create (the never-clobber companion of
/// never-fabricate, [NFR-RA-05]).
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn is_topic_owned(kind: EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::Contains | EdgeKind::Publishes | EdgeKind::Subscribes
    )
}

/// Reconcile the graph's promoted broker nodes to `desired` in one writer batch:
/// delete stale nodes, insert missing ones (id-stable for survivors), and
/// re-prove every promoted edge.
fn commit(
    runtime: &Runtime,
    existing: &[&NodeRow],
    edges: &[crate::graph_store::EdgeRow],
    desired: BTreeMap<String, DesiredNode>,
) -> Result<()> {
    let existing_by_symbol: HashMap<&str, NodeId> =
        existing.iter().map(|n| (n.symbol.as_str(), n.id)).collect();

    let stale: Vec<NodeId> = existing
        .iter()
        .filter(|n| !desired.contains_key(n.symbol.as_str()))
        .map(|n| n.id)
        .collect();

    // Edges currently incident to *surviving* promoted nodes, restricted to the
    // kinds this pass owns — the candidates for edge-level reconciliation. (Edges
    // on stale nodes cascade away with the node delete below.)
    let surviving: HashSet<NodeId> = existing
        .iter()
        .filter(|n| desired.contains_key(n.symbol.as_str()))
        .map(|n| n.id)
        .collect();
    let current_edges: Vec<(NodeId, NodeId, EdgeKind)> = edges
        .iter()
        .filter(|e| is_topic_owned(e.kind))
        .filter(|e| surviving.contains(&e.source) || surviving.contains(&e.target))
        .map(|e| (e.source, e.target, e.kind))
        .collect();

    // The work list, moved into the writer closure. `desired` is a BTreeMap, so
    // the commit order is the symbol order — deterministic ([NFR-RA-06]).
    let plan: Vec<(Option<NodeId>, DesiredNode)> = desired
        .into_values()
        .map(|d| (existing_by_symbol.get(d.symbol.as_str()).copied(), d))
        .collect();

    runtime.submit_write(move |w| {
        // 1) Retire stale promoted nodes (their edges cascade).
        for id in &stale {
            w.delete_node(*id)?;
        }

        // 2) Ensure every desired node exists, remembering its id **by symbol** —
        //    a `Publishes`/`Subscribes` edge names a topic that may have been
        //    created moments ago in this very batch.
        let mut id_by_symbol: HashMap<&str, NodeId> = HashMap::with_capacity(plan.len());
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
            id_by_symbol.insert(item.symbol.as_str(), id);
        }

        // 3) Edge reconciliation: the full desired edge set, with both endpoints
        //    resolved. A broker edge whose topic somehow has no id is dropped
        //    rather than pointed at a fabricated node.
        let mut want: HashSet<(NodeId, NodeId, EdgeKind)> = HashSet::new();
        for (_, item) in &plan {
            let Some(&self_id) = id_by_symbol.get(item.symbol.as_str()) else {
                continue;
            };
            for e in &item.edges {
                let resolved = match e {
                    DesiredEdge::ContainedBy(parent) => Some((*parent, self_id, EdgeKind::Contains)),
                    DesiredEdge::Publishes(topic) => id_by_symbol
                        .get(topic.as_str())
                        .map(|&t| (self_id, t, EdgeKind::Publishes)),
                    DesiredEdge::Subscribes(topic) => id_by_symbol
                        .get(topic.as_str())
                        .map(|&t| (self_id, t, EdgeKind::Subscribes)),
                };
                if let Some(edge) = resolved {
                    want.insert(edge);
                }
            }
        }
        // Stale edges on surviving promoted nodes (a site that no longer publishes
        // the topic it used to).
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
mod tests;
