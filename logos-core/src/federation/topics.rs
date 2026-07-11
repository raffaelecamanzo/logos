//! The **workspace topic inventory** read-model (S-256, [FR-WS-11], [ADR-55]).
//!
//! Projects each member's promoted per-repo topic graph — the
//! [`Topic`](NodeKind::Topic)/[`Producer`](NodeKind::Producer)/[`Consumer`](NodeKind::Consumer)
//! nodes the promotion pass ([`crate::resolve::topics`]) reconciles — onto the
//! repo-qualified summary the workspace surfaces render.
//!
//! # Why an inventory, and not just the bridge edges
//! A [`BridgeEdge`](super::bridge::BridgeEdge) exists only where a publish *met* a
//! subscribe across members. A topic that a single member publishes and nobody
//! consumes yet has **no** bridge edge — and it is exactly the topic the service
//! map must still draw, because [FR-WS-11]'s promise is that a per-repo topic is
//! visible *before* (and independently of) any cross-repo match. Reading the
//! promoted nodes rather than the bind is what keeps that promise: an unbound topic
//! is a real, rendered entity, not an absence.
//!
//! The cross-member bind is then *implied* by the inventory itself — a topic with
//! producers in one member and consumers in another **is** the coupling — and is
//! proven independently by the bridge ([`super::broker`]). The two are projections
//! of one captured fact, keyed on the same topic identity, so they cannot disagree.
//!
//! # Advisory only ([ADR-53])
//! Like every workspace read-model, this is reachable only through an
//! [`EngineRegistry`] and can never move a member's gated signal.
//!
//! [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
//! [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
//! [ADR-55]: ../../../docs/specs/architecture/decisions/ADR-55.md

use serde::Serialize;

use crate::graph_store::{EdgeRow, NodeRow};
use crate::model::{EdgeKind, NodeId, NodeKind};

use super::bridge::{read_members, MemberContracts};
use super::registry::{EngineRegistry, MemberEngine};

/// One topic in one member, with how many sites publish to and subscribe from it.
///
/// Counts are of **promoted nodes**, i.e. of `(declaration, topic)` sites — never
/// of call sites: a method that publishes one topic twice is one producer, exactly
/// as the graph records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TopicSummary {
    /// The topic key — the identity two members meet on (`orders`,
    /// `orders#com.acme.OrderCreated`).
    pub topic: String,
    /// Declarations in this member that publish to the topic.
    pub producers: usize,
    /// Declarations in this member that subscribe from the topic.
    pub consumers: usize,
}

/// One member's topic inventory, repo-qualified ([FR-WS-03]).
///
/// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberTopics {
    /// The owning member's name (its workspace-relative path).
    pub member: String,
    /// Its promoted topics, sorted by key ([NFR-RA-06]).
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub topics: Vec<TopicSummary>,
}

/// Project one member's `(nodes, edges)` read onto its topic inventory.
///
/// A [`Topic`](NodeKind::Topic) node is the unit; its producer/consumer counts come
/// from the [`Publishes`](EdgeKind::Publishes)/[`Subscribes`](EdgeKind::Subscribes)
/// edges pointing at it. An edge whose endpoint is not the kind it should be is
/// **not** counted — the promotion pass is the only writer of these edges, so a
/// mismatch means the graph is mid-write or corrupt, and inflating a count from it
/// would be a fabrication ([NFR-RA-05]).
///
/// A member with no promoted topic yields an empty inventory (no allocation beyond
/// the empty vec) — the no-topic path stays free.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub(super) fn topic_summaries_from(nodes: &[NodeRow], edges: &[EdgeRow]) -> Vec<TopicSummary> {
    use std::collections::HashMap;

    // Topic id → its key. Empty for every repo that indexes no broker topic, which
    // short-circuits the whole projection.
    let topics: HashMap<NodeId, &str> = nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Topic)
        .map(|n| (n.id, n.name.as_str()))
        .collect();
    if topics.is_empty() {
        return Vec::new();
    }

    let kind_of: HashMap<NodeId, NodeKind> = nodes.iter().map(|n| (n.id, n.kind)).collect();
    // (producers, consumers) per topic id.
    let mut counts: HashMap<NodeId, (usize, usize)> = HashMap::new();

    for edge in edges {
        // The source kind each broker edge must have; anything else is not a fact
        // this pass wrote, so it is ignored rather than counted.
        let want_source = match edge.kind {
            EdgeKind::Publishes => NodeKind::Producer,
            EdgeKind::Subscribes => NodeKind::Consumer,
            _ => continue,
        };
        if !topics.contains_key(&edge.target) || kind_of.get(&edge.source) != Some(&want_source) {
            continue;
        }
        let entry = counts.entry(edge.target).or_default();
        match edge.kind {
            EdgeKind::Publishes => entry.0 += 1,
            _ => entry.1 += 1,
        }
    }

    let mut out: Vec<TopicSummary> = topics
        .into_iter()
        .map(|(id, topic)| {
            let (producers, consumers) = counts.get(&id).copied().unwrap_or((0, 0));
            TopicSummary {
                topic: topic.to_string(),
                producers,
                consumers,
            }
        })
        .collect();
    out.sort_by(|a, b| a.topic.cmp(&b.topic)); // deterministic (NFR-RA-06)
    out
}

/// The workspace topic inventory: every member's promoted topics, repo-qualified
/// ([FR-WS-11]).
///
/// A member whose engine fails to start, or whose read fails, is **skipped with a
/// warning** rather than aborting the answer — the degraded-not-fatal discipline
/// every workspace read-model follows ([ADR-53]). A member with no topics
/// contributes an empty inventory, so the caller can still tell "this member has no
/// topics" apart from "this member could not be read" (the latter is simply absent).
///
/// [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
pub fn workspace_topics<E>(registry: &EngineRegistry<E>) -> Vec<MemberTopics>
where
    E: MemberEngine + MemberContracts,
{
    read_members(registry, "topic surface", |engine| engine.topic_surface())
        .into_iter()
        .map(|(member, topics)| MemberTopics { member, topics })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LogosSymbol;

    fn node(id: i64, kind: NodeKind, name: &str) -> NodeRow {
        NodeRow {
            id: NodeId(id),
            symbol: LogosSymbol::parse(&format!("local n{id}")).unwrap(),
            kind,
            name: name.to_string(),
            file_path: None,
            start_line: None,
            end_line: None,
        }
    }

    fn edge(source: i64, target: i64, kind: EdgeKind) -> EdgeRow {
        EdgeRow {
            source: NodeId(source),
            target: NodeId(target),
            kind,
        }
    }

    /// The inventory counts the publish and subscribe *sites* hung off each topic,
    /// sorted by key — the per-repo topic graph the service map renders ([FR-WS-11]).
    ///
    /// [FR-WS-11]: ../../../../docs/specs/requirements/FR-WS-11.md
    #[test]
    fn the_inventory_counts_producers_and_consumers_per_topic() {
        let nodes = [
            node(1, NodeKind::Topic, "shipments"),
            node(2, NodeKind::Topic, "orders"),
            node(3, NodeKind::Producer, "orders"),
            node(4, NodeKind::Producer, "orders"),
            node(5, NodeKind::Consumer, "orders"),
            node(6, NodeKind::Consumer, "shipments"),
        ];
        let edges = [
            edge(3, 2, EdgeKind::Publishes),
            edge(4, 2, EdgeKind::Publishes),
            edge(5, 2, EdgeKind::Subscribes),
            edge(6, 1, EdgeKind::Subscribes),
        ];

        assert_eq!(
            topic_summaries_from(&nodes, &edges),
            [
                TopicSummary {
                    topic: "orders".to_string(),
                    producers: 2,
                    consumers: 1,
                },
                TopicSummary {
                    topic: "shipments".to_string(),
                    producers: 0,
                    consumers: 1,
                },
            ],
            "sorted by key; each topic carries its own site counts"
        );
    }

    /// The per-repo promise ([FR-WS-11]): a topic that is only **published** — no
    /// subscriber in this member, and (as far as this member knows) none anywhere —
    /// is still a first-class entry in the inventory, with an honest zero. This is
    /// the topic that has no bridge edge and would be invisible if the surfaces read
    /// the bind instead of the graph.
    ///
    /// [FR-WS-11]: ../../../../docs/specs/requirements/FR-WS-11.md
    #[test]
    fn a_topic_with_no_consumer_is_still_inventoried_with_an_honest_zero() {
        let nodes = [
            node(1, NodeKind::Topic, "orders"),
            node(2, NodeKind::Producer, "orders"),
        ];
        let edges = [edge(2, 1, EdgeKind::Publishes)];

        assert_eq!(
            topic_summaries_from(&nodes, &edges),
            [TopicSummary {
                topic: "orders".to_string(),
                producers: 1,
                consumers: 0,
            }],
            "a per-repo topic is visible before any cross-repo match"
        );
    }

    /// A graph with no promoted topic yields an empty inventory — the no-topic path
    /// costs one lookup and allocates nothing ([FR-WS-11], [NFR-RA-06]).
    ///
    /// [FR-WS-11]: ../../../../docs/specs/requirements/FR-WS-11.md
    /// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
    #[test]
    fn a_graph_with_no_topics_has_an_empty_inventory() {
        let nodes = [node(1, NodeKind::Function, "main")];
        let edges = [edge(1, 1, EdgeKind::Calls)];
        assert!(topic_summaries_from(&nodes, &edges).is_empty());
        assert!(topic_summaries_from(&[], &[]).is_empty());
    }

    /// Never inflate a count from an edge this pass did not write ([NFR-RA-05]): a
    /// `Publishes` edge whose source is not a `Producer`, or whose target is not a
    /// `Topic`, is ignored — not counted, and never a panic.
    ///
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn a_malformed_broker_edge_is_ignored_not_counted() {
        let nodes = [
            node(1, NodeKind::Topic, "orders"),
            node(2, NodeKind::Function, "not_a_producer"),
            node(3, NodeKind::Producer, "orders"),
            node(4, NodeKind::Function, "not_a_topic"),
        ];
        let edges = [
            // Source is a Function, not a Producer — ignored.
            edge(2, 1, EdgeKind::Publishes),
            // Target is a Function, not a Topic — ignored.
            edge(3, 4, EdgeKind::Publishes),
            // A dangling source id that is in no node row — ignored, not a panic.
            edge(99, 1, EdgeKind::Subscribes),
        ];

        assert_eq!(
            topic_summaries_from(&nodes, &edges),
            [TopicSummary {
                topic: "orders".to_string(),
                producers: 0,
                consumers: 0,
            }],
            "the topic exists, but no malformed edge inflates its counts"
        );
    }
}
