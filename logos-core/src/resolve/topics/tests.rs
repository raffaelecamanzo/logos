//! Unit tests for the pure core of the broker-topic promotion pass (S-256,
//! [FR-WS-11]): [`desired_set`] against in-memory ledger + node fixtures, with no
//! store involved. The end-to-end promotion (a real index over a real broker
//! source, the reconcile, the no-topic invariant) lives in
//! `tests/broker_topic_promotion.rs`.
//!
//! [FR-WS-11]: ../../../../docs/specs/requirements/FR-WS-11.md

use super::*;

use crate::graph_store::FileRecord;
use crate::model::{EdgeKind as EK, RefForm};

/// The file every fixture declaration lives in, and its ledger `file_id`.
const FILE: &str = "src/orders.java";
const FILE_ID: i64 = 7;

/// A code declaration node (the enclosing publisher/subscriber), symbol built the
/// way extraction builds it, so the fixtures exercise real SCIP symbols.
fn decl(id: i64, name: &str) -> NodeRow {
    let symbol = crate::extract::symbol::build_symbol(
        &SymbolContext::default(),
        &crate::extract::symbol::path_segments(FILE),
        &[
            descriptor_for(NodeKind::Class, "OrderService", 0),
            descriptor_for(NodeKind::Method, name, 0),
        ],
    )
    .expect("the fixture declaration symbol builds");
    NodeRow {
        id: NodeId(id),
        symbol,
        kind: NodeKind::Method,
        name: name.to_string(),
        file_path: Some(FILE.to_string()),
        start_line: Some(1),
        end_line: Some(9),
    }
}

/// One broker ledger row: `relation` under the S-254 payload token, targeting
/// `topic`, attributed to `source`'s symbol.
fn ledger(source: &NodeRow, relation: ArtifactRelation, topic: &str, line: i64) -> UnresolvedRefRow {
    UnresolvedRefRow {
        id: 0,
        file_id: Some(FILE_ID),
        source_symbol: source.symbol.as_str().to_string(),
        target: topic.to_string(),
        alias: None,
        form: RefForm::Method,
        kind: EK::ArtifactRef,
        line: Some(line),
        resolved: false,
        payload: Some(relation.as_str().to_string()),
    }
}

fn publish(source: &NodeRow, topic: &str, line: i64) -> UnresolvedRefRow {
    ledger(source, ArtifactRelation::BrokerPublish, topic, line)
}

fn subscribe(source: &NodeRow, topic: &str, line: i64) -> UnresolvedRefRow {
    ledger(source, ArtifactRelation::BrokerSubscribe, topic, line)
}

/// Run the pure core over a ledger + node set, with `FILE` indexed.
fn promote(refs: &[UnresolvedRefRow], nodes: &[NodeRow]) -> BTreeMap<String, DesiredNode> {
    let files = [FileRecord {
        id: FILE_ID,
        path: FILE.to_string(),
        content_hash: None,
    }];
    let by_path: HashMap<&str, i64> = files.iter().map(|f| (f.path.as_str(), f.id)).collect();
    desired_set(refs, nodes, &by_path)
}

/// Every promoted node of one kind, by name, sorted.
fn names_of(desired: &BTreeMap<String, DesiredNode>, kind: NodeKind) -> Vec<&str> {
    let mut names: Vec<&str> = desired
        .values()
        .filter(|d| d.kind == kind)
        .map(|d| d.name.as_str())
        .collect();
    names.sort_unstable();
    names
}

/// The one node of `kind` in the desired set (panics unless there is exactly one).
fn only(desired: &BTreeMap<String, DesiredNode>, kind: NodeKind) -> &DesiredNode {
    let mut found = desired.values().filter(|d| d.kind == kind);
    let node = found.next().unwrap_or_else(|| panic!("no {kind:?} promoted"));
    assert!(found.next().is_none(), "expected exactly one {kind:?}");
    node
}

/// Acceptance (1): a publish and a subscribe on one topic render as a `Topic` with
/// a `Producer`/`Consumer` hung off it — the first-class shape [FR-WS-11] promotes
/// the S-254 ledger to.
///
/// [FR-WS-11]: ../../../../docs/specs/requirements/FR-WS-11.md
#[test]
fn a_publish_and_a_subscribe_promote_a_topic_with_its_producer_and_consumer() {
    let pubr = decl(1, "publish");
    let subr = decl(2, "onOrder");
    let desired = promote(
        &[publish(&pubr, "orders", 12), subscribe(&subr, "orders", 20)],
        &[pubr.clone(), subr.clone()],
    );

    assert_eq!(names_of(&desired, NodeKind::Topic), ["orders"]);
    assert_eq!(names_of(&desired, NodeKind::Producer), ["orders"]);
    assert_eq!(names_of(&desired, NodeKind::Consumer), ["orders"]);

    let topic = only(&desired, NodeKind::Topic);
    let producer = only(&desired, NodeKind::Producer);
    let consumer = only(&desired, NodeKind::Consumer);

    // The producer publishes to the topic and is contained by the publishing
    // method; the consumer subscribes from it and is contained by the listener.
    assert!(producer.edges.iter().any(|e| matches!(
        e, DesiredEdge::Publishes(t) if t == topic.symbol.as_str()
    )));
    assert!(producer
        .edges
        .iter()
        .any(|e| matches!(e, DesiredEdge::ContainedBy(p) if *p == pubr.id)));
    assert!(consumer.edges.iter().any(|e| matches!(
        e, DesiredEdge::Subscribes(t) if t == topic.symbol.as_str()
    )));
    assert!(consumer
        .edges
        .iter()
        .any(|e| matches!(e, DesiredEdge::ContainedBy(p) if *p == subr.id)));

    // A topic is a repo-scoped identity, not a declaration at a line.
    assert_eq!(topic.file_id, None, "a topic is not declared in a file");
    assert_eq!(topic.start_line, None);
    // A site is a real code location — anchored in the file that declares it.
    assert_eq!(producer.file_id, Some(FILE_ID));
    assert_eq!(producer.start_line, Some(12));
    assert_eq!(consumer.start_line, Some(20));
}

/// Acceptance (3): a **per-repo** topic is fully promoted with no counterpart —
/// one publish and no subscriber anywhere still yields the topic node and its
/// producer. This is the whole point of promotion: the topic graph exists *before*
/// (and independently of) any cross-repo match ([FR-WS-11], [ADR-55]).
///
/// [FR-WS-11]: ../../../../docs/specs/requirements/FR-WS-11.md
/// [ADR-55]: ../../../../docs/specs/architecture/decisions/ADR-55.md
#[test]
fn a_lone_publish_still_promotes_its_topic_with_no_consumer_anywhere() {
    let pubr = decl(1, "publish");
    let desired = promote(&[publish(&pubr, "orders", 12)], std::slice::from_ref(&pubr));

    assert_eq!(names_of(&desired, NodeKind::Topic), ["orders"]);
    assert_eq!(names_of(&desired, NodeKind::Producer), ["orders"]);
    assert!(
        names_of(&desired, NodeKind::Consumer).is_empty(),
        "no subscriber exists — none is invented"
    );
    assert!(only(&desired, NodeKind::Producer).edges.iter().any(
        |e| matches!(e, DesiredEdge::Publishes(t) if t == only(&desired, NodeKind::Topic).symbol.as_str())
    ));
}

/// The topic's identity is **repo-scoped**: two different declarations publishing
/// `orders` meet on ONE topic node (not one per site, not one per file). That
/// shared identity is what a cross-member bind later keys on ([FR-WS-11]).
///
/// [FR-WS-11]: ../../../../docs/specs/requirements/FR-WS-11.md
#[test]
fn two_declarations_publishing_one_topic_share_a_single_topic_node() {
    let a = decl(1, "publishA");
    let b = decl(2, "publishB");
    let desired = promote(
        &[publish(&a, "orders", 5), publish(&b, "orders", 9)],
        &[a.clone(), b.clone()],
    );

    assert_eq!(
        desired
            .values()
            .filter(|d| d.kind == NodeKind::Topic)
            .count(),
        1,
        "one topic identity per repo, however many sites name it"
    );
    assert_eq!(
        desired
            .values()
            .filter(|d| d.kind == NodeKind::Producer)
            .count(),
        2,
        "each publishing declaration is its own producer"
    );
    // Both producers publish to the same topic symbol.
    let topic = only(&desired, NodeKind::Topic).symbol.as_str().to_string();
    for producer in desired.values().filter(|d| d.kind == NodeKind::Producer) {
        assert!(producer
            .edges
            .iter()
            .any(|e| matches!(e, DesiredEdge::Publishes(t) if *t == topic)));
    }
}

/// A differing message-schema FQN keeps two topics **apart** — the guard rides the
/// key S-254 normalized, so promotion inherits it for free and never merges two
/// contract-distinct topics into one node ([FR-WS-10], [FR-WS-11]).
///
/// [FR-WS-10]: ../../../../docs/specs/requirements/FR-WS-10.md
/// [FR-WS-11]: ../../../../docs/specs/requirements/FR-WS-11.md
#[test]
fn a_schema_guarded_topic_key_is_its_own_topic() {
    let a = decl(1, "publishA");
    let b = decl(2, "publishB");
    let desired = promote(
        &[
            publish(&a, "orders#com.acme.OrderCreated", 5),
            publish(&b, "orders#com.acme.OrderUpdated", 9),
        ],
        &[a.clone(), b.clone()],
    );
    assert_eq!(
        names_of(&desired, NodeKind::Topic),
        ["orders#com.acme.OrderCreated", "orders#com.acme.OrderUpdated"],
        "two contract-distinct topics stay two nodes"
    );
}

/// One declaration that publishes the same topic twice is **one** producer — a
/// producer is a `(declaration, topic)` fact, not a per-call one — and it carries
/// the earliest line regardless of ledger order (deterministic, [NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
#[test]
fn a_declaration_publishing_one_topic_twice_is_a_single_producer_at_the_first_line() {
    let pubr = decl(1, "publish");
    // Deliberately out of line order: the later site is listed first.
    let desired = promote(
        &[publish(&pubr, "orders", 30), publish(&pubr, "orders", 12)],
        std::slice::from_ref(&pubr),
    );

    assert_eq!(
        desired
            .values()
            .filter(|d| d.kind == NodeKind::Producer)
            .count(),
        1,
        "two calls in one declaration are one producer of that topic"
    );
    assert_eq!(
        only(&desired, NodeKind::Producer).start_line,
        Some(12),
        "the earliest site wins, whatever order the ledger yields"
    );
}

/// A relay declaration that both subscribes to and re-publishes on one topic is a
/// producer **and** a consumer of it — the two promoted nodes are distinct
/// identities, so neither shadows the other (the S-254 dedup-collision shape,
/// carried into the first-class graph).
#[test]
fn a_relay_declaration_is_both_a_producer_and_a_consumer_of_one_topic() {
    let relay = decl(1, "relay");
    let desired = promote(
        &[publish(&relay, "orders", 8), subscribe(&relay, "orders", 6)],
        std::slice::from_ref(&relay),
    );

    assert_eq!(names_of(&desired, NodeKind::Topic), ["orders"]);
    assert_eq!(names_of(&desired, NodeKind::Producer), ["orders"]);
    assert_eq!(names_of(&desired, NodeKind::Consumer), ["orders"]);
    assert_ne!(
        only(&desired, NodeKind::Producer).symbol.as_str(),
        only(&desired, NodeKind::Consumer).symbol.as_str(),
        "the producer and the consumer of one relay are distinct symbols"
    );
}

/// Never fabricate ([NFR-RA-05]): a ledger row whose enclosing declaration is not
/// in the graph (its file was deleted, or the symbol never bound) promotes
/// **nothing** — no orphan producer, and not even the topic it names.
///
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
#[test]
fn a_ref_whose_enclosing_declaration_is_unknown_promotes_nothing() {
    let ghost = decl(1, "vanished");
    // The ledger row survives, but its declaration is NOT in the node set.
    let desired = promote(&[publish(&ghost, "orders", 12)], &[]);
    assert!(
        desired.is_empty(),
        "an unanchorable ref promotes no producer and no topic: {desired:?}"
    );
}

/// A graph with **no broker refs** yields an empty desired set — the promotion is
/// inert, which is what keeps a no-topic graph byte-for-byte unaffected
/// ([FR-WS-11], [NFR-RA-06]). A non-broker artifact relation is not this pass's
/// business and is ignored.
///
/// [FR-WS-11]: ../../../../docs/specs/requirements/FR-WS-11.md
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
#[test]
fn a_ledger_with_no_broker_refs_promotes_nothing() {
    let d = decl(1, "handler");
    let unrelated = ledger(&d, ArtifactRelation::HttpClientCall, "GET /users", 3);
    let plain = UnresolvedRefRow {
        payload: None, // an ordinary code ref
        ..ledger(&d, ArtifactRelation::Route, "whatever", 4)
    };

    let desired = promote(&[unrelated, plain], std::slice::from_ref(&d));
    assert!(
        desired.is_empty(),
        "only broker-arm rows promote; everything else is inert: {desired:?}"
    );
}

/// The promoted symbols are **valid, canonical SCIP** and carry the identity the
/// module's design depends on: a topic is repo-scoped (no file path in its
/// symbol), a site hangs off its enclosing declaration's symbol. Pins the two
/// symbol shapes so a change to either is a deliberate, visible one ([ADR-07]).
///
/// [ADR-07]: ../../../../docs/specs/architecture/decisions/ADR-07.md
#[test]
fn the_promoted_symbols_are_canonical_and_carry_the_intended_identity() {
    let pubr = decl(1, "publish");
    let desired = promote(&[publish(&pubr, "orders", 12)], std::slice::from_ref(&pubr));

    let topic = only(&desired, NodeKind::Topic).symbol.as_str();
    let producer = only(&desired, NodeKind::Producer).symbol.as_str();

    // Both round-trip through the SCIP codec (LogosSymbol::parse already
    // canonicalised them; re-parsing must be the identity).
    for symbol in [topic, producer] {
        assert_eq!(
            LogosSymbol::parse(symbol).expect("a promoted symbol is valid SCIP").as_str(),
            symbol,
            "{symbol} is not canonical"
        );
    }

    // The topic is repo-scoped: its symbol names no file, so every file in the
    // repo that publishes `orders` lands on this one identity.
    assert!(
        topic.ends_with("topic/orders#"),
        "the topic symbol is repo-scoped under the `topic` namespace: {topic}"
    );
    assert!(
        !topic.contains(FILE),
        "a topic symbol must carry no file path, or two files would fork it: {topic}"
    );

    // The producer hangs off the publishing declaration's own symbol under its role
    // namespace, so it is unique per (declaration, role, topic) and lives in that
    // declaration's file.
    assert_eq!(
        producer,
        format!("{}producer/orders#", pubr.symbol.as_str()),
        "the producer symbol extends its enclosing declaration under `producer/`"
    );
}

/// The pass owns exactly the `Contains`/`Publishes`/`Subscribes` edges around its
/// nodes and **nothing else** — the never-clobber fence that stops the reconcile
/// deleting an edge another pass proved ([NFR-RA-05]).
///
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
#[test]
fn the_pass_owns_only_its_own_three_edge_kinds() {
    for kind in EK::ALL {
        let owned = matches!(kind, EK::Contains | EK::Publishes | EK::Subscribes);
        assert_eq!(
            is_topic_owned(kind),
            owned,
            "{} ownership drifted — reconciling a foreign edge kind would clobber \
             the pass that owns it",
            kind.as_str()
        );
    }
}
