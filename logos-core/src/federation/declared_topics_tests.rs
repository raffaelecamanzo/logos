//! Unit tests for the declared-topic relation (S-489, [FR-WS-36], [ADR-72]) —
//! the rules over fixture facts, the stamp-cached [`DeclaredTopics`] holder
//! over fake engines, and the byte-identity of every observed figure with the
//! intake on and off.
//!
//! The fixture workspace is [S-488]'s recorded estate shapes in miniature:
//!
//! - `orders-api` publishes `orders` and `archive-message-queue` (keyed);
//! - `orders-listener` subscribes to `orders`, and its configuration declares
//!   `orders` too — **already observed**;
//! - `aggregator` has no broker site at all and declares
//!   `archive-message-queue` under `filteraggregate.topic` — **net-new,
//!   config-only** (`funnel-aggregator-api`'s shape);
//! - `facade` publishes through a generic wrapper whose topic the product
//!   refused (`topic-not-literal`) and declares `orders` — **net-new,
//!   site-refused** (`pecserver-facade`'s shape);
//! - `relay` subscribes to `metrics` (keyed; a subscribe is a provider, so it
//!   files no coverage row) and declares `orders` — **net-new,
//!   site-uncaptured** through its promoted topic alone;
//! - `emitter` publishes `audit` (keyed, no subscriber: unbound
//!   `no-provider-in-workspace`, an observed site, not a refusal) and declares
//!   `orders` — **net-new, site-uncaptured** through its broker row;
//! - `tools` declares `clickstream`, which no broker site names —
//!   **declared-only** (`pec-services-tools`' shape).
//!
//! [FR-WS-36]: ../../../docs/specs/requirements/FR-WS-36.md
//! [ADR-72]: ../../../docs/specs/architecture/decisions/ADR-72.md
//! [S-488]: ../../../docs/planning/journal.md#s-488-measure-the-net-new-config-declared-topic-memberships-beyond-the-observed-broker-edges

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::*;
use crate::federation::bridge::{ContractBridge, ContractNode, InvocationRef};
use crate::federation::registry::RegistryMode;
use crate::federation::topics::workspace_topics;
use crate::federation::{Federation, Member};
use crate::graph_store::ConfigDefinition;
use crate::model::{ArtifactRelation, LogosSymbol};

// ── fixture builders ──────────────────────────────────────────────────────

const MAIN: &str = "src/main/resources/application.yml";

thread_local! {
    static CONFIG: RefCell<HashMap<String, MemberCorpus>> = RefCell::new(HashMap::new());
    static TOPICS: RefCell<HashMap<String, Vec<TopicSummary>>> = RefCell::new(HashMap::new());
    static REFS: RefCell<HashMap<String, Vec<InvocationRef>>> = RefCell::new(HashMap::new());
    static STAMPS: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    /// `committed_config` reads, all members — what the cache test counts.
    static CONFIG_READS: Cell<usize> = const { Cell::new(0) };
    /// `invocation_refs` reads, all members — one per member per coverage walk.
    static LEDGER_READS: Cell<usize> = const { Cell::new(0) };
}

fn reset() {
    CONFIG.with(|c| c.borrow_mut().clear());
    TOPICS.with(|c| c.borrow_mut().clear());
    REFS.with(|c| c.borrow_mut().clear());
    STAMPS.with(|c| c.borrow_mut().clear());
    CONFIG_READS.with(|c| c.set(0));
    LEDGER_READS.with(|c| c.set(0));
}

/// Commit `key = value` in `member`'s `file`, under `profile`.
fn commit(member: &str, file: &str, profile: Option<&str>, key: &str, value: &str) {
    CONFIG.with(|c| {
        c.borrow_mut()
            .entry(member.to_string())
            .or_default()
            .entry(crate::extract::config::corpus::canonical_key(key))
            .or_default()
            .push(ConfigDefinition {
                path: file.to_string(),
                profile: profile.map(str::to_string),
                value: value.to_string(),
            });
    });
}

/// `member`'s promoted topics: `(topic, producers, consumers)`.
fn promote(member: &str, topics: &[(&str, usize, usize)]) {
    TOPICS.with(|c| {
        c.borrow_mut().insert(
            member.to_string(),
            topics
                .iter()
                .map(|(topic, producers, consumers)| TopicSummary {
                    topic: (*topic).to_string(),
                    producers: *producers,
                    consumers: *consumers,
                })
                .collect(),
        );
    });
}

fn broker(member: &str, relation: ArtifactRelation, target: &str, symbol: &str) {
    REFS.with(|c| {
        c.borrow_mut()
            .entry(member.to_string())
            .or_default()
            .push(InvocationRef {
                relation,
                target: target.to_string(),
                symbol: LogosSymbol::parse(symbol).unwrap(),
            });
    });
}

#[derive(Debug)]
struct FakeEngine {
    member: String,
}

impl MemberEngine for FakeEngine {
    type Watcher = ();
    fn start(
        root: &Path,
        _read_connections: usize,
        _worker_pool: crate::SharedWorkerPool,
    ) -> Result<Arc<Self>> {
        let member = root.file_name().unwrap().to_string_lossy().into_owned();
        if member == "broken" {
            anyhow::bail!("store is corrupt");
        }
        Ok(Arc::new(FakeEngine { member }))
    }
    fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
        Ok(())
    }
}

impl MemberContracts for FakeEngine {
    fn contract_surface(&self) -> Result<Vec<ContractNode>> {
        Ok(Vec::new())
    }
    fn contract_stamp(&self) -> u64 {
        STAMPS.with(|s| s.borrow().get(&self.member).copied().unwrap_or(0))
    }
    fn invocation_refs(&self) -> Result<Vec<InvocationRef>> {
        LEDGER_READS.with(|c| c.set(c.get() + 1));
        if self.member == "covfail" {
            anyhow::bail!("ledger read failed");
        }
        Ok(REFS.with(|c| c.borrow().get(&self.member).cloned().unwrap_or_default()))
    }
    fn topic_surface(&self) -> Result<Vec<TopicSummary>> {
        Ok(TOPICS.with(|c| c.borrow().get(&self.member).cloned().unwrap_or_default()))
    }
    fn config_definitions(&self, keys: &[String]) -> Result<MemberCorpus> {
        Ok(CONFIG.with(|c| {
            let all = c.borrow();
            let Some(corpus) = all.get(&self.member) else {
                return MemberCorpus::new();
            };
            keys.iter()
                .filter_map(|k| corpus.get(k).map(|d| (k.clone(), d.clone())))
                .collect()
        }))
    }
    fn committed_config(&self) -> Result<MemberCorpus> {
        if self.member == "unreadable" {
            anyhow::bail!("store read failed");
        }
        CONFIG_READS.with(|c| c.set(c.get() + 1));
        Ok(CONFIG.with(|c| c.borrow().get(&self.member).cloned().unwrap_or_default()))
    }
}

fn registry(names: &[&str]) -> EngineRegistry<FakeEngine> {
    let root = PathBuf::from("/ws");
    EngineRegistry::new(
        Federation {
            name: "w".to_string(),
            members: names
                .iter()
                .map(|name| Member {
                    name: (*name).to_string(),
                    root: root.join(name),
                })
                .collect(),
            root,
            default: None,
            links: Vec::new(),
            governance: Default::default(),
            warm_concurrency: None,
            member_kinds: Default::default(),
            exclusion: Default::default(),
        },
        RegistryMode::Lazy,
    )
}

const WORKSPACE: [&str; 7] = [
    "orders-api",
    "orders-listener",
    "aggregator",
    "facade",
    "relay",
    "emitter",
    "tools",
];

/// The module docs' workspace.
fn estate_in_miniature() {
    reset();
    promote(
        "orders-api",
        &[("archive-message-queue", 1, 0), ("orders", 1, 0)],
    );
    broker(
        "orders-api",
        ArtifactRelation::BrokerPublish,
        "orders",
        "local api_send",
    );
    broker(
        "orders-api",
        ArtifactRelation::BrokerPublish,
        "archive-message-queue",
        "local api_archive",
    );

    promote("orders-listener", &[("orders", 0, 1)]);
    broker(
        "orders-listener",
        ArtifactRelation::BrokerSubscribe,
        "orders",
        "local on_order",
    );
    commit(
        "orders-listener",
        MAIN,
        None,
        "spring.kafka.topics.orders",
        "orders",
    );

    commit(
        "aggregator",
        MAIN,
        None,
        "filteraggregate.topic",
        "archive-message-queue",
    );

    broker(
        "facade",
        ArtifactRelation::BrokerPublish,
        "",
        "local KafkaProducer_sendMessage",
    );
    commit(
        "facade",
        MAIN,
        None,
        "spring.kafka.topics.outcomes",
        "orders",
    );

    promote("relay", &[("metrics", 0, 1)]);
    broker(
        "relay",
        ArtifactRelation::BrokerSubscribe,
        "metrics",
        "local relay_metrics",
    );
    commit(
        "relay",
        MAIN,
        None,
        "spring.kafka.template.default-topic",
        "orders",
    );

    promote("emitter", &[("audit", 1, 0)]);
    broker(
        "emitter",
        ArtifactRelation::BrokerPublish,
        "audit",
        "local emitter_audit",
    );
    commit("emitter", MAIN, None, "app.outbound.topic", "orders");

    commit("tools", MAIN, None, "kafka.topics", "clickstream");
}

fn relation_over(names: &[&str]) -> Arc<DeclaredTopicRelation> {
    DeclaredTopics::new().relation(&registry(names))
}

/// `(member, topic, status)` of every declaration.
fn statuses(r: &DeclaredTopicRelation) -> Vec<(&str, &str, DeclarationStatus)> {
    r.declarations
        .iter()
        .map(|d| (d.member.as_str(), d.topic.as_str(), d.status))
        .collect()
}

// ── AC1: what a declaration is ────────────────────────────────────────────

/// The three topic-named shapes, each probed with the near miss one character
/// from matching ([CR-157] A1).
///
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
#[test]
fn a_topic_named_key_is_one_of_three_shapes_and_its_near_misses_are_not() {
    assert_eq!(
        key_shape("spring.kafka.topics.orders"),
        Some(KeyShape::SpringKafkaTopics)
    );
    assert_eq!(
        key_shape("filteraggregate.topic"),
        Some(KeyShape::LastSegment)
    );
    assert_eq!(key_shape("kafka.topics"), Some(KeyShape::LastSegment));
    assert_eq!(
        key_shape("spring.kafka.template.defaulttopic"),
        Some(KeyShape::LastSegmentSuffix)
    );
    assert_eq!(
        key_shape("app.inboundtopics"),
        Some(KeyShape::LastSegmentSuffix)
    );
    for near_miss in [
        "app.topicname",
        "app.topic.name",
        "spring.kafka.topicsx.orders",
        "app.name",
        "topical",
    ] {
        assert_eq!(key_shape(near_miss), None, "{near_miss} names no topic");
    }
}

/// Only a `src/main` source is read — the member's own or a module's — and its
/// near misses (`src/maintenance`, a test tree, a `mysrc/main` directory) are
/// not ([CR-157] B2).
///
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
#[test]
fn only_a_src_main_source_is_read() {
    assert!(is_src_main("src/main/resources/application.yml"));
    assert!(is_src_main("manager/src/main/resources/application.yml"));
    for near_miss in [
        "src/maintenance/application.yml",
        "src/test/resources/application.yml",
        "manager/src/test/resources/application.yml",
        "mysrc/main/resources/application.yml",
        "application.yml",
    ] {
        assert!(!is_src_main(near_miss), "{near_miss} is not src/main");
    }

    reset();
    promote("api", &[("orders", 1, 0)]);
    commit(
        "svc",
        "src/test/resources/application.yml",
        None,
        "spring.kafka.topics.orders",
        "orders",
    );
    commit(
        "svc",
        "src/maintenance/application.yml",
        None,
        "spring.kafka.topics.orders",
        "orders",
    );
    commit("svc", MAIN, None, "app.name", "orders");
    let r = relation_over(&["api", "svc"]);
    assert!(
        r.declarations.is_empty(),
        "test tree, near-miss tree and a non-topic key: {:?}",
        r.declarations
    );
    assert!(r.is_empty(), "nothing was considered");

    commit(
        "svc",
        "manager/src/main/resources/application.yml",
        None,
        "spring.kafka.topics.orders",
        "orders",
    );
    let r = relation_over(&["api", "svc"]);
    assert_eq!(
        statuses(&r),
        [("svc", "orders", DeclarationStatus::NetNew)],
        "a module's src/main is read"
    );
}

/// **AC1, the declaration**: a topic-named key whose committed value is a
/// captured topic yields one declaration with `config-declared` provenance,
/// key, file and profile.
#[test]
fn a_topic_named_value_equal_to_a_captured_topic_is_a_declaration_with_key_file_and_profile() {
    estate_in_miniature();
    let r = relation_over(&WORKSPACE);
    let aggregator: Vec<&TopicDeclaration> = r
        .declarations
        .iter()
        .filter(|d| d.member == "aggregator")
        .collect();
    assert_eq!(aggregator.len(), 1, "{:?}", r.declarations);
    let d = aggregator[0];
    assert_eq!(d.topic, "archive-message-queue");
    assert_eq!(d.provenance, CONFIG_DECLARED);
    assert_eq!(d.key, "filteraggregate.topic");
    assert_eq!(d.file, MAIN);
    assert_eq!(d.profile, None, "the unprofiled base");
    assert_eq!(d.via, None, "a literal value takes no hop");
    assert_eq!(d.shape, KeyShape::LastSegment);
    assert_eq!(d.status, DeclarationStatus::NetNew);
}

/// **AC1, the profile**: through a placeholder, one declaration per profile a
/// committed composition proves — each tagged with its own composition's
/// profiles, never the union — and a profiled declaring file reads only its
/// own profile's composition.
#[test]
fn a_placeholder_hop_yields_one_declaration_per_proving_profile() {
    reset();
    promote("api", &[("orders-dev", 1, 0), ("orders-prod", 1, 0)]);
    commit(
        "svc",
        MAIN,
        None,
        "spring.kafka.topics.orders",
        "${orders.name}",
    );
    commit(
        "svc",
        "src/main/resources/application-dev.yml",
        Some("dev"),
        "orders.name",
        "orders-dev",
    );
    commit(
        "svc",
        "src/main/resources/application-prod.yml",
        Some("prod"),
        "orders.name",
        "orders-prod",
    );
    commit(
        "svc",
        "src/main/resources/application-dev.yml",
        Some("dev"),
        "inbound.topic",
        "${orders.name}",
    );

    let r = relation_over(&["api", "svc"]);
    let rows: Vec<(&str, &str, Option<&str>, Vec<&str>)> = r
        .declarations
        .iter()
        .map(|d| {
            let via = d.via.as_ref().expect("every value here hops");
            assert_eq!(via.keys, ["orders.name"]);
            assert!(!via.unprofiled, "no base composition exists");
            (
                d.key.as_str(),
                d.topic.as_str(),
                d.profile.as_deref(),
                via.profiles.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("inbound.topic", "orders-dev", Some("dev"), vec!["dev"]),
            (
                "spring.kafka.topics.orders",
                "orders-dev",
                None,
                vec!["dev"]
            ),
            (
                "spring.kafka.topics.orders",
                "orders-prod",
                None,
                vec!["prod"]
            ),
        ],
        "the dev file reads dev's composition alone; the base reads both, each its own"
    );
    assert_eq!(
        r.headline.declarations_considered, 2,
        "two values, three declarations"
    );
}

/// A profile that sets the hopped key to blank overrides the base under
/// Spring: the profiled declaration proves no topic, so it is refused — never
/// admitted through the base's value (review finding, S-489).
#[test]
fn a_blank_profile_override_is_refused_not_read_through_the_base() {
    reset();
    promote("api", &[("orders", 1, 0)]);
    commit("svc", MAIN, None, "app.name", "orders");
    commit("svc", "src/main/resources/application-p.yml", Some("p"), "app.name", "");
    commit("svc", "src/main/resources/application-p.yml", Some("p"), "x.topic", "${app.name}");
    let r = relation_over(&["api", "svc"]);
    assert!(r.declarations.is_empty(), "p proves a blank composition: {:?}", r.declarations);
    let refused: Vec<(&str, ValueRefusal)> = r.refused.iter().map(|x| (x.key.as_str(), x.reason)).collect();
    assert_eq!(refused, [("x.topic", ValueRefusal::MissingKey)]);
}

/// **AC1, declared-only**: a value no member's broker site names is not a
/// topic. It is listed apart and never counted in `declared_topic_pairs`.
#[test]
fn a_value_no_broker_site_names_is_declared_only() {
    estate_in_miniature();
    let r = relation_over(&WORKSPACE);
    let tools: Vec<_> = statuses(&r)
        .into_iter()
        .filter(|(m, _, _)| *m == "tools")
        .collect();
    assert_eq!(
        tools,
        [("tools", "clickstream", DeclarationStatus::DeclaredOnly)]
    );
    let only = r.declarations.iter().find(|d| d.member == "tools").unwrap();
    assert_eq!(only.site, None, "declared-only carries no site split");
    assert_eq!(
        r.headline.declared_only,
        PairTally {
            pairs: 1,
            declarations: 1
        }
    );
}

/// **AC1, refusals**: a value whose placeholder names no committed key, and one
/// whose committed value is itself a placeholder, each refuse with the
/// existing [`ValueRefusal`] word and declare nothing ([NFR-RA-05]).
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[test]
fn an_uncommitted_or_placeholder_value_refuses_with_the_existing_reasons() {
    reset();
    promote("api", &[("orders", 1, 0)]);
    commit(
        "svc",
        MAIN,
        None,
        "spring.kafka.topics.orders",
        "${KAFKA_ORDERS_TOPIC}",
    );
    commit(
        "svc",
        MAIN,
        None,
        "spring.kafka.topics.events",
        "${events.name}",
    );
    commit("svc", MAIN, None, "events.name", "${EVENTS}");

    let r = relation_over(&["api", "svc"]);
    assert!(
        r.declarations.is_empty(),
        "nothing is guessed: {:?}",
        r.declarations
    );
    let refused: Vec<(&str, &str, ValueRefusal)> = r
        .refused
        .iter()
        .map(|x| (x.key.as_str(), x.value.as_str(), x.reason))
        .collect();
    assert_eq!(
        refused,
        [
            (
                "spring.kafka.topics.events",
                "${events.name}",
                ValueRefusal::PlaceholderValue
            ),
            (
                "spring.kafka.topics.orders",
                "${KAFKA_ORDERS_TOPIC}",
                ValueRefusal::MissingKey
            ),
        ]
    );
    assert_eq!(r.headline.refused, 2);
    assert_eq!(r.headline.declarations_considered, 2);
    assert_eq!(
        serde_json::to_value(&r.refused[0]).unwrap()["reason"],
        "placeholder-value",
        "the shipped wire word"
    );
}

/// **AC1, observed**: a member already shown on the topic by its own observed
/// consumer is `observed`, not net-new, and carries no site split.
#[test]
fn a_declaration_beside_an_observed_site_on_the_topic_is_observed() {
    estate_in_miniature();
    let r = relation_over(&WORKSPACE);
    let listener = r
        .declarations
        .iter()
        .find(|d| d.member == "orders-listener")
        .unwrap();
    assert_eq!(
        (listener.topic.as_str(), listener.status),
        ("orders", DeclarationStatus::Observed)
    );
    assert_eq!(listener.site, None);
}

/// **The B3 split** is read off the coverage tier's broker rows at member
/// grain: no broker row and no promoted topic is config-only; a refused row
/// (`topic-not-literal`, with its symbol) is site-refused; a keyed site none on
/// the topic — a promoted topic with no row, or an unmatched publish's
/// `no-provider-in-workspace` row — is site-uncaptured.
#[test]
fn the_net_new_split_reads_the_members_broker_rows() {
    estate_in_miniature();
    let r = relation_over(&WORKSPACE);
    let site = |member: &str| {
        r.declarations
            .iter()
            .find(|d| d.member == member)
            .unwrap()
            .site
            .clone()
    };
    assert_eq!(site("aggregator"), Some(SiteState::ConfigOnly));
    assert_eq!(
        site("facade"),
        Some(SiteState::SiteRefused {
            refusals: vec![SiteRefusal {
                reason: "topic-not-literal".to_string(),
                symbol: "local KafkaProducer_sendMessage".to_string(),
            }]
        })
    );
    assert_eq!(
        site("relay"),
        Some(SiteState::SiteUncaptured),
        "no broker row, but a promoted topic"
    );
    assert_eq!(
        site("emitter"),
        Some(SiteState::SiteUncaptured),
        "an unmatched publish is no refusal"
    );
    let n = r.headline.net_new;
    assert_eq!(
        (n.pairs, n.config_only, n.site_refused, n.site_uncaptured),
        (4, 1, 1, 2)
    );
    assert!(n.split_covers_all_members);
}

/// A broker site whose topic placeholder the member's configuration cannot
/// prove keeps its placeholder-as-written key: the coverage tier files it
/// `config-unresolved`, and that is a refusal for the split, whatever its
/// bucket ([S-488]'s rule).
///
/// [S-488]: ../../../docs/planning/journal.md#s-488-measure-the-net-new-config-declared-topic-memberships-beyond-the-observed-broker-edges
#[test]
fn a_broker_site_whose_topic_placeholder_refused_is_site_refused() {
    reset();
    promote("api", &[("orders", 1, 0)]);
    broker("api", ArtifactRelation::BrokerPublish, "orders", "local api_send");
    broker("svc", ArtifactRelation::BrokerPublish, "${missing.topic}", "local svc_send");
    commit("svc", MAIN, None, "spring.kafka.topics.orders", "orders");

    let r = relation_over(&["api", "svc"]);
    let svc = r.declarations.iter().find(|d| d.member == "svc").expect("svc declares orders");
    assert_eq!(
        svc.site,
        Some(SiteState::SiteRefused {
            refusals: vec![SiteRefusal {
                reason: format!("config-unresolved: {}", ValueRefusal::MissingKey.label()),
                symbol: "local svc_send".to_string(),
            }]
        })
    );
}

/// A net-new member whose broker sites the coverage walk could not read is
/// `site-unread` — never `config-only` on a read that did not happen — and the
/// split is marked partial even though every member's contract surface was
/// read ([NFR-CC-04]; review finding, S-489).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[test]
fn a_member_whose_broker_sites_were_not_read_is_site_unread_and_the_split_partial() {
    reset();
    promote("api", &[("orders", 1, 0)]);
    broker("api", ArtifactRelation::BrokerPublish, "orders", "local api_send");
    broker("covfail", ArtifactRelation::BrokerPublish, "", "local covfail_send");
    commit("covfail", MAIN, None, "app.outbound.topic", "orders");

    let r = relation_over(&["api", "covfail"]);
    assert_eq!(r.headline.members.read, 2, "its configuration and topics were read");
    assert_eq!(r.declarations[0].site, Some(SiteState::SiteUnread));
    let n = r.headline.net_new;
    assert_eq!((n.pairs, n.config_only, n.site_unread), (1, 0, 1));
    assert!(!n.split_covers_all_members);
    assert!(r.headline.summary.contains("1 site-unread; site split partial"), "{}", r.headline.summary);
    assert_eq!(serde_json::to_value(&r.declarations[0]).unwrap()["site"]["site"], "site-unread");
}

/// A profiled declaration whose profile proves no composition of the hopped
/// key reads the unprofiled base's — Spring under `dev` reads
/// `application.yml` — tagged `unprofiled`; a declaration neither its profile
/// nor the base proves is refused `MissingKey`, never dropped from the
/// accounting (review finding, S-489).
#[test]
fn a_profiled_declaration_falls_back_to_the_base_and_one_nothing_proves_is_refused() {
    reset();
    promote("api", &[("orders", 1, 0), ("orders-prod", 1, 0), ("audit-prod", 1, 0)]);
    commit("svc", MAIN, None, "orders.name", "orders");
    commit("svc", "src/main/resources/application-prod.yml", Some("prod"), "orders.name", "orders-prod");
    commit("svc", "src/main/resources/application-prod.yml", Some("prod"), "audit.name", "audit-prod");
    commit("svc", "src/main/resources/application-dev.yml", Some("dev"), "inbound.topic", "${orders.name}");
    commit("svc", "src/main/resources/application-dev.yml", Some("dev"), "audit.topic", "${audit.name}");

    let r = relation_over(&["api", "svc"]);
    let rows: Vec<(&str, &str, Option<&PlaceholderHop>)> =
        r.declarations.iter().map(|d| (d.key.as_str(), d.topic.as_str(), d.via.as_ref())).collect();
    assert_eq!(
        rows,
        [(
            "inbound.topic",
            "orders",
            Some(&PlaceholderHop {
                keys: vec!["orders.name".to_string()],
                profiles: vec![],
                unprofiled: true,
            })
        )],
        "dev reads the base's value, never prod's"
    );
    let refused: Vec<(&str, ValueRefusal)> = r.refused.iter().map(|x| (x.key.as_str(), x.reason)).collect();
    assert_eq!(refused, [("audit.topic", ValueRefusal::MissingKey)], "only prod proves audit.name");
    assert_eq!(r.headline.declarations_considered, r.declarations.len() as u64 + r.headline.refused);
}

/// An unprofiled hop through the base's own value is tagged `unprofiled`.
#[test]
fn an_unprofiled_hop_through_the_base_is_marked_unprofiled() {
    reset();
    promote("api", &[("orders", 1, 0)]);
    commit("svc", MAIN, None, "orders.name", "orders");
    commit("svc", MAIN, None, "spring.kafka.topics.orders", "${orders.name}");
    let r = relation_over(&["api", "svc"]);
    let via = r.declarations[0].via.as_ref().expect("the value hops");
    assert!(via.unprofiled && via.profiles.is_empty(), "{via:?}");
}

/// The hop reads `src/main` sources only: a `src/main` declaration whose
/// placeholder only a test tree defines resolves to nothing and is refused
/// ([CR-157] B2; review finding, S-489).
///
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
#[test]
fn the_hop_never_resolves_through_a_test_tree_value() {
    reset();
    promote("api", &[("orders", 1, 0)]);
    commit("svc", MAIN, None, "spring.kafka.topics.orders", "${orders.name}");
    commit("svc", "src/test/resources/application.yml", None, "orders.name", "orders");
    let r = relation_over(&["api", "svc"]);
    assert!(r.declarations.is_empty(), "{:?}", r.declarations);
    assert_eq!(r.headline.refused, 1);
}

/// A promoted topic keyed with a message-schema guard is captured by its topic
/// half, so a configuration naming the topic matches it (review finding,
/// S-489).
#[test]
fn a_guarded_promoted_topic_is_captured_by_its_topic_half() {
    reset();
    promote("api", &[("orders#com.acme.OrderCreated", 1, 0)]);
    commit("svc", MAIN, None, "spring.kafka.topics.orders", "orders");
    let r = relation_over(&["api", "svc"]);
    assert_eq!(statuses(&r), [("svc", "orders", DeclarationStatus::NetNew)]);
    assert_eq!(r.headline.captured_topics, 1);
}

/// A topic node with no producer and no consumer is not captured — nothing
/// resolves to it — so a declaration naming it is declared-only ([BR-55];
/// review finding, S-489).
///
/// [BR-55]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn a_promoted_topic_with_no_producer_or_consumer_is_not_captured() {
    reset();
    promote("api", &[("orders", 0, 0)]);
    commit("svc", MAIN, None, "spring.kafka.topics.orders", "orders");
    let r = relation_over(&["api", "svc"]);
    assert_eq!(statuses(&r), [("svc", "orders", DeclarationStatus::DeclaredOnly)]);
    assert_eq!(r.headline.captured_topics, 0);
}

// ── AC2: the headline ─────────────────────────────────────────────────────

/// **AC2**: the headline states every figure beside its denominator, and the
/// accounting closes — considered = resolved + refused; pairs = observed +
/// net-new; net-new = config-only + site-refused + site-uncaptured; members
/// read of members, the unread named with why. Nothing is bare ([BR-51]).
///
/// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn the_headline_states_every_figure_beside_its_denominator() {
    estate_in_miniature();
    commit(
        "tools",
        MAIN,
        None,
        "spring.kafka.topics.missing",
        "${nowhere}",
    );
    let mut names = WORKSPACE.to_vec();
    names.push("unreadable");
    let r = relation_over(&names);
    let h = &r.headline;

    assert_eq!(h.declared_topic_pairs, 5, "1 observed + 4 net-new");
    assert_eq!(
        h.declared_topic_pairs,
        h.already_observed.pairs + h.net_new.pairs
    );
    assert_eq!(
        h.already_observed,
        PairTally {
            pairs: 1,
            declarations: 1
        }
    );
    assert_eq!(
        h.net_new.pairs,
        h.net_new.config_only + h.net_new.site_refused + h.net_new.site_uncaptured + h.net_new.site_unread
    );
    assert_eq!(h.declarations_considered, 7);
    let resolved_values = r.declarations.len() as u64; // one topic per value here
    assert_eq!(h.declarations_considered, resolved_values + h.refused);
    assert_eq!(h.refused, 1);
    assert_eq!(
        h.captured_topics, 4,
        "orders, archive-message-queue, metrics, audit"
    );
    assert_eq!(
        (h.members.members, h.members.read, h.members.declaring),
        (8, 7, 6)
    );
    assert_eq!(h.members.unread, ["unreadable"]);
    assert_eq!(h.members.unread_reasons["unreadable"], UNREAD_FAILED);

    for figure in [
        "declared_topic_pairs 5",
        "1 already observed",
        "4 net-new: 1 config-only, 1 site-refused, 2 site-uncaptured",
        "7 declarations considered",
        "7 of 8 members read",
        "4 captured topics",
        "1 declared-only",
        "1 refused",
        "never an observed broker edge",
    ] {
        assert!(
            h.summary.contains(figure),
            "{figure:?} missing from {:?}",
            h.summary
        );
    }
    let json = serde_json::to_value(h).unwrap();
    for key in [
        "declared_topic_pairs",
        "declarations_considered",
        "already_observed",
        "net_new",
        "declared_only",
        "refused",
        "captured_topics",
        "members",
        "summary",
    ] {
        assert!(json.get(key).is_some(), "{key} missing from {json}");
    }
    assert_eq!(json["net_new"]["site_refused"], 1);
}

/// A workspace that declares nothing is empty — a surface omits the section —
/// and a relation with no net-new pair never walks the coverage tier.
#[test]
fn a_workspace_with_no_net_new_pair_never_walks_the_coverage_tier() {
    let facts = |config: MemberCorpus| -> Vec<MemberTopicFacts> {
        vec![(
            "svc".to_string(),
            config,
            vec![TopicSummary {
                topic: "orders".to_string(),
                producers: 1,
                consumers: 0,
            }],
        )]
    };
    let roster = ["svc".to_string()];
    let r = derive(&roster, &facts(MemberCorpus::new()), || {
        panic!("no declaration, no coverage walk")
    });
    assert!(r.is_empty());
    assert_eq!(r.headline.declared_topic_pairs, 0);

    let mut config = MemberCorpus::new();
    config.insert(
        "spring.kafka.topics.orders".to_string(),
        vec![ConfigDefinition {
            path: MAIN.to_string(),
            profile: None,
            value: "orders".to_string(),
        }],
    );
    let r = derive(&roster, &facts(config), || {
        panic!("observed only, no coverage walk")
    });
    assert!(!r.is_empty());
    assert_eq!(
        statuses(&r),
        [("svc", "orders", DeclarationStatus::Observed)]
    );
}

// ── AC3: never coupling, byte-identical with the intake on and off ─────────

/// The observed figures over [`estate_in_miniature`]: the topic inventory, the
/// coverage payload (`resolved_cross_service_edges` and `egress_resolution`
/// included) and the bridge edge set, serialized.
fn observed_payloads(reg: &EngineRegistry<FakeEngine>) -> (String, String, String) {
    let answer = reg.answer();
    (
        serde_json::to_string(&workspace_topics(&answer)).unwrap(),
        serde_json::to_string(&cross_service_coverage(&answer)).unwrap(),
        serde_json::to_string(&*ContractBridge::new().edges(reg)).unwrap(),
    )
}

/// The three payloads [`observed_payloads`] returns for
/// [`estate_in_miniature`], recorded with the intake **never built** — the
/// "off" side. `coverage.rs`, `topics.rs`, `broker.rs` and the bridge's
/// matcher are untouched by S-489, so these are the bytes the base emits.
const OFF_TOPICS: &str = r#"[{"member":"orders-api","topics":[{"topic":"archive-message-queue","producers":1,"consumers":0},{"topic":"orders","producers":1,"consumers":0}]},{"member":"orders-listener","topics":[{"topic":"orders","producers":0,"consumers":1}]},{"member":"aggregator","topics":[]},{"member":"facade","topics":[]},{"member":"relay","topics":[{"topic":"metrics","producers":0,"consumers":1}]},{"member":"emitter","topics":[{"topic":"audit","producers":1,"consumers":0}]},{"member":"tools","topics":[]}]"#;

/// **AC3**: with the intake on — built, cached, its coverage walk taken for the
/// site split — the topic inventory, the coverage payload and the bridge edge
/// set serialize byte-for-byte as they do with it off, in the same registry
/// and in one that never built it. The relation is a declaration, never a
/// `BridgeEdge`, `Producer`, `Consumer`, `Publishes` or `Subscribes` ([BR-54]).
///
/// [BR-54]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn the_intake_leaves_every_observed_figure_byte_identical_on_and_off() {
    estate_in_miniature();
    let off = registry(&WORKSPACE);
    let off_payloads = observed_payloads(&off);
    assert_eq!(off_payloads.0, OFF_TOPICS, "the topic inventory moved");

    let on = registry(&WORKSPACE);
    let relation = DeclaredTopics::new().relation(&on);
    assert_eq!(
        relation.headline.net_new.pairs, 4,
        "the intake ran, site split included — not vacuous"
    );
    let on_payloads = observed_payloads(&on);
    assert_eq!(
        on_payloads.0, off_payloads.0,
        "the topic inventory moved with the intake on"
    );
    assert_eq!(
        on_payloads.1, off_payloads.1,
        "the coverage payload moved with the intake on"
    );
    assert_eq!(
        on_payloads.2, off_payloads.2,
        "the bridge edge set moved with the intake on"
    );

    let coverage: serde_json::Value = serde_json::from_str(&on_payloads.1).unwrap();
    assert!(
        coverage.get("declared_topics").is_none(),
        "never inside the coverage payload"
    );
    assert!(
        !on_payloads.2.contains("aggregator") && !on_payloads.2.contains("clickstream"),
        "no declaring member reaches the edge set: {}",
        on_payloads.2
    );
}

/// A blank value under a topic-named key is not a declaration and is not
/// considered; a workspace whose only topic-named values are refused is not
/// empty — its refusals must reach the surface (review finding, S-489).
#[test]
fn a_blank_value_is_not_considered_and_refusals_alone_keep_the_section() {
    reset();
    promote("api", &[("orders", 1, 0)]);
    commit("svc", MAIN, None, "spring.kafka.topics.orders", "   ");
    let r = relation_over(&["api", "svc"]);
    assert!(r.is_empty(), "a blank value is not considered: {:?}", r.headline);
    assert!(r.declarations.is_empty() && r.refused.is_empty());

    commit("svc", MAIN, None, "spring.kafka.topics.events", "${MISSING}");
    let r = relation_over(&["api", "svc"]);
    assert_eq!((r.headline.declarations_considered, r.headline.refused), (1, 1));
    assert!(!r.is_empty(), "refusals alone keep the section");
}

// ── The holder ────────────────────────────────────────────────────────────

/// Built on the **first** query and cached on the members' stamps: a second
/// query reads nothing; a member re-sync is a miss ([ADR-52]).
///
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
#[test]
fn the_relation_is_built_on_the_first_query_and_cached_on_the_stamps() {
    estate_in_miniature();
    let reg = registry(&WORKSPACE);
    let holder = DeclaredTopics::new();
    assert_eq!(
        CONFIG_READS.with(Cell::get),
        0,
        "holding the relation reads nothing"
    );
    let first = holder.relation(&reg);
    assert_eq!(
        CONFIG_READS.with(Cell::get),
        WORKSPACE.len(),
        "one configuration read per member"
    );
    let again = holder.relation(&reg);
    assert!(
        Arc::ptr_eq(&first, &again),
        "a hit returns the cached relation"
    );
    assert_eq!(
        CONFIG_READS.with(Cell::get),
        WORKSPACE.len(),
        "a hit reads no configuration"
    );

    STAMPS.with(|s| s.borrow_mut().insert("relay".to_string(), 1));
    let after = holder.relation(&reg);
    assert!(!Arc::ptr_eq(&first, &after), "a re-synced member is a miss");
    assert_eq!(CONFIG_READS.with(Cell::get), 2 * WORKSPACE.len());
}

/// A caller that already walked coverage in its answer hands it on: the site
/// split reads it and walks coverage no second time, and the relation equals the
/// one that walked its own ([NFR-PE-10]; review finding, S-489). `workspace
/// status` itself does not come through `relation_in`: it derives from the walks
/// it already makes (`query::declared_topics_section`).
///
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
#[test]
fn a_caller_holding_the_coverage_walk_hands_it_on_and_coverage_is_not_walked_twice() {
    estate_in_miniature();
    let walked_alone = relation_over(&WORKSPACE);

    let reg = registry(&WORKSPACE);
    let answer = reg.answer();
    let stamps = current_stamps(&answer);
    let coverage = cross_service_coverage(&answer);
    let walks_before = LEDGER_READS.with(Cell::get);
    let handed_on = DeclaredTopics::new().relation_in(&reg, &answer, stamps, Some(&coverage));
    assert_eq!(LEDGER_READS.with(Cell::get), walks_before, "no second coverage walk");
    assert_eq!(*handed_on, *walked_alone, "the same relation either way");
    assert_eq!(handed_on.headline.net_new.pairs, 4, "the split ran — not vacuous");
}

/// A member whose engine will not start is unread, named, and the rest of the
/// relation stands ([ADR-53]).
///
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
#[test]
fn a_member_that_will_not_open_is_named_unread_and_the_rest_stands() {
    estate_in_miniature();
    let mut names = WORKSPACE.to_vec();
    names.insert(0, "broken");
    let r = relation_over(&names);
    assert_eq!(r.headline.members.unread, ["broken"]);
    assert_eq!(r.headline.members.read, WORKSPACE.len() as u64);
    // The coverage walk behind the split read one member fewer, so the split
    // says it is partial — beside its figures, never silently.
    assert!(!r.headline.net_new.split_covers_all_members);
    assert!(
        r.headline.summary.contains("; site split partial: coverage did not read every member"),
        "{}",
        r.headline.summary
    );
    assert_eq!(r.headline.net_new.pairs, 4);
    assert!(!r.was_read("broken"));
    assert!(r.was_read("aggregator"));
}

// ── The accessor ──────────────────────────────────────────────────────────

/// `xservice_declared_topics` groups by captured topic, marks each declaration,
/// lists declared-only and refused apart, scopes to a member, and names an
/// unknown or unread scope instead of answering empty ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[test]
fn the_accessor_lists_per_topic_scopes_to_a_member_and_names_an_unknown_scope() {
    use crate::federation::query::xservice_declared_topics;

    estate_in_miniature();
    let mut names = WORKSPACE.to_vec();
    names.push("broken");
    let r = relation_over(&names);

    let all = xservice_declared_topics(&r, None);
    let grouped: Vec<(&str, Vec<(&str, DeclarationStatus)>)> = all
        .topics
        .iter()
        .map(|t| {
            (
                t.topic.as_str(),
                t.declarations
                    .iter()
                    .map(|d| (d.member.as_str(), d.status))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        grouped,
        [
            (
                "archive-message-queue",
                vec![("aggregator", DeclarationStatus::NetNew)]
            ),
            (
                "orders",
                vec![
                    ("emitter", DeclarationStatus::NetNew),
                    ("facade", DeclarationStatus::NetNew),
                    ("orders-listener", DeclarationStatus::Observed),
                    ("relay", DeclarationStatus::NetNew),
                ]
            ),
        ]
    );
    assert_eq!(all.declared_only.len(), 1);
    assert_eq!(all.scope, None);
    assert_eq!(all.scope_note, None);

    let facade = xservice_declared_topics(&r, Some("facade"));
    assert_eq!(facade.topics.len(), 1);
    assert_eq!(facade.topics[0].declarations.len(), 1);
    assert!(facade.declared_only.is_empty());
    assert_eq!(
        facade.scope_note, None,
        "a member read is answered, even when it lists little"
    );
    assert_eq!(
        facade.headline, r.headline,
        "the headline is workspace-wide whatever the scope"
    );

    let api = xservice_declared_topics(&r, Some("orders-api"));
    assert!(
        api.topics.is_empty() && api.scope_note.is_none(),
        "read, declares nothing: {api:?}"
    );

    let unknown = xservice_declared_topics(&r, Some("nope"));
    assert!(unknown.topics.is_empty());
    assert_eq!(
        unknown.scope_note.as_deref(),
        Some(
            "`nope` is not a member the declared-topic relation was read over (not in the workspace)"
        )
    );
    let broken = xservice_declared_topics(&r, Some("broken"));
    assert_eq!(
        broken.scope_note.as_deref(),
        Some(
            "`broken` is not a member the declared-topic relation was read over (declared-topic facts could not be read)"
        )
    );

    let json = serde_json::to_value(&all).unwrap();
    let net_new = &json["topics"][0]["declarations"][0];
    assert_eq!(net_new["provenance"], "config-declared");
    assert_eq!(net_new["status"], "net-new");
    assert_eq!(net_new["site"]["site"], "config-only");
    assert_eq!(net_new["profile"], serde_json::Value::Null);
    assert!(json.get("scope").is_none() && json.get("scope_note").is_none());
}

/// The `--repo` scope filters the refused values too, and the relation's rows
/// are sorted by member — not by roster order — as `derive` documents (review
/// finding, S-489).
#[test]
fn the_scope_filters_refused_values_and_rows_are_sorted_by_member() {
    use crate::federation::query::xservice_declared_topics;

    reset();
    promote("api", &[("orders", 1, 0)]);
    for member in ["zeta", "alpha"] {
        commit(member, MAIN, None, "spring.kafka.topics.orders", "orders");
        commit(member, MAIN, None, "spring.kafka.topics.events", "${MISSING}");
    }
    let r = relation_over(&["api", "zeta", "alpha"]);
    let declared: Vec<&str> = r.declarations.iter().map(|d| d.member.as_str()).collect();
    let refused: Vec<&str> = r.refused.iter().map(|d| d.member.as_str()).collect();
    assert_eq!(declared, ["alpha", "zeta"], "sorted by member, not roster order");
    assert_eq!(refused, ["alpha", "zeta"], "sorted by member, not roster order");

    let zeta = xservice_declared_topics(&r, Some("zeta"));
    assert_eq!(zeta.refused.len(), 1);
    assert_eq!(zeta.refused[0].member, "zeta");
    assert!(xservice_declared_topics(&r, Some("api")).refused.is_empty());
}
