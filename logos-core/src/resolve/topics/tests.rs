//! Unit tests for the pure core of the broker-topic promotion pass (S-256,
//! [FR-WS-11]): [`desired_set`] against in-memory ledger + node fixtures, with no
//! store involved. The end-to-end promotion (a real index over a real broker
//! source, the reconcile, the no-topic invariant) lives in
//! `tests/broker_topic_promotion.rs`.
//!
//! [FR-WS-11]: ../../../../docs/specs/requirements/FR-WS-11.md

use super::*;

use std::collections::BTreeSet;

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
        receiver: None,
        peeled: None,
        arg_count: None,
        exported: None,
    }
}

fn publish(source: &NodeRow, topic: &str, line: i64) -> UnresolvedRefRow {
    ledger(source, ArtifactRelation::BrokerPublish, topic, line)
}

fn subscribe(source: &NodeRow, topic: &str, line: i64) -> UnresolvedRefRow {
    ledger(source, ArtifactRelation::BrokerSubscribe, topic, line)
}

/// A member that commits nothing at all: every operand then keys exactly as
/// written, which is the pre-[S-424] behaviour every fixture above the
/// committed-value block was written against and still pins.
///
/// [S-424]: ../../../../docs/planning/journal.md#s-424-the-promoted-topic-inventory-keys-on-the-committed-value
fn commits_nothing() -> MemberCorpus {
    MemberCorpus::new()
}

/// A member corpus committing `key` (in any source spelling — it is
/// canonicalised here, as an index would) to one definition per
/// `(profile, value)` pair.
///
/// Mirrors the fixture builder in `federation::broker`'s own tests, deliberately:
/// the two tiers are asserted equal over one roster below, and a corpus built two
/// ways would make that equality a statement about the fixtures rather than about
/// the code.
fn commits(corpus: &mut MemberCorpus, key: &str, values: &[(Option<&str>, &str)]) {
    corpus.insert(
        crate::extract::config::corpus::canonical_key(key),
        values
            .iter()
            .map(|(profile, value)| crate::graph_store::ConfigDefinition {
                path: profile.map_or("application.yml".to_string(), |p| {
                    format!("application-{p}.yml")
                }),
                profile: profile.map(str::to_string),
                value: (*value).to_string(),
            })
            .collect(),
    );
}

/// Run the pure core over a ledger + node set, with `FILE` indexed, against a
/// member that commits nothing.
fn promote(refs: &[UnresolvedRefRow], nodes: &[NodeRow]) -> BTreeMap<String, DesiredNode> {
    promote_with(refs, nodes, &commits_nothing())
}

/// [`promote`] against a member whose committed configuration is `corpus` — the
/// shape every [FR-WS-27] fixture below uses.
///
/// [FR-WS-27]: ../../../../docs/specs/requirements/FR-WS-27.md
fn promote_with(
    refs: &[UnresolvedRefRow],
    nodes: &[NodeRow],
    corpus: &MemberCorpus,
) -> BTreeMap<String, DesiredNode> {
    let files = [FileRecord {
        id: FILE_ID,
        path: FILE.to_string(),
        content_hash: None,
    }];
    let by_path: HashMap<&str, i64> = files.iter().map(|f| (f.path.as_str(), f.id)).collect();
    desired_set(refs, nodes, &by_path, corpus)
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
/// identities, so neither shadows the other.
///
/// This pins the **pure core**. End-to-end a relay now reaches this state too:
/// migration 18 (CR-080) made the ledger's uniqueness key relation-aware, so the
/// relay's subscribe row survives to the ledger and both legs promote (see
/// `a_relay_method_keeps_both_its_publish_and_subscribe_after_migration_18` in
/// `tests/broker_topic_promotion.rs`, the end-to-end proof).
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

/// [S-410] AC3, on the population [S-408] and [S-409] created: a **resolved**
/// Kafka Streams site — one whose `stream(…)`/`to(…)` operand was a
/// `@ConfigurationProperties` accessor and which [S-409] therefore stored as the
/// canonical `${prefix.key}` placeholder — promotes its `Producer`/`Consumer`
/// node and its `Publishes`/`Subscribes` edge, exactly as the header and
/// annotation forms always have.
///
/// This pass reads the ledger's `payload` and `target` and nothing else, so it is
/// deliberately blind to which *form* captured a site; that is precisely the
/// claim under test, and it is what discharges [S-410] AC3's *"resolved Streams
/// **or header-form** publish"* with one fixture rather than two: a resolved
/// header-form publish is byte-identical to the `to(…)` row below — the same
/// relation, the same canonical `${prefix.key}` target — so a second fixture
/// would differ only in its variable name. A **refused** Streams site leaves a keyless row and
/// promotes nothing — the never-fabricate half, re-pinned here on this form
/// because a fabricated `Topic` named `${…}` is exactly what a careless
/// placeholder rule would produce ([NFR-RA-05]).
///
/// The promoted topic is keyed here by the **operand as written**, and since
/// [S-424] that is a statement about this fixture's corpus rather than about the
/// pass: `promote` runs against a member that commits **nothing**, so both
/// operands resolve to nothing and keep their placeholders — [FR-WS-27] AC3's
/// refusal branch. Hand the same rows a corpus that commits those keys and the
/// topics are the committed values, which
/// `two_spellings_of_one_property_promote_one_topic_with_one_producer_and_one_consumer`
/// pins directly.
///
/// This paragraph used to say the pass was *"per-repo and pre-federation"* and
/// therefore *"untouched by [S-410]"*. That was the drift [CR-136] was filed on:
/// the resolution is member-scoped, so being per-repo never made it inapplicable
/// — a member's own configuration is in scope for its own graph.
///
/// [CR-136]: ../../../../docs/requests/CR-136-promoted-topic-identity-is-the-committed-value.md
/// [FR-WS-11]: ../../../../docs/specs/requirements/FR-WS-11.md
/// [FR-WS-27]: ../../../../docs/specs/requirements/FR-WS-27.md
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
/// [S-424]: ../../../../docs/planning/journal.md#s-424-the-promoted-topic-inventory-keys-on-the-committed-value
/// [S-408]: ../../../../docs/planning/journal.md#s-408-a-kafka-streams-topology-link-is-a-broker-publish-or-subscribe-site
/// [S-409]: ../../../../docs/planning/journal.md#s-409-the-accessor-hop-reaches-the-broker-arm
/// [S-410]: ../../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
#[test]
fn a_resolved_streams_site_promotes_its_producer_and_consumer_and_a_refused_one_promotes_nothing() {
    let to = decl(1, "buildTopology");
    let stream = decl(2, "readTopology");
    let refused = decl(3, "dynamicTopology");
    let desired = promote(
        &[
            // `…​.to(kafkaTopics.getArchiveEvents())`, resolved by S-409.
            publish(&to, "${spring.kafka.topics.archiveevents}", 31),
            // `builder.stream(kafkaTopics.getArchiveVolumeCounters())`.
            subscribe(&stream, "${spring.kafka.topics.archivevolumecounters}", 44),
            // `builder.stream(topicFor(x))` — refused `topic-not-literal`.
            subscribe(&refused, "", 57),
        ],
        &[to.clone(), stream.clone(), refused.clone()],
    );

    assert_eq!(
        names_of(&desired, NodeKind::Producer),
        ["${spring.kafka.topics.archiveevents}"],
        "the resolved `to(…)` publish promotes a Producer"
    );
    assert_eq!(
        names_of(&desired, NodeKind::Consumer),
        ["${spring.kafka.topics.archivevolumecounters}"],
        "the resolved `stream(…)` subscribe promotes a Consumer, and the refused \
         one promotes nothing"
    );
    let producer = only(&desired, NodeKind::Producer);
    let consumer = only(&desired, NodeKind::Consumer);
    let topic_symbol = |name: &str| {
        desired
            .values()
            .find(|d| d.kind == NodeKind::Topic && d.name == name)
            .unwrap_or_else(|| panic!("no Topic named {name}"))
            .symbol
            .as_str()
            .to_string()
    };
    let published = topic_symbol("${spring.kafka.topics.archiveevents}");
    let subscribed = topic_symbol("${spring.kafka.topics.archivevolumecounters}");
    assert!(
        producer
            .edges
            .iter()
            .any(|e| matches!(e, DesiredEdge::Publishes(t) if *t == published)),
        "the Producer carries its Publishes edge: {:?}",
        producer.edges
    );
    assert!(
        consumer
            .edges
            .iter()
            .any(|e| matches!(e, DesiredEdge::Subscribes(t) if *t == subscribed)),
        "the Consumer carries its Subscribes edge: {:?}",
        consumer.edges
    );
    assert_eq!(
        names_of(&desired, NodeKind::Topic),
        [
            "${spring.kafka.topics.archiveevents}",
            "${spring.kafka.topics.archivevolumecounters}"
        ],
        "two topics, and no third fabricated from the keyless row"
    );
}

// ── S-424 / FR-WS-27: one identify function, called from both tiers ──────────

/// The two spellings [ADR-64]'s 2026-09-15 amendment records as the reproduction
/// of the estate's gap, and the corpus that commits them to one value.
///
/// `archive-commands` is the committed value; `${…archive-commands}` and
/// `${…archivecommands}` are the two operand spellings that reach it — the first
/// written by hand in an annotation, the second the canonical relaxed-binding
/// form a `@ConfigurationProperties` accessor is stored under. They are not
/// byte-equal and never become so; they meet at the **value**.
///
/// [ADR-64]: ../../../../docs/specs/architecture/decisions/ADR-64.md
const HAND_WRITTEN: &str = "${spring.kafka.topics.archive-commands}";
const VIA_ACCESSOR: &str = "${spring.kafka.topics.archivecommands}";
const COMMITTED: &str = "archive-commands";

/// A member that commits the archive-commands property to the one value it
/// really has.
///
/// **One entry covers both operand spellings**, and that is the relaxed-binding
/// rule doing its job rather than a shortcut in the fixture: `canonical_key`
/// folds `…archive-commands` and `…archivecommands` onto one canonical key, so
/// a source that writes either spelling defines the same property. The spellings
/// diverge in the *operand*, which is why the two sites did not meet before this
/// story; they converge in the *key*, which is why they can.
fn archive_commands_corpus() -> MemberCorpus {
    let mut corpus = MemberCorpus::new();
    commits(
        &mut corpus,
        "spring.kafka.topics.archive-commands",
        &[(None, COMMITTED)],
    );
    corpus
}

/// **[FR-WS-27] AC2 — the committed value keys all three promoted kinds.**
///
/// A publish keyed `${…archive-commands}` and a subscribe keyed
/// `${…archivecommands}`, both committing to `archive-commands`, promote **one**
/// `Topic` node carrying one `Producer` and one `Consumer`.
///
/// This is the fixture the whole story exists for. Before [S-424] it promoted
/// **two** `Topic` nodes — one per placeholder spelling — with the producer
/// hanging off one and the consumer off the other, so the coupling they express
/// had no node to be expressed on. The assertion that catches that regression is
/// the `Publishes`/`Subscribes` pair pointing at the SAME topic symbol, not the
/// topic count alone: two nodes named alike would still count as two.
///
/// [FR-WS-27]: ../../../../docs/specs/requirements/FR-WS-27.md
/// [S-424]: ../../../../docs/planning/journal.md#s-424-the-promoted-topic-inventory-keys-on-the-committed-value
#[test]
fn two_spellings_of_one_property_promote_one_topic_with_one_producer_and_one_consumer() {
    let pubr = decl(1, "publish");
    let subr = decl(2, "onArchiveCommand");
    let desired = promote_with(
        &[
            publish(&pubr, HAND_WRITTEN, 12),
            subscribe(&subr, VIA_ACCESSOR, 20),
        ],
        &[pubr.clone(), subr.clone()],
        &archive_commands_corpus(),
    );

    assert_eq!(
        names_of(&desired, NodeKind::Topic),
        [COMMITTED],
        "ONE topic, named by the committed value — not one per placeholder spelling"
    );
    assert_eq!(names_of(&desired, NodeKind::Producer), [COMMITTED]);
    assert_eq!(names_of(&desired, NodeKind::Consumer), [COMMITTED]);

    // …and the producer and the consumer hang off THAT topic, which is the half
    // a count cannot establish.
    let topic = only(&desired, NodeKind::Topic).symbol.as_str().to_string();
    let producer = only(&desired, NodeKind::Producer);
    let consumer = only(&desired, NodeKind::Consumer);
    assert!(
        producer
            .edges
            .iter()
            .any(|e| matches!(e, DesiredEdge::Publishes(t) if *t == topic)),
        "the Producer publishes to the one topic: {:?}",
        producer.edges
    );
    assert!(
        consumer
            .edges
            .iter()
            .any(|e| matches!(e, DesiredEdge::Subscribes(t) if *t == topic)),
        "the Consumer subscribes from the SAME topic: {:?}",
        consumer.edges
    );

    // The site-scoped symbols carry the committed value in their topic segment
    // too ([FR-WS-27] AC2: "a producer and the topic it hangs off can never
    // disagree, because one value keys both"). Asserted on the symbol string,
    // because that is the identity a re-sync reconciles on — a matching `name`
    // with a placeholder still in the symbol would leave two site nodes.
    for site in [producer, consumer] {
        assert!(
            site.symbol.as_str().contains(COMMITTED),
            "the site symbol is keyed on the committed value: {}",
            site.symbol.as_str()
        );
        assert!(
            !site.symbol.as_str().contains("${"),
            "no placeholder survives in a resolved site symbol: {}",
            site.symbol.as_str()
        );
    }
}

/// **[FR-WS-27] AC3 — a refusal keeps the placeholder and fabricates nothing.**
///
/// Two refusals, each under its own existing reason, and each keeping the
/// operand exactly as written:
///
/// - (a) `config-key-missing` — no committed source defines the key;
/// - (b) `config-placeholder-value` — the committed value is itself a `${…}`
///   indirection.
///
/// The near miss in (b) is the one that matters and it is asserted explicitly:
/// the site must NOT be promoted under the indirection. A subscriber that
/// literally writes `${another.key}` is not on this publisher's topic, and
/// keying the publish under `${another.key}` would couple them ([NFR-RA-05]).
///
/// [FR-WS-27]: ../../../../docs/specs/requirements/FR-WS-27.md
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
#[test]
fn a_refused_operand_keeps_its_placeholder_as_written_and_promotes_no_indirection() {
    // (a) Nothing commits the key at all.
    let pubr = decl(1, "publish");
    let desired = promote_with(
        &[publish(&pubr, HAND_WRITTEN, 12)],
        std::slice::from_ref(&pubr),
        &commits_nothing(),
    );
    assert_eq!(
        names_of(&desired, NodeKind::Topic),
        [HAND_WRITTEN],
        "an unresolved operand keeps the placeholder AS WRITTEN — no topic is invented"
    );

    // (b) The committed value is itself a placeholder.
    let mut corpus = MemberCorpus::new();
    commits(
        &mut corpus,
        "spring.kafka.topics.archive-commands",
        &[(None, "${another.key}")],
    );
    let subr = decl(2, "onAnother");
    let desired = promote_with(
        &[
            publish(&pubr, HAND_WRITTEN, 12),
            // A subscriber that really does write the indirection. If the
            // publish were keyed on `${another.key}`, these two would collapse
            // onto one topic and assert a coupling nothing proves.
            subscribe(&subr, "${another.key}", 20),
        ],
        &[pubr.clone(), subr.clone()],
        &corpus,
    );
    assert_eq!(
        names_of(&desired, NodeKind::Topic),
        ["${another.key}", HAND_WRITTEN],
        "the refused publish keeps its own placeholder and never lands on the indirection"
    );
    assert_eq!(names_of(&desired, NodeKind::Producer), [HAND_WRITTEN]);
    assert_eq!(names_of(&desired, NodeKind::Consumer), ["${another.key}"]);
}

/// **[FR-WS-27] AC3 — overlays that disagree promote one topic per overlay**
/// ([FR-WS-19] AC2), exactly as the bridge fans out under each.
///
/// Neither value is chosen over the other and neither is averaged: the site is a
/// producer of both topics, because under one profile it publishes to one and
/// under the other to the other. Both facts are true; representing one would be
/// a guess.
///
/// [FR-WS-19]: ../../../../docs/specs/requirements/FR-WS-19.md
/// [FR-WS-27]: ../../../../docs/specs/requirements/FR-WS-27.md
#[test]
fn a_key_whose_overlays_disagree_promotes_one_topic_per_overlay() {
    let mut corpus = MemberCorpus::new();
    commits(
        &mut corpus,
        "spring.kafka.topics.archive-commands",
        &[(Some("prod"), "commands-prod"), (Some("staging"), "commands-staging")],
    );
    let pubr = decl(1, "publish");
    let desired = promote_with(
        &[publish(&pubr, HAND_WRITTEN, 12)],
        std::slice::from_ref(&pubr),
        &corpus,
    );

    assert_eq!(
        names_of(&desired, NodeKind::Topic),
        ["commands-prod", "commands-staging"],
        "both overlays' values are retained — neither picked, neither averaged"
    );
    assert_eq!(
        names_of(&desired, NodeKind::Producer),
        ["commands-prod", "commands-staging"],
        "the ONE publish site is a producer of each, at its own line"
    );
    // One site, two nodes — and they are distinct symbols, not one node
    // overwritten twice.
    let producers: Vec<&str> = desired
        .values()
        .filter(|d| d.kind == NodeKind::Producer)
        .map(|d| d.symbol.as_str())
        .collect();
    assert_eq!(producers.len(), 2, "two distinct producer symbols: {producers:?}");
    assert_ne!(producers[0], producers[1]);
}

/// **[FR-WS-27] AC4 — only a member naming a broker key reads a corpus.**
///
/// [`broker_config_keys`] is the gate: it is what `member_corpus` short-circuits
/// on, so an empty answer here IS "no store read is issued". The three cases
/// below are the three that decide whether a repository pays anything at all.
///
/// The third is a **near miss** rather than a formality: an HTTP client call's
/// target is very often `${services.orders.base-url}`, and a gate that tested
/// for `${…}` without testing the relation would open this member's store to
/// resolve a key the promotion pass promotes nothing for.
///
/// [FR-WS-27]: ../../../../docs/specs/requirements/FR-WS-27.md
#[test]
fn only_a_ledger_naming_a_broker_configuration_key_asks_for_a_corpus() {
    let pubr = decl(1, "publish");

    // (a) Literal broker operands: no key, so no read.
    assert!(
        broker_config_keys(&[publish(&pubr, "orders", 12)]).is_empty(),
        "a literal topic names no configuration key"
    );

    // (b) A placeholder operand names exactly its CANONICAL key, once. Three
    //     rows in two spellings ask for ONE key, because relaxed binding folds
    //     the spellings — so the estate's duplicate-spelling groups cost one
    //     lookup between them, not one each.
    assert_eq!(
        broker_config_keys(&[
            publish(&pubr, HAND_WRITTEN, 12),
            subscribe(&pubr, HAND_WRITTEN, 20),
            subscribe(&pubr, VIA_ACCESSOR, 30),
        ]),
        ["spring.kafka.topics.archivecommands"],
        "one canonical key, de-duplicated across rows AND across spellings"
    );

    // (c) THE NEAR MISS: a non-broker relation carrying a placeholder target
    //     contributes nothing. The gate is on the relation, not on the `${`.
    let http = ledger(
        &pubr,
        ArtifactRelation::HttpClientCall,
        "GET ${services.orders.base-url}/orders",
        40,
    );
    assert!(
        broker_config_keys(&[http]).is_empty(),
        "an HTTP client call's configured target is not this pass's business"
    );
}

/// **[FR-WS-27] AC4 / [NFR-PE-10] — a ledger naming no broker key issues NO
/// read**, observed rather than inferred.
///
/// The sibling above asserts what [`broker_config_keys`] answers; this asserts
/// what [`corpus_for`] *does* with that answer, which is the half the criterion
/// actually names. The distinction is not pedantic: with only the key-list
/// assertion in place, deleting the `if keys.is_empty()` short-circuit left the
/// whole suite green while every repository with a broker footprint and none but
/// literal topics paid a store round-trip per index.
///
/// [FR-WS-27]: ../../../../docs/specs/requirements/FR-WS-27.md
/// [NFR-PE-10]: ../../../../docs/specs/requirements/NFR-PE-10.md
#[test]
fn a_ledger_naming_no_broker_key_issues_no_corpus_read() {
    let pubr = decl(1, "publish");
    let count_reads = |refs: &[UnresolvedRefRow]| -> usize {
        let mut reads = 0usize;
        let corpus = corpus_for(refs, |keys| {
            reads += 1;
            assert!(!keys.is_empty(), "the gate must never issue an empty read");
            Ok(MemberCorpus::new())
        })
        .expect("the fixture reader cannot fail");
        assert!(corpus.is_empty());
        reads
    };

    // Literal operands only: the read is never issued at all.
    assert_eq!(
        count_reads(&[publish(&pubr, "orders", 12), subscribe(&pubr, "shipments", 20)]),
        0,
        "a literal-only ledger must not open its store for a corpus"
    );
    // No broker row at all — the same, and the cheapest path there is.
    assert_eq!(count_reads(&[]), 0);
    // A non-broker relation carrying a placeholder: still no read.
    let http = ledger(
        &pubr,
        ArtifactRelation::HttpClientCall,
        "GET ${services.orders.base-url}/orders",
        40,
    );
    assert_eq!(count_reads(&[http]), 0);

    // …and exactly ONE read, over the whole key set, when a key IS named —
    // never one read per row, which three rows would otherwise show as three.
    assert_eq!(
        count_reads(&[
            publish(&pubr, HAND_WRITTEN, 12),
            subscribe(&pubr, HAND_WRITTEN, 20),
            subscribe(&pubr, VIA_ACCESSOR, 30),
        ]),
        1,
        "one read for the member, not one per site"
    );
}

/// **[FR-WS-27] AC1 — the no-drift walk. One function, called from both tiers.**
///
/// Walks **every** broker fixture shape through both tiers and asserts each
/// against a **written expectation**, then against the other. The criterion this
/// discharges is the one that survives the next change to either tier.
///
/// # Why the expectation column exists, and what it fixes
///
/// The first version of this test asserted only `promoted == bridged`. Since the
/// promotion pass obtains its keys *by calling the very function the bridge keys
/// on*, that comparison is `f(x) == f(x)`: it has exactly one live failure mode
/// (someone stops the pass delegating) and is **blind** to a change in the shared
/// rule, because both sides move together. Delete `identify`'s `Committed` arm
/// and every config-bound site silently re-keys on its placeholder in both tiers
/// — [CR-136] fully reintroduced — with a green test whose docstring claimed it
/// would fail. The roster's third column is the oracle that closes that: the
/// *fixture* says what each shape must key to, so a change to the shared rule
/// alone fails here.
///
/// The two entry points are each tier's own:
///
/// - the **promotion tier** runs [`desired_set`] — the exact function
///   [`super::run`] calls — and the identities are read off the `Topic` nodes it
///   wants promoted, which is the tier's whole output;
/// - the **bridge tier** runs [`broker_identity::identify`] — the exact function
///   [`crate::federation::broker::broker_edges`] and
///   `federation::coverage::arm_identity` call, and the only place either of them
///   obtains a topic key.
///
/// Calling `identify` for the bridge side rather than `broker_edges` is
/// deliberate: `broker_edges` emits an edge only for a publish that **meets a
/// cross-member subscribe**, so over a roster of refusals and lone publishes it
/// would answer the empty set and the comparison would be vacuous for precisely
/// the fixtures that matter. The consequence is that this walk is **asymmetric**
/// — a local keying rule re-introduced inside `broker_edges` is invisible here.
/// That half is covered by `two_spellings_of_one_property_fan_out_on_the_committed_value`
/// in `federation::broker`, which runs the real fan-out and asserts the BEFORE
/// state too. Stated rather than implied, because the criterion's words are
/// "either tier" and one reading of them is not delivered here.
///
/// [CR-136]: ../../../../docs/requests/CR-136-promoted-topic-identity-is-the-committed-value.md
/// [FR-WS-27]: ../../../../docs/specs/requirements/FR-WS-27.md
#[test]
fn both_tiers_key_every_broker_fixture_identically() {
    // The roster: one entry per shape a broker operand can take, its corpus, and
    // **the keys that shape must produce, written out**. Extended when a shape is
    // added, which is the point — a new shape must be walked by both tiers.
    let overlays = {
        let mut c = MemberCorpus::new();
        commits(
            &mut c,
            "spring.kafka.topics.archive-commands",
            &[(Some("prod"), "commands-prod"), (Some("staging"), "commands-staging")],
        );
        c
    };
    let indirection = {
        let mut c = MemberCorpus::new();
        commits(
            &mut c,
            "spring.kafka.topics.archive-commands",
            &[(None, "${another.key}")],
        );
        c
    };
    let blank = {
        let mut c = MemberCorpus::new();
        commits(
            &mut c,
            "spring.kafka.topics.archive-commands",
            &[(None, "   ")],
        );
        c
    };
    let composed = {
        let mut c = MemberCorpus::new();
        commits(&mut c, "env", &[(None, "prod")]);
        commits(&mut c, "base", &[(None, "orders")]);
        c
    };
    let roster: Vec<(&str, &str, MemberCorpus, &[&str])> = vec![
        ("a plain literal", "orders", commits_nothing(), &["orders"]),
        (
            "a schema-guarded literal",
            "orders#com.acme.OrderCreated",
            commits_nothing(),
            &["orders#com.acme.OrderCreated"],
        ),
        (
            "a whitespace-padded literal",
            "  orders  ",
            commits_nothing(),
            &["orders"],
        ),
        (
            "a hand-written placeholder, committed",
            HAND_WRITTEN,
            archive_commands_corpus(),
            &[COMMITTED],
        ),
        (
            "an accessor-spelled placeholder, committed to the same value",
            VIA_ACCESSOR,
            archive_commands_corpus(),
            &[COMMITTED],
        ),
        (
            "a schema-guarded placeholder, committed",
            "${spring.kafka.topics.archive-commands}#com.acme.Cmd",
            archive_commands_corpus(),
            &["archive-commands#com.acme.Cmd"],
        ),
        (
            "an operand composing TWO keys with literal text between them",
            "${env}-${base}",
            composed,
            &["prod-orders"],
        ),
        (
            "a placeholder nothing commits",
            HAND_WRITTEN,
            commits_nothing(),
            &[HAND_WRITTEN],
        ),
        (
            "a placeholder committed to an indirection",
            HAND_WRITTEN,
            indirection,
            &[HAND_WRITTEN],
        ),
        (
            "a placeholder committed to blank",
            HAND_WRITTEN,
            blank,
            &[HAND_WRITTEN],
        ),
        (
            "a placeholder two overlays disagree on",
            HAND_WRITTEN,
            overlays,
            &["commands-prod", "commands-staging"],
        ),
        // The keyless row: the arm's `topic-not-literal` refusal. BOTH tiers must
        // answer *nothing*, and the empty expectation below is what asserts it —
        // set equality alone would be satisfied by two tiers that both wrongly
        // admitted a topic named `""`.
        ("a keyless row", "", commits_nothing(), &[]),
        ("an all-whitespace row", "   ", commits_nothing(), &[]),
    ];

    for (what, operand, corpus, expected) in roster {
        let expected: BTreeSet<&str> = expected.iter().copied().collect();
        for (relation, kind) in [
            (ArtifactRelation::BrokerPublish, NodeKind::Producer),
            (ArtifactRelation::BrokerSubscribe, NodeKind::Consumer),
        ] {
            let site = decl(1, "site");
            let row = ledger(&site, relation, operand, 12);

            // Tier 1 — the promotion pass, through the function `run` calls.
            let desired = promote_with(&[row], std::slice::from_ref(&site), &corpus);
            let promoted: BTreeSet<&str> = desired
                .values()
                .filter(|d| d.kind == NodeKind::Topic)
                .map(|d| d.name.as_str())
                .collect();

            // Tier 2 — the bridge, through the function it keys on.
            let identity = broker_identity::identify(relation, operand, &corpus);
            let bridged: BTreeSet<&str> = identity
                .as_ref()
                .map(|i| i.topics.iter().map(String::as_str).collect())
                .unwrap_or_default();

            // Against the WRITTEN expectation first, and independently per tier.
            // This is what makes the walk a contract rather than a call-graph
            // check: with the oracle, a change to the shared rule alone fails
            // here, which "the two tiers agree" can never do once they share it.
            assert_eq!(
                promoted, expected,
                "the PROMOTION tier keys {what} ({operand:?}) as a {kind:?} wrongly"
            );
            assert_eq!(
                bridged, expected,
                "the BRIDGE tier keys {what} ({operand:?}) as a {kind:?} wrongly"
            );
            // …and against each other, which is the criterion's own words and
            // catches a divergence in a shape the expectation column got wrong.
            assert_eq!(
                promoted, bridged,
                "the two tiers disagree on {what} ({operand:?}) as a {kind:?}: \
                 the promotion pass says {promoted:?}, the bridge says {bridged:?}"
            );
        }
    }
}
