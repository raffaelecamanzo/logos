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
//! no static topic slot is **refused** (`None`), contributing no reference. The
//! per-language `.scm` narrows the binding capture to `(string_literal)` topics, so
//! a dynamically-composed topic (a constant, a variable, a concatenation) never
//! binds — the normalizer is defence-in-depth behind that.
//!
//! What a literal *contains* is not part of that boundary. A Spring property
//! placeholder — `@KafkaListener(topics = "${spring.kafka.topics.orders}")` — is a
//! static string literal like any other and binds, keyed by the placeholder text as
//! written; resolving it against committed configuration is [CR-117] §3.2's
//! canonical-identity rule, downstream of this capture ([CR-107], [FR-WS-10] AC3).
//!
//! # A refusal is recorded, not silent ([FR-WS-05], [NFR-CC-04])
//! A refused topic used to leave *nothing* — no reference, no ledger row, no
//! coverage entry — so a Spring estate whose topics are all externalised was
//! indistinguishable from one with no broker wiring at all. A `.scm` that captures
//! the topic **operand** (`@broker.*.topic.slot`) alongside the **site** it belongs
//! to (`@broker.*.site`) now leaves one **keyless** broker-arm ledger row per
//! refused site: it fabricates no topic (the target is empty, which
//! [`crate::resolve::topics`] already refuses to promote — "a keyless row is not a
//! topic"), and the [FR-WS-05] coverage tier reports it as `topic-not-literal`.
//!
//! The grain is the **site** the `.scm` declares, not the annotation: the array form
//! reports nothing because its operand matches no slot pattern, and a
//! multi-attribute annotation reports nothing for the sibling attributes its key
//! predicate excludes — but a *second* topic-keyed attribute whose own operand is
//! not a literal is its own site and does report, alongside the first attribute's
//! bound topic.
//!
//! **Both roles of the arm use this vocabulary** (S-370, [CR-117]). [CR-107] reached
//! it from the subscribe side alone; the publish side now records refusals too, from
//! its *header* form — a `MessageBuilder…setHeader(KafkaHeaders.TOPIC, …)` site,
//! which is where idiomatic Spring actually puts a topic and which the arm did not
//! recognise at all while it looked only in argument position. The interpreter below
//! needed no change for it: `@broker.publish.topic.slot` / `@broker.publish.site`
//! were already interpreted, and `unkeyable_reason` already maps the publish
//! relation to `topic-not-literal`, so recognising the site was purely a `.scm`
//! edit. What that buys on a real estate is refusals, not edges — measured, **all
//! 54** header-form sites of the 84-member reference workspace carry a non-literal
//! operand, so the arm's honest output there is 54 recorded refusals and zero
//! producers ([CR-117] §2).
//!
//! [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
//! [CR-117]: ../../../docs/requests/CR-117-broker-publish-capture-and-the-topic-key-namespace.md
//! [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//!
//! [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
//! [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [`capture_invocation_refs`]: crate::extract::config::refs::capture_invocation_refs

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

use tree_sitter::{Node, Query, QueryCursor, StreamingIterator};

use crate::extract::config::refs::{capture_invocation_refs, push_artifact_ref, InvocationSite};
use crate::extract::refs::unquote;
use crate::extract::Facts;
use crate::model::{ArtifactRelation, LogosSymbol, RefForm};

/// Which side of the broker arm a captured site is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    // The byte range of every topic literal that bound, and every refusal
    // candidate with the site range it belongs to. Both are collected across the
    // whole file and reconciled *after* the loop, so the outcome cannot depend on
    // the order tree-sitter reports competing patterns in.
    //
    // No SHIPPED `.scm` produces a cancellable pair: the Java slot patterns
    // enumerate only non-literal operand shapes, so the array
    // (`topics = {"a","b"}`) and multi-attribute forms yield no candidate at all —
    // that is the query's enumeration and its `@_sub_slot_key` predicate doing the
    // work, not this reconcile. The reconcile guards the case a **droppable** query
    // can still create ([FR-PL-04]): a slot pointed at an operand that is, or
    // contains, a literal another pattern admitted. Covered by
    // `a_site_that_bound_a_literal_records_no_refusal_even_when_a_slot_matched_it`,
    // which supplies such a query, because nothing in the shipped set reaches it.
    let mut bound: Vec<(Side, Range<usize>)> = Vec::new();
    let mut candidates: Vec<RefusalCandidate<'_>> = Vec::new();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, source);
    while let Some(m) = matches.next() {
        // One match is one publish or subscribe site: find its topic node, its
        // side, and (optionally) its message-schema node and its refusal
        // slot/site pair. The `@_*` predicate captures are ignored.
        let mut side: Option<Side> = None;
        let mut topic_node: Option<Node<'_>> = None;
        let mut schema_node: Option<Node<'_>> = None;
        let mut slot: Option<(Side, Node<'_>)> = None;
        let mut site_node: Option<Node<'_>> = None;
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
                // The refusal vocabulary: the topic OPERAND whatever its shape,
                // plus the site it is deduped by. Both are required — a slot with
                // no site names nothing to dedup on and is ignored, so a `.scm`
                // cannot half-adopt the vocabulary.
                "broker.publish.topic.slot" => slot = Some((Side::Publish, cap.node)),
                "broker.subscribe.topic.slot" => slot = Some((Side::Subscribe, cap.node)),
                "broker.publish.site" | "broker.subscribe.site" => site_node = Some(cap.node),
                _ => {}
            }
        }

        if let (Some((slot_side, slot_node)), Some(site)) = (slot, site_node) {
            candidates.push(RefusalCandidate {
                side: slot_side,
                site: site.byte_range(),
                node: slot_node,
            });
        }

        let (Some(side), Some(topic_node)) = (side, topic_node) else {
            continue;
        };
        // A static string-literal topic only — never a fabricated dynamic key.
        let Some(topic) = literal_text(topic_node, source) else {
            continue;
        };
        // Recorded before the enclosing-symbol lookup: a literal whose enclosing
        // declaration is unknown still binds *nothing*, but it did parse as a
        // literal, so reporting its site `topic-not-literal` would be a wrong
        // reason ([NFR-CC-04]).
        bound.push((side, topic_node.byte_range()));
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
    emitted += record_refusals(candidates, &bound, &enclosing, facts);
    emitted
}

/// One site whose topic operand was captured but may not be a literal — a
/// `topic-not-literal` refusal *candidate*, pending the reconcile against what
/// actually bound.
struct RefusalCandidate<'t> {
    side: Side,
    /// The byte range of the `@broker.*.site` node: the grain a refusal is deduped
    /// by, and the range an admitted literal must fall inside to cancel it.
    site: Range<usize>,
    /// The `@broker.*.topic.slot` node — the operand itself, which supplies the
    /// enclosing declaration and the reported line.
    node: Node<'t>,
}

/// Record one **keyless** broker-arm ledger row per refused site, and return how
/// many were recorded ([FR-WS-05], [NFR-CC-04], [CR-107]).
///
/// A candidate survives iff **no** admitted topic literal of the same side lies
/// within its site range. With the shipped Java query that condition is never
/// false, because its slot patterns enumerate non-literal operand shapes only — so
/// the array form (`topics = {"a","b"}`) and the multi-attribute form yield no
/// candidate to cancel in the first place, and it is the query's enumeration, not
/// this reconcile, that keeps them quiet. The reconcile is what stops a
/// **droppable** query ([FR-PL-04]) from reporting a refusal at a site that bound.
///
/// A genuinely non-literal operand (`topics = TOPIC`, `Topics.ORDERS`,
/// `PREFIX + "orders"`, `config.topic()`) reports once.
///
/// Survivors are then deduped to one row per `(side, enclosing declaration, line)`
/// — the "at most once per registration site" discipline
/// [`crate::resolve::framework`] applies to `path-not-composed`. The dedup is
/// deliberately coarser than the raw site, because a `.scm` may legitimately
/// capture nested sites for one operand (an attribute pair *and* its argument
/// list). Note this is not the only thing collapsing rows: the production caller
/// re-runs `dedup_sort_refs`, which keys on `(source, target, form, kind, relation)`
/// and ignores `line`, so two refused sites in one declaration reach the ledger as
/// one row even on different lines. This dedup keeps the grain local and
/// independent of that key — it is asserted directly, through the interpreter, by
/// `two_refused_topic_attributes_on_one_site_record_one_refusal`.
///
/// The row's target is **empty**: no topic key is fabricated, not even the
/// operand's source text. A keyless broker row is inert by contracts that already
/// exist — [`crate::resolve::topics`] refuses to promote one ("a keyless row is not
/// a topic"), and the bridge's own key builders refuse it — so a refusal can never
/// become a `Topic` node or a cross-service edge ([NFR-RA-05]).
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
fn record_refusals<F>(
    candidates: Vec<RefusalCandidate<'_>>,
    bound: &[(Side, Range<usize>)],
    enclosing: &F,
    facts: &mut Facts,
) -> usize
where
    F: Fn(Node<'_>) -> Option<LogosSymbol>,
{
    let mut seen: HashSet<(Side, String, u32)> = HashSet::new();
    let mut recorded = 0;
    for candidate in candidates {
        // Did anything at this site bind? Then it is not a refusal.
        if bound.iter().any(|(side, at)| {
            *side == candidate.side
                && at.start >= candidate.site.start
                && at.end <= candidate.site.end
        }) {
            continue;
        }
        let Some(symbol) = enclosing(candidate.node) else {
            continue; // no enclosing declaration to attribute the refusal to
        };
        let line = candidate.node.start_position().row as u32 + 1;
        if !seen.insert((candidate.side, symbol.as_str().to_string(), line)) {
            continue; // this site already recorded its one refusal
        }
        let relation = match candidate.side {
            Side::Publish => ArtifactRelation::BrokerPublish,
            Side::Subscribe => ArtifactRelation::BrokerSubscribe,
        };
        if push_artifact_ref(facts, &symbol, "", relation, RefForm::Method, line) {
            recorded += 1;
        }
    }
    recorded
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
/// **Separator invariant (unreachable today):** the `#` join is injective only
/// while topic names contain no `#`. No shipped `brokers.scm` populates a
/// `schema` slot yet, so the guarded form never arises in practice; the first
/// arm to capture `@broker.*.schema` (notably for a broker like RabbitMQ whose
/// routing keys admit `#`) must pick a separator that cannot occur in a topic
/// name, or escape it, to keep the key injective and avoid a false bind
/// ([NFR-RA-05]).
///
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
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

// The end-to-end `.scm` capture over real Rust source runs only when the Rust
// grammar is compiled in (S-291): Rust is the language that declares BOTH the
// `brokers` capability and `reachability`, so its capture is what makes the
// app-wide reachability promotion path reachable on a real index ([CR-081],
// [FR-WS-12] AC1).
#[cfg(all(test, feature = "lang-rust"))]
mod rust_capture_tests {
    use super::*;
    use crate::extract::{extract, FileInput, SymbolContext};
    use crate::plugin::LanguageRegistry;

    fn extract_rust(src: &str) -> Facts {
        let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
        let plugin = registry.for_extension("rs").expect("rust plugin present");
        extract(&FileInput::new("svc.rs", src), plugin, &SymbolContext::default())
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

    /// A `bus.subscribe("orders")` consumer and a `bus.publish("orders", …)`
    /// producer are each captured as a topic-keyed broker reference, under the
    /// correct relation, attributed to their enclosing function. A dynamic
    /// (non-literal) topic captures nothing — honestly unbound, never guessed
    /// ([FR-WS-10], [NFR-RA-05]).
    #[test]
    fn bus_publish_and_subscribe_capture_static_topics_only() {
        let src = r#"
const TOPIC: &str = "never-captured";

fn on_order(bus: &Bus) {
    bus.subscribe("orders");
}

fn emit(bus: &Bus, payload: &str) {
    bus.publish("orders", payload);
}

// The `send` producer verb, on a distinct topic — exercises the second half of
// the `#any-of? "publish" "send"` predicate and a second static publish topic.
fn ship(bus: &Bus, payload: &str) {
    bus.send("shipments", payload);
}

fn emit_dynamic(bus: &Bus, payload: &str) {
    bus.publish(TOPIC, payload);
}
"#;
        let facts = extract_rust(src);

        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["orders".to_string()],
            "the bus.subscribe topic is captured as a subscribe ref: {:?}",
            facts.refs
        );
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerPublish),
            vec!["orders".to_string(), "shipments".to_string()],
            "both the `publish` and the `send` static-topic producers are captured; \
             publish(TOPIC, …) is not: {:?}",
            facts.refs
        );

        // Both refs are ledger-only artifact→artifact facts under the arm tokens.
        for r in facts.refs.iter().filter(|r| {
            matches!(
                r.relation,
                Some(ArtifactRelation::BrokerPublish) | Some(ArtifactRelation::BrokerSubscribe)
            )
        }) {
            assert_eq!(r.kind, crate::model::EdgeKind::ArtifactRef);
            assert_eq!(r.form, RefForm::Method);
        }

        // The subscribe ref is attributed to its handler fn, the publish ref to
        // the publishing fn — not the file module.
        let sub = facts
            .refs
            .iter()
            .find(|r| r.relation == Some(ArtifactRelation::BrokerSubscribe))
            .unwrap();
        assert!(
            sub.source.as_str().contains("on_order"),
            "the subscribe is sourced from its handler fn: {}",
            sub.source.as_str()
        );
        // Each producer ref is attributed to its own enclosing fn — the `publish`
        // site to `emit`, the `send` site to `ship`.
        for (topic, enclosing) in [("orders", "emit"), ("shipments", "ship")] {
            let publish = facts
                .refs
                .iter()
                .find(|r| {
                    r.relation == Some(ArtifactRelation::BrokerPublish) && r.target == topic
                })
                .unwrap_or_else(|| panic!("a publish ref for `{topic}` exists: {:?}", facts.refs));
            assert!(
                publish.source.as_str().contains(enclosing),
                "the `{topic}` publish is sourced from its publishing fn `{enclosing}`: {}",
                publish.source.as_str()
            );
        }
    }

    /// **[CR-107]'s Rust audit, recorded as a test.** The audit finding is that
    /// `plugins/rust/queries/brokers.scm` carried **no** character sensitivity of its
    /// own — the `$`/`{` rule lived in `ArtifactRelation::classify_target`, which is
    /// language-agnostic, so the Rust arm dropped a `$`- or `{`-bearing topic for the
    /// *same* reason the Java arm did and is repaired by the *same* change. No Rust
    /// `.scm` edit was needed for the character class.
    ///
    /// This test is the finding's evidence, and the guard that the fix is genuinely
    /// in the shared layer rather than tuned per language.
    ///
    /// Two further audit findings, recorded here because they are decisions not to
    /// change the file:
    /// - **the array form was already handled** — the rdkafka slice pattern
    ///   (`subscribe(&["a", "b"])`) predates this CR and is asserted by
    ///   [`rdkafka_slice_subscribe_captures_every_topic`];
    /// - **no refusal slot was added.** The Rust arm keys on bare method verbs with
    ///   no receiver typing (its own header comment records the resulting
    ///   false-positive exposure), so a `.slot` pattern would record a
    ///   `topic-not-literal` refusal for every `channel.send(x)` and
    ///   `.subscribe(handler)` in an arbitrary codebase — manufacturing a coverage
    ///   denominator out of ordinary code, which is the failure mode
    ///   `resolve::framework::drop_non_path_routes` documents on the route side.
    ///   Refusals are recorded where a site is *identifiable* as a broker site;
    ///   for Rust that needs receiver scoping first.
    #[test]
    fn a_rust_topic_literal_binds_whatever_characters_it_carries() {
        let src = r#"
fn consume(bus: &Bus) {
    bus.subscribe("${env}-orders");
}

fn consume_braced(bus: &Bus) {
    bus.subscribe("braces{only}");
}

fn emit(bus: &Bus, payload: &str) {
    bus.publish("dollar$only", payload);
}

fn consume_slice(consumer: &Consumer) {
    consumer.subscribe(&["${a}", "b{c}"]);
}
"#;
        let facts = extract_rust(src);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec![
                "${a}".to_string(),
                "${env}-orders".to_string(),
                "braces{only}".to_string(),
                "b{c}".to_string(),
            ],
            "scalar and slice Rust subscribe topics bind whatever they contain: {:?}",
            facts.refs
        );
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerPublish),
            vec!["dollar$only".to_string()],
            "the Rust publish side too: {:?}",
            facts.refs
        );
        // And the audit's third finding: no Rust `.scm` slot pattern, so no refusal
        // row is manufactured for a non-literal operand here.
        let dynamic = extract_rust("fn emit(bus: &Bus) { bus.publish(TOPIC, 1); }");
        assert!(
            dynamic.refs.iter().all(|r| r.relation.is_none()),
            "the Rust arm records no refusal — see this test's doc comment: {:?}",
            dynamic.refs
        );
    }

    /// **S-370 / [CR-117]'s Rust publish-side audit, recorded as a test.**
    ///
    /// S-370 requires `plugins/rust/queries/brokers.scm` to be audited for the same
    /// publish-side blind spot the Java arm had — a topic that never reaches the
    /// `send` call — and the finding recorded **either way**, including "no
    /// equivalent form exists in the Rust corpus" if that is the answer. It is that
    /// answer, and this test is the finding's evidence: the file is unchanged and
    /// its behaviour is pinned so the gap is a stated decision rather than an
    /// omission.
    ///
    /// Measured 2026-09-07: the 84-member reference workspace holds **0** `.rs`
    /// files, and this repository declares no broker-client dependency and carries
    /// no real producer site — so there is no Rust broker source for the arm to be
    /// blind to. The nearest structural analogue, rdkafka's `FutureRecord::to(…)`
    /// record builder, is asserted below to capture nothing today and to
    /// manufacture no refusal either. That is Java's blind spot in a different
    /// shape — a builder method, not a header constant — and closing it needs its
    /// own reasoning and a corpus to measure a `to("…")` pattern's false-positive
    /// rate against, which `brokers.scm`'s own scope note explains this repository
    /// cannot supply.
    ///
    /// [CR-117]: ../../../docs/requests/CR-117-broker-publish-capture-and-the-topic-key-namespace.md
    #[test]
    fn the_rust_publish_side_has_no_header_form_equivalent_and_is_unchanged() {
        // The argument-position form the Rust arm DOES recognise still does — the
        // baseline this audit leaves untouched.
        let captured = extract_rust(r#"
fn emit(bus: &Bus, payload: &str) {
    bus.publish("orders", payload);
}
"#);
        assert_eq!(
            targets(&captured, ArtifactRelation::BrokerPublish),
            vec!["orders".to_string()],
            "the audit changes nothing about what the Rust arm already captures: {:?}",
            captured.refs
        );

        // The analogue of Java's blind spot: the topic is on the record builder and
        // `send` receives the assembled record. Captured: nothing. Refused: nothing
        // recorded either — the Rust arm records no refusals at all, by the decision
        // `brokers.scm` documents (bare method verbs with no receiver typing would
        // manufacture a coverage denominator out of ordinary code).
        let builder_form = extract_rust(r#"
async fn emit(producer: &FutureProducer, payload: &str) {
    let record = FutureRecord::to("orders").payload(payload).key("k");
    producer.send(record, Duration::from_secs(0)).await.unwrap();
}
"#);
        assert!(
            builder_form
                .refs
                .iter()
                .all(|r| r.relation != Some(ArtifactRelation::BrokerPublish)),
            "the rdkafka builder form is NOT recognised — the recorded gap, pinned so \
             a future story finds a decision rather than an omission: {:?}",
            builder_form.refs
        );
        assert!(
            builder_form.refs.iter().all(|r| r.relation.is_none()),
            "and no refusal is manufactured for it either: {:?}",
            builder_form.refs
        );
    }

    /// The rdkafka slice form `consumer.subscribe(&["a", "b"])` captures each
    /// topic in the borrowed array, attributed to the enclosing handler.
    #[test]
    fn rdkafka_slice_subscribe_captures_every_topic() {
        let src = r#"
fn consume(consumer: &Consumer) {
    consumer.subscribe(&["orders", "shipments"]);
}
"#;
        let facts = extract_rust(src);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["orders".to_string(), "shipments".to_string()],
            "each topic in the subscribe slice binds: {:?}",
            facts.refs
        );
        // The slice form routes the topic node through a different tree shape
        // (reference_expression → array_expression) than the scalar form, so its
        // enclosing-symbol attribution is asserted explicitly: every slice topic is
        // sourced from the handler `consume`, not the file module.
        for r in facts
            .refs
            .iter()
            .filter(|r| r.relation == Some(ArtifactRelation::BrokerSubscribe))
        {
            assert!(
                r.source.as_str().contains("consume"),
                "the slice subscribe is sourced from its handler fn: {}",
                r.source.as_str()
            );
        }
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

    /// Parse `src` and run `capture_broker_invocations` with a **custom** query,
    /// bypassing the shipped `brokers.scm`.
    ///
    /// The interpreter is called directly, which is what makes the two guards
    /// below observable at all: the production path (`extract::capture_broker_invocation_arm`)
    /// re-runs `dedup_sort_refs` afterwards, and that collapses rows on
    /// `(source, target, form, kind, relation)` — ignoring `line` — so it masks
    /// anything either guard does. Every site is attributed to one fixed symbol,
    /// since attribution is not what these tests are about.
    fn capture_with(query_src: &str, src: &str) -> Facts {
        let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
        let plugin = registry.for_extension("java").expect("java plugin present");
        let language = plugin.language();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(language).expect("java language loads");
        let tree = parser.parse(src, None).expect("java parses");
        let query = Query::new(language, query_src).expect("the test query compiles");
        let symbol = LogosSymbol::parse("local handler").expect("symbol parses");
        let mut facts = Facts {
            path: "Svc.java".to_string(),
            language: "java".to_string(),
            partial: false,
            nodes: Vec::new(),
            edges: Vec::new(),
            refs: Vec::new(),
            warnings: Vec::new(),
        };
        capture_broker_invocations(
            &query,
            tree.root_node(),
            src.as_bytes(),
            |_| Some(symbol.clone()),
            &mut facts,
        );
        facts
    }

    /// **The reconcile, covered.** A `.scm` that points a refusal slot at an
    /// operand another pattern admitted as a topic must record **no** refusal — the
    /// site bound, so `topic-not-literal` would be a wrong reason ([NFR-CC-04]).
    ///
    /// No **shipped** query can reach this: the Java slot patterns enumerate only
    /// non-literal operand shapes, so a literal never produces a candidate. But a
    /// `brokers.scm` is droppable on disk ([FR-PL-04]), so a query outside this
    /// repository can create exactly the pair this guard exists for — and the
    /// `value: (_)` wildcard that this story reverted is precisely that shape. This
    /// test uses that wildcard deliberately, which is the only way to exercise the
    /// containment check; mutation testing showed it otherwise unreachable across
    /// the whole suite and all 2,447 Java files of the reference corpus.
    ///
    /// [FR-PL-04]: ../../../docs/specs/requirements/FR-PL-04.md
    #[test]
    fn a_site_that_bound_a_literal_records_no_refusal_even_when_a_slot_matched_it() {
        // A container slot: the topic pattern admits each literal INSIDE the array,
        // while the slot points at the array node itself. The candidate's operand is
        // therefore not a literal — so no node-kind test can save it — and only the
        // range containment tells us the site bound. This is the shape a droppable
        // query most plausibly takes, and the one that isolates the check.
        let query = r#"
(element_value_pair
  key: (identifier) @_k
  value: (element_value_array_initializer
    (string_literal) @broker.subscribe.topic))

(element_value_pair
  key: (identifier) @_k2
  value: (element_value_array_initializer) @broker.subscribe.topic.slot) @broker.subscribe.site
"#;
        let facts = capture_with(query, r#"
class C {
    @KafkaListener(topics = {"orders", "shipments"})
    void a(String m) {}
}
"#);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["orders".to_string(), "shipments".to_string()],
            "both literals bind and their site records no refusal: {:?}",
            facts.refs
        );

        // The complement, through the same query shape: an array that admits NO
        // literal still refuses, so the check is discriminating rather than simply
        // suppressing every candidate that has a site.
        let refused = capture_with(query, r#"
class C {
    @KafkaListener(topics = {FIRST, SECOND})
    void a(String m) {}
}
"#);
        assert_eq!(
            targets(&refused, ArtifactRelation::BrokerSubscribe),
            vec![String::new()],
            "an array with no literal in it records its refusal: {:?}",
            refused.refs
        );

        // And the degenerate form — a slot pointed at the admitted literal itself
        // (the `value: (_)` wildcard this story reverted) — is cancelled by the same
        // containment test, since equal ranges are contained.
        let wildcard = r#"
(element_value_pair
  key: (identifier) @_k
  value: (string_literal) @broker.subscribe.topic)

(element_value_pair
  key: (identifier) @_k2
  value: (_) @broker.subscribe.topic.slot) @broker.subscribe.site
"#;
        let scalar = capture_with(wildcard, r#"
class C {
    @KafkaListener(topics = "orders")
    void a(String m) {}
}
"#);
        assert_eq!(
            targets(&scalar, ArtifactRelation::BrokerSubscribe),
            vec!["orders".to_string()],
            "a slot pointed at the admitted literal itself records no refusal: {:?}",
            scalar.refs
        );
    }

    /// **The per-site dedup, covered.** Two refused topic attributes on one
    /// annotation are one refused *site* and record **one** row — the "at most once
    /// per site" grain, matching `resolve::framework`'s `path-not-composed`
    /// discipline.
    ///
    /// Asserted through the interpreter directly because `dedup_sort_refs` collapses
    /// keyless rows on `(source, target, …)` regardless, so on the production path
    /// this guarantee is unobservable and the dedup untestable.
    #[test]
    fn two_refused_topic_attributes_on_one_site_record_one_refusal() {
        let query = r#"
(element_value_pair
  key: (identifier) @_k
  value: (identifier) @broker.subscribe.topic.slot) @broker.subscribe.site
"#;
        // Two topic-keyed attributes, both non-literal, on the same line.
        let facts = capture_with(query, r#"
class C {
    @KafkaListener(topics = TOPIC, queues = OTHER)
    void a(String m) {}
}
"#);
        let keyless = facts
            .refs
            .iter()
            .filter(|r| {
                r.relation == Some(ArtifactRelation::BrokerSubscribe) && r.target.is_empty()
            })
            .count();
        assert_eq!(
            keyless, 1,
            "one refused site is one row, before any ledger dedup: {:?}",
            facts.refs
        );
    }

    /// **[CR-107] acceptance: the literal-shape table.** One test, every shape a
    /// real Spring listener writes, asserted as a table so the next character class
    /// cannot regress silently — plain, dotted, dashed, a property placeholder
    /// `${x}`, a brace-bearing literal, a `$`-bearing literal, and the multi-topic
    /// array form. Every one of them is a **static string literal**, so every one
    /// binds, keyed by the literal's own text.
    ///
    /// Before the fix, `dotted`/`dashed`/`plain` bound and the last four did not:
    /// the drop was `ArtifactRelation::classify_target`'s `$`/`{` character rule,
    /// not the grammar (the parse tree for `"${x}"` is shape-identical to
    /// `"orders"` — `string_literal` → `string_fragment` — under the pinned
    /// `tree-sitter-java`). A single `${…}` case would have left the class untested,
    /// which is exactly why this is a table.
    ///
    /// The negative half — a genuinely non-literal topic refused **and** recorded —
    /// is [`a_non_literal_topic_is_refused_and_recorded_once_per_site`].
    ///
    /// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
    #[test]
    fn every_static_topic_literal_shape_is_captured_whatever_characters_it_carries() {
        // (source fragment, the topic keys it must yield) — the shape table.
        let cases: &[(&str, &[&str])] = &[
            (r#"@KafkaListener(topics = "orders")"#, &["orders"]),
            (
                r#"@KafkaListener(topics = "dotted.topic.name")"#,
                &["dotted.topic.name"],
            ),
            (r#"@KafkaListener(topics = "has-dash")"#, &["has-dash"]),
            (
                r#"@KafkaListener(topics = "${spring.kafka.topics.orders}")"#,
                &["${spring.kafka.topics.orders}"],
            ),
            (r#"@KafkaListener(topics = "braces{only}")"#, &["braces{only}"]),
            (r#"@KafkaListener(topics = "dollar$only")"#, &["dollar$only"]),
            // The array form: one declaration, one topic per literal.
            (
                r#"@KafkaListener(topics = {"orders", "shipments"})"#,
                &["orders", "shipments"],
            ),
            // The single-value array form real listeners also write.
            (
                r#"@KafkaListener({"alpha", "beta"})"#,
                &["alpha", "beta"],
            ),
            // Sibling attributes do not disturb the capture — real listeners carry
            // a container factory, a group id, an ack mode.
            (
                r#"@KafkaListener(topics = "${orders.topic}", containerFactory = KafkaConfig.FACTORY, groupId = "g")"#,
                &["${orders.topic}"],
            ),
            // The other two listener annotations and their own attribute keys.
            (r#"@RabbitListener(queues = "${q.name}")"#, &["${q.name}"]),
            (
                r#"@JmsListener(destination = "dest{0}")"#,
                &["dest{0}"],
            ),
        ];

        for (annotation, want) in cases {
            let src = format!(
                "package com.acme;\nclass C {{\n    {annotation}\n    public void handle(String m) {{}}\n}}\n"
            );
            let facts = extract_java(&src);
            let mut expected: Vec<String> = want.iter().map(|t| (*t).to_string()).collect();
            expected.sort();
            assert_eq!(
                targets(&facts, ArtifactRelation::BrokerSubscribe),
                expected,
                "`{annotation}` must bind {want:?}: {:?}",
                facts.refs
            );
            // Nothing is refused at a site that bound: no keyless companion row.
            assert!(
                !facts.refs.iter().any(|r| {
                    r.relation == Some(ArtifactRelation::BrokerSubscribe) && r.target.is_empty()
                }),
                "`{annotation}` bound, so it records no refusal: {:?}",
                facts.refs
            );
        }
    }

    /// **The interaction guard, and the regression that motivated it.** Every shape
    /// in ONE file, extracted in ONE pass — because a per-shape table extracts each
    /// annotation on its own and therefore cannot see the patterns *compete*.
    ///
    /// It caught a real defect during this story. The refusal slot was first written
    /// as `value: (_)`, which overlaps `value: (string_literal)` on the same node;
    /// tree-sitter then reported only one of the competing patterns per site, and the
    /// measured effect was that the **scalar** literal patterns lost their matches
    /// while the array pattern kept its own. Every shape still passed in isolation,
    /// so the shape table was green and `topics = "${x}"` had silently stopped
    /// binding — precisely the "passes the fixtures and misses the class" failure
    /// [CR-107] §7 names. The `.scm` now enumerates the non-literal operand shapes
    /// instead of wildcarding them, and this test is what holds that.
    ///
    /// So the assertion is deliberately about co-existence: four bound topics from
    /// three literal-bearing declarations, plus one refusal from the declaration that
    /// bound nothing, all from a single extraction.
    #[test]
    fn every_shape_in_one_file_binds_without_the_patterns_shadowing_each_other() {
        let src = r#"
package com.acme;
class Mixed {
    @KafkaListener(topics = "${spring.kafka.topics.orders}")
    public void onOrder(String m) {}

    @KafkaListener(topics = {"plain-one", "plain-two"})
    public void onBoth(String m) {}

    @KafkaListener(topics = "${archive.topic}", containerFactory = KafkaConfig.FACTORY)
    public void onArchive(String m) {}

    @KafkaListener(topics = TOPIC)
    public void onDynamic(String m) {}
}
"#;
        let facts = extract_java(src);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec![
                // The refused site's keyless row sorts first.
                String::new(),
                "${archive.topic}".to_string(),
                "${spring.kafka.topics.orders}".to_string(),
                "plain-one".to_string(),
                "plain-two".to_string(),
            ],
            "scalar, array, multi-attribute and refused sites all co-exist in one \
             extraction: {:?}",
            facts.refs
        );
        // The refusal belongs to the one declaration that bound nothing — a shape
        // that binds must never also report a refusal, and vice versa.
        let refused: Vec<&str> = facts
            .refs
            .iter()
            .filter(|r| {
                r.relation == Some(ArtifactRelation::BrokerSubscribe) && r.target.is_empty()
            })
            .map(|r| r.source.as_str())
            .collect();
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(refused[0].contains("onDynamic"), "{refused:?}");
    }

    /// **[CR-107] acceptance: the array form's cardinality.**
    /// `topics = {"orders", "shipments"}` is **one** declaration on **two** topics,
    /// which the `(declaration, topic)` counting contract already covers ([FR-WS-11]) —
    /// so it yields two subscribe references from one listener method, both
    /// attributed to that method, and no publish reference at all.
    #[test]
    fn an_array_topic_form_yields_one_subscribe_per_literal_from_one_declaration() {
        let src = r#"
package com.acme;
class C {
    @KafkaListener(topics = {"orders", "shipments"})
    public void handle(String m) {}
}
"#;
        let facts = extract_java(src);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["orders".to_string(), "shipments".to_string()],
            "one declaration on two topics is two (declaration, topic) pairs: {:?}",
            facts.refs
        );
        assert!(
            targets(&facts, ArtifactRelation::BrokerPublish).is_empty(),
            "a listener is not a producer — the array form yields no publish: {:?}",
            facts.refs
        );
        // Both are attributed to the one listener method, not the file module: this
        // is what makes them two `Consumer` nodes off one declaration downstream.
        let subs: Vec<_> = facts
            .refs
            .iter()
            .filter(|r| r.relation == Some(ArtifactRelation::BrokerSubscribe))
            .collect();
        assert_eq!(subs.len(), 2);
        assert!(
            subs.iter().all(|r| r.source.as_str().contains("handle")),
            "each array topic is sourced from its listener method: {subs:?}"
        );
    }

    /// The **other two listener annotations'** array forms. The array patterns'
    /// `#any-of?` predicates cover `@RabbitListener`/`@JmsListener` and all four
    /// attribute keys, but the shape table exercises only the Kafka array — so
    /// deleting an array pattern is caught while *narrowing its predicate lists* is
    /// not. This closes that.
    #[test]
    fn rabbit_and_jms_array_attribute_forms_capture_every_element() {
        let src = r#"
package com.acme;
class C {
    @RabbitListener(queues = {"${q.a}", "q-b"})
    public void onQueue(String m) {}

    @JmsListener(destination = {"d{0}", "d$1"})
    public void onDestination(String m) {}
}
"#;
        let facts = extract_java(src);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec![
                "${q.a}".to_string(),
                "d$1".to_string(),
                "d{0}".to_string(),
                "q-b".to_string(),
            ],
            "every element of a Rabbit/JMS array binds, characters and all: {:?}",
            facts.refs
        );
    }

    /// A **blank** topic literal is deliberately silent, and this pins that choice
    /// so it is a decision on the record rather than an accident.
    ///
    /// `topics = ""` matches the *binding* pattern — it is a `(string_literal)` —
    /// so it produces no refusal candidate, and `broker_topic_key` then refuses its
    /// empty key. The result is a site that reports nothing, which is the shape of
    /// silence this story otherwise removes ([NFR-CC-04]).
    ///
    /// Recording it would mean adding `(string_literal)` to the slot enumeration,
    /// which reintroduces exactly the pattern overlap that cost this story its
    /// scalar-literal matches once already (see
    /// [`every_shape_in_one_file_binds_without_the_patterns_shadowing_each_other`]),
    /// in exchange for a shape no real listener writes. If a later story does want
    /// it, the hook is `literal_text` returning `None` — not a wider slot pattern.
    #[test]
    fn a_blank_topic_literal_binds_nothing_and_is_deliberately_not_recorded() {
        let src = r#"
package com.acme;
class C {
    @KafkaListener(topics = "")
    public void onEmpty(String m) {}

    @KafkaListener(topics = "   ")
    public void onBlank(String m) {}

    @KafkaListener(topics = "kept")
    public void onKept(String m) {}
}
"#;
        let facts = extract_java(src);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["kept".to_string()],
            "a blank literal neither binds nor records; only the real topic does: {:?}",
            facts.refs
        );
    }

    /// Refusals are attributed per **declaration**, so two refused listeners that
    /// share a source line are two rows, and a refusal inside a nested class is
    /// attributed to the nested method rather than to the outer class.
    ///
    /// The per-site dedup keys on `(side, symbol, line)`, so the same-line pair is
    /// the case that would collapse if the symbol were ever dropped from that key —
    /// which is what makes it worth asserting rather than assuming.
    #[test]
    fn refusals_are_attributed_per_declaration_not_per_line() {
        let src = r#"
package com.acme;
class Outer {
    @KafkaListener(topics = A) public void m1(String m) {} @KafkaListener(topics = B) public void m2(String m) {}

    class Inner {
        @KafkaListener(topics = C)
        public void onNested(String m) {}
    }
}
"#;
        let facts = extract_java(src);
        let mut sources: Vec<&str> = facts
            .refs
            .iter()
            .filter(|r| {
                r.relation == Some(ArtifactRelation::BrokerSubscribe) && r.target.is_empty()
            })
            .map(|r| r.source.as_str())
            .collect();
        sources.sort();
        assert_eq!(
            sources.len(),
            3,
            "two same-line declarations and one nested one are three sites: {sources:?}"
        );
        for want in ["m1", "m2", "onNested"] {
            assert!(
                sources.iter().any(|s| s.contains(want)),
                "the refusal at `{want}` is attributed to it: {sources:?}"
            );
        }
        // The nested one is attributed inside `Inner`, not to the outer class.
        assert!(
            sources.iter().any(|s| s.contains("Inner") && s.contains("onNested")),
            "the nested refusal names its own enclosing declaration: {sources:?}"
        );
    }

    /// **[CR-107] acceptance: the refusal is recorded, not silent.** A topic that is
    /// genuinely **not** a string literal — a constant reference, a variable, a
    /// concatenation — is still refused ([NFR-RA-05]): it binds nothing and
    /// fabricates no topic key. What changes is that it no longer *vanishes*. The
    /// site leaves one keyless broker-arm ledger row, at most once per site, which
    /// the [FR-WS-05] coverage tier reports as `topic-not-literal`.
    ///
    /// A keyless row is inert everywhere else by an already-documented contract:
    /// `resolve::topics::broker_refs` refuses an empty target ("a keyless row is not
    /// a topic — never fabricate one"), so no `Topic`/`Consumer` node is promoted
    /// from it and no bridge edge can be built on it.
    #[test]
    fn a_non_literal_topic_is_refused_and_recorded_once_per_site() {
        let src = r#"
package com.acme;
class C {
    private static final String TOPIC = "never-captured";

    // A constant reference.
    @KafkaListener(topics = TOPIC)
    public void byConstant(String m) {}

    // A field/variable reference through a qualifier.
    @KafkaListener(topics = Topics.ORDERS)
    public void byField(String m) {}

    // A concatenation.
    @KafkaListener(topics = PREFIX + "orders")
    public void byConcatenation(String m) {}

    // A call. The fourth shape the `.scm` enumerates — asserted because deleting
    // `(method_invocation)` from both slot patterns was otherwise undetectable.
    @KafkaListener(topics = config.topic())
    public void byCall(String m) {}
}
"#;
        let facts = extract_java(src);

        // Nothing bound: no fabricated key, and in particular never the operand's
        // source text (`TOPIC`, `Topics.ORDERS`) masquerading as a topic name.
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec![String::new(), String::new(), String::new(), String::new()],
            "four refused sites, four keyless rows, no fabricated topic: {:?}",
            facts.refs
        );

        // One row per refused site, attributed to that site's own method.
        let mut sources: Vec<&str> = facts
            .refs
            .iter()
            .filter(|r| {
                r.relation == Some(ArtifactRelation::BrokerSubscribe) && r.target.is_empty()
            })
            .map(|r| r.source.as_str())
            .collect();
        sources.sort();
        assert_eq!(sources.len(), 4, "at most once per site: {:?}", facts.refs);
        for want in ["byCall", "byConcatenation", "byConstant", "byField"] {
            assert!(
                sources.iter().any(|s| s.contains(want)),
                "the refusal at `{want}` is attributed to it: {sources:?}"
            );
        }
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

    /// A relay method that both **subscribes to** and **re-publishes on** the same
    /// topic emits a `BrokerSubscribe` and a `BrokerPublish` that coincide on
    /// `(source, target, form, kind)` and differ only in relation. Both must
    /// survive the ledger dedup — dropping the subscribe would lose a real
    /// cross-service fan-out fact ([FR-WS-10], [NFR-RA-05]). Regression for the
    /// relation-blind `dedup_sort_refs` collision.
    #[test]
    fn a_relay_method_keeps_both_its_publish_and_subscribe_on_one_topic() {
        let src = r#"
package com.acme;
class Relay {
    private KafkaTemplate<String, String> kafkaTemplate;

    @KafkaListener(topics = "orders")
    public void relay(String msg) {
        kafkaTemplate.send("orders", msg);
    }
}
"#;
        let facts = extract_java(src);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["orders".to_string()],
            "the subscribe survives alongside the same-topic publish: {:?}",
            facts.refs
        );
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerPublish),
            vec!["orders".to_string()],
            "the publish survives alongside the same-topic subscribe: {:?}",
            facts.refs
        );
        // Both are attributed to the relay method, coincide on target/form/kind,
        // and differ only in relation — the exact dedup-collision shape.
        let broker: Vec<_> = facts
            .refs
            .iter()
            .filter(|r| {
                matches!(
                    r.relation,
                    Some(ArtifactRelation::BrokerPublish) | Some(ArtifactRelation::BrokerSubscribe)
                )
            })
            .collect();
        assert_eq!(broker.len(), 2, "both broker refs survive: {broker:?}");
        assert!(broker.iter().all(|r| r.source.as_str().contains("relay")
            && r.target == "orders"
            && r.form == RefForm::Method));
    }
    /// **S-370 / [CR-117] acceptance: the header-form publish site, and its whole
    /// operand table.**
    ///
    /// The publish arm recognised a topic only in **argument** position
    /// (`kafkaTemplate.send("orders", …)`), and idiomatic Spring never puts it
    /// there: the topic reaches the message through
    /// `MessageBuilder…setHeader(KafkaHeaders.TOPIC, …)` and `send(message)`
    /// receives the assembled `Message`. Measured on the 84-member reference
    /// estate, **all 54** `KafkaHeaders.TOPIC` sites are written that way and
    /// **zero** publish sites anywhere carry a topic-shaped first argument, so
    /// [FR-WS-10]'s producer promise yielded 0 `Producer` nodes
    /// ([CR-117] §2).
    ///
    /// One test, one table — because a single case would leave the class untested,
    /// and because each row's outcome is decided by a *different* part of the
    /// query. Each operand shape is either **captured** (a static literal, whatever
    /// characters it carries), **refused with the reason it earns** (one keyless
    /// `topic-not-literal` row, [NFR-CC-04]), or one of the three **deliberate
    /// silences** — a blank literal, an all-whitespace literal, and an operand
    /// shape outside the four the slot enumerates — which bind nothing and report
    /// nothing, exactly as the subscribe side treats them. The silences are
    /// asserted by name, not merely absent from a count: every refusal filter in
    /// these tests keys on `target.is_empty()`, so a blank literal that ever *did*
    /// emit a row would be indistinguishable from a refusal.
    ///
    /// The whole table lives in one file so the patterns are also proven not to
    /// shadow each other — the failure mode S-339 measured when a wildcard slot
    /// silently cost the literal patterns their matches. It covers both setters
    /// (`setHeader` and `setHeaderIfAbsent`) and one method carrying **both** a
    /// bound and a refused publish, which is what holds the site reconcile: the
    /// bound literal must not cancel the refusal at a different call in the same
    /// method.
    ///
    /// **Zero captures is the expected outcome on the real corpus, and it is not a
    /// failure of this story.** S-365 measured that none of the 54 header-form sites
    /// carries a literal topic, so recognition alone admits nothing; keying a
    /// configuration-bound operand is [CR-117] §3.2's canonical-identity rule, which
    /// S-371 owns and which is not planned. What this story delivers is the honest
    /// refusal rows — the site is visible and says why it did not bind, instead of
    /// being indistinguishable from a codebase with no producer at all.
    ///
    /// [CR-117]: ../../../docs/requests/CR-117-broker-publish-capture-and-the-topic-key-namespace.md
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn every_header_form_publish_operand_shape_is_captured_or_refused_with_its_reason() {
        // The two literal rows are written exactly as the reference estate writes a
        // topic *would* it write one (`${…}` is Spring's standard form), and the five
        // refused rows are the shapes the estate actually writes: 16 sites pass the
        // topic as a method parameter and 35 read it off a `@ConfigurationProperties`
        // getter.
        let src = r#"
package com.acme;
class KafkaProducer {
    private static final String PREFIX = "acme.";
    private KafkaTemplate<String, SpecificRecord> kafkaTemplate;
    private KafkaTopics kafkaTopics;
    private String topicField;

    @Value("${spring.kafka.topics.notifications}")
    private String notificationTopic;

    // ── captured: a plain static literal.
    public void byLiteral(SpecificRecord payload) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, "orders")
                .build());
    }

    // ── captured: a `${…}` placeholder literal — a static literal like any other,
    //    keyed by its own text ([CR-107]).
    public void byPlaceholderLiteral(SpecificRecord payload) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, "${spring.kafka.topics.shipments}")
                .build());
    }

    // ── refused: a `@Value`-injected field, referenced bare. The estate's own
    //    shape for three of its sites.
    public void byInjectedField(SpecificRecord payload) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, notificationTopic)
                .build());
    }

    // ── refused: a `@ConfigurationProperties` getter — the estate's majority shape.
    public void byConfigurationGetter(SpecificRecord payload) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, kafkaTopics.getArchiveCommands())
                .build());
    }

    // ── refused: a method parameter. All 13 src/main sites are this shape.
    public void byMethodParameter(SpecificRecord payload, String topic) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, topic)
                .build());
    }

    // ── refused: a concatenation. Note the operand *contains* a literal, and that
    //    literal must not bind — the topic is `PREFIX + "orders"`, which has no
    //    static identity.
    public void byConcatenation(SpecificRecord payload) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, PREFIX + "orders")
                .build());
    }

    // ── refused: a qualified field. Asserted because deleting `(field_access)`
    //    from the slot enumeration would otherwise be undetectable — the mutation
    //    S-339's review caught on the subscribe side.
    public void byQualifiedField(SpecificRecord payload) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, this.topicField)
                .build());
    }

    // ── captured / refused through `setHeaderIfAbsent`, `MessageBuilder`'s sibling
    //    setter. The estate writes only `setHeader`, so this arm of the predicate
    //    rides on the API's shape — and rode untested: narrowing both predicates to
    //    `#eq? "setHeader"` removed the capability with the whole suite still
    //    green. One row per half makes a later narrowing a visible decision.
    public void byIfAbsentLiteral(SpecificRecord payload) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeaderIfAbsent(KafkaHeaders.TOPIC, "shipments-if-absent")
                .build());
    }

    public void byIfAbsentParameter(SpecificRecord payload, String topic) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeaderIfAbsent(KafkaHeaders.TOPIC, topic)
                .build());
    }

    // ── DELIBERATELY SILENT: a blank literal. It matches the binding pattern, so
    //    it produces no refusal candidate, and `broker_topic_key` then refuses its
    //    empty key — the same shape the subscribe side treats this way. Pinned here
    //    because every refusal filter in these tests keys on `target.is_empty()`,
    //    so a blank literal that ever DID emit a row would be indistinguishable
    //    from a refusal.
    public void byBlankLiteral(SpecificRecord payload) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, "")
                .build());
    }

    public void byWhitespaceLiteral(SpecificRecord payload) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, "   ")
                .build());
    }

    // ── DELIBERATELY SILENT: an operand shape outside the four the slot
    //    enumerates. It binds nothing (the half that matters, [NFR-RA-05]) and
    //    reports nothing — exactly as the subscribe side treats a shape outside its
    //    own four. Asserted so the boundary stays a decision rather than an
    //    accident; widening it is a change to BOTH sides' enumerations, never a
    //    wildcard on one.
    public void byTernary(SpecificRecord payload, boolean urgent) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, urgent ? "urgent" : "normal")
                .build());
    }

    // ── ONE METHOD, BOTH OUTCOMES: a bound literal beside a refused parameter.
    //    The site reconcile must not let the bound sibling suppress the refusal —
    //    the candidate's site is its OWN `setHeader` argument list, so a literal
    //    from a different call in the same method never falls inside it.
    public void byBothBoundAndRefused(SpecificRecord payload, String topic) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, "both-bound")
                .build());
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, topic)
                .build());
    }
}
"#;
        let facts = extract_java(src);

        // ── The captured half of the table: exactly the two literals, keyed by their
        //    own text. `send(message)` carries no string argument, so the pre-existing
        //    argument-position pattern contributes nothing here — no site is counted
        //    twice.
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerPublish)
                .into_iter()
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>(),
            vec![
                "${spring.kafka.topics.shipments}".to_string(),
                "both-bound".to_string(),
                "orders".to_string(),
                "shipments-if-absent".to_string(),
            ],
            "every header-form literal binds, keyed as written, through either setter, \
             and nothing else does — no blank literal and no ternary: {:?}",
            facts.refs
        );

        // ── The refused half: one keyless row per site, attributed to the publishing
        //    method that earned it. Named per method rather than counted, so a
        //    refusal recorded against the wrong shape cannot pass.
        let mut refused: Vec<&str> = facts
            .refs
            .iter()
            .filter(|r| {
                r.relation == Some(ArtifactRelation::BrokerPublish) && r.target.is_empty()
            })
            .map(|r| r.source.as_str())
            .collect();
        refused.sort();
        for want in [
            "byInjectedField",
            "byConfigurationGetter",
            "byMethodParameter",
            "byConcatenation",
            "byQualifiedField",
            "byIfAbsentParameter",
            "byBothBoundAndRefused",
        ] {
            assert!(
                refused.iter().any(|s| s.contains(want)),
                "the `{want}` operand is refused and its site recorded: {refused:?}"
            );
        }
        // Seven refusals, and specifically NOT nine: the two blank-literal sites and
        // the ternary site are the deliberate silences, and the four literal sites
        // record nothing because they bound.
        assert_eq!(
            refused.len(),
            7,
            "seven non-keyable operands are seven refusals; the blank literals and \
             the ternary are silent, and the bound sites record none: {refused:?}"
        );
        for silent in ["byBlankLiteral", "byWhitespaceLiteral", "byTernary"] {
            assert!(
                !refused.iter().any(|s| s.contains(silent)),
                "`{silent}` is a DELIBERATE silence — it binds nothing and reports \
                 nothing, the same boundary the subscribe side draws: {refused:?}"
            );
        }
        // `byBothBoundAndRefused` appears in BOTH halves: one keyed row and one
        // keyless row on the same source symbol. They survive the ledger dedup
        // because their targets differ, and the reconcile does not let the bound
        // literal cancel the refusal at the other call.
        assert!(
            targets(&facts, ArtifactRelation::BrokerPublish)
                .iter()
                .any(|t| t == "both-bound"),
            "the bound half of `byBothBoundAndRefused` keys: {:?}",
            facts.refs
        );

        // ── Never fabricated: no refusal carries the operand's source text as a
        //    topic key, which is the failure `broker_topic_key` and this row's empty
        //    target exist to prevent ([NFR-RA-05]).
        for forbidden in [
            "topic",
            "notificationTopic",
            "this.topicField",
            "topicField",
            "kafkaTopics.getArchiveCommands()",
            "PREFIX + \"orders\"",
            "PREFIX",
        ] {
            assert!(
                !targets(&facts, ArtifactRelation::BrokerPublish)
                    .iter()
                    .any(|t| t == forbidden),
                "`{forbidden}` is an operand, never a topic key: {:?}",
                facts.refs
            );
        }
    }

    /// **S-370's near-miss guard.** The site is recognised by the **topic header
    /// constant**, never by the builder type — so a message builder from another
    /// library setting an unrelated header is not a publish site, and no coverage
    /// denominator is manufactured out of ordinary code.
    ///
    /// The sharp case is the last one: a *different* class's `TOPIC` constant. It
    /// proves the query keys on `KafkaHeaders.TOPIC` **as written** rather than on a
    /// bare `TOPIC` name — the over-match a `field: (identifier) @k (#eq? @k
    /// "TOPIC")` predicate alone would let through. The reference estate writes all
    /// 54 of its sites in the qualified form and none through a static import, so
    /// the qualified form is what is recognised; a statically-imported bare `TOPIC`
    /// is deliberately not, and that decision is recorded in `brokers.scm`.
    #[test]
    fn a_near_miss_builder_setting_an_unrelated_header_is_not_a_publish_site() {
        let src = r#"
package com.acme;
class NotAProducer {
    private String topic;

    // A message builder from another library, setting headers that have nothing to
    // do with a Kafka topic.
    public void enrich(String payload, String correlationId) {
        SomeOtherBuilder.withPayload(payload)
                .setHeader(HttpHeaders.CONTENT_TYPE, "application/json")
                .setHeader("X-Correlation-Id", correlationId)
                .setHeader(AmqpHeaders.ROUTING_KEY, topic)
                .build();
    }

    // Spring's own builder, but an unrelated Kafka header: the message key is not
    // the topic.
    public void keyOnly(String payload, String key) {
        MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.MESSAGE_KEY, key)
                .build();
    }

    // The sharp near miss: another class's `TOPIC` constant.
    public void otherTopicConstant(String payload) {
        MessageBuilder.withPayload(payload)
                .setHeader(MyHeaders.TOPIC, topic)
                .build();
    }

    // A *read* of the Kafka topic header is not a publish either.
    public String inspect(MessageHeaders headers) {
        return (String) headers.get(KafkaHeaders.TOPIC);
    }

    // Two arguments in exactly the recognised positions, but the call is not a
    // header SETTER — it builds a map. This is the case that gives the
    // `setHeader`/`setHeaderIfAbsent` predicate its coverage: `headers.get(…)`
    // above is excluded by arity alone, so without this row deleting the predicate
    // would be undetectable.
    public Map<String, Object> headerMap(String topic) {
        return Map.of(KafkaHeaders.TOPIC, topic);
    }

    // INVERTED: the Kafka topic header appears as the header's *value*, not as its
    // key — a trace header whose value happens to be the string `"kafka_topic"`.
    // Nothing about it is a publish. (Sibling order is enough to exclude it: a
    // tree-sitter query matches sibling child patterns in order, so the header
    // pattern can never be assigned to a node that follows the operand slot.)
    public void invertedHeader(String payload) {
        MessageBuilder.withPayload(payload)
                .setHeader(Tracing.FORWARDED_FROM, KafkaHeaders.TOPIC)
                .build();
    }

    // NOT THE FIRST TWO ARGUMENTS: a three-argument `setHeader` from some other
    // API, where the Kafka topic header sits in the middle. This is what gives the
    // query's position anchors their coverage — sibling order alone admits it,
    // because the header still precedes the operand; only the leading `.` (the
    // header is the FIRST argument) and the middle `.` (the operand is IMMEDIATELY
    // after it) exclude it.
    public void scopedSetHeader(String payload, String topic) {
        ScopedHeaders.withPayload(payload)
                .setHeader(Scope.OUTBOUND, KafkaHeaders.TOPIC, topic)
                .build();
    }

    // The same two near misses again with a LITERAL operand, which is what reaches
    // the BINDING pattern rather than the refusal slot. Without these rows, dropping
    // either of the binding pattern's own predicates would be undetectable: every
    // other row here carries a non-literal operand and so exercises only the slot.
    public void otherTopicConstantWithLiteral(String payload) {
        MessageBuilder.withPayload(payload)
                .setHeader(MyHeaders.TOPIC, "orders")
                .build();
    }

    public Map<String, Object> headerMapWithLiteral() {
        return Map.of(KafkaHeaders.TOPIC, "orders");
    }

    public void scopedSetHeaderWithLiteral(String payload) {
        ScopedHeaders.withPayload(payload)
                .setHeader(Scope.OUTBOUND, KafkaHeaders.TOPIC, "orders")
                .build();
    }

    // HEADER FIRST but a THIRD argument follows — a foreign 3-arity
    // `setHeader(name, value, flag)` API. Spring's `MessageBuilder.setHeader` and
    // `MessageHeaderAccessor.setHeader` are both strictly 2-arity, so a third
    // argument means this is not them. Sibling order and the leading anchor both
    // ADMIT this shape (the header really is first); only the TRAILING anchor
    // excludes it. Measured before that anchor existed: the literal row below
    // bound `target="orders"` — a foreign API fabricating a Kafka topic — and the
    // dynamic row manufactured a refusal for it.
    public void trailingArgWithLiteral(String payload) {
        Weird.setHeader(KafkaHeaders.TOPIC, "orders", true);
    }

    public void trailingArgDynamic(String payload, String topic) {
        Weird.setHeader(KafkaHeaders.TOPIC, topic, Scope.OUTBOUND);
    }

    // A different Kafka header with a LITERAL value: a message key is not a topic.
    // This row exists because without it, dropping the BINDING pattern's
    // `#eq? @_pub_hdr_key "TOPIC"` predicate is undetectable — `keyOnly` above
    // carries a non-literal operand and so exercises only the refusal slot's copy
    // of that predicate. Measured with the predicate deleted, `GROUP_ID`,
    // `MESSAGE_KEY` and `RECEIVED_PARTITION` literals all became `Topic` nodes:
    // a fabricated topic identity, which is the [NFR-RA-05] failure this arm
    // exists to prevent.
    public void messageKeyWithLiteral(String payload) {
        MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.MESSAGE_KEY, "customer-42")
                .build();
    }
}
"#;
        let facts = extract_java(src);
        let publishes: Vec<String> = facts
            .refs
            .iter()
            .filter(|r| r.relation == Some(ArtifactRelation::BrokerPublish))
            .map(|r| format!("{} -> {:?}", r.source.as_str(), r.target))
            .collect();
        assert!(
            publishes.is_empty(),
            "no near-miss header is a publish site — neither bound nor refused, \
             because there is no broker site to attribute a refusal to: {publishes:?}"
        );
    }

    /// **The one shape the anchors cannot see, recorded rather than hidden.**
    ///
    /// Tree-sitter counts a `comment` as an intervening named sibling, so a comment
    /// inside the argument list breaks the anchors' adjacency and the site matches
    /// NEITHER pattern — captured as nothing and refused as nothing. That is the
    /// invisible loss [NFR-CC-04] forbids, in the one shape this arm cannot
    /// express: no tree-sitter construct skips extras inside an anchored sequence.
    ///
    /// So it is pinned here instead of fixed, and `brokers.scm` records it in the
    /// same "not recognised" list as the static-import form. The reference estate
    /// writes 0 such sites. A future story that needs them must drop the middle
    /// anchor and re-measure the false-positive cost the leading/trailing pair
    /// buys — the 3-arity over-match this story's review found is what that pair
    /// currently excludes — never silently widen one pattern.
    ///
    /// The subscribe side is unaffected: it matches on the `value:` field rather
    /// than on an anchor, so `@KafkaListener(topics = /*x*/ TOPIC)` still records
    /// its refusal. The hazard arrives with the anchored publish patterns, which is
    /// why it is asserted on this side only.
    #[test]
    fn a_commented_argument_list_is_the_one_shape_the_anchors_cannot_see() {
        let src = r#"
package com.acme;
class C {
    private KafkaTemplate<String, String> kafkaTemplate;

    public void commentBetweenTheArguments(String payload, String topic) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, /*topic*/ topic)
                .build());
    }

    public void commentBeforeTheHeader(String payload, String topic) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeader(/*hdr*/ KafkaHeaders.TOPIC, topic)
                .build());
    }
}
"#;
        let facts = extract_java(src);
        let publishes: Vec<String> = facts
            .refs
            .iter()
            .filter(|r| r.relation == Some(ArtifactRelation::BrokerPublish))
            .map(|r| format!("{} -> {:?}", r.source.as_str(), r.target))
            .collect();
        assert!(
            publishes.is_empty(),
            "a commented argument list is silent — NOT the behaviour anyone wants, \
             but the measured behaviour, recorded so it is a known boundary rather \
             than an invisible loss. If this test starts failing because the shape \
             became recognised, that is an improvement: delete the test and the \
             `brokers.scm` note together: {publishes:?}"
        );

        // The subscribe side, through the same comment, still records its refusal —
        // so the silence is attributable to the publish anchors specifically.
        let subscribe = extract_java(r#"
package com.acme;
class D {
    @KafkaListener(topics = /*x*/ TOPIC)
    public void onOrder(String msg) {}
}
"#);
        assert_eq!(
            subscribe
                .refs
                .iter()
                .filter(|r| {
                    r.relation == Some(ArtifactRelation::BrokerSubscribe) && r.target.is_empty()
                })
                .count(),
            1,
            "the subscribe side matches on `value:`, not on an anchor, so a comment \
             does not silence it: {:?}",
            subscribe.refs
        );
    }

    /// **The no-re-keying guard.** This story *adds* sites; it must not change the
    /// key of a topic that already bound. The argument-position publish and the
    /// annotation subscribe keep byte-identical keys with the header-form patterns
    /// in place, including in one file where a header-form site sits beside them.
    #[test]
    fn adding_the_header_form_leaves_already_captured_topic_keys_byte_identical() {
        let src = r#"
package com.acme;
class Mixed {
    private KafkaTemplate<String, String> kafkaTemplate;

    @KafkaListener(topics = "${spring.kafka.topics.orders}")
    public void onOrder(String msg) {}

    public void byArgument(String payload) {
        kafkaTemplate.send("orders", payload);
    }

    public void byHeader(String payload) {
        kafkaTemplate.send(MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, "orders")
                .build());
    }
}
"#;
        let facts = extract_java(src);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["${spring.kafka.topics.orders}".to_string()],
            "the subscribe key is unchanged, byte for byte: {:?}",
            facts.refs
        );
        // Two publish sites on one topic in one class: the argument form and the
        // header form. They key identically — which is the point — and BOTH rows
        // survive here, but not for the reason an earlier version of this comment
        // claimed. `extract_java` IS the production caller (dedup included); the
        // rows survive because their SOURCE symbols differ (`byArgument` vs
        // `byHeader`). Two header-form publishes in ONE method on one topic would
        // legitimately collapse to a single row, which is what
        // `refusals_are_attributed_per_declaration_not_per_line` asserts on the
        // refusal side.
        let publishes: Vec<&str> = facts
            .refs
            .iter()
            .filter(|r| r.relation == Some(ArtifactRelation::BrokerPublish))
            .map(|r| r.target.as_str())
            .collect();
        assert_eq!(
            publishes,
            vec!["orders", "orders"],
            "both publish forms key the topic identically: {:?}",
            facts.refs
        );
        let sources: Vec<&str> = facts
            .refs
            .iter()
            .filter(|r| r.relation == Some(ArtifactRelation::BrokerPublish))
            .map(|r| r.source.as_str())
            .collect();
        assert!(
            sources.iter().any(|s| s.contains("byArgument"))
                && sources.iter().any(|s| s.contains("byHeader")),
            "each form is attributed to its own publishing method: {sources:?}"
        );
    }
}
