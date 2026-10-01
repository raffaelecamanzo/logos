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

// S-256 ([FR-WS-11]) wired this classifier into the live edge stream: it is now
// called by [`super::bridge::compute_edges`], which routes every broker-arm ledger
// reference here (both roles) instead of into its own consumer-only index. Until
// then it was proven only by this module's tests, mirroring how S-251 shipped
// `capture_invocation_refs` ahead of its arm callers.

use std::collections::{BTreeMap, HashSet};

use crate::model::ArtifactRelation;
use crate::resolve::binding::{MemberCorpus, Provenance};
// The topic-identity rule lives in `resolve` because the intra-repo promotion
// pass calls it on every single-root index, where this module is absent
// ([CR-136] §4.4, [FR-WS-27]). The dependency runs federation -> resolve, which
// is the direction `architecture.md` §4.1 declares.
use crate::resolve::broker_identity::{identify, BrokerIdentity};

use super::bridge::{
    index_provider, match_indexed, BridgeEdge, BridgeEndpoint, BridgeIntake, PortableKey,
    ProviderIndex, Role,
};

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

/// Fan out captured broker candidates into cross-service edges through the
/// **unchanged** namespace-generic match loop (S-254, [FR-WS-10]).
///
/// Publishers become [`Consumer`](Role::Consumer) keys and subscribers the
/// [`Provider`](Role::Provider) index; each publish then binds **every**
/// cross-member subscribe on its topic. Endpoints are de-duplicated to one per
/// `(key, role, member, symbol)` first, because the loop does not de-duplicate
/// fan-out edges (see the module docs). Non-broker candidates are ignored.
///
/// # Topic identity is the committed value where one is committed ([S-410])
///
/// `corpora` holds, per member, that member's own committed configuration — and
/// **only** the members that name a configuration key, so a workspace whose
/// broker operands are all literals hands an empty map here and is resolved
/// against nothing. Each candidate is reduced through [`identify`], so a site
/// whose operand names a committed key is filed under the **value** rather than
/// under the placeholder that names it, and two members spelling one property
/// differently still meet. A site with no placeholder, and one whose keys the
/// corpus refuses, are both filed under their operand exactly as before.
///
/// A candidate can therefore yield **more than one** key — one per overlay that
/// commits a distinct value — and it is de-duplicated, indexed and fanned out
/// under each of them independently ([FR-WS-19] AC2).
///
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
/// [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
pub(super) fn broker_edges(
    candidates: impl IntoIterator<Item = BrokerCandidate>,
    corpora: &BTreeMap<String, MemberCorpus>,
) -> Vec<BridgeEdge> {
    let mut providers: ProviderIndex = ProviderIndex::new();
    // A publish/subscribe is a captured call site ([FR-WS-10]): every broker edge
    // is invocation intake, so it seeds an app-wide reachability root ([CR-083]).
    let mut consumers: Vec<(PortableKey, BridgeEndpoint, BridgeIntake, Provenance)> = Vec::new();
    // One endpoint per (key, role, member, symbol): the fan-out loop emits an
    // edge per pair, so a duplicated endpoint here would duplicate the edge.
    let mut seen: HashSet<(PortableKey, bool, String, String)> = HashSet::new();
    // A member that named no configuration key is absent from `corpora` and
    // resolves against nothing — never against another member's configuration,
    // which is [ADR-64]'s within-reach rule.
    let empty = MemberCorpus::new();

    for cand in candidates {
        let corpus = corpora.get(&cand.endpoint.member).unwrap_or(&empty);
        let Some(BrokerIdentity {
            topics,
            bridge_role,
            value,
        }) = identify(cand.relation, &cand.key, corpus)
        else {
            continue;
        };
        let is_provider = matches!(bridge_role, Role::Provider);
        // The bridge wraps the shared identity into its own match vocabulary
        // HERE, at its own call site — the promotion pass wraps the same answer
        // into a symbol descriptor at its own. One resolution, two renderings
        // ([FR-WS-27] AC1).
        for key in topics.into_iter().map(PortableKey::broker) {
            let dedup_key = (
                key.clone(),
                is_provider,
                cand.endpoint.member.clone(),
                cand.endpoint.symbol.as_str().to_string(),
            );
            if !seen.insert(dedup_key) {
                continue; // a repeat of this exact endpoint on this topic — drop it
            }
            match bridge_role {
                Role::Provider => {
                    index_provider(&mut providers, key, cand.endpoint.clone(), value.clone())
                }
                Role::Consumer => consumers.push((
                    key,
                    cand.endpoint.clone(),
                    BridgeIntake::Invocation,
                    value.clone(),
                )),
            }
        }
    }

    // **De-duplicate the emitted edges, not just the endpoints.**
    //
    // The endpoint collapse above is per `(key, role, member, symbol)`, which was
    // the whole of it while a site had exactly one key. Since [S-410] a site can
    // carry one key **per overlay**, and when the publish and the subscribe commit
    // the *same* key under the *same* overlays they meet under each of them —
    // `match_indexed` then emits one edge per meeting, and those edges are
    // identical in every field, provenance included (the `ConfigBound` evidence
    // already names every overlay's value). Two rows for one coupling is a
    // fabricated count: `resolved_cross_service_edges` reconciles the bridge's
    // edge total against the coverage tier's bound rows, and the coverage tier
    // emits ONE row per site by construction ([NFR-RA-05], [CR-118]).
    //
    // `match_indexed` returns its edges sorted, so `dedup` removes exactly the
    // adjacent duplicates and nothing else.
    let mut edges = match_indexed(providers, consumers);
    edges.dedup();
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    // The identity rule itself lives in `crate::resolve::broker_identity` since
    // [S-424]; these fan-out fixtures assert against it because it is what
    // `broker_edges` keys on.
    //
    // [S-424]: ../../../docs/planning/journal.md#s-424-the-promoted-topic-inventory-keys-on-the-committed-value
    use crate::resolve::binding::ValueRefusal;
    use crate::resolve::broker_identity::{topic_identity, TopicIdentity};
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
    /// A workspace whose members commit no configuration at all: every operand
    /// is then keyed exactly as written, which is the pre-[S-410] behaviour every
    /// test below this line was written against and still pins.
    fn literal_only() -> BTreeMap<String, MemberCorpus> {
        BTreeMap::new()
    }

    fn subc(key: &str, member: &str, symbol: &str) -> BrokerCandidate {
        cand(ArtifactRelation::BrokerSubscribe, key, member, symbol)
    }

    /// `member` commits `key` (in any spelling) to one value per `(profile,
    /// value)` pair, in `application.yml` / `application-<profile>.yml`.
    fn commits(
        corpora: &mut BTreeMap<String, MemberCorpus>,
        member: &str,
        key: &str,
        values: &[(Option<&str>, &str)],
    ) {
        corpora.entry(member.to_string()).or_default().insert(
            crate::extract::config::corpus::canonical_key(key),
            values
                .iter()
                .map(|(profile, value)| crate::graph_store::ConfigDefinition {
                    path: profile
                        .map_or("application.yml".to_string(), |p| {
                            format!("application-{p}.yml")
                        }),
                    profile: profile.map(str::to_string),
                    value: (*value).to_string(),
                })
                .collect(),
        );
    }

    /// [S-410] / [FR-WS-10]'s re-proposed criterion, at the **edge**: a subscribe
    /// keyed by a hand-written placeholder and a publish whose accessor was
    /// stored in the canonical relaxed-binding spelling fan out across members,
    /// because the topic identity is neither spelling — it is the committed
    /// value both resolve to.
    ///
    /// The two operands here are the exact pair [ADR-64]'s 2026-09-15 amendment
    /// records as *not* meeting byte-equal on the reference estate.
    ///
    /// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    /// [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
    #[test]
    fn two_spellings_of_one_property_fan_out_on_the_committed_value() {
        let mut corpora = BTreeMap::new();
        for member in ["downsampler", "projector"] {
            commits(
                &mut corpora,
                member,
                "spring.kafka.topics.archive-volume-counters",
                &[(None, "archive.volume.counters.v1")],
            );
        }

        // Byte-unequal operands: the accessor hop stores the canonical spelling,
        // a source-written literal keeps its hyphens.
        let candidates = vec![
            pubc(
                "${spring.kafka.topics.archivevolumecounters}",
                "downsampler",
                "local emit_counters",
            ),
            subc(
                "${spring.kafka.topics.archive-volume-counters}",
                "projector",
                "local on_counters",
            ),
        ];

        assert!(
            broker_edges(candidates.clone(), &literal_only()).is_empty(),
            "BEFORE: with nothing committed the two spellings are two namespaces \
             and do not meet — the gap ADR-64's amendment recorded"
        );

        let edges = broker_edges(candidates, &corpora);
        assert_eq!(edges.len(), 1, "AFTER: they meet on the committed value: {edges:?}");
        assert_eq!(edges[0].from.member, "downsampler");
        assert_eq!(edges[0].to.member, "projector");
        // FR-WS-19 AC6: BOTH ends name their own key, sources and profile set —
        // which is why the edge carries two provenance fields and not one.
        for (side, value) in [("from", &edges[0].from_value), ("to", &edges[0].to_value)] {
            let Provenance::ConfigBound { bound } = value else {
                panic!("the {side} end was admitted from configuration, not {value:?}");
            };
            assert_eq!(bound.len(), 1, "{side}");
            assert_eq!(
                bound[0].key, "spring.kafka.topics.archivevolumecounters",
                "{side} names its canonical key"
            );
            assert_eq!(bound[0].values[0].value, "archive.volume.counters.v1", "{side}");
            assert_eq!(bound[0].values[0].sources, ["application.yml"], "{side}");
        }
    }

    /// A key whose overlays disagree yields **one identity per overlay** and the
    /// site fans out under each of them — never one averaged or default-profile
    /// winner ([FR-WS-19] AC2, [ADR-64] decision point 3).
    ///
    /// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
    /// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
    #[test]
    fn a_key_whose_overlays_disagree_fans_out_under_each_overlay() {
        let mut corpora = BTreeMap::new();
        commits(
            &mut corpora,
            "api",
            "spring.kafka.topics.orders",
            &[(Some("prod"), "orders-prod"), (Some("staging"), "orders-staging")],
        );

        let edges = broker_edges(
            [
                pubc("${spring.kafka.topics.orders}", "api", "local emit"),
                subc("orders-prod", "prod-worker", "local on_prod"),
                subc("orders-staging", "staging-worker", "local on_staging"),
                subc("orders-dev", "dev-worker", "local on_dev"),
            ],
            &corpora,
        );

        let reached: Vec<&str> = edges.iter().map(|e| e.to.member.as_str()).collect();
        assert_eq!(
            reached,
            ["prod-worker", "staging-worker"],
            "one publish binds under BOTH overlays, and under no third value \
             nothing commits: {edges:?}"
        );
        let Provenance::ConfigBound { bound } = &edges[0].from_value else {
            panic!("a divergent key still admits: {:?}", edges[0].from_value);
        };
        assert_eq!(
            bound[0].values.iter().map(|v| v.value.as_str()).collect::<Vec<_>>(),
            ["orders-prod", "orders-staging"],
            "every overlay's value is retained, each tagged with its profile"
        );
        assert_eq!(bound[0].profiles(), ["prod", "staging"]);
    }

    /// The **never-fabricate** half ([NFR-RA-05]). An operand whose key no
    /// committed source defines, and one whose committed value is itself a
    /// `${…}` indirection, are both admitted as *nothing*: the site keeps the
    /// placeholder-as-written key [FR-WS-10]'s rule in force gives it, and the
    /// refusal travels as `config-unresolved` rather than being swallowed.
    ///
    /// The near miss this pins is the one that matters: neither refusal may
    /// produce an edge keyed on the *indirection* — `${another.key}` is not a
    /// topic two members can meet on.
    ///
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn a_refused_key_admits_no_topic_and_keeps_its_as_written_key() {
        // (a) No committed source defines it. The two sides still meet, on the
        //     placeholder they both wrote — exactly as they did before S-410.
        let edges = broker_edges(
            [
                pubc("${spring.kafka.topics.orders}", "api", "local emit"),
                subc("${spring.kafka.topics.orders}", "worker", "local on_order"),
            ],
            &literal_only(),
        );
        assert_eq!(edges.len(), 1, "the as-written key still binds: {edges:?}");
        assert_eq!(
            edges[0].from_value,
            Provenance::ConfigUnresolved {
                keys: vec!["spring.kafka.topics.orders".to_string()],
                refusal: ValueRefusal::MissingKey,
            },
            "the edge names the key it could not resolve, under the existing reason"
        );

        // (b) The committed value is itself a placeholder. Nothing is admitted,
        //     and in particular NOT the indirection: a subscriber that literally
        //     names `${another.key}` must not meet this publish.
        let mut corpora = BTreeMap::new();
        commits(
            &mut corpora,
            "api",
            "spring.kafka.topics.orders",
            &[(None, "${another.key}")],
        );
        let edges = broker_edges(
            [
                pubc("${spring.kafka.topics.orders}", "api", "local emit"),
                subc("${another.key}", "worker", "local on_order"),
            ],
            &corpora,
        );
        assert!(
            edges.is_empty(),
            "an indirection is not a topic identity — nothing may meet on it: {edges:?}"
        );
    }

    /// The **never-fabricate** rule one layer up ([NFR-RA-05], [CR-107]): a
    /// committed value that is blank, or blank once trimmed, is no more a topic
    /// identity than an absent one.
    ///
    /// Pinned because the consequence is not obvious and is severe. Without the
    /// blank filter, two members that commit **different** keys to blank values
    /// are filed under one empty topic and fan out a real cross-service edge —
    /// a coupling asserted between two services on the strength of two empty
    /// strings. The site must instead fall back to its placeholder-as-written key
    /// under the existing [`ValueRefusal::MissingKey`].
    ///
    /// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn a_blank_committed_value_is_no_topic_and_never_meets_another_blank() {
        let mut corpora = BTreeMap::new();
        // Two DIFFERENT keys, both committed blank — one empty, one whitespace.
        commits(&mut corpora, "api", "spring.kafka.topics.orders", &[(None, "")]);
        commits(&mut corpora, "worker", "spring.kafka.topics.inbound", &[(None, "   ")]);

        let edges = broker_edges(
            [
                pubc("${spring.kafka.topics.orders}", "api", "local emit"),
                subc("${spring.kafka.topics.inbound}", "worker", "local on_order"),
            ],
            &corpora,
        );
        assert!(
            edges.is_empty(),
            "two blank committed values are not one topic — and are not a topic \
             at all: {edges:?}"
        );

        // …and the site keeps its as-written key, under the existing refusal.
        let empty = MemberCorpus::new();
        let corpus = corpora.get("api").unwrap_or(&empty);
        assert_eq!(
            topic_identity("${spring.kafka.topics.orders}", corpus),
            TopicIdentity::Unresolved {
                keys: vec!["spring.kafka.topics.orders".to_string()],
                refusal: ValueRefusal::MissingKey,
            },
            "a blank committed value admits nothing, under the existing reason"
        );
    }

    /// A committed value is **trimmed** before it becomes a topic identity, and
    /// that is load-bearing in both directions.
    ///
    /// A YAML block scalar or a trailing space makes `"orders "` a different
    /// namespace from `"orders"` — the exact class of silent miss this story
    /// exists to close — and it is the trim, not the blank filter, that turns a
    /// whitespace-only value into nothing.
    #[test]
    fn a_committed_value_is_trimmed_before_it_becomes_a_topic() {
        let mut corpora = BTreeMap::new();
        // The publisher's yaml carries a trailing space; the subscriber's does not.
        commits(&mut corpora, "api", "t.out", &[(None, "orders ")]);
        commits(&mut corpora, "worker", "t.in", &[(None, "orders")]);

        let edges = broker_edges(
            [
                pubc("${t.out}", "api", "local emit"),
                subc("${t.in}", "worker", "local on_order"),
            ],
            &corpora,
        );
        assert_eq!(
            edges.len(),
            1,
            "a padded committed value is the same topic as an unpadded one: {edges:?}"
        );
    }

    /// The `#`-guard rides the **resolved** value ([FR-WS-10] acceptance 2).
    ///
    /// Every pre-[S-410] schema-guard fixture uses literal operands, so nothing
    /// pinned the guard once the topic half became a placeholder. Losing it there
    /// would silently bind a schema-guarded publish to an unguarded subscribe —
    /// the bind [FR-WS-10] says must not happen, reappearing through the one door
    /// this story opened.
    ///
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    #[test]
    fn a_schema_guard_rides_the_resolved_value() {
        let mut corpora = BTreeMap::new();
        commits(&mut corpora, "api", "t.out", &[(None, "orders")]);

        // The guarded publish must NOT reach a subscriber on the bare topic.
        let unguarded = broker_edges(
            [
                pubc("${t.out}#com.acme.OrderCreated", "api", "local emit"),
                subc("orders", "billing", "local on_any"),
            ],
            &corpora,
        );
        assert!(
            unguarded.is_empty(),
            "the guard survives resolution, so a guarded publish does not reach an \
             unguarded subscribe: {unguarded:?}"
        );

        // …and the matching-FQN subscriber still binds, on the committed value.
        let guarded = broker_edges(
            [
                pubc("${t.out}#com.acme.OrderCreated", "api", "local emit"),
                subc("orders#com.acme.OrderCreated", "billing", "local on_created"),
            ],
            &corpora,
        );
        assert_eq!(guarded.len(), 1, "the matching-FQN pair binds: {guarded:?}");
        assert_eq!(guarded[0].to.symbol.as_str(), "local on_created");
    }

    /// A **relay** — one declaration that subscribes to a topic and re-publishes
    /// on it — keeps both of its sides through the de-duplication.
    ///
    /// The role term in `broker_edges`' dedup key is what makes that true, and
    /// nothing on this side pinned it (the promotion pass's twin hazard has been
    /// pinned since CR-080 by `site_symbol`'s role namespace). Without it the
    /// relay's publish and subscribe collapse to one endpoint and a real
    /// downstream coupling silently disappears.
    #[test]
    fn a_relay_keeps_both_of_its_sides_through_the_dedup() {
        let edges = broker_edges(
            [
                pubc("orders", "api", "local emit"),
                // One declaration, both roles, one topic.
                subc("orders", "hub", "local relay"),
                pubc("orders", "hub", "local relay"),
                subc("orders", "sink", "local on_order"),
            ],
            &literal_only(),
        );
        let pairs: Vec<(&str, &str)> = edges
            .iter()
            .map(|e| (e.from.member.as_str(), e.to.member.as_str()))
            .collect();
        assert!(
            pairs.contains(&("hub", "sink")),
            "the relay's PUBLISH side survives the dedup — its subscribe must not \
             swallow it: {pairs:?}"
        );
        assert!(
            pairs.contains(&("api", "hub")),
            "the relay's SUBSCRIBE side survives too: {pairs:?}"
        );
    }

    /// One coupling is **one** edge, however many overlays the two sides meet
    /// under ([NFR-RA-05], [CR-118]).
    ///
    /// Reachable only since [S-410]: a publish and a subscribe that commit the
    /// same key under the same two overlays are each keyed twice, so the fan-out
    /// loop meets them twice and emits two edges identical in every field —
    /// provenance included, because the `ConfigBound` evidence already names both
    /// overlay values. Two rows for one coupling would inflate
    /// `resolved_cross_service_edges` against a coverage tier that emits one row
    /// per site by construction.
    ///
    /// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn two_sides_meeting_under_two_overlays_are_one_edge_not_two() {
        let mut corpora = BTreeMap::new();
        for member in ["api", "worker"] {
            commits(
                &mut corpora,
                member,
                "spring.kafka.topics.orders",
                &[(Some("prod"), "orders-prod"), (Some("staging"), "orders-staging")],
            );
        }

        let edges = broker_edges(
            [
                pubc("${spring.kafka.topics.orders}", "api", "local emit"),
                subc("${spring.kafka.topics.orders}", "worker", "local on_order"),
            ],
            &corpora,
        );

        assert_eq!(
            edges.len(),
            1,
            "one publish and one subscribe are one coupling, even meeting under \
             both overlays: {edges:#?}"
        );
        // …and the surviving edge still names BOTH overlay values, so collapsing
        // the pair loses no evidence.
        let Provenance::ConfigBound { bound } = &edges[0].from_value else {
            panic!("the publish was admitted: {:?}", edges[0].from_value);
        };
        assert_eq!(
            bound[0].values.iter().map(|v| v.value.as_str()).collect::<Vec<_>>(),
            ["orders-prod", "orders-staging"],
            "the one surviving edge carries every overlay's value"
        );
    }

    /// A workspace whose broker operands are plain literals is **byte for byte**
    /// what it was before [S-410], whatever its members commit: an operand that
    /// names no configuration key reads nothing from configuration.
    ///
    /// [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
    #[test]
    fn a_literal_topic_is_never_resolved_however_much_is_committed() {
        let mut corpora = BTreeMap::new();
        // A committed key whose VALUE is the literal topic, and a committed key
        // whose NAME is it. Neither may touch a literal operand.
        commits(&mut corpora, "api", "orders", &[(None, "orders-v9")]);
        commits(&mut corpora, "api", "spring.kafka.topics.orders", &[(None, "orders")]);

        let with_corpus = broker_edges(
            [
                pubc("orders", "api", "local emit"),
                subc("orders", "worker", "local on_order"),
            ],
            &corpora,
        );
        let without = broker_edges(
            [
                pubc("orders", "api", "local emit"),
                subc("orders", "worker", "local on_order"),
            ],
            &literal_only(),
        );
        assert_eq!(with_corpus, without, "a literal operand is unaffected by any corpus");
        assert_eq!(with_corpus.len(), 1);
        assert_eq!(with_corpus[0].from_value, Provenance::Literal);
        assert_eq!(with_corpus[0].to_value, Provenance::Literal);
    }

    /// **[CR-107] never-fabricate guard.** Two sites whose topics were *refused*
    /// must not bind to each other. Each leaves a keyless ledger row, and an empty
    /// key is not an identity two members can meet on — so the fan-out produces
    /// **no** edge ([NFR-RA-05]).
    ///
    /// This pins the consequence rather than the classifier: with `classify`'s
    /// keyless guard removed, this exact fixture fabricates a real
    /// `broker-topic` bridge edge from `api/emitDynamic` to `billing/onDynamic` —
    /// a cross-service coupling asserted between two services that named no topic
    /// at all. Recording a refusal must never become a way to bind one.
    #[test]
    fn keyless_refusal_rows_never_fan_out_an_edge() {
        assert!(
            broker_edges([
                pubc("", "api", "local emitDynamic"),
                subc("", "billing", "local onDynamic"),
                subc("   ", "ship", "local onBlank"),
            ], &literal_only())
            .is_empty(),
            "a refused topic is reported, never matched"
        );

        // And a refusal alongside a real topic leaves the real bind untouched.
        let edges = broker_edges([
            pubc("orders", "api", "local pub_orders"),
            pubc("", "api", "local emitDynamic"),
            subc("orders", "billing", "local sub_bill"),
            subc("", "billing", "local onDynamic"),
        ], &literal_only());
        assert_eq!(edges.len(), 1, "only the keyed pair binds: {edges:?}");
        assert_eq!(edges[0].from.symbol.as_str(), "local pub_orders");
        assert_eq!(edges[0].to.symbol.as_str(), "local sub_bill");
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
        ], &literal_only());

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
        ], &literal_only());
        assert!(
            diff.is_empty(),
            "a differing schema FQN keeps the topics apart — no bind: {diff:?}"
        );

        let same = broker_edges([
            pubc("orders#com.acme.OrderCreated", "api", "local pub"),
            subc("orders#com.acme.OrderCreated", "billing", "local sub"),
        ], &literal_only());
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
        ], &literal_only());
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
        ], &literal_only());
        assert!(
            edges.is_empty(),
            "an in-repo publish→subscribe pair is owned by the local graph: {edges:?}"
        );
    }

    /// Acceptance (3): the broker arm itself is **ledger-only** — it introduces
    /// no schema migration of its own. The relation token rides the existing
    /// free `unresolved_refs.payload` column (MIGRATION_14); no new node/edge
    /// kind and no migration are added by this arm ([FR-WS-10]). The
    /// fully-migrated database's `PRAGMA user_version` reflects only later,
    /// separate migrations — the first-class-topic promotion this arm precedes
    /// ([S-255], [FR-WS-11], migration 17), CR-080's relation-aware ledger key
    /// ([S-290], migration 18), CR-121's configuration-corpus tables (S-380,
    /// migration 19), CR-096's check-run marker (S-313, migration 20),
    /// CR-140's evaluated set on that marker (S-437, migration 21) and CR-148's
    /// build-manifest facts (S-462, migration 22), CR-156's Modularity
    /// applicability flag (S-487, migration 23) and CR-152's declared-type facts
    /// (S-472, migration 24) — not the ledger-only binding under test here.
    ///
    /// [S-255]: ../../../../docs/planning/journal.md#s-255-migration-17-first-class-broker-topic-node-and-edge-kinds
    /// [S-290]: ../../../../docs/planning/journal.md#s-290-relation-aware-reference-ledger-dedup-for-broker-relays-migration-18
    #[test]
    fn the_broker_arm_introduces_no_schema_migration() {
        let store = SqliteGraphStore::open_in_memory().expect("in-memory store opens");
        assert_eq!(
            store.schema_version().expect("read PRAGMA user_version"),
            24,
            "no migration is added by the ledger-only arm itself — user_version reflects \
             only the later, separate broker-kind widening (migration 17), the \
             relation-aware ledger key (migration 18), the configuration-corpus \
             tables (migration 19), the check-run marker (migration 20), its \
             evaluated set (migration 21), the build-manifest facts (migration 22), \
             Modularity's applicability flag (migration 23) and the declared-type \
             facts (migration 24)"
        );
    }
}
