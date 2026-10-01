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
//! **Amended 2026-09-15 (S-408, [CR-131] §3.2 A1): one bounded exception, and why
//! it is not a per-language one.** The Kafka Streams topology form added a
//! `@broker.*.receiver` capture and a RECEIVER GATE in Rust ([`receiver_is_topology`]
//! and its helpers below), so "no new Rust plumbing" is no longer literally true
//! of this arm and the sentence above must not be read as if it were. It could
//! not be pure data: the gate has to relate a call site to a typed declaration
//! elsewhere in the file, and a tree-sitter pattern matches within one bounded
//! subtree — `#eq?`/`#match?` compare captures of the SAME match and cannot reach
//! a second, independently-located one. That is the same reason
//! [`crate::extract::composer`] and `extract::config::accessor` are generic Rust
//! rather than query data.
//!
//! What the claim's *spirit* still holds is the part that matters for
//! [NFR-MA-01]: the gate contains **no per-language branching**. It dispatches on
//! grammar field names the three grammars share (`object`/`value`/`operand`/
//! `function` for a receiver, `type`/`name`/`pattern`/`declarator` for a
//! declaration), never on a language id, so a fourth language shipping a
//! `brokers.scm` still costs no core change. The one genuinely framework-specific
//! datum is [`TOPOLOGY_RECEIVER_TYPES`], and whether it belongs here or in a
//! plugin descriptor beside `http_client_detectors` is recorded as an open review
//! question on that constant rather than settled silently here.
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
//! # The one non-literal operand that binds, and why it is not an exception
//! **(S-409, 2026-09-15, [FR-WS-19], [CR-131] §3.2 A2)**
//!
//! Read the paragraphs above with this correction: "a non-literal operand never
//! binds" is no longer the whole rule. A topic operand that reads a
//! `@ConfigurationProperties` getter — `kafkaTopics.getArchiveEvents()`, bare or
//! self-qualified — resolves to the **canonical `${prefix.key}` placeholder** for
//! the key its owning class declares, and is then stored exactly as a
//! source-written placeholder is. So it does not weaken [NFR-RA-05]: nothing is
//! composed, guessed or read from the operand's source text. The placeholder names
//! a key the repository *commits*, and every hop of the chain that produced it —
//! accessor → field → owning class → annotation prefix → canonical key — is proved
//! from committed source or the operand refuses whole
//! ([`crate::extract::config::accessor::BindingView::placeholder_for`]).
//!
//! It is also not a second mechanism. This is the identical hop the HTTP arm has
//! resolved request paths with since S-397/S-398 — literally the same
//! `BindingView`, built by the same constructor one level up. It is taken at the
//! **refusal slot**, for a reason stated once, on the branch that takes it; see
//! "THE ACCESSOR HOP" in [`capture_broker_invocations`].
//!
//! What still refuses is everything the chain cannot prove: the nine named
//! accessor faults of [FR-WS-19] (a nested getter, a method parameter, an unbound
//! or ambiguous name, a non-getter, an unknown receiver type, a type naming no
//! bound class, an undeclared property, an unrecognised shape) and every operand
//! that was never an accessor — a qualified constant, a concatenation. All of them
//! report through this arm's existing `topic-not-literal` vocabulary, which is the
//! same collapse the HTTP arm makes into `base-url-runtime`; neither arm carries
//! the reason to the ledger.
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
//! **That last sentence is S-370's reading and S-409 moved it.** "Non-literal" is
//! still true of all 54, but non-literal no longer means unkeyable: most of those
//! operands are accessors, and the hop above resolves them. The current reading is
//! reported per member, with its denominator and its date, by
//! `the_reference_workspace_reports_its_resolved_broker_sites_before_and_after_the_hop`
//! in `logos-core/tests/broker_topic_corpus.rs` — one place, so it cannot go stale
//! in two.
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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::Range;

use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

use crate::extract::config::accessor::{BindingView, DeclaredTypes};
use crate::extract::config::binding::PropertiesIndex;
use crate::extract::config::refs::{
    capture_invocation_refs, record_refusals, surviving_refusals, InvocationSite, RefusalCandidate,
};
use crate::extract::refs::unquote;
use crate::extract::{Facts, FileInput};
use crate::model::{ArtifactRelation, EdgeKind, LogosSymbol, RefForm};
use crate::plugin::{LanguagePlugin, LanguageRegistry};

/// Which side of the broker arm a captured site is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Side {
    Publish,
    Subscribe,
}

impl Side {
    /// The ledger relation this side records under. One place, so the emission
    /// pass and the refusal pass can never file one side under two relations.
    fn relation(self) -> ArtifactRelation {
        match self {
            Side::Publish => ArtifactRelation::BrokerPublish,
            Side::Subscribe => ArtifactRelation::BrokerSubscribe,
        }
    }
}

/// Run a grammar's `brokers` query over `root` and emit one broker reference per
/// captured static-topic publish/subscribe site (S-254, [FR-WS-10]).
///
/// `enclosing` attributes a captured node to the symbol of its innermost
/// enclosing declaration (the publishing/subscribing function/method), the same
/// resolver the code-reference collector uses. Returns the number of references
/// captured (a language without the `brokers` capability never calls this).
///
/// `binding` yields the reading file's configuration-binding view (S-409,
/// [FR-WS-19]), or [`None`] for a member declaring no `@ConfigurationProperties`
/// class. It is a **closure, not a value**, and deliberately: building the view
/// walks the file's whole AST, this arm has no detector gate to build it behind,
/// and it is asked about the **refusal slot's** operand and nowhere else — so a
/// file carrying no slot must not pay for it. The caller memoises, so the walk
/// happens at most once however many slots a file has. See the accessor hop below
/// for why the slot is the only node it could be asked about.
///
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
pub(super) fn capture_broker_invocations<'b, F, B>(
    query: &Query,
    root: Node<'_>,
    source: &[u8],
    enclosing: F,
    binding: B,
    facts: &mut Facts,
) -> usize
where
    F: Fn(Node<'_>) -> Option<LogosSymbol>,
    B: Fn() -> Option<&'b BindingView<'b>>,
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
    let mut bound: Vec<(ArtifactRelation, Range<usize>)> = Vec::new();
    let mut candidates: Vec<RefusalCandidate> = Vec::new();
    // The S-417 forwarding shape of each refusal candidate, by its index in
    // `candidates`, so the two lists are reconciled after the loop by position
    // rather than by a second reading of the tree.
    let mut pending_forwards: Vec<(usize, Option<PendingForward>)> = Vec::new();

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
        let mut receiver_node: Option<Node<'_>> = None;
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
                // The RECEIVER GATE's carrier (S-408, [CR-131] §3.2 A1). A
                // pattern that captures it is asking for the whole match to be
                // discarded — binding and refusal alike — unless the receiver is
                // a stream-topology receiver. A pattern that does NOT capture it
                // is ungated, which is every pattern that shipped before S-408.
                "broker.publish.receiver" | "broker.subscribe.receiver" => {
                    receiver_node = Some(cap.node)
                }
                _ => {}
            }
        }

        // The gate runs BEFORE the refusal candidate is built: a `to(…)` outside
        // a topology chain must be silent, not refused. Recording it would
        // manufacture a coverage denominator out of every unrelated `to(…)` call
        // in the tree — the exact failure the Rust query header declines refusal
        // slots over, and the reason this arm can key on a verb this common at
        // all.
        if let Some(receiver) = receiver_node {
            if !receiver_is_topology(receiver, root, source) {
                continue;
            }
        }

        // ── THE ACCESSOR HOP (S-409, [FR-WS-19], [CR-131] §3.2 A2) ──────────
        //
        // The refusal slot is where it has to happen, and that is a fact about
        // the queries rather than a choice made here: every shipped `brokers.scm`
        // narrows `@broker.*.topic` to a `(string_literal)`, so a
        // `kafkaTopics.getArchiveEvents()` operand never reaches `topic_node` at
        // all — it arrives as the `(method_invocation)` arm of the slot's own
        // enumeration, on its way to being reported `topic-not-literal`. Asking
        // the binding view here is therefore asking it about the ONLY node that
        // can carry an accessor.
        //
        // A resolved accessor takes the SAME two steps a literal takes below —
        // its range joins `bound`, and it becomes an `InvocationSite` whose
        // `topic` slot holds the canonical `${prefix.key}` placeholder — so the
        // site re-enters the existing config-bound path with no second rule
        // anywhere, and the refusal candidate it would otherwise have raised is
        // cancelled by `record_refusals`' own site reconcile rather than by a
        // flag set here. That reconcile is the mechanism the HTTP arm's
        // `judged_operands` already uses; it is documented once, on
        // `record_refusals`.
        //
        // The range joins `bound` BEFORE the enclosing-symbol lookup, for the
        // reason the literal arm states about itself: an operand this arm
        // RESOLVED but could not attribute binds nothing, and reporting its site
        // `topic-not-literal` would be a wrong reason ([NFR-CC-04]).
        //
        // The operand's enclosing declaration is resolved here rather than in the
        // recorder: an operand with no attributable declaration is not a
        // candidate at all, which is exactly how it was treated when the
        // attribution lived inside the recorder.
        if let (Some((slot_side, slot_node)), Some(site)) = (slot, site_node) {
            match binding().and_then(|b| b.placeholder_for(slot_node, source)) {
                Some(placeholder) => {
                    bound.push((slot_side.relation(), slot_node.byte_range()));
                    if let Some(source_symbol) = enclosing(slot_node) {
                        let mut slots = BTreeMap::new();
                        slots.insert("topic".to_string(), placeholder);
                        // The same optional schema guard the literal arm applies,
                        // read from the same match. No shipped `.scm` captures
                        // `@broker.*.schema` beside a slot, so this composes the
                        // key identically for a query that does rather than
                        // leaving one arm of the key behind for the other to
                        // drift from.
                        if let Some(schema) = schema_node.and_then(|n| literal_text(n, source)) {
                            slots.insert("schema".to_string(), schema);
                        }
                        let resolved = InvocationSite {
                            source: source_symbol,
                            slots,
                            line: slot_node.start_position().row as u32 + 1,
                        };
                        match slot_side {
                            Side::Publish => publishes.push(resolved),
                            Side::Subscribe => subscribes.push(resolved),
                        }
                    }
                }
                // Unresolved — every one of [FR-WS-19]'s nine named accessor
                // faults, and every operand that was never an accessor, lands
                // here together and reports through the arm's existing
                // `topic-not-literal` vocabulary. That collapse is not a loss of
                // fidelity introduced by this story: it is exactly what the HTTP
                // arm does with the same nine, which all reach its ledger as one
                // `base-url-runtime` row. Widening the ledger to carry the reason
                // is a change to the [FR-WS-05] reason set no acceptance
                // criterion asks for, on either arm.
                None => {
                    if let Some(source_symbol) = enclosing(slot_node) {
                        // The S-417 forwarding population is a SUBSET of this
                        // one: a refused operand that is a bare parameter of the
                        // method enclosing it. It is derived here because this
                        // is where the operand's node is, and reconciled after
                        // the loop against the refusals that actually survive —
                        // a site whose refusal is cancelled by a sibling literal
                        // bound already, so there is nothing for the hop to
                        // retract.
                        pending_forwards
                            .push((candidates.len(), forwarding_at(slot_node, source)));
                        candidates.push(RefusalCandidate {
                            relation: slot_side.relation(),
                            site: site.byte_range(),
                            source: source_symbol,
                            line: slot_node.start_position().row as u32 + 1,
                        });
                    }
                }
            }
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
        bound.push((side.relation(), topic_node.byte_range()));
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
    // The refused half, recorded through the SHARED refusal recorder (S-370
    // built it here; S-374 generalized it for the HTTP arm, so there is one copy
    // rather than a hand-mirrored twin per arm). The mechanism — a bound operand
    // of the same relation cancels a candidate at the same site, then survivors
    // dedup to one row per `(relation, declaration, line)` — is documented once
    // on `record_refusals` and deliberately not restated here. What is local to
    // this arm is only WHICH nodes fill the carrier, and that is stated where the
    // candidates are built above: the site is the `@broker.*.site` node the
    // `.scm` declares (wider than the operand on purpose, so a sibling literal
    // admitted inside the same site cancels the refusal), and the operand is the
    // `@broker.*.topic.slot` node, which supplies the declaration and the line.
    //
    // The S-417 forwarding population is read off the SAME reconcile, through
    // the same function the recorder uses ([`surviving_refusals`]), so the hop
    // can never be handed a site whose refusal was cancelled — it would then
    // retract a keyless row that was never written and emit a second reference
    // beside a literal that already bound.
    record_forwarding_candidates(facts, &candidates, &bound, pending_forwards);
    emitted += record_refusals(facts, RefForm::Method, candidates, &bound);
    emitted
}

/// The tree-derived half of a [`ForwardingCandidate`], read at the refusal slot
/// and held until the reconcile says whether that refusal survives.
struct PendingForward {
    wrapper: Callee,
    slot: usize,
    declaring_class: Option<String>,
    signature_supported: bool,
}

/// Record one [`ForwardingCandidate`] per **surviving** refusal whose operand is
/// a bare parameter of the method enclosing it.
///
/// The dedup grain is the recorder's own — one row per
/// `(relation, declaration, line)` — so a candidate is recorded exactly where a
/// keyless row is, and `refusals_at_source` counts the survivors sharing that
/// row's ledger identity.
fn record_forwarding_candidates(
    facts: &mut Facts,
    candidates: &[RefusalCandidate],
    bound: &[(ArtifactRelation, Range<usize>)],
    pending: Vec<(usize, Option<PendingForward>)>,
) {
    if pending.iter().all(|(_, forward)| forward.is_none()) {
        return;
    }
    let survivors = surviving_refusals(candidates, bound);
    // Counted over surviving SITES, not over the rows they dedup to. Two refused
    // sites on ONE line are two sites behind one row, and counting the row would
    // let the hop retract it after resolving only the first — the second site
    // would then carry neither a topic nor a refusal, which is less evidence than
    // before this hop existed ([NFR-CC-04]).
    let mut refusals_at_source: HashMap<(ArtifactRelation, String), usize> = HashMap::new();
    for at in &survivors {
        let candidate = &candidates[*at];
        *refusals_at_source
            .entry((candidate.relation, candidate.source.as_str().to_string()))
            .or_default() += 1;
    }

    let mut forwards: HashMap<usize, PendingForward> = pending
        .into_iter()
        .filter_map(|(at, forward)| forward.map(|f| (at, f)))
        .collect();
    // One candidate per surviving site, in the order the sites were raised. No
    // dedup by line here, for the same reason the count above does not: a second
    // site on one line is a second operand, and dropping it means never looking
    // it up.
    for at in survivors {
        let Some(forward) = forwards.remove(&at) else {
            continue;
        };
        let candidate = &candidates[at];
        let at_source = refusals_at_source
            .get(&(candidate.relation, candidate.source.as_str().to_string()))
            .copied()
            .unwrap_or(1);
        facts.forwarding.push(ForwardingCandidate {
            relation: candidate.relation,
            source: candidate.source.clone(),
            line: candidate.line,
            wrapper: forward.wrapper,
            slot: forward.slot,
            declaring_class: forward.declaring_class,
            signature_supported: forward.signature_supported,
            refusals_at_source: at_source,
            outcome: None,
        });
    }
}

/// The forwarding shape of a refused topic operand, or `None` when the operand
/// is not a bare parameter of a method.
///
/// Three shapes return `None` and they are the same answer: the operand is not a
/// bare name at all; the name is bound by no enclosing parameter list (a field,
/// a local, a constant); or the list that binds it belongs to a constructor or a
/// **lambda**, neither of which is a positional method call site. The lambda
/// case is the one worth naming — `(topic) -> producer.send(topic)` binds
/// `topic` at the lambda, so the slot arithmetic must not be done against the
/// enclosing method's parameter list, which is a different list entirely.
fn forwarding_at(operand: Node<'_>, src: &[u8]) -> Option<PendingForward> {
    if operand.kind() != "identifier" {
        return None;
    }
    let name = operand.utf8_text(src).ok()?;
    let scope = declaring_scope(operand, name, src)?;
    if scope.kind() != "method_declaration" {
        return None;
    }
    let params = scope.child_by_field_name("parameters")?;
    let (slot, arity) = parameter_slot(scope, name, src)?;
    Some(PendingForward {
        wrapper: Callee {
            name: scope
                .child_by_field_name("name")?
                .utf8_text(src)
                .ok()?
                .to_string(),
            arity,
        },
        slot,
        declaring_class: enclosing_type_name(scope, src),
        signature_supported: !unsupported_signature(params),
    })
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

/// The receiver type names that make a `stream(…)` / `to(…)` call a **stream
/// topology link** rather than one of the many unrelated calls that share those
/// two method names (S-408, [CR-131] §3.2 A1).
///
/// The roster is Kafka Streams' own topology types and nothing else, because the
/// gate's whole job is to separate `builder.stream(t)` from `orders.stream()` and
/// `chain.to(t)` from `converter.to(t)`:
///
/// - `StreamsBuilder` — the source of every topology. 8 of the 8 `stream(…)`
///   receivers on the 84-member reference estate are a `StreamsBuilder`, all of
///   them a declared formal parameter (`public void kStream(StreamsBuilder b)`)
///   or a local (`StreamsBuilder builder = new StreamsBuilder()`).
/// - `KStream` — what a topology link returns, so it is what a `to(…)` chain
///   bottoms out on when the stream is held in a binding rather than chained in
///   one expression (`KStream<K,V> lines = builder.stream(t); lines.…to(u)`, 1 of
///   the 8 estate `to(…)` sites).
///
/// `KTable` is deliberately absent: it has no `to(…)` and it is reached by
/// `builder.table(…)`, a verb this arm does not capture, so admitting it would
/// widen the gate for a shape nothing here recognises.
///
/// The roster is shared by every language rather than declared per plugin, and
/// that is a decision with a stated cost: these are **library** type names, not
/// language keywords, so a Go or Rust type that happens to be called `KStream`
/// would be admitted by the Java roster entry. Measured rather than assumed: the
/// reference estate has 0 `.rs` files and 262 `.go` files, and over all 262 the
/// gated Go arm produces 0 broker rows — so on the one real cross-language corpus
/// available the collision risk is observed at zero, not argued to be
/// (`tests/broker_topic_corpus.rs`, 2026-09-15). The alternative — a per-manifest
/// roster — buys nothing until a second ecosystem ships a differently-named
/// topology type, and would put a capture rule in a descriptor that carries none
/// of the arm's other capture rules.
///
/// **OPEN REVIEW QUESTION (S-408 review, 2026-09-15) — deliberately not settled
/// here.** This constant is the one genuinely framework-specific datum the
/// receiver gate carries, and the architecture review argued it belongs in a
/// plugin descriptor rather than in core, citing [CR-131] §3.1 ("which method on
/// which receiver is a publish [stays] in plugin descriptors and queries") and the
/// repository's own precedent for exactly this shape: `http_client_detectors`
/// (CR-108) and the `[properties]` table's `accessor_prefixes` (CR-121), both
/// rosters interpreted generically by core but supplied per language in
/// `plugin.toml`.
///
/// The counter-argument is the one stated above — this roster is IDENTICAL across
/// all three languages, where `http_client_detectors` genuinely differs per
/// language (`net::http` for Go), so moving it would put three copies of one list
/// in three descriptors, which is the drift hazard `plugin-registry.md` warns
/// about in the same breath. Both readings are defensible and the choice changes a
/// descriptor contract (a new manifest field, `deny_unknown_fields`, three
/// `plugin.toml` edits), so it is a design decision for a human rather than
/// something this story settles by picking one. Recorded here, next to the code it
/// is about, so the next reader finds the question rather than re-deriving it.
///
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
const TOPOLOGY_RECEIVER_TYPES: [&str; 2] = ["StreamsBuilder", "KStream"];

/// Whether `receiver` — the object a gated `stream(…)` / `to(…)` call is invoked
/// on — is a stream-topology receiver (S-408, [CR-131] §3.2 A1).
///
/// This is the **receiver gate**, and it is what lets the arm key on two method
/// names as common as `stream` and `to` at all. Without it a `to(…)` pattern
/// records a broker publish for every `Duration.to(…)` and `converter.to(…)` in
/// an arbitrary codebase — the manufacture-a-denominator failure the Rust
/// `brokers.scm` header already declines refusal slots over, and the same one
/// S-402 fixed on the HTTP arm by making candidacy receiver-grained.
///
/// It is **per-file pure and type-name based**, the same shape as
/// [`crate::extract::rust_dyn_receiver_trait`]: no cross-file type resolution, no
/// import graph. There is exactly ONE admission: the receiver is a simple name
/// the file declares, and *every* typed declaration of that name in the file
/// carries a roster type.
///
/// It requires **every** declaration rather than any, so a same-named binding of
/// another type makes the site UNDER-capture instead of over-capture
/// ([NFR-RA-05] — never fabricate is the half that matters). All four `builder`
/// declarations in the estate's `PunctuatorsPocStream` and all three
/// `streamsBuilder` declarations in `DownsamplerStream` are `StreamsBuilder`, so
/// the estate is unaffected by the stricter reading.
///
/// **A RECEIVER SPELLED LIKE A TYPE IS NOT A TYPE**, and an earlier draft of this
/// gate thought otherwise. It carried a second admission — "the receiver names a
/// topology type directly", meant for a type-qualified call like
/// `StreamsBuilder.stream(t)` — which tested the receiver's *text* against the
/// roster before consulting any declaration. That is not a weaker check, it is a
/// wrong one, and it manufactured exactly the over-capture this gate exists to
/// prevent. Measured, all four admitted and none is a topology:
///
/// ```text
///   Go    func KStream(s string) *Logger …  KStream("cfg").To("t")   → publish
///   Go    func StreamsBuilder(…) *Logger …  StreamsBuilder(c).Stream(t) → subscribe
///   Java  any unrelated class `KStream`  …  KStream.to("t")          → publish
///   Go    func f(KStream Helper)         …  KStream.To("t")          → publish
/// ```
///
/// The last row is the one that settles it: the receiver is a parameter
/// **explicitly declared `Helper`**, and the name test fired first, so a correct
/// refusal was overridden by a spelling coincidence. What the clause was written
/// for — Java's `StreamsBuilder.stream(t)` — is not a Kafka Streams idiom at all
/// (`stream` is an instance method), so it bought nothing real in exchange. The
/// gate now resolves a declared type or refuses, full stop. Pinned by
/// `a_receiver_spelled_like_a_topology_type_is_not_one`.
///
/// **Stated ceilings**, each an under-capture, each observed rather than guessed
/// (the rows are in `a_receiver_whose_type_is_not_written_in_the_file_is_refused`):
///
///   - A receiver that is not a simple name after the chain walk — a
///     parenthesised expression, a cast, an array element.
///   - A receiver whose type is declared in another file: an inherited field, an
///     interface-typed injection point.
///   - **A receiver whose type is INFERRED rather than written.** Java `var b =
///     new StreamsBuilder()`, Rust `let b = StreamsBuilder::new()`, Go
///     `b := NewStreamsBuilder()` all bind no `type:` node, so the gate sees no
///     declaration and refuses. Worth naming separately because it is not
///     uniformly rare: `:=` is the *normal* way to declare a variable in Go, so
///     the Go arm's reach is materially narrower than Java's, and
///     `plugins/go/queries/brokers.scm` says so where a Go reader will find it.
///   - A chain broken by an intervening operator the walk has no field for —
///     Rust's `builder.stream(t)?.to(u)`, where the `?` makes the `to` receiver a
///     `try_expression`. The `stream` half still binds; the `to` half is lost.
///
/// Every one errs the same way: nothing captured, so nothing fabricated
/// ([NFR-RA-05]). Widening any of them is a story with its own measurement, not a
/// quiet edit here.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
fn receiver_is_topology(receiver: Node<'_>, root: Node<'_>, source: &[u8]) -> bool {
    let Some(name) = chain_base_name(receiver, source) else {
        return false;
    };
    let declared = declared_type_names(root, &name, source);
    !declared.is_empty()
        && declared
            .iter()
            .all(|t| TOPOLOGY_RECEIVER_TYPES.contains(&t.as_str()))
}

/// The simple name a receiver chain bottoms out on, walking down the
/// receiver-of-the-receiver edge until a bare name is reached.
///
/// `streamsBuilder.stream(t).filter(f).transform(g)` — the receiver of the
/// estate's `.to(…)` — walks to `streamsBuilder`. A `this.`/`self.` qualifier
/// names the FIELD rather than the receiver object, so `this.builder.stream(t)`
/// walks to `builder`; without that arm the walk would bottom out on the `this`
/// node and refuse the ordinary Spring injected-field shape.
///
/// The descent is by **field name**, covering the three grammars this arm ships
/// queries for: Java `method_invocation`/`field_access` (`object`), Rust
/// `call_expression` (`function`) and `field_expression` (`value`), Go
/// `call_expression` (`function`) and `selector_expression` (`operand`).
fn chain_base_name(receiver: Node<'_>, source: &[u8]) -> Option<String> {
    const RECEIVER_FIELDS: [&str; 4] = ["object", "value", "operand", "function"];
    let mut node = receiver;
    // Bounded so a malformed tree can never spin: no real receiver chain is
    // anywhere near this deep, and the bound is a refusal, not a panic.
    for _ in 0..64 {
        if matches!(node.kind(), "identifier" | "field_identifier" | "type_identifier") {
            return node.utf8_text(source).ok().map(str::to_string);
        }
        let next = RECEIVER_FIELDS
            .iter()
            .find_map(|f| node.child_by_field_name(f))?;
        node = if matches!(next.kind(), "this" | "self") {
            node.child_by_field_name("field")
                .or_else(|| node.child_by_field_name("name"))?
        } else {
            next
        };
    }
    None
}

/// Every type name the file declares for the binding `name`, normalized to a
/// bare type name.
///
/// A declaration is any node that carries a `type:` field child AND binds `name`
/// — which is one rule across the three grammars rather than a per-language
/// enumeration: Java `formal_parameter`/`local_variable_declaration`/
/// `field_declaration`, Rust `parameter`/`let_declaration`, Go
/// `parameter_declaration`/`var_spec` all spell the type under `type:` and the
/// bound name under `name:`, `pattern:` or a `declarator:`'s own `name:`.
///
/// The walk is whole-file and scope-blind on purpose: the caller's rule is that
/// *every* declaration must agree, so a shadowing binding of a different type
/// refuses the site rather than being resolved to the wrong one.
///
/// **A CALLABLE IS NOT A BINDING**, and missing that cost the gate correctness in
/// both directions. Java's `method_declaration` carries a `type:` field — its
/// RETURN type — beside a `name:` field, so the rule above read
/// `private String builder() { … }` as "`builder` is declared `String`".
/// Measured consequences, both on ordinary Spring code:
///
///   - FALSE REFUSAL. `void topo(StreamsBuilder builder)` next to any helper or
///     getter named `builder()` disagreed with itself under the every-declaration
///     rule, and the whole topology went silent — captured as nothing, refused as
///     nothing, the invisible loss [NFR-CC-04] forbids. A getter named after the
///     field it returns is about as idiomatic as Java gets.
///   - FALSE ADMISSION. A receiver declared in no binding at all was admitted
///     because a *method* of that name returned a roster type — exactly the
///     inherited-field case [`receiver_is_topology`]'s ceilings promise to refuse.
///
/// The discriminator is STRUCTURAL, not a list of node kinds: a declaration that
/// also carries a `parameters:` field declares a callable, and its `type:` is a
/// return type rather than the type of a binding. That holds across the three
/// grammars without naming any of them, and it needs no extension when a fourth
/// ships a brokers query — unlike a kind deny-list, which is the closed selector a
/// later language silently falls out of. (Go and Rust spell a return type
/// `result:` / `return_type:` and never reached this, so the bug was Java's alone;
/// the guard is written once for all of them anyway.) Pinned by
/// `a_method_named_like_the_receiver_is_not_a_declaration_of_it`.
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
fn declared_type_names(root: Node<'_>, name: &str, source: &[u8]) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if let Some(ty) = node.child_by_field_name("type") {
            // A callable declaration's `type:` is its RETURN type — never the
            // type of the name it binds. See the note above; this one condition
            // is the whole guard.
            if node.child_by_field_name("parameters").is_none() && binds_name(node, name, source) {
                if let Some(t) = bare_type_name(ty, source) {
                    found.push(t);
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    found
}

/// Whether a declaration node binds `name`, by any of the three spellings the
/// shipped grammars use for the declared name: a `name:` field (Java
/// `formal_parameter`, Go `parameter_declaration`/`var_spec` — both of which may
/// carry several), a `pattern:` field (Rust), or a `declarator:`'s own `name:`
/// (Java `local_variable_declaration` / `field_declaration`).
fn binds_name(node: Node<'_>, name: &str, source: &[u8]) -> bool {
    let text_is = |n: Node<'_>| n.utf8_text(source).map(|t| t == name).unwrap_or(false);
    let mut cursor = node.walk();
    for field in ["name", "pattern"] {
        if node.children_by_field_name(field, &mut cursor).any(text_is) {
            return true;
        }
    }
    let declarators: Vec<Node<'_>> = node.children_by_field_name("declarator", &mut cursor).collect();
    declarators
        .iter()
        .filter_map(|d| d.child_by_field_name("name"))
        .any(text_is)
}

/// A declared type node's **bare** type name: generics, pointer/reference
/// sigils, `mut`/`dyn` qualifiers and any package or module qualifier removed.
///
/// `KStream<String, VolumeCounters>` → `KStream`, `&mut StreamsBuilder` →
/// `StreamsBuilder`, `*kafka.StreamsBuilder` → `StreamsBuilder`. A type whose
/// head is not a name (a slice, a tuple, a function type) yields `None`, which
/// the caller reads as "declared, but not a roster type" — a refusal.
///
/// **A bracket anywhere refuses.** Go's `[]StreamsBuilder` and `map[string]KStream`
/// and Rust's `[StreamsBuilder; 3]` already fell out as `None` because their head
/// is empty or `map`; Java's `StreamsBuilder[]` did NOT, because its bracket is a
/// suffix — so an array of builders was admitted as a builder. A collection of
/// topology objects is not a topology receiver in any of the three languages, and
/// refusing on the bracket makes that one rule instead of three accidents.
fn bare_type_name(ty: Node<'_>, source: &[u8]) -> Option<String> {
    let raw = ty.utf8_text(source).ok()?;
    if raw.contains('[') {
        return None; // an array/slice/map/index type is not a topology receiver
    }
    let head = raw.split(['<', '(']).next()?;
    let head = head.trim_matches(|c: char| c == '*' || c == '&' || c.is_whitespace());
    let last_token = head.split_whitespace().last()?;
    let bare = last_token.rsplit(['.', ':']).next()?.trim();
    (!bare.is_empty()).then(|| bare.to_string())
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


// ── THE TWO-FRAME WRAPPER HOP (S-417, [FR-WS-26], [CR-131] §3.2 C2) ─────────
//
// Read the accessor hop above first: this is that hop's population **minus**
// the part it can prove. A publish site whose topic operand is a
// `@ConfigurationProperties` accessor resolves inside one file. A site whose
// operand is a bare **parameter** of the method that encloses it proves nothing
// in that file at all — the topic is written at the wrapper's callers, one or
// two call frames up, and [FR-WS-19]'s accessor chain names it as one of its
// nine faults.
//
// S-392 measured that residue at one frame and FALSIFIED it (0 of 13 production
// publish sites, floor 7), which is why [FR-WS-23] is WITHDRAWN in place. S-416
// re-measured the SAME population at two frames and it HOLDS (8 of 13, floor
// 7). This section spends that verdict, and nothing wider: **two frames on this
// idiom, and no dataflow**. [CR-131] §7 records "the two-frame bound is read as
// a licence for general dataflow" as a risk against [ADR-64]; the bound is in
// [FR-WS-26]'s acceptance criteria, not in this comment.
//
// # Why it cannot live where the accessor hop lives
//
// The accessor hop is taken at the refusal slot, inside `extract_one`, because
// everything it needs is in one parse tree. This hop needs the wrapper's
// **callers**, which are in other files by construction, so it runs once per
// pass in [`resolve_forwarded_topics`] — after the parallel map, over the facts
// that map produced. The shape is [`crate::extract::config::binding::PropertiesIndex`]'s:
// a cross-file answer assembled from the text the pass has already read, adding
// no file IO of its own ([FR-WS-19] AC7's rule, applied to a second index).
//
// # Direction from the ledger, operand from the file
//
// The hop asks two different questions and takes each from the place that can
// answer it, which is the whole of why it is affordable:
//
// * **Which files call the wrapper?** — the `Calls` ledger this pass has just
//   produced. Its target text is a bare method name and nothing else, so the
//   answer is a superset (every file calling *any* method of that name), which
//   is exactly the right direction for a work list: it may open a file that
//   proves nothing, never miss one that proves something.
// * **What does the caller pass?** — re-read from the named file's own parse
//   tree. The ledger cannot answer this; it records no operand. That split is
//   pinned on the estate by S-416's `the_ledger_answers_the_direction_and_never_the_operand`.
//
// # What it refuses, and where each refusal is written down
//
// [FR-WS-26]'s acceptance criteria, not this comment, fix the bound. Named here
// only so the code below is readable: a third frame; a caller outside the build
// module; callers that disagree; a caller passing anything the file does not
// prove (a bare variable, a folded constant, a runtime expression); and a
// wrapper whose signature is varargs or carries an explicit receiver, refused
// before any slot arithmetic because the positional correspondence the hop rests
// on does not hold for either. A call site in a **test** tree neither admits nor
// vetoes: it is excluded before the agreement is taken, which is the
// `src/main`-only reading S-416's decisive figure is stated over.
//
// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
// [FR-WS-23]: ../../../docs/specs/requirements/FR-WS-23.md
// [FR-WS-26]: ../../../docs/specs/requirements/FR-WS-26.md

/// A method the hop looks up, keyed the only two ways a positional lookup can
/// be: by the name the `Calls` ledger carries, and by the arity that makes the
/// slot index meaningful.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Callee {
    /// The method's simple name.
    pub name: String,
    /// The number of parameters it declares, comments and an explicit receiver
    /// excluded (see [`parameter_slots`]).
    pub arity: usize,
}

/// A publish or subscribe site whose topic operand is a bare parameter of the
/// method enclosing it — the population [FR-WS-26] is about.
///
/// Recorded by [`capture_broker_invocations`] at the refusal slot, for a site
/// whose keyless `topic-not-literal` row actually **survived** the reconcile,
/// and decided later by [`resolve_forwarded_topics`]. A site recorded here has
/// already been reported refused; the hop can only retract that refusal, never
/// add one.
///
/// [FR-WS-26]: ../../../docs/specs/requirements/FR-WS-26.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardingCandidate {
    /// Which side of the arm the refused site is.
    pub relation: ArtifactRelation,
    /// The declaration the refusal was attributed to — the wrapper itself.
    pub source: LogosSymbol,
    /// The 1-based line the refusal was reported at.
    pub line: u32,
    /// The wrapper's `(name, arity)`, the signature a caller is matched on.
    pub wrapper: Callee,
    /// The positional slot the topic operand occupies in that signature.
    pub slot: usize,
    /// The simple name of the type declaring the wrapper, for the receiver rule
    /// ([`dispatches_past`]). `None` inside an anonymous class body.
    pub declaring_class: Option<String>,
    /// `false` when the wrapper is varargs or declares an explicit receiver.
    ///
    /// Decided where the parameter list is — at capture time — and carried
    /// rather than re-derived, because the hop runs after the parse tree is
    /// gone. A candidate is still **recorded** when this is `false`, so the
    /// refusal is reported as [`ForwardingRefusal::UnsupportedSignature`]
    /// instead of the site vanishing from the population it belongs to.
    pub signature_supported: bool,
    /// How many keyless refusal rows this file recorded for this
    /// `(relation, source)` pair.
    ///
    /// The ledger collapses them to **one** row, so the hop may retract it only
    /// once every one of them has resolved. Carried on the candidate because the
    /// count is knowable only where the refusals are raised, and the hop runs
    /// after the parse tree is gone.
    pub refusals_at_source: usize,
    /// What the hop decided. `None` when the hop did not run — the single-file
    /// [`crate::extract::extract`] entry point, which cannot answer a cross-file
    /// question and says so rather than reporting a refusal it never tested.
    pub outcome: Option<ForwardingOutcome>,
}

/// What the two-frame hop proved about one [`ForwardingCandidate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardingOutcome {
    /// The callers agree, and this is the topic they agree on.
    Resolved(Forwarded),
    /// They do not, or there are none this reading admits.
    Refused(ForwardingRefusal),
}

/// A resolved forwarded topic and the chain that proved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forwarded {
    /// The key the site is now stored under: the canonical `${prefix.key}`
    /// placeholder when the caller passed a `@ConfigurationProperties` accessor
    /// (the same form the in-file accessor hop stores, so the two cannot produce
    /// two spellings of one topic), or the literal's own text when the caller
    /// wrote one.
    pub topic: String,
    /// The hop chain, innermost frame first: the wrapper itself, and then every
    /// method whose callers the second frame read.
    ///
    /// **Length is not a frame count.** One entry means one frame, but two
    /// admitted call sites can forward into two DIFFERENT methods and both
    /// resolve at the second frame, which lists three entries for a two-frame
    /// resolution. The bound is enforced by not taking a third frame (see
    /// [`ForwardingRefusal::ThreeOrMoreFrames`]), never by this length, and a
    /// reader must not treat it as the depth.
    ///
    /// This is the provenance [NFR-CC-04] asks for on an admitted value: a topic
    /// on this path was *proved from another file*, and a surface that cannot
    /// tell it from one written at the site would be presenting an inference as
    /// an observation.
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub chain: Vec<String>,
}

/// Why a [`ForwardingCandidate`] stayed refused. Every variant emits no
/// reference and leaves the keyless `topic-not-literal` row exactly as the
/// capture pass wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardingRefusal {
    /// The wrapper is varargs, or declares an explicit receiver: the positional
    /// correspondence between a parameter slot and an argument slot does not
    /// hold, so there is no arithmetic to do.
    UnsupportedSignature,
    /// Nothing this reading admits calls it: no call site at all, or none left
    /// after the module and `src/main` rules, or none whose receiver is the type
    /// declaring it.
    NoCallSite,
    /// It is called, and every caller is outside the wrapper's build module.
    OutOfModule,
    /// Admitted callers pass operands that do not agree. Never averaged.
    Disagree,
    /// An admitted caller passes something this pass cannot prove — a bare
    /// variable, a constant, a runtime expression, a `Mockito.any()` stub in a
    /// file the test-tree rule did not exclude.
    ///
    /// A wrapper reached only by a **method reference** lands here rather than on
    /// [`NoCallSite`](Self::NoCallSite), and the distinction is the point: a
    /// `Foo::bar` IS an observed call site, it simply supplies no argument, so it
    /// vetoes the agreement instead of being absent from it. Pinned by the
    /// method-reference row of
    /// `each_named_forwarding_refusal_is_pinned_on_its_own_tree`.
    UnresolvableOperand,
    /// The chain is longer than the bound: an admitted caller forwards a
    /// parameter of its own, whose own callers forward again.
    ThreeOrMoreFrames,
}

impl ForwardingRefusal {
    /// The stable token a report or a fixture names this refusal by.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedSignature => "unsupported-signature",
            Self::NoCallSite => "no-call-site",
            Self::OutOfModule => "out-of-module",
            Self::Disagree => "disagree",
            Self::UnresolvableOperand => "unresolvable-operand",
            Self::ThreeOrMoreFrames => "three-or-more-frames",
        }
    }
}

/// Every call site of a named method, **and every method reference** to one.
///
/// Compiled against each plugin's own grammar, which is also the admission rule:
/// a language whose grammar spells none of these node kinds fails
/// [`Query::new`] and contributes nothing — absence, not error ([NFR-MA-01]).
/// The name is filtered in Rust rather than by a `#match?` predicate because the
/// wanted set is only known at runtime.
///
/// The reference arm is the direction in which this could over-read.
/// `list.forEach(producer::sendMessage)` reaches the wrapper and proves nothing
/// about its topic, so it must **veto** the agreement rather than be skipped —
/// skipping it would let a module containing one report the wrapper resolved off
/// its other call sites alone.
///
/// # This half of the hop is Java-only today — stated, not implied
///
/// `method_invocation` and `method_reference` are Java's spellings. Go and Rust
/// spell a call `call_expression`, so [`Query::new`] fails for both and
/// [`observe_calls`] returns immediately: **no Go or Rust file can ever supply a
/// caller**, and the whole cross-file half of the hop is Java-only in practice
/// rather than merely "absent until a grammar ships the capture".
///
/// The capture half is not query-gated and walks node kinds directly, and Go
/// happens to spell `method_declaration` the same way — so a Go wrapper IS
/// recorded as a [`ForwardingCandidate`] and then always refuses
/// [`ForwardingRefusal::NoCallSite`]. That is wasted work, not a wrong answer,
/// and [NFR-RA-05] holds either way; it is written down because the module
/// header's "no per-language branching" claim ([NFR-MA-01]) is about the
/// *receiver gate* above and must not be read as covering this.
///
/// Two latent correspondence gaps sit behind that dead path and would matter the
/// moment a grammar made it live: Go's varargs node is
/// `variadic_parameter_declaration`, which [`unsupported_signature`] does not
/// name, and Go's comma-grouped `func f(a, b int)` puts two names in one
/// parameter node, which [`parameter_slots`] counts as one slot. Both are
/// recorded here rather than guessed at, because the safety net that makes them
/// inert today is the query gate above and nothing stronger.
///
/// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
const FORWARDING_CALL_QUERY: &str = r"
[
  (method_invocation
    name: (identifier) @call.name
    arguments: (argument_list) @call.args)
  (method_reference) @call.ref
]
";

/// What one positional argument at one call site proves.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ArgValue {
    /// A committed key: the canonical `${prefix.key}` an accessor resolved to,
    /// or the text of a string literal written at the call site.
    Proved(String),
    /// A bare parameter of the calling method — one frame further out.
    Forwarded(Forward),
    /// Anything the file does not prove: a bare variable, a constant, a
    /// concatenation, a `Mockito.any()` stub, a runtime expression.
    Unprovable,
}

/// Where the next frame must look, for an argument that was itself a parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Forward {
    callee: Callee,
    slot: usize,
    /// The simple name of the type declaring the forwarding method. A method in
    /// an **anonymous** class has none, and there is then no receiver to compare
    /// a `super.m(…)` against, so [`forward_from`] declines the forward outright
    /// rather than following it under a rule it cannot apply.
    declaring_class: String,
}

/// One observed call site of a looked-up method, with the arguments at the
/// wanted slots already reduced to what they prove.
struct Observation {
    callee: Callee,
    module: String,
    main_tree: bool,
    values: BTreeMap<usize, ArgValue>,
    /// `true` when the call was written `super.m(…)`.
    receiver_is_super: bool,
    /// For such a call, the simple name of the class it dispatches to.
    super_dispatches_to: Option<String>,
    /// The simple name of the type this call's receiver is **declared** with,
    /// when the calling file declares it and the receiver is a shape this pass
    /// can read. [`None`] means *unknown*, never *unrelated* — the distinction is
    /// the whole of [`declares_a_different_receiver`]'s safety argument.
    receiver_type: Option<String>,
}

/// The build modules a set of project-relative paths declares, and which one
/// owns a path.
///
/// The roots are read from the **module descriptor** basenames
/// [`crate::extract::config::corpus::MODULE_DESCRIPTORS`] already names, so the
/// hop's notion of "the same build module" is the one the configuration corpus
/// uses and not a second one free to disagree with it. It is a path-only answer:
/// no descriptor is opened, and a tree declaring none is one module rooted at
/// `""`.
struct ModuleMap {
    roots: Vec<String>,
}

impl ModuleMap {
    fn from_paths<'p>(paths: impl IntoIterator<Item = &'p str>) -> Self {
        let mut roots: Vec<String> = paths
            .into_iter()
            .filter(|rel| {
                let name = rel.rsplit('/').next().unwrap_or(rel);
                crate::extract::config::corpus::MODULE_DESCRIPTORS.contains(&name)
            })
            .map(|rel| rel.rfind('/').map_or(String::new(), |i| rel[..i].to_string()))
            .collect();
        roots.sort();
        roots.dedup();
        Self { roots }
    }

    /// The longest declared module root that is a path-segment prefix of `rel`,
    /// or `""` when none is.
    fn module_of<'s>(&'s self, rel: &str) -> &'s str {
        self.roots
            .iter()
            .filter(|m| m.is_empty() || rel.starts_with(&format!("{m}/")))
            .max_by_key(|m| m.len())
            .map_or("", String::as_str)
    }
}

/// The named children of an argument or parameter list, **minus comments and an
/// explicit receiver**.
///
/// Tree-sitter counts a comment as a named sibling, so an unfiltered
/// `named_children` shifts every slot after a documented argument by one and the
/// hop would read the *wrong* positional operand — worse than reading none. The
/// predicate is `ends_with("comment")`, not `== "comment"`: tree-sitter-java
/// spells them `line_comment` and `block_comment`, so an exact test filters
/// neither. Both facts are S-416's, found by running its harness rather than by
/// reading it.
fn parameter_slots<'t>(list: Node<'t>) -> Vec<Node<'t>> {
    let mut cursor = list.walk();
    list.named_children(&mut cursor)
        .filter(|n| !n.kind().ends_with("comment") && n.kind() != "receiver_parameter")
        .collect()
}

/// `true` when a parameter list admits no positional lookup at all: it declares
/// a varargs parameter, or an explicit receiver.
///
/// Both break the correspondence the hop rests on — one argument per parameter,
/// in order. A varargs slot absorbs any number of arguments, so the argument at
/// index *i* need not be the parameter at index *i*; an explicit receiver
/// occupies a parameter slot that **no** argument ever supplies. [FR-WS-26]
/// refuses the signature rather than guessing which of the two readings the call
/// site meant.
///
/// [FR-WS-26]: ../../../docs/specs/requirements/FR-WS-26.md
fn unsupported_signature(list: Node<'_>) -> bool {
    let mut cursor = list.walk();
    let unsupported = list
        .named_children(&mut cursor)
        .any(|n| n.kind() == "spread_parameter" || n.kind() == "receiver_parameter");
    drop(cursor);
    unsupported
}

/// A parameter's declared name, across the shapes a grammar writes one in: a
/// `formal_parameter` with a `name` field, a varargs parameter whose name hangs
/// off a `variable_declarator`, and a lambda's bare `identifier`.
fn parameter_name(param: Node<'_>, src: &[u8]) -> Option<String> {
    if param.kind() == "identifier" {
        return param.utf8_text(src).ok().map(str::to_string);
    }
    if let Some(name) = param.child_by_field_name("name") {
        return name.utf8_text(src).ok().map(str::to_string);
    }
    let mut cursor = param.walk();
    let declarator = param
        .named_children(&mut cursor)
        .find(|c| c.kind() == "variable_declarator");
    drop(cursor);
    declarator
        .and_then(|d| d.child_by_field_name("name"))
        .and_then(|n| n.utf8_text(src).ok())
        .map(str::to_string)
}

/// The positional slot `name` occupies in `scope`'s parameter list, and the
/// list's length — `None` when `scope` declares no such parameter.
fn parameter_slot(scope: Node<'_>, name: &str, src: &[u8]) -> Option<(usize, usize)> {
    let params = scope.child_by_field_name("parameters")?;
    // A single-parameter lambda written without parentheses — `topic -> …` — puts
    // a bare `identifier` in the `parameters` field, with no list around it. It
    // has to be recognised HERE, or `declaring_scope` walks past the lambda and
    // attributes the name to the enclosing method's parameter list: a different
    // list, a different arity, and a slot index that means nothing. Found by
    // writing the negative fixture, not by reading the grammar.
    if params.kind() == "identifier" {
        return (params.utf8_text(src).ok()? == name).then_some((0, 1));
    }
    let list = parameter_slots(params);
    let index = list
        .iter()
        .position(|p| parameter_name(*p, src).as_deref() == Some(name))?;
    Some((index, list.len()))
}

/// The innermost declaration that binds `name` as a parameter.
///
/// A lambda is in the list deliberately: `(topic) -> producer.send(topic)` binds
/// `topic` at the lambda, not at the enclosing method, and stopping at the
/// method would read the wrong parameter list and the wrong slot. A lambda scope
/// is not a method call site, so the hop then refuses — but it refuses knowing
/// what bound the name.
fn declaring_scope<'t>(node: Node<'t>, name: &str, src: &[u8]) -> Option<Node<'t>> {
    let mut current = node.parent();
    while let Some(scope) = current {
        if matches!(
            scope.kind(),
            "method_declaration" | "constructor_declaration" | "lambda_expression"
        ) && parameter_slot(scope, name, src).is_some()
        {
            return Some(scope);
        }
        current = scope.parent();
    }
    None
}

/// `true` for the body of an anonymous class — `new Foo() { … }`. Shared with
/// receiver typing (S-467), whose `this` stops at the same body.
pub(super) fn anonymous_class_body(node: Node<'_>) -> bool {
    node.kind() == "class_body"
        && node
            .parent()
            .is_some_and(|p| p.kind() == "object_creation_expression")
}

/// The simple name of the type declaration enclosing `node`, or `None` inside an
/// anonymous class body.
///
/// The walk **stops** at an anonymous body rather than passing through it: an
/// anonymous class has no name a `super.m()` written elsewhere can target, and
/// climbing past it attributes the method to whatever the OUTER class declares.
fn enclosing_type_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(scope) = current {
        if anonymous_class_body(scope) {
            return None;
        }
        if matches!(
            scope.kind(),
            "class_declaration" | "enum_declaration" | "record_declaration" | "interface_declaration"
        ) {
            return scope
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(src).ok())
                .map(str::to_string);
        }
        current = scope.parent();
    }
    None
}

/// A type node's simple name, through the two wrappers a grammar puts around one
/// in an `extends` clause: `Foo<K, V>` is a `generic_type` whose FIRST named
/// child is the identifier, `a.b.Foo` a `scoped_type_identifier` whose LAST is.
fn simple_type_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" | "identifier" => node.utf8_text(src).ok().map(str::to_string),
        "generic_type" | "scoped_type_identifier" => {
            let mut cursor = node.walk();
            let children: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
            drop(cursor);
            let pick = if node.kind() == "generic_type" {
                children.first()
            } else {
                children.last()
            };
            pick.and_then(|n| simple_type_name(*n, src))
        }
        _ => None,
    }
}

/// The simple name of the class a `super.m(…)` at `node` dispatches to: what the
/// enclosing type extends, or — inside an anonymous class — the type being
/// instantiated. `None` when there is no `extends` clause at all.
fn enclosing_superclass_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(scope) = current {
        if anonymous_class_body(scope) {
            let created = scope.parent()?.child_by_field_name("type")?;
            return simple_type_name(created, src);
        }
        if scope.kind() == "class_declaration" {
            let extends = scope.child_by_field_name("superclass")?;
            let mut cursor = extends.walk();
            let first = extends.named_children(&mut cursor).next();
            drop(cursor);
            return first.and_then(|t| simple_type_name(t, src));
        }
        current = scope.parent();
    }
    None
}

/// A call site that is **not** a caller of the method being looked up, although
/// its name and arity match — [FR-WS-26]'s *receiver-aware*.
///
/// One shape, decidable from the tokens rather than from a type: a `super.m(…)`
/// written inside `class C extends B` calls **`B.m`**. It is a caller of the
/// looked-up method only when that method is declared in `B` — never when it is
/// declared in `C` itself, and never when it is declared in a SIBLING subclass
/// of `B`.
///
/// Both halves were found by S-416 running against the estate, not by reading
/// it, and each cost the measurement a wrong figure: without the first, a
/// `super.sendMessage(…)` inside the very override being looked up reads that
/// override's own parameter and reports three frames (0 of 13, every named site
/// mis-filed); without the second, two sibling producers extending one base each
/// pick up the other's `super` line as a phantom caller (8 of 13 with five sites
/// still wrong). Neither conjunct is a proxy for a type, and the cases that must
/// stay in do: a recursive `m()` or `this.m()` has no `super` receiver, and the
/// legitimate `D.send` → `super.send` → `C.send` chain has
/// `super_dispatches_to == Some("C")`, which is the method being looked up.
///
/// **Applied at both frames**, unlike S-416's harness, which applies it where it
/// found the fault. Nothing about the fault is frame-two-specific — a phantom
/// caller at frame one over-reads exactly the same way — and the rule can only
/// remove callers, so applying it wider cannot admit a site the harness refused.
///
/// [FR-WS-26]: ../../../docs/specs/requirements/FR-WS-26.md
fn dispatches_past(observation: &Observation, declaring_class: Option<&str>) -> bool {
    observation.receiver_is_super && observation.super_dispatches_to.as_deref() != declaring_class
}

/// A call site whose receiver is declared with a type that is **not** the one
/// declaring the method being looked up — so it calls a homonym, not this method.
///
/// [`Callee`] is `(name, arity)`, which is all the `Calls` ledger's target text
/// can express, so without this rule any same-signature call in the module is
/// admitted as a caller. That is not a theoretical over-read; it fabricates.
/// Reproduced during review: a module declaring `KafkaProducer.sendMessage/3`
/// (the wrapper, with **no** callers) beside an unrelated
/// `AuditProducer.sendMessage/3` resolved the wrapper's publish site to
/// `"audit-log"` — the topic passed to the *other* class — and then retracted the
/// keyless row, so the fabricated key was the only thing left ([NFR-RA-05]). The
/// same shape cuts the other way: a homonym beside a genuine accessor caller
/// turned a correct resolution into `Disagree`.
///
/// **A veto, and only ever a veto.** `None` — a receiver whose type this file does
/// not declare, a chained call, an injected field declared in a supertype — is
/// *unknown*, and unknown stays admitted. So the rule can only remove call sites,
/// never add one, which is the same safety argument [`dispatches_past`] makes:
/// the worst it can do is refuse something that would have resolved, and
/// refusing is the side of [NFR-RA-05] this whole arm sits on.
///
/// It is deliberately **not** a type check. It compares two simple names read off
/// the same file, with no inheritance knowledge — a caller holding a declared
/// **base** type is therefore vetoed for the *subclass*'s override and admitted
/// for the base's own declaration, which is exactly what static dispatch says and
/// is what makes the two-frame chain land on the right frame.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn declares_a_different_receiver(observation: &Observation, declaring_class: Option<&str>) -> bool {
    match (observation.receiver_type.as_deref(), declaring_class) {
        (Some(receiver), Some(declarer)) => receiver != declarer,
        // Unknown on either side: nothing to compare, so nothing is vetoed.
        _ => false,
    }
}

/// What one positional argument proves, in the order the questions must be
/// asked.
///
/// **A parameter is decided first**, and that precedence is load-bearing rather
/// than stylistic: a method parameter shadowing a same-named field would
/// otherwise resolve to the *field's* binding and the hop would key the site on
/// a value the call site never passed ([NFR-RA-05]).
///
/// Then the accessor chain — the same [`BindingView::placeholder_for`] the
/// in-file accessor hop takes, so a topic proved here and a topic proved there
/// are stored in one spelling — and then a string literal written at the call
/// site. Everything else is unprovable, and that deliberately includes a
/// same-unit `static final` constant: folding one is evidence this pass does not
/// gather, and [FR-WS-26] refuses a bare variable rather than guessing.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [FR-WS-26]: ../../../docs/specs/requirements/FR-WS-26.md
fn arg_value(node: Node<'_>, src: &[u8], binding: Option<&BindingView<'_>>) -> ArgValue {
    if node.kind() == "identifier" {
        return match forward_from(node, src) {
            Some(forward) => ArgValue::Forwarded(forward),
            // A bare name that no enclosing parameter list binds is a field, a
            // local or a constant: unprovable here by the paragraph above.
            None => ArgValue::Unprovable,
        };
    }
    if let Some(placeholder) = binding.and_then(|b| b.placeholder_for(node, src)) {
        return ArgValue::Proved(placeholder);
    }
    if node.kind().ends_with("string_literal") {
        if let Some(text) = literal_text(node, src) {
            return ArgValue::Proved(text);
        }
    }
    ArgValue::Unprovable
}

/// The next frame's lookup target for a forwarded argument, or `None` when there
/// is no method call site to follow.
///
/// `None` covers three shapes and they are all the same answer: a name bound by
/// a constructor, a name bound by a lambda, and a name bound by a method whose
/// signature [`unsupported_signature`] refuses. None of them is a positional
/// method call site, so the chain stops and the argument is unprovable.
fn forward_from(argument: Node<'_>, src: &[u8]) -> Option<Forward> {
    let name = argument.utf8_text(src).ok()?;
    let scope = declaring_scope(argument, name, src)?;
    if scope.kind() != "method_declaration" {
        return None;
    }
    let params = scope.child_by_field_name("parameters")?;
    if unsupported_signature(params) {
        return None;
    }
    let (slot, arity) = parameter_slot(scope, name, src)?;
    let method = scope
        .child_by_field_name("name")?
        .utf8_text(src)
        .ok()?
        .to_string();
    Some(Forward {
        callee: Callee { name: method, arity },
        slot,
        declaring_class: enclosing_type_name(scope, src)?,
    })
}

/// Read every call site of a wanted method out of one already-read source,
/// resolving the argument at each wanted slot as it goes.
///
/// The argument's value is read **here**, at the only point where its AST node
/// and its own file's [`BindingView`] are both in hand. A later pass holding
/// only the ledger could not: the ledger records a call's target name and never
/// its operands.
#[allow(clippy::too_many_arguments)]
fn observe_calls(
    rel: &str,
    text: &str,
    plugin: &dyn LanguagePlugin,
    modules: &ModuleMap,
    properties: &PropertiesIndex,
    want: &BTreeMap<Callee, BTreeSet<usize>>,
    names: &BTreeSet<String>,
    out: &mut Vec<Observation>,
) {
    let mut parser = Parser::new();
    if parser.set_language(plugin.language()).is_err() {
        return;
    }
    let Some(tree) = parser.parse(text, None) else {
        return;
    };
    let Ok(query) = Query::new(plugin.language(), FORWARDING_CALL_QUERY) else {
        return;
    };
    let root = tree.root_node();
    let src = text.as_bytes();
    let binding = super::accessor_binding(plugin, root, src, properties);
    // The receiver-type view, built unconditionally — unlike the binding view
    // above, whose `!properties.is_empty()` guard is a cost guard for a member
    // with no configuration-bound class. This one has to answer for EVERY module,
    // because `declares_a_different_receiver` is what stops a homonymous method
    // fabricating a topic, and a member with no `@ConfigurationProperties` class
    // is no less exposed to that. It is the same walk `accessor_binding` makes
    // when it does build a view; paying it twice on those files is the price of
    // not threading a prebuilt index through two call sites in `extract::mod`.
    let declared_types = DeclaredTypes::build(root, src);
    let module = modules.module_of(rel).to_string();
    let main_tree = !crate::navigate::is_test_path(rel);

    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        let mut name_node = None;
        let mut args_node = None;
        let mut ref_node = None;
        for cap in m.captures {
            match capture_names[cap.index as usize] {
                "call.name" => name_node = Some(cap.node),
                "call.args" => args_node = Some(cap.node),
                "call.ref" => ref_node = Some(cap.node),
                _ => {}
            }
        }
        // A `Foo::bar` reference reaches the wrapper and supplies no argument.
        // Its arity is unknown, so it is recorded against EVERY wanted arity of
        // that name with an unprovable value at every wanted slot: it vetoes the
        // agreement rather than being absent from it.
        if let Some(reference) = ref_node {
            let Ok(text) = reference.utf8_text(src) else {
                continue;
            };
            let Some(name) = text.rsplit("::").next().map(str::trim) else {
                continue;
            };
            if !names.contains(name) {
                continue;
            }
            for (callee, slots) in want.iter().filter(|(c, _)| c.name == name) {
                out.push(Observation {
                    callee: callee.clone(),
                    module: module.clone(),
                    main_tree,
                    values: slots.iter().map(|s| (*s, ArgValue::Unprovable)).collect(),
                    receiver_is_super: false,
                    super_dispatches_to: None,
                    // `Foo::bar` names a type, but the reference proves nothing
                    // about the operand either way — it is already recorded as
                    // unprovable, so narrowing it further would change nothing.
                    receiver_type: None,
                });
            }
            continue;
        }
        let (Some(name_node), Some(args_node)) = (name_node, args_node) else {
            continue;
        };
        let Ok(name) = name_node.utf8_text(src) else {
            continue;
        };
        if !names.contains(name) {
            continue;
        }
        let arguments = parameter_slots(args_node);
        let callee = Callee { name: name.to_string(), arity: arguments.len() };
        let Some(wanted_slots) = want.get(&callee) else {
            continue;
        };
        let values: BTreeMap<usize, ArgValue> = wanted_slots
            .iter()
            .map(|s| {
                let value = arguments
                    .get(*s)
                    .map_or(ArgValue::Unprovable, |a| arg_value(*a, src, binding.as_ref()));
                (*s, value)
            })
            .collect();
        let receiver_is_super = name_node
            .parent()
            .and_then(|call| call.child_by_field_name("object"))
            .is_some_and(|object| object.kind() == "super");
        let super_dispatches_to = receiver_is_super
            .then(|| enclosing_superclass_name(name_node, src))
            .flatten();
        out.push(Observation {
            callee,
            module: module.clone(),
            main_tree,
            values,
            receiver_is_super,
            super_dispatches_to,
            receiver_type: receiver_type(name_node, src, &declared_types),
        });
    }
}

/// The simple type a call's receiver is declared with, or [`None`] when this
/// file does not say.
///
/// Four shapes, and the answer for three of them is deliberately `None`:
///
/// * **no receiver** — `sendMessage(…)`, an implicit `this` — is the enclosing
///   type. Read from the tree, not guessed.
/// * **a bare identifier** — `producer.sendMessage(…)` — is what the file
///   declares that name as. [`DeclaredTypes::get`] is scope-blind and poisons a
///   disagreement, so it answers only where the file is unambiguous.
/// * **`super`** is [`dispatches_past`]'s question, not this one, and answering
///   it here would apply two rules to one call.
/// * **anything else** — `this.producer`, a chained call, a qualified name — is
///   `None`, which admits the call site. Narrowing those needs the self-reference
///   vocabulary [`crate::extract::config::accessor`] owns, and reaching for it
///   here would cost this function that module's `PropertiesIndex`, which a
///   member with no bound class does not have.
///
/// `None` means unknown and never unrelated; see
/// [`declares_a_different_receiver`] for why that asymmetry is what makes the
/// rule safe.
fn receiver_type(
    name_node: Node<'_>,
    src: &[u8],
    declared: &DeclaredTypes,
) -> Option<String> {
    let call = name_node.parent()?;
    let Some(receiver) = call.child_by_field_name("object") else {
        // An unqualified call: the receiver is the enclosing type itself.
        return enclosing_type_name(name_node, src);
    };
    if receiver.kind() != "identifier" {
        return None;
    }
    declared
        .get(receiver.utf8_text(src).ok()?)
        .map(str::to_string)
}

/// Group observations by the method they call.
fn index_by_callee(observations: &[Observation]) -> BTreeMap<&Callee, Vec<&Observation>> {
    let mut out: BTreeMap<&Callee, Vec<&Observation>> = BTreeMap::new();
    for o in observations {
        out.entry(&o.callee).or_default().push(o);
    }
    out
}

/// The call sites of `callee` this reading admits for a caller in `module`:
/// same build module, `src/main` only, and the receiver rule applied.
///
/// Returns the structural refusal instead when the reading admits none, so the
/// three causes — never called, called only across the boundary, called only
/// from the test tree — are named apart rather than collapsed into one absence.
fn admitted_call_sites<'o>(
    callee: &Callee,
    module: &str,
    declaring_class: Option<&str>,
    index: &BTreeMap<&Callee, Vec<&'o Observation>>,
) -> Result<Vec<&'o Observation>, ForwardingRefusal> {
    let all: Vec<&Observation> = index
        .get(callee)
        .map(|sites| {
            sites
                .iter()
                .copied()
                .filter(|o| {
                    !dispatches_past(o, declaring_class)
                        && !declares_a_different_receiver(o, declaring_class)
                })
                .collect()
        })
        .unwrap_or_default();
    if all.is_empty() {
        return Err(ForwardingRefusal::NoCallSite);
    }
    let in_module: Vec<&Observation> =
        all.iter().copied().filter(|o| o.module == module).collect();
    if in_module.is_empty() {
        return Err(ForwardingRefusal::OutOfModule);
    }
    let admitted: Vec<&Observation> =
        in_module.iter().copied().filter(|o| o.main_tree).collect();
    if admitted.is_empty() {
        // Every caller is in this module and every one is in a test tree. A test
        // call site neither admits nor vetoes, so the main-tree reading is left
        // with no call site at all — which is what it reports.
        return Err(ForwardingRefusal::NoCallSite);
    }
    Ok(admitted)
}

/// Reduce the values a set of admitted call sites supply at one slot to a single
/// proved key, or to the refusal that stopped them.
///
/// The precedence is **declared, not incidental**, and it is the one S-416's
/// harness applies: an unfinished chain outranks an unprovable operand, and both
/// outrank disagreement — because two sites, one of which has not finished
/// resolving, are not yet known to disagree at all.
fn agree_on(values: &[ArgValue]) -> Result<String, ForwardingRefusal> {
    if values.iter().any(|v| matches!(v, ArgValue::Forwarded(_))) {
        return Err(ForwardingRefusal::ThreeOrMoreFrames);
    }
    if values.contains(&ArgValue::Unprovable) {
        return Err(ForwardingRefusal::UnresolvableOperand);
    }
    let keys: BTreeSet<&str> = values
        .iter()
        .filter_map(|v| match v {
            ArgValue::Proved(key) => Some(key.as_str()),
            _ => None,
        })
        .collect();
    match keys.len() {
        1 => Ok(keys.iter().next().expect("one key").to_string()),
        _ => Err(ForwardingRefusal::Disagree),
    }
}

/// Take the second frame for one forwarded argument: what do the forwarding
/// method's own callers supply?
///
/// This is the first frame applied once more, to a target the first frame named
/// — [`admitted_call_sites`] and [`agree_on`] unchanged. A value that is itself
/// forwarded reaches [`agree_on`] and becomes
/// [`ForwardingRefusal::ThreeOrMoreFrames`]: the bound is enforced by not taking
/// a third frame, never by a depth counter that could be raised.
fn second_frame(
    forward: &Forward,
    module: &str,
    index: &BTreeMap<&Callee, Vec<&Observation>>,
) -> Result<String, ForwardingRefusal> {
    let admitted = admitted_call_sites(
        &forward.callee,
        module,
        Some(forward.declaring_class.as_str()),
        index,
    )?;
    let values: Vec<ArgValue> = admitted
        .iter()
        .map(|o| o.values.get(&forward.slot).cloned().unwrap_or(ArgValue::Unprovable))
        .collect();
    agree_on(&values)
}

/// How a refusal ranks when several admitted call sites refuse for different
/// reasons — lower wins.
///
/// Declared once, in the order [`agree_on`] itself applies: the hop-count
/// refusal outranks everything, then an operand nothing proves, then the two
/// structural boundaries, then disagreement. A refusal that reports the *wrong*
/// cause is the reporting fault [NFR-CC-04] exists to prevent, so the ranking is
/// a table rather than whichever site happened to be read first.
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
fn refusal_rank(refusal: ForwardingRefusal) -> u8 {
    match refusal {
        ForwardingRefusal::UnsupportedSignature => 0,
        ForwardingRefusal::ThreeOrMoreFrames => 1,
        ForwardingRefusal::UnresolvableOperand => 2,
        ForwardingRefusal::NoCallSite => 3,
        ForwardingRefusal::OutOfModule => 4,
        ForwardingRefusal::Disagree => 5,
    }
}

/// One frame of a hop chain: the declaring type and the signature whose callers
/// were read, `Producer.sendMessage/3`.
///
/// The type is part of the name because two frames of one chain routinely share
/// a signature — an override and the base it calls `super` on — and a chain that
/// printed `sendMessage/3` twice would name one frame twice rather than two
/// frames.
fn frame_name(declaring_class: Option<&str>, callee: &Callee) -> String {
    format!(
        "{}.{}/{}",
        declaring_class.unwrap_or("<anonymous>"),
        callee.name,
        callee.arity
    )
}

/// Decide one candidate at up to two frames.
fn decide(
    candidate: &ForwardingCandidate,
    module: &str,
    frame_one: &BTreeMap<&Callee, Vec<&Observation>>,
    frame_two: &BTreeMap<&Callee, Vec<&Observation>>,
) -> ForwardingOutcome {
    let admitted = match admitted_call_sites(
        &candidate.wrapper,
        module,
        candidate.declaring_class.as_deref(),
        frame_one,
    ) {
        Ok(sites) => sites,
        Err(refusal) => return ForwardingOutcome::Refused(refusal),
    };

    // Each admitted call site's operand, with a forwarded one replaced by what
    // the second frame proved about it. The result is fed to the same
    // `agree_on` the first frame uses, so the two readings share one agreement
    // rule rather than two spellings of it.
    let mut second_frame_refusals: Vec<ForwardingRefusal> = Vec::new();
    let mut frames = 1usize;
    let values: Vec<ArgValue> = admitted
        .iter()
        .map(|o| {
            let value = o
                .values
                .get(&candidate.slot)
                .cloned()
                .unwrap_or(ArgValue::Unprovable);
            let ArgValue::Forwarded(forward) = &value else {
                return value;
            };
            frames = 2;
            match second_frame(forward, &o.module, frame_two) {
                Ok(key) => ArgValue::Proved(key),
                Err(refusal) => {
                    second_frame_refusals.push(refusal);
                    // A third frame keeps the hop-count refusal; every other
                    // second-frame refusal is an admitted call site whose
                    // argument did not resolve, which is the same fault one
                    // frame deeper.
                    match refusal {
                        ForwardingRefusal::ThreeOrMoreFrames => value,
                        _ => ArgValue::Unprovable,
                    }
                }
            }
        })
        .collect();

    match agree_on(&values) {
        Ok(topic) => {
            let mut chain = vec![frame_name(
                candidate.declaring_class.as_deref(),
                &candidate.wrapper,
            )];
            if frames == 2 {
                chain.extend(admitted.iter().filter_map(|o| {
                    match o.values.get(&candidate.slot) {
                        Some(ArgValue::Forwarded(f)) => {
                            Some(frame_name(Some(&f.declaring_class), &f.callee))
                        }
                        _ => None,
                    }
                }));
                chain.dedup();
            }
            ForwardingOutcome::Resolved(Forwarded { topic, chain })
        }
        // `agree_on` saw the frame-two refusals only as unprovable operands, so
        // where it says so the named second-frame cause is the honest one.
        Err(ForwardingRefusal::UnresolvableOperand) => ForwardingOutcome::Refused(
            second_frame_refusals
                .into_iter()
                .min_by_key(|r| refusal_rank(*r))
                .unwrap_or(ForwardingRefusal::UnresolvableOperand),
        ),
        Err(refusal) => ForwardingOutcome::Refused(refusal),
    }
}

/// Resolve every [`ForwardingCandidate`] the pass recorded, and rewrite the
/// sites that resolved — the cross-file half of the broker arm (S-417,
/// [FR-WS-26]).
///
/// Returns the number of references emitted. Called once from
/// [`crate::extract::extract_files`], after the parallel map, because this is
/// the first point at which the whole set's `Calls` ledger and the whole set's
/// text are both in hand.
///
/// **A workspace with no candidate pays one `is_empty` test.** That is the guard
/// below and it is a correctness-neutral cost guard, the same kind
/// [`super::accessor_binding`]'s `!properties.is_empty()` is: with it removed the
/// output is byte-identical, because no candidate means nothing to look up.
///
/// # What an incremental sync sees, stated rather than discovered
///
/// `inputs` is the dirty set on an incremental sync, exactly as it is for the
/// [`PropertiesIndex`] pre-pass, so the same sentence applies here: a wrapper
/// re-extracted **without** its callers sees a ledger and a text set that do not
/// hold them, and its site records no topic — reverting to the keyless
/// `topic-not-literal` row it carried before this hop existed, until the next
/// full walk restores it. That is the conservative direction and not a
/// correctness hole: the failure mode is losing evidence, never inventing it
/// ([NFR-RA-05]). Closing it needs an index that outlives one pass, which is the
/// same unbuilt thing [FR-WS-19] AC7 declines for the accessor chain.
///
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
/// [FR-WS-26]: ../../../docs/specs/requirements/FR-WS-26.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// Public for one reason and one caller: the reference-workspace harness measures
/// this pass's cost **apart from** the extraction it rides on
/// ([FR-WS-26](../../../docs/specs/requirements/FR-WS-26.md) AC7,
/// [NFR-PE-02](../../../docs/specs/requirements/NFR-PE-02.md)). Timing
/// `extract_files` as a whole cannot answer "what did the hop add"; timing this
/// can, without a second build of the estate to compare against.
pub fn resolve_forwarded_topics(
    facts: &mut [Facts],
    inputs: &[FileInput],
    registry: &LanguageRegistry,
    properties: &PropertiesIndex,
) -> usize {
    if facts.iter().all(|f| f.forwarding.is_empty()) {
        return 0;
    }

    let modules = ModuleMap::from_paths(inputs.iter().map(|i| i.path.as_str()));
    let by_path: HashMap<&str, &str> = inputs
        .iter()
        .map(|i| (i.path.as_str(), i.source.as_str()))
        .collect();

    let mut wanted: BTreeMap<Callee, BTreeSet<usize>> = BTreeMap::new();
    for candidate in facts.iter().flat_map(|f| f.forwarding.iter()) {
        if candidate.signature_supported {
            wanted
                .entry(candidate.wrapper.clone())
                .or_default()
                .insert(candidate.slot);
        }
    }
    let frame_one = observe_wanted(&wanted, facts, &by_path, registry, &modules, properties);

    // The second frame's lookup set is read off the FIRST frame's own
    // observations, so a method reaches it only because a real admitted call
    // site forwarded a real parameter into it. It is not a second scan for
    // "methods that look like wrappers", which would be a second population.
    let mut wanted_two: BTreeMap<Callee, BTreeSet<usize>> = BTreeMap::new();
    for value in frame_one.iter().flat_map(|o| o.values.values()) {
        if let ArgValue::Forwarded(forward) = value {
            wanted_two
                .entry(forward.callee.clone())
                .or_default()
                .insert(forward.slot);
        }
    }
    let frame_two = observe_wanted(&wanted_two, facts, &by_path, registry, &modules, properties);

    let index_one = index_by_callee(&frame_one);
    let index_two = index_by_callee(&frame_two);

    let mut decisions: Vec<(usize, usize, ForwardingOutcome)> = Vec::new();
    for (file, f) in facts.iter().enumerate() {
        let module = modules.module_of(&f.path);
        for (at, candidate) in f.forwarding.iter().enumerate() {
            let outcome = if candidate.signature_supported {
                decide(candidate, module, &index_one, &index_two)
            } else {
                ForwardingOutcome::Refused(ForwardingRefusal::UnsupportedSignature)
            };
            decisions.push((file, at, outcome));
        }
    }

    let mut emitted = 0;
    for (file, f) in facts.iter_mut().enumerate() {
        for (_, at, outcome) in decisions.iter().filter(|(i, _, _)| *i == file) {
            f.forwarding[*at].outcome = Some(outcome.clone());
        }
        emitted += admit_resolved_topics(f);
    }
    emitted
}

/// Observe every call site of every wanted method, over the files the `Calls`
/// ledger names as callers of one.
///
/// The ledger's target text is a bare method name, so the file set it yields is
/// a **superset** of the real callers. That is the right direction for a work
/// list — it may open a file that proves nothing, it cannot miss one that proves
/// something — and it is why this pass reads a handful of files rather than the
/// corpus.
fn observe_wanted(
    want: &BTreeMap<Callee, BTreeSet<usize>>,
    facts: &[Facts],
    by_path: &HashMap<&str, &str>,
    registry: &LanguageRegistry,
    modules: &ModuleMap,
    properties: &PropertiesIndex,
) -> Vec<Observation> {
    if want.is_empty() {
        return Vec::new();
    }
    let names: BTreeSet<String> = want.keys().map(|c| c.name.clone()).collect();
    // A `Foo::bar` method reference files no `Calls` row, so the ledger cannot
    // name the file that writes one — and a reference must **veto** the
    // agreement, so a file the hop never opens is the one direction in which it
    // could over-read. The needle closes it, and it is a pre-filter in the sense
    // [`PropertiesIndex`]'s annotation test is: it can only cause a parse, never
    // admit one, because the parse then decides with the same query as every
    // other file.
    let reference_needles: Vec<String> = names.iter().map(|n| format!("::{n}")).collect();
    let mut out = Vec::new();
    for f in facts {
        let Some(text) = by_path.get(f.path.as_str()) else {
            continue;
        };
        let names_a_wanted_method = f.refs.iter().any(|r| {
            r.kind == EdgeKind::Calls
                && names.contains(r.target.rsplit("::").next().unwrap_or(&r.target))
        });
        if !names_a_wanted_method && !reference_needles.iter().any(|n| text.contains(n)) {
            continue;
        }
        let Some(plugin) = registry.for_path(&f.path) else {
            continue;
        };
        observe_calls(
            &f.path, text, plugin, modules, properties, want, &names, &mut out,
        );
    }
    out
}

/// Turn one file's resolved candidates into bound broker references, retracting
/// the keyless rows they supersede.
///
/// **A keyless row is retracted only when every refusal it stands for has
/// resolved.** The ledger dedups on `(source, target, form, kind, relation)` and
/// ignores the line, so two refused sites in one declaration reach it as ONE
/// row; retracting on the first resolution would silence the second site, which
/// is still refused. The count each candidate carries
/// ([`ForwardingCandidate::refusals_at_source`]) is what makes that decidable
/// here, after the parse tree is gone.
fn admit_resolved_topics(facts: &mut Facts) -> usize {
    let resolved: Vec<(ArtifactRelation, LogosSymbol, u32, String)> = facts
        .forwarding
        .iter()
        .filter_map(|c| match &c.outcome {
            Some(ForwardingOutcome::Resolved(f)) => {
                Some((c.relation, c.source.clone(), c.line, f.topic.clone()))
            }
            _ => None,
        })
        .collect();
    if resolved.is_empty() {
        return 0;
    }

    let mut resolved_at: HashMap<(ArtifactRelation, String), usize> = HashMap::new();
    for (relation, source, _, _) in &resolved {
        *resolved_at
            .entry((*relation, source.as_str().to_string()))
            .or_default() += 1;
    }
    let retract: HashSet<(ArtifactRelation, String)> = facts
        .forwarding
        .iter()
        .filter_map(|c| {
            let key = (c.relation, c.source.as_str().to_string());
            let resolved_here = resolved_at.get(&key).copied().unwrap_or(0);
            (resolved_here >= c.refusals_at_source).then_some(key)
        })
        .collect();
    facts.refs.retain(|r| {
        !(r.target.is_empty()
            && r.relation.is_some_and(|relation| {
                retract.contains(&(relation, r.source.as_str().to_string()))
            }))
    });

    let mut emitted = 0;
    for (relation, source, line, topic) in resolved {
        let mut slots = BTreeMap::new();
        slots.insert("topic".to_string(), topic);
        emitted += capture_invocation_refs(
            facts,
            relation,
            RefForm::Method,
            [InvocationSite { source, slots, line }],
            broker_topic_key,
        );
    }
    super::dedup_sort_refs(&mut facts.refs);
    emitted
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

    /// The `self.`-qualified receiver — the Rust twin of
    /// `java_capture_tests::a_this_qualified_receiver_walks_to_its_field`, and
    /// the other half of the one [`chain_base_name`] branch no estate source
    /// exercises. A struct field's declared type is what the gate reads.
    #[test]
    fn a_self_qualified_receiver_walks_to_its_field() {
        let src = r#"
struct Topology { builder: StreamsBuilder }

impl Topology {
    fn run(&mut self) {
        self.builder.stream("self-in").to("self-out");
    }
}
"#;
        let facts = extract_rust(src);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["self-in".to_string()],
            "`self.builder` names the FIELD, not the `self` node: {:?}",
            facts.refs
        );
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerPublish),
            vec!["self-out".to_string()],
            "the publish half of the same chain likewise resolves: {:?}",
            facts.refs
        );
    }

    /// The receiver-gated topology form (S-408, [CR-131] §3.2 A1), pinned by
    /// FIXTURE and by fixture alone: Kafka Streams is a JVM library, this
    /// workspace has no Rust broker corpus, and the 84-member reference estate
    /// contains 0 `.rs` files (re-measured 2026-09-15 and asserted in
    /// `tests/broker_topic_corpus.rs`). A `0` for this arm in any estate report is
    /// *unexercised*, never *zero captured*. The Go arm's position is different —
    /// the estate carries 262 real `.go` files, so its over-capture half IS
    /// measured; this one has nothing.
    ///
    /// Three rows, one per thing that can go wrong: a literal operand binds, a
    /// non-literal operand records a refusal (the slots this file ships only for
    /// the gated patterns), and the same two verbs on a non-topology receiver are
    /// silent — captured as nothing AND refused as nothing.
    #[test]
    fn the_rust_topology_form_is_receiver_gated() {
        let src = r#"
fn topology(builder: &StreamsBuilder) {
    builder.stream("orders").to("shipments");
}

fn dynamic(builder: &StreamsBuilder, cfg: &Config) {
    builder.stream(cfg.inbound()).to(cfg.outbound());
}

// Neither of these is a topology: `reader` and `converter` are declared, and
// declared as something else. One rename away from matching, which is what
// makes them a near miss rather than unrelated code.
fn not_a_topology(reader: &Reader, converter: &Converter) {
    reader.stream("orders");
    converter.to("shipments");
}
"#;
        let facts = extract_rust(src);

        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["".to_string(), "orders".to_string()],
            "one bound subscribe topic and one recorded refusal, and nothing from \
             the `Reader` receiver: {:?}",
            facts.refs
        );
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerPublish),
            vec!["".to_string(), "shipments".to_string()],
            "one bound publish topic and one recorded refusal, and nothing from \
             the `Converter` receiver: {:?}",
            facts.refs
        );
        // The refusals are attributed to the topology fn, never to the near-miss
        // one — the check that would catch the gate leaking into the wrong site.
        for r in facts
            .refs
            .iter()
            .filter(|r| r.target.is_empty() && r.relation.is_some())
        {
            assert!(
                r.source.as_str().contains("dynamic"),
                "a refusal belongs to the gated site that produced it: {}",
                r.source.as_str()
            );
        }
    }
}

// Go's whole broker arm is the receiver-gated topology form (S-408); the
// language gained the `brokers` capability for it. Runs only when the Go grammar
// is compiled in.
#[cfg(all(test, feature = "lang-go"))]
mod go_capture_tests {
    use super::*;
    use crate::extract::{extract, FileInput, SymbolContext};
    use crate::plugin::LanguageRegistry;

    fn extract_go(src: &str) -> Facts {
        let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
        let plugin = registry.for_extension("go").expect("go plugin present");
        extract(&FileInput::new("svc.go", src), plugin, &SymbolContext::default())
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

    /// **A receiver spelled like a topology type is not one.** Two near misses,
    /// each one character from nothing: a Go *function* and a Go *parameter* that
    /// merely share a name with a roster type.
    ///
    /// The `Helper` row settles the gate's design. That receiver is explicitly
    /// declared as another type, and an earlier draft captured it anyway — the
    /// name test fired before the declaration was ever consulted, so a correct
    /// refusal was overridden by a spelling coincidence. Go is where this bites
    /// hardest: a package-level `func KStream(...)` is ordinary Go, and every one
    /// of its call sites would have become a broker row.
    #[test]
    fn a_receiver_spelled_like_a_topology_type_is_not_one() {
        let by_function = r#"
package p
func KStream(s string) *Logger { return nil }
func run() { KStream("cfg").To("not-a-topic") }
"#;
        let facts = extract_go(by_function);
        assert!(
            facts.refs.iter().all(|r| !matches!(
                r.relation,
                Some(ArtifactRelation::BrokerPublish) | Some(ArtifactRelation::BrokerSubscribe)
            )),
            "a function sharing a roster name is not a topology receiver: {:?}",
            facts.refs
        );

        let by_parameter = r#"
package p
func run(KStream Helper) { KStream.To("not-a-topic") }
"#;
        let facts = extract_go(by_parameter);
        assert!(
            facts.refs.iter().all(|r| !matches!(
                r.relation,
                Some(ArtifactRelation::BrokerPublish) | Some(ArtifactRelation::BrokerSubscribe)
            )),
            "an explicit `Helper` declaration outranks a name that looks like a \
             roster type — the declaration decides, never the spelling: {:?}",
            facts.refs
        );
    }

    /// The receiver-gated topology form. CAPTURE is fixture-only — the estate
    /// writes no Kafka Streams topology in Go — but OVER-capture is measured on a
    /// real corpus: the estate's one Go member is 262 `.go` files and the gated
    /// arm produces 0 broker rows over all of them
    /// (`tests/broker_topic_corpus.rs`, 2026-09-15). So "0" for this arm means two
    /// different things on the two halves and must never be reported as one.
    ///
    /// The exported spelling (`Stream`/`To`, what a Go port of the Streams API
    /// exports) and the unexported one (`stream`/`to`, an in-package helper) are
    /// both admitted, and both only behind the gate — the pointer-typed parameter
    /// `*StreamsBuilder` is what `bare_type_name` has to strip to reach the
    /// roster.
    #[test]
    fn the_go_topology_form_is_receiver_gated() {
        let src = r#"
package stream

func Topology(builder *StreamsBuilder) {
	builder.Stream("orders").To("shipments")
}

func helper(builder *StreamsBuilder, cfg Config) {
	builder.stream(cfg.Inbound()).to(cfg.Outbound())
}

// The unexported verb spelling with a LITERAL operand. The `helper` rows above
// reach the lowercase arm of each `#any-of?` only through the REFUSAL slot, so
// without this the binding patterns could be narrowed to the exported spelling
// alone and no test would notice.
func unexported(builder *StreamsBuilder) {
	builder.stream("lower-in").to("lower-out")
}

// A PACKAGE-QUALIFIED receiver type, which is how Go actually imports a type.
// `bare_type_name` has to strip the `kafka.` qualifier to reach the roster.
func qualified(builder *kafka.StreamsBuilder) {
	builder.Stream("qualified-in").To("qualified-out")
}

// Neither of these is a topology: both receivers are declared as something else.
func notATopology(reader *Reader, converter Converter) {
	reader.Stream("orders")
	converter.To("shipments")
}
"#;
        let facts = extract_go(src);

        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec![
                "".to_string(),
                "lower-in".to_string(),
                "orders".to_string(),
                "qualified-in".to_string()
            ],
            "both verb spellings bind a literal, a package-qualified receiver type \
             normalizes, the call operand refuses, and the `Reader` receiver \
             produces neither: {:?}",
            facts.refs
        );
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerPublish),
            vec![
                "".to_string(),
                "lower-out".to_string(),
                "qualified-out".to_string(),
                "shipments".to_string()
            ],
            "both verb spellings bind a literal, a package-qualified receiver type \
             normalizes, the call operand refuses, and the `Converter` receiver \
             produces neither: {:?}",
            facts.refs
        );
        for r in facts
            .refs
            .iter()
            .filter(|r| r.target.is_empty() && r.relation.is_some())
        {
            assert!(
                r.source.as_str().contains("helper"),
                "a refusal belongs to the gated site that produced it: {}",
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
        // No binding view: most guards here are about the reconcile and the
        // receiver gate, not about the accessor hop, and a member with no
        // `@ConfigurationProperties` class is exactly what `None` models.
        capture_with_properties(query_src, src, None)
    }

    /// [`capture_with`], plus the source of a `@ConfigurationProperties` class the
    /// member declares — so the accessor hop is live and a slot operand can resolve.
    fn capture_with_properties(query_src: &str, src: &str, props: Option<&str>) -> Facts {
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
            config_source: None,
            forwarding: Vec::new(),
            declared_types: Vec::new(),
        };
        let index = props.map(|p| {
            crate::extract::config::binding::PropertiesIndex::from_sources(
                &registry,
                [("src/main/java/Props.java", p)],
            )
        });
        let view = index.as_ref().map(|index| BindingView {
            index,
            types: crate::extract::config::accessor::DeclaredTypes::build(
                tree.root_node(),
                src.as_bytes(),
            ),
            language: plugin.name(),
            module: crate::extract::config::binding::MEMBER_SCOPE,
        });
        capture_broker_invocations(
            &query,
            tree.root_node(),
            src.as_bytes(),
            |_| Some(symbol.clone()),
            || view.as_ref(),
            &mut facts,
        );
        facts
    }

    /// **The reconcile, on the ACCESSOR side (S-409).** A resolved accessor must
    /// cancel a refusal candidate raised over the same site by a *second* pattern —
    /// the job of the `bound.push` in the hop's resolved arm.
    ///
    /// This mirrors, deliberately and for the same reason, the literal arm's own
    /// `a_site_that_bound_a_literal_records_no_refusal_even_when_a_slot_matched_it`:
    /// no SHIPPED query creates the pair, because one operand is captured by one
    /// pattern, so a droppable on-disk `brokers.scm` ([FR-PL-04]) is the only thing
    /// that can. Without this fixture the `bound.push` is dead to every test —
    /// deleting it left the whole suite and the estate measurement green, which is
    /// what this test exists to stop.
    ///
    /// [FR-PL-04]: ../../../docs/specs/requirements/FR-PL-04.md
    #[test]
    fn a_site_whose_accessor_resolved_records_no_refusal_even_when_a_wider_slot_matched_it() {
        // Two patterns over one call: the inner one slots the accessor operand (and
        // resolves it), the outer one slots the whole argument list and would refuse
        // the same site. Only the range containment can tell the site bound.
        let query = r#"
(method_invocation
  name: (identifier) @_m
  arguments: (argument_list
    . (method_invocation) @broker.publish.topic.slot) @broker.publish.site
  (#eq? @_m "send"))

(method_invocation
  name: (identifier) @_m2
  arguments: (argument_list) @broker.publish.topic.slot @broker.publish.site
  (#eq? @_m2 "send"))
"#;
        const PROPS: &str = "package a;\n            @ConfigurationProperties(prefix = \"kafka.topics\")\n            public class KafkaTopics { private String archiveEvents; }\n";
        let src = r#"
class C {
    private final KafkaTopics kafkaTopics;
    void publish() { template.send(kafkaTopics.getArchiveEvents()); }
}
"#;
        let resolved = capture_with_properties(query, src, Some(PROPS));
        assert_eq!(
            targets(&resolved, ArtifactRelation::BrokerPublish),
            vec!["${kafka.topics.archiveevents}".to_string()],
            "the accessor bound, so the wider slot's candidate is cancelled and the              site records NO keyless row beside the key: {:?}",
            resolved.refs
        );

        // The complement, through the SAME query and the SAME source: with no
        // properties class the accessor resolves nothing, so the site refuses. This
        // is what makes the assertion above about the cancellation rather than
        // about the query simply never producing a candidate.
        let refused = capture_with_properties(query, src, None);
        assert_eq!(
            targets(&refused, ArtifactRelation::BrokerPublish),
            vec![String::new()],
            "with nothing to resolve, the same two patterns record the refusal: {:?}",
            refused.refs
        );
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

    // ── The Kafka Streams topology form (S-408, CR-131 §3.2 A1) ─────────────
    //
    // The arm's first RECEIVER-GATED form. Everything before it is recognised by
    // a name no ordinary code writes — an annotation, `KafkaHeaders.TOPIC`, a
    // broker template verb — while `stream` and `to` are two of the most common
    // method names in Java. So these tests come in pairs: one that the estate's
    // real shape is admitted, and one that the same verb on an unrelated
    // receiver is silent.

    /// Helper: true when a file produced no broker row of either role — the
    /// "captured as nothing AND refused as nothing" state the receiver gate is
    /// supposed to leave a non-topology site in.
    fn no_broker_rows(facts: &Facts) -> bool {
        facts.refs.iter().all(|r| {
            !matches!(
                r.relation,
                Some(ArtifactRelation::BrokerPublish) | Some(ArtifactRelation::BrokerSubscribe)
            )
        })
    }

    /// Helper: the 1-based lines of the keyless (refused) rows of one relation.
    fn refusal_lines(facts: &Facts, relation: ArtifactRelation) -> Vec<u32> {
        let mut lines: Vec<u32> = facts
            .refs
            .iter()
            .filter(|r| r.relation == Some(relation) && r.target.is_empty())
            .map(|r| r.line)
            .collect();
        lines.sort_unstable();
        lines
    }

    /// The estate's real topology shape — a `StreamsBuilder` formal parameter,
    /// a chained `stream(topic, serde) … .to(topic, serde)` — produces one
    /// subscribe row and one publish row. What S-408 owns is that they are
    /// reported at all rather than absent ([NFR-CC-04]).
    ///
    /// Both are REFUSALS **on this fixture**, and since S-409 that is a fact
    /// about the fixture rather than about the operand: `extract_java` is a
    /// single-file driver, so the arm receives no `BindingView` and the
    /// accessor cannot resolve — exactly the `None` arm the production pass
    /// takes for a member declaring no `@ConfigurationProperties` class. The
    /// same two sites with the owning class in scope bind their canonical keys,
    /// which is pinned end to end by
    /// `extract::tests::an_accessor_topic_reaches_the_ledger_in_every_captured_broker_form`.
    #[test]
    fn the_estate_topology_shape_records_a_subscribe_and_a_publish_refusal() {
        let src = r#"
package com.acme;
class ArchiveEventStream {
    private final KafkaTopics kafkaTopics;

    @Autowired
    public void kStream(StreamsBuilder streamsBuilder) {
        streamsBuilder
                .stream(kafkaTopics.getArchiveEvents(), buildConsumeSerdeConfig())
                .filter((key, payload) -> true)
                .transform(this::getTransformer)
                .to(kafkaTopics.getArchiveReporting(), buildProduceSerdeConfig());
    }
}
"#;
        let facts = extract_java(src);

        assert_eq!(
            refusal_lines(&facts, ArtifactRelation::BrokerSubscribe).len(),
            1,
            "the `stream(accessor, serde)` link records exactly one subscribe refusal: {:?}",
            facts.refs
        );
        assert_eq!(
            refusal_lines(&facts, ArtifactRelation::BrokerPublish).len(),
            1,
            "the `to(accessor, serde)` link records exactly one publish refusal: {:?}",
            facts.refs
        );
        // Nothing was fabricated: an accessor operand binds no topic.
        assert!(
            targets(&facts, ArtifactRelation::BrokerSubscribe)
                .iter()
                .all(String::is_empty),
            "a non-literal operand binds nothing ([NFR-RA-05]): {:?}",
            facts.refs
        );
        // Attribution is the topology method, not the file module.
        let sub = facts
            .refs
            .iter()
            .find(|r| r.relation == Some(ArtifactRelation::BrokerSubscribe))
            .expect("a subscribe row exists");
        assert!(
            sub.source.as_str().contains("kStream"),
            "the topology link is attributed to its own method: {}",
            sub.source.as_str()
        );
    }

    /// A literal operand BINDS on both roles, keyed by the literal's own text —
    /// the same grammatical-shape rule the annotation and header forms apply,
    /// and the same non-rule about the characters it carries ([FR-WS-10] AC3).
    #[test]
    fn a_literal_topology_operand_binds_on_both_roles() {
        let src = r#"
package com.acme;
class LiteralTopology {
    public void build(StreamsBuilder streamsBuilder) {
        streamsBuilder
                .stream("${spring.kafka.topics.orders}")
                .to("shipments");
    }
}
"#;
        let facts = extract_java(src);

        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["${spring.kafka.topics.orders}".to_string()],
            "a placeholder literal binds as written: {:?}",
            facts.refs
        );
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerPublish),
            vec!["shipments".to_string()],
            "the topology publish binds its literal: {:?}",
            facts.refs
        );
        // A bound site records no refusal — the reconcile, not luck.
        assert!(
            refusal_lines(&facts, ArtifactRelation::BrokerSubscribe).is_empty()
                && refusal_lines(&facts, ArtifactRelation::BrokerPublish).is_empty(),
            "a site that bound a literal records no refusal: {:?}",
            facts.refs
        );
    }

    /// **The receiver gate, inverted.** A `stream(…)` on a non-Streams receiver
    /// and a `to(…)` outside a topology chain are captured as nothing AND
    /// refused as nothing — silence, not a manufactured coverage denominator.
    ///
    /// Each row sits one declaration away from matching: change `Repository` to
    /// `StreamsBuilder` and `Converter` to `KStream` and both are admitted, which
    /// is what makes this a near-miss test rather than an unrelated-code test.
    #[test]
    fn a_stream_or_to_on_a_non_topology_receiver_is_neither_captured_nor_refused() {
        let src = r#"
package com.acme;
class NotATopology {
    public void notASubscribe(Repository repository, Converter converter) {
        repository.stream("orders");
        converter.to("shipments");
        repository.stream(someProperties.getName());
        converter.to(Topics.OUTPUT);
    }
}
"#;
        let facts = extract_java(src);

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
        assert!(
            broker.is_empty(),
            "a non-topology receiver produces neither a binding nor a refusal: {broker:?}"
        );
    }

    /// `Collection.stream()` is excluded by ARITY, before the receiver gate is
    /// ever consulted — the patterns require a first argument. Stated as its own
    /// test because "the gate excludes it" is the plausible-sounding wrong
    /// reason: a `List` receiver would fail the gate too, but the query never
    /// gets that far.
    #[test]
    fn the_zero_argument_stream_form_is_excluded_by_arity() {
        let src = r#"
package com.acme;
class Collections {
    public void iterate(java.util.List<String> orders) {
        orders.stream().forEach(System.out::println);
    }
}
"#;
        let facts = extract_java(src);
        assert!(
            facts.refs.iter().all(|r| !matches!(
                r.relation,
                Some(ArtifactRelation::BrokerPublish) | Some(ArtifactRelation::BrokerSubscribe)
            )),
            "a 0-arity `.stream()` matches no topology pattern: {:?}",
            facts.refs
        );
    }

    /// The `to(…)` chain walk reaches the `StreamsBuilder` at the bottom of a
    /// multi-link chain, and reaches a `KStream` binding when the stream is held
    /// in a local rather than chained — the estate writes both (7 members the
    /// first way, `punctuators-poc` the second).
    #[test]
    fn the_receiver_gate_walks_a_chain_and_a_kstream_binding() {
        let src = r#"
package com.acme;
class TwoShapes {
    private void chained(StreamsBuilder builder) {
        builder.stream("in").mapValues(v -> v).filter((k, v) -> true).to("chained-out");
    }

    private void viaBinding(StreamsBuilder builder) {
        KStream<String, String> lines = builder.stream("in-2");
        lines.process(() -> null).to("binding-out");
    }
}
"#;
        let facts = extract_java(src);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerPublish),
            vec!["binding-out".to_string(), "chained-out".to_string()],
            "both a chained receiver and a KStream-typed binding pass the gate: {:?}",
            facts.refs
        );
    }

    /// A same-named binding of another type in the same file makes the site
    /// **under**-capture, never mis-capture: the gate requires EVERY declaration
    /// of the receiver name to be a topology type ([NFR-RA-05]). Pinned because
    /// the natural reading — "any declaration admits" — is the over-capturing
    /// one, and the two differ by a single word in `receiver_is_topology`.
    #[test]
    fn a_shadowing_declaration_of_another_type_refuses_the_site() {
        let src = r#"
package com.acme;
class Shadowed {
    private void topology(StreamsBuilder builder) {
        builder.stream("in").to("out");
    }

    private void unrelated(StringBuilder builder) {
        builder.append("x");
    }
}
"#;
        let facts = extract_java(src);
        assert!(
            facts.refs.iter().all(|r| !matches!(
                r.relation,
                Some(ArtifactRelation::BrokerPublish) | Some(ArtifactRelation::BrokerSubscribe)
            )),
            "an ambiguous receiver name refuses rather than guesses: {:?}",
            facts.refs
        );
    }

    /// **A method named like the receiver is not a declaration of it.** Java's
    /// `method_declaration` carries a `type:` field (its RETURN type) beside a
    /// `name:` field, so the whole-file declaration walk read a helper named
    /// `builder()` as a declaration of `builder`. Under the
    /// every-declaration-must-agree rule that made the topology **silent** —
    /// captured as nothing AND refused as nothing, the one outcome [NFR-CC-04]
    /// forbids — and it did so on about the most ordinary Java shape there is, a
    /// getter named after what it returns.
    ///
    /// The same root cause admitted in the other direction: a receiver with no
    /// binding anywhere, whose name happened to match a method returning a roster
    /// type, was captured — the inherited-field case the gate's ceilings promise
    /// to refuse. Both directions are pinned here, because a fix that only
    /// restored the refusal would leave the false admission standing.
    #[test]
    fn a_method_named_like_the_receiver_is_not_a_declaration_of_it() {
        // (a) a getter of another type must not silence the topology
        let getter = r#"
package com.acme;
class WithGetter {
    public void topo(StreamsBuilder streamsBuilder) {
        streamsBuilder.stream("getter-in").to("getter-out");
    }
    public String streamsBuilder() { return ""; }
}
"#;
        let facts = extract_java(getter);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["getter-in".to_string()],
            "a same-named METHOD is not a binding and must not refuse the site: {:?}",
            facts.refs
        );
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerPublish),
            vec!["getter-out".to_string()],
            "the publish half likewise survives the same-named method: {:?}",
            facts.refs
        );

        // (b) the Spring `@Bean` factory shape, whose return type IS the roster
        //     type — admitted for the right reason (the parameter), not because
        //     the factory's return type happens to match.
        let factory = r#"
package com.acme;
class WithFactory {
    @Bean public StreamsBuilder builder() { return new StreamsBuilder(); }
    void topo(StreamsBuilder builder) { builder.stream("factory-in").to("factory-out"); }
}
"#;
        let facts = extract_java(factory);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["factory-in".to_string()],
            "the @Bean factory shape is admitted by its parameter: {:?}",
            facts.refs
        );

        // (c) the inverse: NO binding of `builder` anywhere, only a method that
        //     returns a roster type. The receiver's type is not written in this
        //     file, so the gate must refuse — silence, not a guess ([NFR-RA-05]).
        let inherited = r#"
package com.acme;
class Inherited extends Base {
    void topo() { builder.stream("inherited-in").to("inherited-out"); }
    protected StreamsBuilder builder() { return null; }
}
"#;
        let facts = extract_java(inherited);
        assert!(
            facts.refs.iter().all(|r| !matches!(
                r.relation,
                Some(ArtifactRelation::BrokerPublish) | Some(ArtifactRelation::BrokerSubscribe)
            )),
            "a method's return type is not the receiver's type — refuse, never guess: {:?}",
            facts.refs
        );
    }

    /// **A receiver spelled like a topology type is not one** (Java half; the Go
    /// half, including the row that settles the design, is
    /// `go_capture_tests::a_receiver_spelled_like_a_topology_type_is_not_one`).
    ///
    /// An earlier draft of the gate carried a second admission that tested the
    /// receiver's *text* against the roster before consulting any declaration, so
    /// any class merely SHARING a name with a roster type had its calls captured.
    /// A name is not evidence of a type.
    #[test]
    fn a_receiver_spelled_like_a_topology_type_is_not_one() {
        let java_class = r#"
package com.acme;
class UsesUnrelatedClass {
    void run() { KStream.to("not-a-topic"); }
}
"#;
        let facts = extract_java(java_class);
        assert!(
            no_broker_rows(&facts),
            "a bare class name that matches the roster declares nothing here: {:?}",
            facts.refs
        );
    }

    /// The `this.`-qualified receiver — the ordinary Spring injected-field shape,
    /// and the one branch of [`chain_base_name`] that the estate does not write.
    /// The estate injects its `StreamsBuilder` as a method parameter, so without
    /// this fixture the `this`/`self` arm rests on a docstring claim and nothing
    /// else; its Rust twin is
    /// `rust_capture_tests::a_self_qualified_receiver_walks_to_its_field`.
    #[test]
    fn a_this_qualified_receiver_walks_to_its_field() {
        let src = r#"
package com.acme;
class Injected {
    private final StreamsBuilder builder = new StreamsBuilder();

    public void topo() {
        this.builder.stream("this-in").to("this-out");
    }
}
"#;
        let facts = extract_java(src);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["this-in".to_string()],
            "`this.builder` names the FIELD, not the `this` node: {:?}",
            facts.refs
        );
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerPublish),
            vec!["this-out".to_string()],
            "the publish half of the same chain likewise resolves: {:?}",
            facts.refs
        );
    }

    /// A **fully-qualified** receiver type normalizes to its bare name. The Go
    /// half of this lives in `go_capture_tests::the_go_topology_form_is_receiver_gated`
    /// (`*kafka.StreamsBuilder`, the ordinary Go import shape); this is the Java
    /// one, where an inline FQN is rarer but legal.
    ///
    /// Pinned because `bare_type_name`'s qualifier strip was documented and
    /// unverified: dropping the `rsplit(['.', ':'])` left the whole suite green,
    /// so the doc comment's own `*kafka.StreamsBuilder` -> `StreamsBuilder`
    /// example was a claim about code nothing exercised.
    #[test]
    fn a_fully_qualified_receiver_type_normalizes_to_its_bare_name() {
        let src = r#"
package com.acme;
class Qualified {
    void topo(org.apache.kafka.streams.StreamsBuilder builder) {
        builder.stream("fqn-in").to("fqn-out");
    }
}
"#;
        let facts = extract_java(src);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["fqn-in".to_string()],
            "an inline FQN receiver type reaches the roster by its last segment: {:?}",
            facts.refs
        );
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerPublish),
            vec!["fqn-out".to_string()],
            "the publish half of the same chain likewise resolves: {:?}",
            facts.refs
        );
    }

    /// The gate's stated ceilings, asserted rather than only described. Each row
    /// is an UNDER-capture: nothing captured, nothing fabricated ([NFR-RA-05]).
    /// Pinned so that widening one is a deliberate change with a failing test
    /// behind it rather than a silent drift in either direction.
    #[test]
    fn a_receiver_whose_type_is_not_written_in_the_file_is_refused() {
        // `var` — the type is inferred, so no `type:` node is bound.
        let inferred = r#"
package com.acme;
class Inferred {
    void topo() {
        var builder = new StreamsBuilder();
        builder.stream("var-in").to("var-out");
    }
}
"#;
        let facts = extract_java(inferred);
        assert!(
            facts.refs.iter().all(|r| !matches!(
                r.relation,
                Some(ArtifactRelation::BrokerPublish) | Some(ArtifactRelation::BrokerSubscribe)
            )),
            "an inferred (`var`) receiver type is a stated ceiling, refused: {:?}",
            facts.refs
        );

        // An ARRAY of builders is not a builder. Java's bracket is a suffix, so
        // this is the one language where the head-of-type rule did not already
        // refuse it.
        let array = r#"
package com.acme;
class Arrays {
    private StreamsBuilder[] builder;
    void topo() { builder.stream("array-in").to("array-out"); }
}
"#;
        let facts = extract_java(array);
        assert!(
            facts.refs.iter().all(|r| !matches!(
                r.relation,
                Some(ArtifactRelation::BrokerPublish) | Some(ArtifactRelation::BrokerSubscribe)
            )),
            "an array of topology objects is not a topology receiver: {:?}",
            facts.refs
        );

        // An explicit local DOES bind — the contrast row, so the two assertions
        // above cannot both pass by the capture being broken outright.
        let explicit = r#"
package com.acme;
class Explicit {
    void topo() {
        StreamsBuilder builder = new StreamsBuilder();
        builder.stream("explicit-in").to("explicit-out");
    }
}
"#;
        let facts = extract_java(explicit);
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["explicit-in".to_string()],
            "an explicitly typed local still binds — the ceilings are about \
             INFERRED types, not about locals: {:?}",
            facts.refs
        );
    }

    /// Every pre-S-408 form still behaves exactly as it did: the annotation
    /// subscribe, the array form, the template-send publish and the header-form
    /// publish (bound and refused). The receiver gate is opt-in per pattern, so a
    /// pattern that captures no `@broker.*.receiver` is ungated — this is the
    /// assertion that says so, over the shapes the estate actually writes.
    #[test]
    fn the_pre_existing_annotation_and_header_forms_are_unmoved_by_the_receiver_gate() {
        let src = r#"
package com.acme;
class Mixed {
    private KafkaTemplate<String, String> kafkaTemplate;
    private static final String TOPIC = "dynamic";

    @KafkaListener(topics = {"orders", "shipments"})
    public void onOrder(String msg) {}

    @KafkaListener(topics = TOPIC)
    public void onDynamic(String msg) {}

    public void byArgument(String payload) {
        kafkaTemplate.send("argument-topic", payload);
    }

    public void byHeader(String payload, String topic) {
        Message<String> bound = MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, "header-topic")
                .build();
        Message<String> refused = MessageBuilder.withPayload(payload)
                .setHeader(KafkaHeaders.TOPIC, topic)
                .build();
    }
}
"#;
        let facts = extract_java(src);

        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerSubscribe),
            vec!["".to_string(), "orders".to_string(), "shipments".to_string()],
            "the array form binds both topics and the constant form refuses: {:?}",
            facts.refs
        );
        assert_eq!(
            targets(&facts, ArtifactRelation::BrokerPublish),
            vec![
                "".to_string(),
                "argument-topic".to_string(),
                "header-topic".to_string()
            ],
            "the argument and header publish forms bind, the parameter operand refuses: {:?}",
            facts.refs
        );
    }
}
