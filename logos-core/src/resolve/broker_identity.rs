//! The **broker topic identity** rule — the one function every tier that names a
//! topic resolves its operand through ([FR-WS-27], [S-424]).
//!
//! # Why it lives in `resolve`, and not in `federation`
//!
//! [S-410] built this rule inside `federation::broker` because the federation
//! bridge was its only caller. [S-424] gave it a second and a third: the
//! intra-repo promotion pass ([`crate::resolve::topics`]) and the coverage
//! read-model. The promotion pass runs on **every single-root `logos index`**,
//! where federation is absent by design — `architecture.md` §4.1 records
//! federation's own responsibility as ending *"Absent from single-root
//! operation"*, and its declared dependencies point **at** the resolution engine,
//! not away from it.
//!
//! Leaving the rule in `federation` and widening its visibility would have
//! inverted that edge and made [ADR-52]'s reversibility claim — *"removing the
//! overlay leaves every member graph and all single-root behavior exactly as
//! before"* — false, because deleting `federation/` would then change which nodes
//! a single-root index writes. [CR-136] §4.4, §7, §9.2 and §10 all say
//! **relocate** rather than widen, and this module is that relocation. Nothing
//! here names a federation type: the rule is composed of [`crate::model`]
//! relations and [`crate::resolve::binding`] resolution, and each tier wraps the
//! answer in its own vocabulary at its own call site.
//!
//! # The two halves
//!
//! [`admit`] decides **whether** a relation and operand are a broker site at all,
//! and yields the operand's one canonical as-written spelling.
//! [`topic_identity`] decides **what** committed configuration proves about it.
//! [`identify`] is the two composed, and is what the three tiers call.
//!
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
//! [CR-136]: ../../../docs/requests/CR-136-promoted-topic-identity-is-the-committed-value.md
//! [FR-WS-27]: ../../../docs/specs/requirements/FR-WS-27.md
//! [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
//! [S-424]: ../../../docs/planning/journal.md#s-424-the-promoted-topic-inventory-keys-on-the-committed-value

use std::collections::BTreeSet;

use crate::model::{ArtifactRelation, BridgeNamespace, BridgeRole};
use crate::resolve::binding::{
    placeholder_keys, ConfigBound, ConfigLookup, Provenance, Resolver, ValueRefusal,
};

/// Admit a broker site and give its operand's **one canonical as-written
/// spelling**, or [`None`] when this is not a broker-arm site.
///
/// The single admission gate for the arm, and the only place the as-written key
/// is spelled. Two refusals live here and nowhere else:
///
/// - a relation outside the [`BrokerTopic`](BridgeNamespace::BrokerTopic)
///   namespace — an HTTP/gRPC arm, or a non-arm contract relation, is classified
///   elsewhere or not at all;
/// - a **keyless** row, which is the arm's recorded `topic-not-literal` refusal
///   ([CR-107]) and never a topic: it fans out to nothing, is indexed as nothing
///   and promotes nothing, so a refusal can never become an edge or a node
///   ([NFR-RA-05]).
///
/// **Trimmed**, and that is the one spelling of "the operand as written" in this
/// crate. The capture normalizer already trims, so this changes no stored row; it
/// matters because the promotion pass trimmed its own operand
/// (`row.target.trim()`) long before it called anything here. Two tiers agreeing
/// by coincidence is what [FR-WS-27] exists to replace with agreeing by
/// construction — and `a_whitespace_padded_operand_keys_on_its_trimmed_text` in
/// `crate::resolve::topics::tests` is what holds the trim to it.
///
/// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
/// [FR-WS-27]: ../../../docs/specs/requirements/FR-WS-27.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub(crate) fn admit(relation: ArtifactRelation, target: &str) -> Option<(String, BridgeRole)> {
    if relation.bridge_namespace()? != BridgeNamespace::BrokerTopic {
        return None;
    }
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some((trimmed.to_string(), relation.bridge_role()?))
}

/// What committed configuration proves about a broker site's topic operand —
/// the **committed-value topic identity** ([FR-WS-10] as re-proposed 2026-09-15,
/// [CR-131] §3.2 A3, delivered by [S-410]).
///
/// The three variants are the whole rule, and the third is the half a reader is
/// most likely to get wrong:
///
/// - [`Literal`](Self::Literal) — the operand carries no `${…}` at all, so no
///   configuration is read for it. Unchanged from every release before [S-410]:
///   *a topic literal is keyed by its own text, exactly as written.*
/// - [`Committed`](Self::Committed) — the operand names configuration keys and
///   the committed sources prove a value for them. **That value is the topic
///   identity**, one per profile-distinct composition, so a subscribe spelled
///   `${spring.kafka.topics.archive-volume-counters}` and a publish whose
///   accessor resolved to the canonical `${spring.kafka.topics.archivevolumecounters}`
///   meet — they resolve to one committed value even though the two placeholder
///   spellings are not byte-equal. Closing that spelling gap is what [ADR-64]'s
///   2026-09-15 amendment handed this story, and it is closed **at the value**,
///   never by rewriting how a literal row is stored.
/// - [`Unresolved`](Self::Unresolved) — the operand names keys and the sources
///   prove nothing (or prove only a further indirection). **No topic is
///   fabricated** ([NFR-RA-05]), and the site keeps the placeholder-as-written
///   key it had before [S-410]: [FR-WS-10]'s re-proposed criterion says so
///   outright — *a literal that resolves to nothing keeps the
///   placeholder-as-written key*. The refusal still travels, as
///   [`Provenance::ConfigUnresolved`], so the row names the key and the existing
///   reason rather than reading as an ordinary literal.
///
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
/// [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopicIdentity {
    /// No `${…}` placeholder in the operand: the topic is the operand's own
    /// text and no configuration was read.
    Literal,
    /// The committed sources prove the operand's value.
    ///
    /// The keys are deliberately **not** repeated here: each entry of `bound`
    /// carries its own canonical `key`, so a second list would be a copy that
    /// can go stale. [`Unresolved`](Self::Unresolved) has no such carrier and
    /// therefore does name its keys.
    Committed {
        /// One topic identity per profile-distinct committed composition,
        /// sorted and de-duplicated. Two entries mean the overlays disagree and
        /// **both** are retained — the site binds under each ([FR-WS-19] AC2).
        ///
        /// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
        topics: Vec<String>,
        /// The provenance of each key: defining sources and profile set.
        bound: Vec<ConfigBound>,
    },
    /// The operand names keys and the committed sources admit no value.
    Unresolved {
        /// The canonical configuration keys the operand names, in source order.
        keys: Vec<String>,
        /// Why they admitted nothing — the **existing** value-refusal
        /// vocabulary, never a reason minted for this arm.
        refusal: ValueRefusal,
    },
}

/// Resolve one broker site's topic operand against `corpus` — the member's own
/// committed configuration ([ADR-64]'s within-reach rule is the caller's, which
/// reads one member's corpus and no other's).
///
/// `target` is the arm-normalized topic key as the ledger stores it: a topic
/// name, optionally `#`-guarded by a message-schema FQN. A guard is carried
/// through the substitution untouched, so `${x}#com.acme.Foo` resolves its topic
/// half and keeps guarding on the same FQN.
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
pub fn topic_identity(target: &str, corpus: &dyn ConfigLookup) -> TopicIdentity {
    let Some(keys) = placeholder_keys(target) else {
        return TopicIdentity::Literal;
    };
    let resolver = Resolver { corpus, module: "" };
    match resolver.resolve_template(target) {
        // `placeholder_keys` above already said the target carries a placeholder,
        // so `resolve_template` cannot answer `None` here. Mapped to the literal
        // rule rather than unwrapped: a panic in a read-model is never the right
        // answer to a disagreement between two scans (the same choice
        // `record_config_bound` makes for the identical impossibility).
        None => TopicIdentity::Literal,
        Some(Err(refusal)) => TopicIdentity::Unresolved { keys, refusal },
        Some(Ok(resolved)) => {
            let topics: BTreeSet<String> = resolved
                .candidates
                .iter()
                .map(|candidate| candidate.template.trim().to_string())
                .filter(|topic| !topic.is_empty())
                .collect();
            if topics.is_empty() {
                // Every committed composition is blank. A blank string is no more
                // a topic identity than an absent one — the same test `classify`
                // applies to a keyless row — so nothing is admitted and the site
                // falls back to its placeholder-as-written key. Reported under
                // the refusal an empty corpus gives, because that is what the
                // sources proved: no value.
                return TopicIdentity::Unresolved {
                    keys,
                    refusal: ValueRefusal::MissingKey,
                };
            }
            TopicIdentity::Committed {
                topics: topics.into_iter().collect(),
                bound: resolved.bound,
            }
        }
    }
}

/// Every topic key one broker site meets on, and the provenance of the value
/// behind them — the answer [`identify`] gives, and the **one** topic identity
/// every tier that names a topic is keyed by ([FR-WS-27], [S-424]).
///
/// Keys are plain topic strings rather than [`PortableKey`]s because the bridge
/// is no longer the only caller: the intra-repo promotion pass
/// ([`crate::resolve::topics`]) names a `Topic`/`Producer`/`Consumer` node by
/// this same value and has no portable key to wrap it in. Each tier wraps what
/// it needs at its own call site — the bridge into
/// `PortableKey::broker`, the promotion pass into a symbol
/// descriptor — from **one** resolution, which is the whole point of the shape.
///
/// [FR-WS-27]: ../../../docs/specs/requirements/FR-WS-27.md
/// [S-424]: ../../../docs/planning/journal.md#s-424-the-promoted-topic-inventory-keys-on-the-committed-value
#[derive(Debug, Clone)]
pub(crate) struct BrokerIdentity {
    /// One topic key per profile-distinct committed composition, sorted and
    /// de-duplicated — or exactly one, the operand as written and trimmed, for a
    /// literal and for an operand the corpus refuses.
    pub(crate) topics: Vec<String>,
    /// Which side of the **bridge's fan-out** the site is, and the name says
    /// `bridge_` because the orientation is the *inverse* of what a reader
    /// expects: a **publish** is the [`Consumer`](BridgeBridgeRole::Consumer) (the edge
    /// `from`) and a **subscribe** the [`Provider`](BridgeBridgeRole::Provider), because
    /// a consumer binds every provider of its key.
    ///
    /// The intra-repo promotion pass names the same two sides
    /// `Producer`/`Consumer` with the opposite sense, and it therefore derives its
    /// `NodeKind` from the relation rather than from this field. **Deriving it
    /// from here would silently swap every producer and consumer in the
    /// inventory** — the field is named to make that trade visible at the use
    /// site rather than only in a comment.
    pub(crate) bridge_role: BridgeRole,
    /// The provenance of the **site's** value — what a coverage row carries, and
    /// what the bridge files a provider under.
    ///
    /// The promotion pass deliberately **drops** it: a promoted
    /// `Topic`/`Producer`/`Consumer` node carries no provenance field, which is
    /// why the service map borrows a hop's provenance from the binding it stands
    /// for rather than from the inventory. Named here so a fourth caller does not
    /// assume the promoted subgraph records it.
    pub(crate) value: Provenance,
}

/// Reduce a broker relation + its stored topic operand to **every** topic key
/// the site meets on, its role, and the provenance of the value behind those
/// keys ([S-410], carried into the promotion pass by [S-424]).
///
/// The committed-value twin of [`admit`], and **the single place the
/// committed-value rule is applied**. Three callers, in three tiers, and that is
/// the requirement rather than an implementation detail ([FR-WS-27] AC1,
/// [ADR-52]):
///
/// - the bridge's cross-member fan-out (`crate::federation::broker::broker_edges`);
/// - the coverage read-model (`crate::federation::coverage::arm_identity`); and
/// - the intra-repo promotion pass (`crate::resolve::topics`), which keys its
///   `Topic`/`Producer`/`Consumer` nodes on the same answer.
///
/// So *"why did this bind"*, *"why didn't this bind"* and *"what node is this"*
/// cannot drift the way they could if each tier resolved its own operands —
/// which is exactly what happened between [S-410] and [S-424], while both tiers
/// were individually correct. **A predicate duplicated at two call sites does
/// not satisfy [FR-WS-27]; one function does.**
///
/// **A `Vec` of keys, not one key**, because overlays are allowed to disagree: a
/// key two profiles commit differently yields one identity per overlay and the
/// site is indexed — or fans out, or promotes — under each of them
/// ([FR-WS-19] AC2). A literal, and an operand the corpus refuses, both yield
/// exactly one.
///
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
/// [FR-WS-27]: ../../../docs/specs/requirements/FR-WS-27.md
/// [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
/// [S-424]: ../../../docs/planning/journal.md#s-424-the-promoted-topic-inventory-keys-on-the-committed-value
pub(crate) fn identify(
    relation: ArtifactRelation,
    target: &str,
    corpus: &dyn ConfigLookup,
) -> Option<BrokerIdentity> {
    // `admit` first, so every refusal it already makes — a non-broker relation,
    // a keyless row — is made in exactly one place and nothing below can
    // re-admit one. It also yields the operand's one canonical as-written
    // spelling, which the bridge wraps into its own key type unchanged.
    let (as_written, bridge_role) = admit(relation, target)?;
    // `role` is independent of provenance — it comes from the relation, which no
    // branch below touches. Stated once here rather than repeated three times,
    // which is what the earlier shape did and what hid the invariant.
    let (topics, value) = match topic_identity(target, corpus) {
        TopicIdentity::Literal => (vec![as_written], Provenance::Literal),
        TopicIdentity::Committed { topics, bound } => (topics, Provenance::ConfigBound { bound }),
        TopicIdentity::Unresolved { keys, refusal } => (
            vec![as_written],
            Provenance::ConfigUnresolved { keys, refusal },
        ),
    };
    Some(BrokerIdentity {
        topics,
        bridge_role,
        value,
    })
}


#[cfg(test)]
mod tests {
    use super::*;

    /// [`admit`] maps the arm's relations onto the fan-out `BrokerTopic`
    /// namespace by its pure descriptors, and refuses a non-broker relation —
    /// proving it is scoped to this arm and drives the generic namespace rather
    /// than a hardcoded match.
    ///
    /// Lifted here from `federation::broker` by [S-424] along with the rule it
    /// guards: the admission is no longer the bridge's, so neither is its test.
    ///
    /// [S-424]: ../../../docs/planning/journal.md#s-424-the-promoted-topic-inventory-keys-on-the-committed-value
    #[test]
    fn admit_maps_broker_relations_and_refuses_others() {
        let (key, role) = admit(ArtifactRelation::BrokerPublish, "orders").unwrap();
        assert_eq!(key, "orders");
        assert!(
            matches!(role, BridgeRole::Consumer),
            "a publish is the bridge's consumer side"
        );

        let (_, role) = admit(ArtifactRelation::BrokerSubscribe, "orders").unwrap();
        assert!(matches!(role, BridgeRole::Provider), "a subscribe provides");

        assert!(
            admit(ArtifactRelation::Route, "GET /x").is_none(),
            "a non-broker relation is not this arm's site"
        );

        // A **keyless** row is the arm's recorded `topic-not-literal` refusal
        // ([CR-107]), never a topic, so it admits nothing. This guard is on a
        // live path, not defence in depth: `ContractBridge::compute_edges` builds
        // its broker candidates straight from each member's raw ledger, refusal
        // rows included, and the promotion pass walks the same rows.
        for relation in [
            ArtifactRelation::BrokerPublish,
            ArtifactRelation::BrokerSubscribe,
        ] {
            assert!(admit(relation, "").is_none(), "{}", relation.as_str());
            assert!(admit(relation, "   ").is_none(), "{}", relation.as_str());
        }
    }

    /// The **trim** is load-bearing and this is what holds it to that.
    ///
    /// Without it a whitespace-padded operand keys as `" orders "` and never
    /// meets `orders` — in *both* tiers, since both read the same `admit`, so no
    /// cross-tier equality check can catch it. The capture normalizer trims
    /// today, which is an invariant of a different module; this pins the
    /// consequence here rather than trusting it from a distance.
    #[test]
    fn a_whitespace_padded_operand_admits_on_its_trimmed_text() {
        let (padded, _) = admit(ArtifactRelation::BrokerPublish, "  orders  ").unwrap();
        let (bare, _) = admit(ArtifactRelation::BrokerSubscribe, "orders").unwrap();
        assert_eq!(padded, "orders", "the as-written key is the TRIMMED operand");
        assert_eq!(padded, bare, "so a padded publish meets a bare subscribe");
    }
}
