//! Integration tests for the broker-topic promotion pass (S-256, [FR-WS-11],
//! [ADR-55]), exercised end-to-end through [`Engine::index`]/[`Engine::sync`]
//! against real temp-directory fixtures.
//!
//! Coverage by acceptance criterion:
//! - broker coupling renders as `topic`/`producer`/`consumer` nodes joined by
//!   `publishes`/`subscribes` edges (FR-WS-11);
//! - a **per-repo** topic is visible with no cross-repo match anywhere — the whole
//!   point of promoting the ledger-only arm (FR-WS-11, ADR-55);
//! - a **no-topic graph is byte-for-byte unaffected**: a repo that indexes no
//!   broker coupling has an identical store with and without this pass (FR-WS-11,
//!   NFR-RA-06);
//! - the pass reconciles rather than accumulates — a deleted publish retires its
//!   producer on the next sync, and a surviving one keeps its node id;
//! - a dynamic (non-literal) topic is never promoted (NFR-RA-05).
//!
//! The cross-member bind these per-repo topics meet on is unit-proven through the
//! live bridge in `logos-core/src/federation/bridge.rs` (a workspace of real
//! on-disk member engines is the federation suite's fixture, not this one's).
//!
//! [FR-WS-11]: ../../docs/specs/requirements/FR-WS-11.md
//! [ADR-55]: ../../docs/specs/architecture/decisions/ADR-55.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md

#![cfg(feature = "lang-java")]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::Engine;
use logos_core::Runtime;
use tempfile::TempDir;

/// A Spring service that publishes to `orders` and listens on `shipments`.
const ORDER_SERVICE: &str = r#"
package com.acme;
class OrderService {
    private KafkaTemplate<String, String> kafkaTemplate;
    private static final String DYNAMIC = "never-promoted";

    public void publish(String payload) {
        kafkaTemplate.send("orders", payload);
    }

    @KafkaListener(topics = "shipments")
    public void onShipment(String msg) {}

    public void publishDynamic(String payload) {
        kafkaTemplate.send(DYNAMIC, payload);
    }
}
"#;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Every node of `kind` as `(id, name)`, sorted by name.
fn nodes_of(rt: &Runtime, kind: NodeKind) -> Vec<(NodeId, String)> {
    let mut nodes: Vec<(NodeId, String)> = rt
        .submit_read(move |store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .filter(|n| n.kind == kind)
                .map(|n| (n.id, n.name))
                .collect::<Vec<_>>())
        })
        .expect("read runs");
    nodes.sort_by(|a, b| a.1.cmp(&b.1));
    nodes
}

/// The names of every node of `kind`.
fn names_of(rt: &Runtime, kind: NodeKind) -> Vec<String> {
    nodes_of(rt, kind).into_iter().map(|(_, n)| n).collect()
}

/// All `(source, target)` pairs of edges with `kind`.
fn edges_of(rt: &Runtime, kind: EdgeKind) -> Vec<(NodeId, NodeId)> {
    rt.submit_read(move |store| {
        Ok(store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| (e.source, e.target))
            .collect())
    })
    .expect("read runs")
}

/// node id → kind, for asserting an edge's endpoints are the kinds they claim.
fn kinds_by_id(rt: &Runtime) -> HashMap<NodeId, NodeKind> {
    rt.submit_read(|store| Ok(store.all_nodes()?.into_iter().map(|n| (n.id, n.kind)).collect()))
        .expect("read runs")
}

/// Acceptance (1): broker coupling renders as first-class `topic`/`producer`/
/// `consumer` nodes joined by `publishes`/`subscribes` edges — the promotion of the
/// S-254 ledger-only arm onto S-255's migrated schema ([FR-WS-11], [ADR-55]).
///
/// Acceptance (2, per-repo half): the topics are fully visible in a **single** repo
/// with no workspace and no cross-repo match anywhere in sight.
///
/// [FR-WS-11]: ../../docs/specs/requirements/FR-WS-11.md
/// [ADR-55]: ../../docs/specs/architecture/decisions/ADR-55.md
#[test]
fn broker_coupling_is_promoted_to_topic_producer_and_consumer_nodes() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/OrderService.java", ORDER_SERVICE);

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // A topic per distinct static topic key — and NOT one for the dynamic send,
    // whose topic was refused at capture and can never reach the pass (NFR-RA-05).
    assert_eq!(
        names_of(rt, NodeKind::Topic),
        ["orders", "shipments"],
        "one topic node per statically-captured topic key; the dynamic send promotes none"
    );
    assert_eq!(
        names_of(rt, NodeKind::Producer),
        ["orders"],
        "the publishing method is a producer of `orders`"
    );
    assert_eq!(
        names_of(rt, NodeKind::Consumer),
        ["shipments"],
        "the @KafkaListener method is a consumer of `shipments`"
    );

    // The two broker edges join them, and their endpoints are genuinely the kinds
    // the ontology says: producer --publishes--> topic, consumer --subscribes--> topic.
    let kinds = kinds_by_id(rt);
    let publishes = edges_of(rt, EdgeKind::Publishes);
    let subscribes = edges_of(rt, EdgeKind::Subscribes);
    assert_eq!(publishes.len(), 1, "one publishes edge: {publishes:?}");
    assert_eq!(subscribes.len(), 1, "one subscribes edge: {subscribes:?}");
    for (source, target, want_source) in [
        (publishes[0].0, publishes[0].1, NodeKind::Producer),
        (subscribes[0].0, subscribes[0].1, NodeKind::Consumer),
    ] {
        assert_eq!(kinds.get(&source), Some(&want_source));
        assert_eq!(kinds.get(&target), Some(&NodeKind::Topic));
    }

    // The producer/consumer is anchored under the declaration that publishes or
    // subscribes — a `Contains` from the enclosing method, so it appears in that
    // method's scope rather than floating at the file root.
    let contains = edges_of(rt, EdgeKind::Contains);
    let producer_id = nodes_of(rt, NodeKind::Producer)[0].0;
    let consumer_id = nodes_of(rt, NodeKind::Consumer)[0].0;
    for (site, method) in [(producer_id, "publish"), (consumer_id, "onShipment")] {
        let parent = contains
            .iter()
            .find(|(_, target)| *target == site)
            .map(|(source, _)| *source)
            .unwrap_or_else(|| panic!("the promoted site has no containing declaration"));
        assert_eq!(kinds.get(&parent), Some(&NodeKind::Method));
        let parent_name = rt
            .submit_read(move |store| {
                Ok(store
                    .all_nodes()?
                    .into_iter()
                    .find(|n| n.id == parent)
                    .map(|n| n.name))
            })
            .expect("read runs")
            .expect("the parent node exists");
        assert_eq!(
            parent_name, method,
            "the site is contained by the method that declares it"
        );
    }
}

/// Acceptance (3): a repo with **no broker coupling** is byte-for-byte unaffected —
/// the promotion adds no node, no edge, and (being a pure reconcile over an empty
/// desired set) writes nothing at all ([FR-WS-11], [NFR-RA-06]).
///
/// The strongest available statement of "unaffected": the store's own content
/// digest is unchanged by a re-sync, and no broker node or edge exists.
///
/// [FR-WS-11]: ../../docs/specs/requirements/FR-WS-11.md
/// [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
#[test]
fn a_repo_with_no_broker_topics_is_unaffected() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/Plain.java",
        r#"
package com.acme;
class Plain {
    public int add(int a, int b) { return a + b; }
    public int twice(int a) { return add(a, a); }
}
"#,
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    for kind in [NodeKind::Topic, NodeKind::Producer, NodeKind::Consumer] {
        assert!(
            names_of(rt, kind).is_empty(),
            "a topic-free repo promotes no {kind:?} node"
        );
    }
    for kind in [EdgeKind::Publishes, EdgeKind::Subscribes] {
        assert!(
            edges_of(rt, kind).is_empty(),
            "a topic-free repo emits no {kind:?} edge"
        );
    }

    // And the node/edge counts are stable across a sync — the pass's incremental
    // gate short-circuits on the empty broker footprint, so it cannot perturb a
    // graph it has no business in.
    let counts = |rt: &Runtime| {
        rt.submit_read(|store| Ok((store.all_nodes()?.len(), store.all_edges()?.len())))
            .expect("read runs")
    };
    let before = counts(rt);
    engine.sync(&[PathBuf::from("src/Plain.java")]);
    assert_eq!(
        counts(rt),
        before,
        "a no-topic graph is untouched by the promotion pass, on index and on sync"
    );
}

/// The pass **reconciles, never accumulates**: deleting the publish retires its
/// producer *and* the now-orphaned topic on the next sync — the same self-healing
/// discipline the framework pass follows, so a full index and an incremental sync
/// converge on one graph.
///
/// The **surviving topic keeps its node id** across the re-extraction of a file
/// that publishes to it. That is a direct consequence of repo-scoping the topic
/// (`file_id = NULL`): a re-extract calls `delete_nodes_for_file`, which deletes
/// every node bound to the file, so a *site* node (producer/consumer) is rebuilt
/// with its file and does not keep its id — exactly as the framework pass's
/// promoted route nodes behave, and for the same reason. A topic, belonging to no
/// file, is untouched by that delete and stays stable for anything referencing it.
#[test]
fn a_deleted_publish_is_demoted_on_sync_and_the_topic_keeps_its_id() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/OrderService.java", ORDER_SERVICE);

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    assert_eq!(names_of(rt, NodeKind::Topic), ["orders", "shipments"]);
    let shipments_before = nodes_of(rt, NodeKind::Topic)
        .into_iter()
        .find(|(_, name)| name == "shipments")
        .expect("the shipments topic exists")
        .0;

    // Drop the publishing method entirely; keep the listener.
    write(
        tmp.path(),
        "src/OrderService.java",
        r#"
package com.acme;
class OrderService {
    @KafkaListener(topics = "shipments")
    public void onShipment(String msg) {}
}
"#,
    );
    engine.sync(&[PathBuf::from("src/OrderService.java")]);

    assert_eq!(
        names_of(rt, NodeKind::Topic),
        ["shipments"],
        "the `orders` topic is retired with its last producer — not accumulated"
    );
    assert!(
        names_of(rt, NodeKind::Producer).is_empty(),
        "the deleted publish's producer is demoted"
    );
    assert_eq!(names_of(rt, NodeKind::Consumer), ["shipments"]);

    // The surviving topic is id-stable even though the file that publishes to it was
    // re-extracted: the reconcile matched it by symbol, and — being repo-scoped
    // (`file_id = NULL`) — it was never in `delete_nodes_for_file`'s blast radius.
    // The consumer, which *is* file-bound, was rebuilt with its file (the framework
    // pass's promoted nodes behave identically), so its id is deliberately not
    // asserted here.
    assert_eq!(
        nodes_of(rt, NodeKind::Topic)[0].0,
        shipments_before,
        "the surviving topic keeps its node id across its publisher's re-extraction"
    );
    assert_eq!(
        edges_of(rt, EdgeKind::Publishes).len(),
        0,
        "the retired producer's edge went with it"
    );
    assert_eq!(edges_of(rt, EdgeKind::Subscribes).len(), 1);
}

/// A **relay** — one method that subscribes to a topic and re-publishes on it — is
/// promoted only as a **producer** today. Its subscribe is lost *before* this pass
/// ever sees it, to a **pre-existing defect in the reference ledger's uniqueness
/// rule**, and this test pins that honest, defective behaviour rather than the
/// behaviour we want.
///
/// # The defect (pre-existing; NOT introduced by S-256)
/// `unresolved_refs` is keyed `UNIQUE (source_symbol, target, form, kind)` and
/// `insert_unresolved_ref` inserts `ON CONFLICT(…) DO NOTHING` over exactly that
/// key. The relation token lives in `payload`, which is **not in the key**. A
/// relay's publish and subscribe coincide on all four key columns
/// (`(relay(), "orders", Method, ArtifactRef)`) and differ *only* in relation, so
/// the second row inserted is silently dropped and never reaches the ledger.
///
/// S-254 fixed precisely this relation-blindness one layer up — its
/// [`dedup_sort_refs`] carries the relation in its key, and its extraction test
/// `a_relay_method_keeps_both_its_publish_and_subscribe_on_one_topic` proves both
/// refs survive into `Facts` — but the **store** constraint was not widened with
/// it. So the arm loses the subscribe on the way to disk.
///
/// The blast radius is narrow: the four key columns collide only when *one*
/// declaration publishes and subscribes on the *same* topic. A different method or
/// a different topic yields a different key and both rows persist (proven by
/// [`broker_coupling_is_promoted_to_topic_producer_and_consumer_nodes`], which has
/// a publisher and a listener on different topics and promotes both sides).
///
/// It also silently degrades S-254's own cross-member fan-out ([FR-WS-10]): a relay
/// can never be bound *as a subscriber* by another member's publish, because its
/// subscribe is not in the ledger the bridge indexes. That was invisible until
/// S-256 wired the provider side into the live edge stream.
///
/// **Fixing it requires a forward-only migration 18** widening the ledger's UNIQUE
/// to include `payload` — out of scope here ([ADR-55] scopes CR-061 to the single
/// migration 17), and irreversible once shipped ([NFR-MA-06]). Filed as a
/// follow-up; this test will need inverting when that lands.
///
/// [FR-WS-10]: ../../docs/specs/requirements/FR-WS-10.md
/// [ADR-55]: ../../docs/specs/architecture/decisions/ADR-55.md
/// [NFR-MA-06]: ../../docs/specs/requirements/NFR-MA-06.md
#[test]
fn a_relay_method_loses_its_subscribe_to_the_relation_blind_ledger_key() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/Relay.java",
        r#"
package com.acme;
class Relay {
    private KafkaTemplate<String, String> kafkaTemplate;

    @KafkaListener(topics = "orders")
    public void relay(String msg) {
        kafkaTemplate.send("orders", msg);
    }
}
"#,
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // Extraction emits BOTH refs (S-254 proves it) — but only one survives the
    // ledger's relation-blind UNIQUE key, so only one side is promotable.
    let broker_rows: Vec<String> = rt
        .submit_read(|store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter_map(|r| r.payload)
                .filter(|p| p.starts_with("broker-"))
                .collect())
        })
        .expect("read runs");
    assert_eq!(
        broker_rows,
        ["broker-publish"],
        "THE DEFECT: the relay's `broker-subscribe` row is dropped by \
         `ON CONFLICT(source_symbol, target, form, kind) DO NOTHING` — the relation \
         `payload` is not in the ledger's UNIQUE key. Requires migration 18 to fix. \
         If this assertion starts failing with BOTH rows present, the migration has \
         landed: invert this test to assert the relay promotes a producer AND a consumer."
    );

    // The pass promotes exactly what the ledger honestly holds — the topic and the
    // producer. It never invents the consumer whose row it cannot see (NFR-RA-05).
    assert_eq!(names_of(rt, NodeKind::Topic), ["orders"], "one topic identity");
    assert_eq!(names_of(rt, NodeKind::Producer), ["orders"]);
    assert!(
        names_of(rt, NodeKind::Consumer).is_empty(),
        "the subscribe never reached the ledger, so no consumer is promoted — and \
         none is fabricated"
    );
    assert_eq!(edges_of(rt, EdgeKind::Publishes).len(), 1);
    assert_eq!(edges_of(rt, EdgeKind::Subscribes).len(), 0);
}

/// Two declarations publishing the same topic meet on **one** topic node — the
/// repo-scoped identity a cross-member bind later keys on ([FR-WS-11]). Were the
/// topic file-scoped, two files would fork it and the shared identity would be lost.
///
/// [FR-WS-11]: ../../docs/specs/requirements/FR-WS-11.md
#[test]
fn two_files_publishing_one_topic_share_a_single_topic_node() {
    let tmp = TempDir::new().unwrap();
    for (file, class) in [("src/A.java", "A"), ("src/B.java", "B")] {
        write(
            tmp.path(),
            file,
            &format!(
                r#"
package com.acme;
class {class} {{
    private KafkaTemplate<String, String> kafkaTemplate;
    public void emit(String payload) {{
        kafkaTemplate.send("orders", payload);
    }}
}}
"#
            ),
        );
    }

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    assert_eq!(
        nodes_of(rt, NodeKind::Topic).len(),
        1,
        "one topic identity per repo, however many files publish to it"
    );
    assert_eq!(
        nodes_of(rt, NodeKind::Producer).len(),
        2,
        "each publishing declaration is its own producer"
    );
    // Both producers publish to the one shared topic.
    let topic_id = nodes_of(rt, NodeKind::Topic)[0].0;
    let publishes = edges_of(rt, EdgeKind::Publishes);
    assert_eq!(publishes.len(), 2);
    assert!(
        publishes.iter().all(|(_, target)| *target == topic_id),
        "both producers point at the same topic node: {publishes:?}"
    );
}
