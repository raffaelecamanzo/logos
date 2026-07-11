//! The message-broker publish/subscribe **capture** side of the S-254 arm
//! ([FR-WS-10], [ADR-54]).
//!
//! A per-language `brokers.scm` query captures publish/subscribe call and
//! annotation sites and their topic string literal; this interpreter turns each
//! match into an [`InvocationSite`] and funnels it through the generic,
//! arm-agnostic [`capture_invocation_refs`] emission point under the
//! [`BrokerPublish`](ArtifactRelation::BrokerPublish) /
//! [`BrokerSubscribe`](ArtifactRelation::BrokerSubscribe) relations. Adding a
//! new broker framework/language is therefore *pure data* — one `.scm` file with
//! the `@broker.publish.topic` / `@broker.subscribe.topic` (and optional
//! `@broker.*.schema`) captures — with no new Rust plumbing ([NFR-MA-01]).
//!
//! # Never fabricate a dynamic topic ([NFR-RA-05])
//! A site's topic is normalized by [`broker_topic_key`]: a captured static topic
//! (optionally guarded by a message-schema FQN) yields a stable key; a site with
//! no static topic slot is **refused** (`None`), contributing no reference and no
//! ledger entry. The per-language `.scm` already narrows the capture to
//! `(string_literal)` topics, so a dynamically-composed topic (a constant, a
//! variable, a concatenation) never matches — the refusal is defence-in-depth.
//!
//! [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
//! [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [`capture_invocation_refs`]: crate::extract::config::refs::capture_invocation_refs

use std::collections::BTreeMap;

use tree_sitter::{Node, Query, QueryCursor, StreamingIterator};

use crate::extract::config::refs::{capture_invocation_refs, InvocationSite};
use crate::extract::refs::unquote;
use crate::extract::Facts;
use crate::model::{ArtifactRelation, LogosSymbol, RefForm};

/// Which side of the broker arm a captured site is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Publish,
    Subscribe,
}

/// Run a grammar's `brokers` query over `root` and emit one broker reference per
/// captured static-topic publish/subscribe site (S-254, [FR-WS-10]).
///
/// `enclosing` attributes a captured node to the symbol of its innermost
/// enclosing declaration (the publishing/subscribing function/method), the same
/// resolver the code-reference collector uses. Returns the number of references
/// captured (a language without the `brokers` capability never calls this).
///
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
pub(super) fn capture_broker_invocations<F>(
    query: &Query,
    root: Node<'_>,
    source: &[u8],
    enclosing: F,
    facts: &mut Facts,
) -> usize
where
    F: Fn(Node<'_>) -> Option<LogosSymbol>,
{
    let capture_names = query.capture_names();
    let mut publishes: Vec<InvocationSite> = Vec::new();
    let mut subscribes: Vec<InvocationSite> = Vec::new();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, source);
    while let Some(m) = matches.next() {
        // One match is one publish or subscribe site: find its topic node, its
        // side, and (optionally) its message-schema node. The `@_*` predicate
        // captures are ignored.
        let mut side: Option<Side> = None;
        let mut topic_node: Option<Node<'_>> = None;
        let mut schema_node: Option<Node<'_>> = None;
        for cap in m.captures {
            match capture_names[cap.index as usize] {
                "broker.publish.topic" => {
                    side = Some(Side::Publish);
                    topic_node = Some(cap.node);
                }
                "broker.subscribe.topic" => {
                    side = Some(Side::Subscribe);
                    topic_node = Some(cap.node);
                }
                "broker.publish.schema" | "broker.subscribe.schema" => {
                    schema_node = Some(cap.node);
                }
                _ => {}
            }
        }
        let (Some(side), Some(topic_node)) = (side, topic_node) else {
            continue;
        };
        // A static string-literal topic only — never a fabricated dynamic key.
        let Some(topic) = literal_text(topic_node, source) else {
            continue;
        };
        // The site is attributed to its enclosing publishing/subscribing symbol.
        let Some(source_symbol) = enclosing(topic_node) else {
            continue;
        };

        let mut slots = BTreeMap::new();
        slots.insert("topic".to_string(), topic);
        // The optional message-schema FQN guard (secondary key component). Kept
        // generic: no shipped framework populates it yet, but a `.scm` that
        // captures `@broker.*.schema` needs no code change to key on it.
        if let Some(schema) = schema_node.and_then(|n| literal_text(n, source)) {
            slots.insert("schema".to_string(), schema);
        }

        let site = InvocationSite {
            source: source_symbol,
            slots,
            line: topic_node.start_position().row as u32 + 1,
        };
        match side {
            Side::Publish => publishes.push(site),
            Side::Subscribe => subscribes.push(site),
        }
    }

    // Two emission passes — one per relation — through the shared interpreter,
    // each with the same topic normalizer. `RefForm::Method` marks the target as
    // a bare-name-like key (a topic), not a filesystem path.
    let mut emitted = capture_invocation_refs(
        facts,
        ArtifactRelation::BrokerPublish,
        RefForm::Method,
        publishes,
        broker_topic_key,
    );
    emitted += capture_invocation_refs(
        facts,
        ArtifactRelation::BrokerSubscribe,
        RefForm::Method,
        subscribes,
        broker_topic_key,
    );
    emitted
}

/// Normalize a captured broker site's slots into the portable topic key two
/// members meet on, or `None` to refuse the site (never fabricate).
///
/// The key is the topic name, optionally guarded by a `#`-appended
/// message-schema FQN when the site named a message type: two sides bind iff
/// their whole key is byte-equal, so a **differing** schema FQN keeps the topics
/// apart (honest at the contract grain, [FR-WS-10]). A site with no static topic
/// slot — a dynamically-composed topic the `.scm` refused to capture — yields
/// `None`.
///
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
fn broker_topic_key(slots: &BTreeMap<String, String>) -> Option<String> {
    let topic = slots.get("topic").map(String::as_str).unwrap_or("").trim();
    if topic.is_empty() {
        return None; // no static topic — refuse (never fabricate)
    }
    match slots.get("schema").map(String::as_str).map(str::trim) {
        Some(schema) if !schema.is_empty() => Some(format!("{topic}#{schema}")),
        _ => Some(topic.to_string()),
    }
}

/// The literal text of a captured string-literal node with one surrounding pair
/// of quotes stripped, or `None` when empty. Non-literal captures (which a
/// well-formed `.scm` should not produce for a topic slot) fall through to the
/// node's raw text, still unquoted defensively.
fn literal_text(node: Node<'_>, source: &[u8]) -> Option<String> {
    let raw = node.utf8_text(source).ok()?;
    let value = unquote(raw).trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slots(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// The normalizer keys a bare topic by its name, appends a message-schema FQN
    /// guard when present, and refuses a site with no static topic (the dynamic
    /// case) — the never-fabricate gate ([FR-WS-10], [NFR-RA-05]).
    #[test]
    fn broker_topic_key_keys_topic_optionally_guards_by_schema_and_refuses_dynamic() {
        assert_eq!(
            broker_topic_key(&slots(&[("topic", "orders")])),
            Some("orders".to_string())
        );
        assert_eq!(
            broker_topic_key(&slots(&[("topic", "orders"), ("schema", "com.acme.OrderCreated")])),
            Some("orders#com.acme.OrderCreated".to_string()),
            "a message-schema FQN guards the topic key"
        );
        // A differing schema yields a different key — the two never meet.
        assert_ne!(
            broker_topic_key(&slots(&[("topic", "orders"), ("schema", "A")])),
            broker_topic_key(&slots(&[("topic", "orders"), ("schema", "B")])),
        );
        // No topic slot (a dynamically-composed topic the capture refused) → None.
        assert_eq!(broker_topic_key(&slots(&[])), None);
        assert_eq!(broker_topic_key(&slots(&[("topic", "  ")])), None);
        // An empty schema is ignored — the bare topic still keys.
        assert_eq!(
            broker_topic_key(&slots(&[("topic", "orders"), ("schema", "")])),
            Some("orders".to_string())
        );
    }
}

// The end-to-end `.scm` capture over real Java source runs only when the Java
// grammar is compiled in (the default feature set); the interpreter logic above
// compiles unconditionally.
#[cfg(all(test, feature = "lang-java"))]
mod java_capture_tests {
    use super::*;
    use crate::extract::{extract, FileInput, SymbolContext};
    use crate::plugin::LanguageRegistry;

    fn extract_java(src: &str) -> Facts {
        let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
        let plugin = registry.for_extension("java").expect("java plugin present");
        extract(&FileInput::new("Svc.java", src), plugin, &SymbolContext::default())
    }

    fn targets(facts: &Facts, relation: ArtifactRelation) -> Vec<String> {
        let mut t: Vec<String> = facts
            .refs
            .iter()
            .filter(|r| r.relation == Some(relation))
            .map(|r| r.target.clone())
            .collect();
        t.sort();
        t
    }

    /// A Spring `@KafkaListener(topics = "orders")` subscribe and a
    /// `kafkaTemplate.send("orders", …)` publish are each captured as a
    /// topic-keyed broker reference, under the correct relation, attributed to
    /// their enclosing method. A `send(TOPIC, …)` with a **dynamic** (non-literal)
    /// topic captures nothing — honestly unbound, never guessed ([FR-WS-10],
    /// [NFR-RA-05]).
    #[test]
    fn kafka_listener_and_template_send_capture_static_topics_only() {
        let src = r#"
package com.acme;
class OrderService {
    private KafkaTemplate<String, String> kafkaTemplate;
    private static final String TOPIC = "shipments";

    @KafkaListener(topics = "orders")
    public void onOrder(String msg) {}

    public void publish(String payload) {
        kafkaTemplate.send("orders", payload);
    }

    public void publishDynamic(String payload) {
        kafkaTemplate.send(TOPIC, payload);
    }
}
"#;
        let facts = extract_java(src);

        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["orders".to_string()],
            "the @KafkaListener topic is captured as a subscribe ref: {:?}",
            facts.refs
        );
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerPublish),
            vec!["orders".to_string()],
            "only the static-topic send is captured; send(TOPIC, …) is not: {:?}",
            facts.refs
        );

        // Both refs are ledger-only artifact→artifact facts under the arm tokens.
        for r in facts
            .refs
            .iter()
            .filter(|r| matches!(
                r.relation,
                Some(ArtifactRelation::BrokerPublish) | Some(ArtifactRelation::BrokerSubscribe)
            ))
        {
            assert_eq!(r.kind, crate::model::EdgeKind::ArtifactRef);
            assert_eq!(r.form, RefForm::Method);
        }

        // The subscribe ref is attributed to its listener method, the publish ref
        // to the publishing method — not the file module.
        let sub = facts
            .refs
            .iter()
            .find(|r| r.relation == Some(ArtifactRelation::BrokerSubscribe))
            .unwrap();
        assert!(
            sub.source.as_str().contains("onOrder"),
            "the subscribe is sourced from its @KafkaListener method: {}",
            sub.source.as_str()
        );
        let publish = facts
            .refs
            .iter()
            .find(|r| r.relation == Some(ArtifactRelation::BrokerPublish))
            .unwrap();
        assert!(
            publish.source.as_str().contains("publish"),
            "the publish is sourced from its sending method: {}",
            publish.source.as_str()
        );
    }

    /// The single-value annotation form `@KafkaListener("orders")` is captured
    /// too — the second subscribe pattern in `brokers.scm`.
    #[test]
    fn kafka_listener_single_value_form_is_captured() {
        let src = r#"
package com.acme;
class C {
    @KafkaListener("events")
    public void handle(String m) {}
}
"#;
        let facts = extract_java(src);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["events".to_string()],
            "the single-value @KafkaListener(\"events\") form binds: {:?}",
            facts.refs
        );
    }
}
