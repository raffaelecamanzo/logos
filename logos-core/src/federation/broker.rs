//! The **message-broker publish/subscribe** cross-service arm's bind side
//! (S-254, [FR-WS-10], [ADR-54]).
//!
//! This is the [`BrokerTopic`](crate::model::BridgeNamespace::BrokerTopic)
//! fan-out arm's *classifier*: it turns the topic-keyed publish/subscribe
//! references the extraction capture emits
//! ([`capture_invocation_refs`](crate::extract::config::capture_invocation_refs))
//! into the `(PortableKey, Role)` candidates the **unchanged** namespace-generic
//! match loop ([`bridge::match_indexed`]) already fans out. Adding the arm
//! therefore touches neither the match loop nor `bridge::classify`; it only maps
//! the arm's own relations onto the pre-existing `BrokerTopic` namespace via the
//! two pure descriptors [`ArtifactRelation::bridge_namespace`] /
//! [`ArtifactRelation::bridge_role`].
//!
//! # Fan-out orientation
//! A **publish** is the [`Consumer`](Role::Consumer) (the edge `from`); a
//! **subscribe** is the [`Provider`](Role::Provider), indexed by topic. The
//! match loop's fan-out discipline — "a consumer binds every provider of its
//! key" — therefore reads exactly as the acceptance criterion demands: *one
//! publish binds every subscribe on the same topic across members*
//! ([FR-WS-10]). Same-member pairs are the intra-repo fan-out the per-repo graph
//! already owns and are excluded by the match loop.
//!
//! # De-duplicate endpoints before the fan-out ([NFR-RA-05])
//! The match loop does **not** de-duplicate fan-out edges — it emits one edge per
//! `(consumer, provider)` pair it is handed. Two captures of the *same*
//! `(member, symbol)` endpoint on one topic (a method carrying two
//! `@KafkaListener` annotations for the same topic, a publish captured by two
//! overlapping patterns) would therefore fabricate a duplicate edge. So this
//! classifier collapses endpoints to one per `(key, role, member, symbol)`
//! **before** handing them to the loop — the S-251 review caveat made explicit.
//!
//! [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
//! [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [`bridge::match_indexed`]: super::bridge::match_indexed
//! [`ArtifactRelation::bridge_namespace`]: crate::model::ArtifactRelation::bridge_namespace
//! [`ArtifactRelation::bridge_role`]: crate::model::ArtifactRelation::bridge_role

// The arm's bind-side classifier is the reuse foundation the cross-service
// bridge-consumption story wires into the live edge stream; today it is proven
// end-to-end by this module's tests, mirroring how S-251 shipped
// `capture_invocation_refs` ahead of its arm callers.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use crate::model::{ArtifactRelation, BridgeNamespace};

use super::bridge::{match_indexed, BridgeEdge, BridgeEndpoint, PortableKey, Role};

/// One captured broker reference promoted to a bridge candidate: which side it
/// is (its [`ArtifactRelation`] arm), the already-normalized topic key it was
/// captured under (a topic name, optionally guarded by a `#`-appended
/// message-schema FQN), and its portable `(member, symbol)` identity.
#[derive(Debug, Clone)]
pub(super) struct BrokerCandidate {
    /// The arm relation — [`BrokerPublish`](ArtifactRelation::BrokerPublish) or
    /// [`BrokerSubscribe`](ArtifactRelation::BrokerSubscribe).
    pub(super) relation: ArtifactRelation,
    /// The normalized topic key two sides meet on (`"orders"`,
    /// `"orders#com.acme.OrderCreated"`).
    pub(super) key: String,
    /// The database-portable endpoint identity.
    pub(super) endpoint: BridgeEndpoint,
}

/// Reduce a broker relation + its normalized topic key to the portable key and
/// role the bridge matches on, or `None` when `relation` is not a
/// [`BrokerTopic`](BridgeNamespace::BrokerTopic) arm.
///
/// Purely a function of the arm's two pure descriptors — the namespace-generic
/// contract ([FR-WS-07], [ADR-54]) — so it never names a match discipline: the
/// loop reads the namespace's own [`match_discipline`] (fan-out).
///
/// [FR-WS-07]: ../../../docs/specs/requirements/FR-WS-07.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
/// [`match_discipline`]: crate::model::BridgeNamespace::match_discipline
pub(super) fn classify(relation: ArtifactRelation, topic_key: &str) -> Option<(PortableKey, Role)> {
    // Only the broker-topic namespace is this arm's business; an HTTP/gRPC arm
    // (or a non-arm contract relation) is classified elsewhere / not at all.
    let namespace = relation.bridge_namespace()?;
    if namespace != BridgeNamespace::BrokerTopic {
        return None;
    }
    let role = relation.bridge_role()?;
    Some((PortableKey::broker(topic_key.to_string()), role))
}

/// Fan out captured broker candidates into cross-service edges through the
/// **unchanged** namespace-generic match loop (S-254, [FR-WS-10]).
///
/// Publishers become [`Consumer`](Role::Consumer) keys and subscribers the
/// [`Provider`](Role::Provider) index; each publish then binds **every**
/// cross-member subscribe on its topic. Endpoints are de-duplicated to one per
/// `(key, role, member, symbol)` first, because the loop does not de-duplicate
/// fan-out edges (see the module docs). Non-broker candidates are ignored.
///
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
pub(super) fn broker_edges(
    candidates: impl IntoIterator<Item = BrokerCandidate>,
) -> Vec<BridgeEdge> {
    let mut providers: HashMap<PortableKey, Vec<BridgeEndpoint>> = HashMap::new();
    let mut consumers: Vec<(PortableKey, BridgeEndpoint)> = Vec::new();
    // One endpoint per (key, role, member, symbol): the fan-out loop emits an
    // edge per pair, so a duplicated endpoint here would duplicate the edge.
    let mut seen: HashSet<(PortableKey, bool, String, String)> = HashSet::new();

    for cand in candidates {
        let Some((key, role)) = classify(cand.relation, &cand.key) else {
            continue;
        };
        let is_provider = matches!(role, Role::Provider);
        let dedup_key = (
            key.clone(),
            is_provider,
            cand.endpoint.member.clone(),
            cand.endpoint.symbol.as_str().to_string(),
        );
        if !seen.insert(dedup_key) {
            continue; // a repeat of this exact endpoint on this topic — drop it
        }
        match role {
            Role::Provider => providers.entry(key).or_default().push(cand.endpoint),
            Role::Consumer => consumers.push((key, cand.endpoint)),
        }
    }

    match_indexed(providers, consumers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_store::SqliteGraphStore;
    use crate::model::LogosSymbol;

    fn cand(relation: ArtifactRelation, key: &str, member: &str, symbol: &str) -> BrokerCandidate {
        BrokerCandidate {
            relation,
            key: key.to_string(),
            endpoint: BridgeEndpoint {
                member: member.to_string(),
                symbol: LogosSymbol::parse(symbol).unwrap(),
            },
        }
    }
    fn pubc(key: &str, member: &str, symbol: &str) -> BrokerCandidate {
        cand(ArtifactRelation::BrokerPublish, key, member, symbol)
    }
    fn subc(key: &str, member: &str, symbol: &str) -> BrokerCandidate {
        cand(ArtifactRelation::BrokerSubscribe, key, member, symbol)
    }

    /// The classifier maps the arm's relations onto the fan-out `BrokerTopic`
    /// namespace by its pure descriptors, and refuses a non-broker relation —
    /// proving it is scoped to this arm and drives the generic namespace, not a
    /// hardcoded match.
    #[test]
    fn classify_maps_broker_relations_and_refuses_others() {
        let (key, role) = classify(ArtifactRelation::BrokerPublish, "orders").unwrap();
        assert_eq!(key.relation(), "broker-topic");
        assert!(matches!(role, Role::Consumer), "a publish is the consumer side");

        let (_, role) = classify(ArtifactRelation::BrokerSubscribe, "orders").unwrap();
        assert!(matches!(role, Role::Provider), "a subscribe is the provider side");

        assert!(
            classify(ArtifactRelation::Route, "GET /x").is_none(),
            "a non-broker relation is not this arm's candidate"
        );
    }

    /// Acceptance (1): a publish on `orders` binds **every** subscribe on
    /// `orders` across members (fan-out), and a same-member subscribe is the
    /// intra-repo fan-out, excluded ([FR-WS-10]).
    #[test]
    fn one_publish_fans_out_to_every_cross_member_subscribe() {
        let edges = broker_edges([
            pubc("orders", "api", "local pub_orders"),
            subc("orders", "billing", "local sub_bill"),
            subc("orders", "ship", "local sub_ship"),
            subc("orders", "api", "local sub_local"), // same member — intra-repo
        ]);

        assert_eq!(
            edges.len(),
            2,
            "one publish fans out to both cross-member subscribers: {edges:?}"
        );
        let tos: Vec<&str> = edges.iter().map(|e| e.to.member.as_str()).collect();
        assert!(tos.contains(&"billing") && tos.contains(&"ship"));
        assert!(
            !tos.contains(&"api"),
            "the same-member subscriber is intra-repo, not a bridge edge"
        );
        for e in &edges {
            assert_eq!(e.relation, "broker-topic");
            assert_eq!(e.from.member, "api", "the publish is the edge source");
            assert_eq!(e.from.symbol.as_str(), "local pub_orders");
        }
    }

    /// Acceptance (2a): a publish and a subscribe on the same topic but with
    /// **different** message-schema FQNs do not bind — the FQN guard rides the
    /// key, so the two sides never meet. The matching-FQN pair still binds
    /// ([FR-WS-10]).
    #[test]
    fn a_differing_message_schema_fqn_prevents_the_bind() {
        let diff = broker_edges([
            pubc("orders#com.acme.OrderCreated", "api", "local pub"),
            subc("orders#com.acme.OrderUpdated", "billing", "local sub"),
        ]);
        assert!(
            diff.is_empty(),
            "a differing schema FQN keeps the topics apart — no bind: {diff:?}"
        );

        let same = broker_edges([
            pubc("orders#com.acme.OrderCreated", "api", "local pub"),
            subc("orders#com.acme.OrderCreated", "billing", "local sub"),
        ]);
        assert_eq!(same.len(), 1, "the matching-FQN pair binds: {same:?}");
        assert_eq!(same[0].to.member, "billing");
    }

    /// The S-254/S-251 review caveat: the fan-out loop does not de-duplicate its
    /// edges, so the classifier must emit no repeated `(member, symbol)` endpoint.
    /// A publish and a subscriber each captured twice on one topic must yield
    /// exactly **one** edge, not four.
    #[test]
    fn duplicate_endpoints_are_deduped_to_a_single_edge() {
        let edges = broker_edges([
            pubc("orders", "api", "local pub"),
            pubc("orders", "api", "local pub"), // same publish captured twice
            subc("orders", "billing", "local sub"),
            subc("orders", "billing", "local sub"), // same subscribe captured twice
        ]);
        assert_eq!(
            edges.len(),
            1,
            "a repeated endpoint must not fabricate a repeated fan-out edge: {edges:?}"
        );
        assert_eq!(edges[0].from.symbol.as_str(), "local pub");
        assert_eq!(edges[0].to.symbol.as_str(), "local sub");
    }

    /// An intra-repo publish→subscribe pair (both in one member) is the local
    /// graph's own fan-out — never a cross-service bridge edge ([FR-WS-10] neutral
    /// consequence, [ADR-54]).
    #[test]
    fn an_intra_repo_publish_subscribe_pair_is_not_a_bridge_edge() {
        let edges = broker_edges([
            pubc("orders", "api", "local pub_local"),
            subc("orders", "api", "local sub_local"),
        ]);
        assert!(
            edges.is_empty(),
            "an in-repo publish→subscribe pair is owned by the local graph: {edges:?}"
        );
    }

    /// Acceptance (3): the broker arm is **ledger-only** — it introduces no
    /// schema migration, so a freshly-migrated database's `PRAGMA user_version`
    /// is unchanged at 16. The relation token rides the existing free
    /// `unresolved_refs.payload` column (MIGRATION_14); no new node/edge kind and
    /// no migration are added ([FR-WS-10]).
    #[test]
    fn the_broker_arm_introduces_no_schema_migration() {
        let store = SqliteGraphStore::open_in_memory().expect("in-memory store opens");
        assert_eq!(
            store.schema_version().expect("read PRAGMA user_version"),
            16,
            "the broker arm must add no migration — user_version stays at 16"
        );
    }
}
