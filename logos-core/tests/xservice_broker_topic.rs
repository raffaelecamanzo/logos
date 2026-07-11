//! End-to-end test for **first-class broker topics across a real 2-repo workspace**
//! (S-256, [CR-061], [FR-WS-11], [ADR-55]).
//!
//! This is the sprint's stated verification, executed literally: *"Index a 2-repo
//! workspace with a publish in one member and a subscribe on the same topic in
//! another; assert `Topic`/`Producer`/`Consumer` nodes and `Publishes`/`Subscribes`
//! edges are emitted and the cross-member bind holds via shared topic identity."*
//!
//! Where the unit tests drive the bridge with in-memory fixtures, this drives the
//! **real** pipeline — two member repositories, each indexed by its own [`Engine`] over
//! the real `tree-sitter-java` grammar, behind an [`EngineRegistry`]. It is the only
//! test that exercises the production reads the workspace surfaces actually depend on:
//! `Engine::invocation_refs` (the broker half) and `Engine::topic_surface`. Modelled on
//! `xservice_http_client_call.rs`, the S-252 arm's equivalent.
//!
//! It proves:
//! - each member carries its own promoted per-repo topic graph (acceptance 1);
//! - a publish in `api` binds the subscribe on the same topic in `billing` through the
//!   **live** bridge, via shared topic identity (acceptance 2);
//! - the workspace topic inventory the service map renders is repo-qualified and honest
//!   about producers vs consumers;
//! - the coverage tier reports that same publish **bound** — it does not deny the
//!   provider the bridge just found ([NFR-CC-04]);
//! - computing the bridge mutates no member database ([ADR-52]).
//!
//! Gated on the Java grammar (the language whose `brokers.scm` ships today).
//!
//! [FR-WS-11]: ../../docs/specs/requirements/FR-WS-11.md
//! [ADR-55]: ../../docs/specs/architecture/decisions/ADR-55.md
//! [ADR-52]: ../../docs/specs/architecture/decisions/ADR-52.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
#![cfg(feature = "lang-java")]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::federation::{
    cross_service_coverage, workspace_topics, ContractBridge, EngineRegistry, Federation, Member,
    RegistryMode,
};
use logos_core::model::{EdgeKind, NodeKind};
use logos_core::Engine;

/// The `api` member: publishes to `orders`.
const PUBLISHER: &str = r#"
package com.acme;
class OrderService {
    private KafkaTemplate<String, String> kafkaTemplate;

    public void emitOrder(String payload) {
        kafkaTemplate.send("orders", payload);
    }
}
"#;

/// The `billing` member: subscribes to the same `orders` topic.
const SUBSCRIBER: &str = r#"
package com.acme;
class BillingListener {
    @KafkaListener(topics = "orders")
    public void onOrder(String msg) {}
}
"#;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// Index a member repo into its own `.logos/logos.db`, then drop the engine so the
/// store is closed before the registry re-opens it.
fn index_member(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

/// The names of every node of `kind` in a member's own store, sorted.
fn member_nodes(root: &Path, kind: NodeKind) -> Vec<String> {
    let engine = Engine::start(root).expect("engine starts");
    let rt = engine.runtime().expect("runtime");
    let mut names: Vec<String> = rt
        .submit_read(move |store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .filter(|n| n.kind == kind)
                .map(|n| n.name)
                .collect::<Vec<_>>())
        })
        .expect("read runs");
    names.sort();
    names
}

/// How many edges of `kind` a member's own store carries.
fn member_edges(root: &Path, kind: EdgeKind) -> usize {
    let engine = Engine::start(root).expect("engine starts");
    let rt = engine.runtime().expect("runtime");
    rt.submit_read(move |store| Ok(store.all_edges()?.iter().filter(|e| e.kind == kind).count()))
        .expect("read runs")
}

fn member(name: &str, root: &Path) -> Member {
    Member {
        name: name.to_string(),
        root: root.to_path_buf(),
    }
}

fn federation(root: &Path, members: Vec<Member>) -> Federation {
    Federation {
        name: "shop".to_string(),
        root: root.to_path_buf(),
        members,
        default: None,
        links: Vec::new(),
        governance: Default::default(),
    }
}

fn db_bytes(root: &Path) -> Vec<u8> {
    fs::read(root.join(".logos").join("logos.db")).expect("member db exists")
}

/// The sprint's Testing & Verification clause, end to end over two real repos.
#[test]
fn a_publish_in_one_member_binds_a_subscribe_on_the_same_topic_in_another() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let api = root.join("api");
    let billing = root.join("billing");
    write(&api, "src/OrderService.java", PUBLISHER);
    write(&billing, "src/BillingListener.java", SUBSCRIBER);
    index_member(&api);
    index_member(&billing);

    // ── Acceptance (1): each member carries its own per-repo topic graph ──────
    // The publisher's repo knows the topic and its producer — and NOT a consumer.
    assert_eq!(member_nodes(&api, NodeKind::Topic), ["orders"]);
    assert_eq!(member_nodes(&api, NodeKind::Producer), ["orders"]);
    assert!(
        member_nodes(&api, NodeKind::Consumer).is_empty(),
        "the publishing member has no subscriber of its own — none is invented"
    );
    assert_eq!(member_edges(&api, EdgeKind::Publishes), 1);
    assert_eq!(member_edges(&api, EdgeKind::Subscribes), 0);

    // The subscriber's repo independently promotes the SAME topic identity.
    assert_eq!(member_nodes(&billing, NodeKind::Topic), ["orders"]);
    assert_eq!(member_nodes(&billing, NodeKind::Consumer), ["orders"]);
    assert!(member_nodes(&billing, NodeKind::Producer).is_empty());
    assert_eq!(member_edges(&billing, EdgeKind::Subscribes), 1);
    assert_eq!(member_edges(&billing, EdgeKind::Publishes), 0);

    let registry = EngineRegistry::<Engine>::new(
        federation(root, vec![member("api", &api), member("billing", &billing)]),
        RegistryMode::Lazy,
    );
    let bridge = ContractBridge::new();

    // Warm the engines before snapshotting so the checksum isolates the bridge's reads
    // from the one-time store open.
    let _ = bridge.edges(&registry);
    let api_before = db_bytes(&api);
    let billing_before = db_bytes(&billing);

    // ── Acceptance (2): the cross-member bind, through the LIVE bridge ────────
    let edges = bridge.edges(&registry);
    assert_eq!(
        edges.len(),
        1,
        "the publish binds the cross-member subscribe on the shared topic: {edges:?}"
    );
    let edge = &edges[0];
    assert_eq!(edge.relation, "broker-topic");
    assert_eq!(edge.from.member, "api", "the publish is the edge source");
    assert!(
        edge.from.symbol.as_str().contains("emitOrder"),
        "the endpoint is the real publishing METHOD — the symbol impact/callers can walk, \
         not a promoted marker: {}",
        edge.from.symbol.as_str()
    );
    assert_eq!(edge.to.member, "billing");
    assert!(
        edge.to.symbol.as_str().contains("onOrder"),
        "the far endpoint is the real subscribing method: {}",
        edge.to.symbol.as_str()
    );

    // ── The workspace topic inventory the service map renders ─────────────────
    let inventory = workspace_topics(&registry);
    let api_topics = inventory
        .iter()
        .find(|m| m.member == "api")
        .expect("the api member reports its topics");
    assert_eq!(api_topics.topics.len(), 1);
    assert_eq!(api_topics.topics[0].topic, "orders");
    assert_eq!(api_topics.topics[0].producers, 1);
    assert_eq!(
        api_topics.topics[0].consumers, 0,
        "api publishes to `orders`; it does not consume it"
    );

    let billing_topics = inventory
        .iter()
        .find(|m| m.member == "billing")
        .expect("the billing member reports its topics");
    assert_eq!(billing_topics.topics[0].topic, "orders");
    assert_eq!(billing_topics.topics[0].producers, 0);
    assert_eq!(billing_topics.topics[0].consumers, 1);

    // ── The coverage board must AGREE with the bridge ([NFR-CC-04]) ───────────
    let cov = cross_service_coverage(&registry);
    assert_eq!(
        cov.bound, 1,
        "the coverage tier reports the publish BOUND — it must not deny the provider the \
         bridge just found: {:?}",
        cov.references
    );
    assert_eq!(cov.no_provider_in_workspace, 0);
    assert_eq!(cov.references[0].relation, "broker-topic");

    // ── The bridge mutates no member database ([ADR-52]) ──────────────────────
    assert_eq!(db_bytes(&api), api_before, "the bridge wrote to the api member's store");
    assert_eq!(
        db_bytes(&billing),
        billing_before,
        "the bridge wrote to the billing member's store"
    );
}

/// A topic published in one member with **no subscriber anywhere in the workspace** is
/// still a first-class, visible per-repo entity — the [FR-WS-11] promise that a topic is
/// visible *before* any cross-repo match. The bridge binds nothing (nothing to bind to),
/// and no edge is fabricated.
///
/// [FR-WS-11]: ../../docs/specs/requirements/FR-WS-11.md
#[test]
fn a_per_repo_topic_is_visible_across_the_workspace_with_no_cross_repo_match() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let api = root.join("api");
    let billing = root.join("billing");
    write(&api, "src/OrderService.java", PUBLISHER);
    // `billing` indexes real code, but nothing broker-related.
    write(
        &billing,
        "src/Plain.java",
        "package com.acme;\nclass Plain { public int add(int a, int b) { return a + b; } }\n",
    );
    index_member(&api);
    index_member(&billing);

    let registry = EngineRegistry::<Engine>::new(
        federation(root, vec![member("api", &api), member("billing", &billing)]),
        RegistryMode::Lazy,
    );

    // No cross-repo match exists…
    let edges = ContractBridge::new().edges(&registry);
    assert!(
        edges.is_empty(),
        "no subscriber anywhere — no edge is invented: {edges:?}"
    );

    // …yet the topic is fully visible in the workspace inventory the map draws.
    let inventory = workspace_topics(&registry);
    let api_topics = inventory.iter().find(|m| m.member == "api").expect("api reports");
    assert_eq!(api_topics.topics[0].topic, "orders");
    assert_eq!(api_topics.topics[0].producers, 1);
    assert_eq!(api_topics.topics[0].consumers, 0);

    // The topic-free member is present and honestly empty — "no topics" is a fact, and
    // it is distinct from "could not be read" (which would drop the member entirely).
    let billing_topics = inventory
        .iter()
        .find(|m| m.member == "billing")
        .expect("a healthy member with no topics is still reported");
    assert!(billing_topics.topics.is_empty());
}
