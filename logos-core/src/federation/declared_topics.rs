//! The **declared-topic relation** — `declares-topic(M → T)`, provenance
//! `config-declared` — reported apart from every observed broker edge
//! ([FR-WS-36], [ADR-72], [BR-54]).
//!
//! A member's committed `src/main` application configuration often names the
//! topics it works with under a topic-named key. Where the member's **code** has
//! no capturable or resolvable broker site on that topic, the observed broker
//! graph cannot show the member on it at all. This module reads the
//! configuration's statement and reports it as what it is: a declaration.
//!
//! # The rules ([ADR-72] decision point 1, [CR-157] A1–A4)
//!
//! - **A declaration.** Member M declares topic T when a committed value in a
//!   `src/main` application-configuration source of M ([`is_src_main`]), under a
//!   **topic-named key** ([`key_shape`]), resolves to T. A value with a `${…}`
//!   placeholder is resolved by the shipped
//!   [`topic_identity`](crate::resolve::broker_identity::topic_identity) — one
//!   hop to a committed value, profile-tagged per overlay ([FR-WS-19]); a value
//!   it refuses is [refused](RefusedDeclaration) with the existing
//!   [`ValueRefusal`] words, never guessed ([NFR-RA-05]).
//! - **A topic.** T must be a topic identity some member read **captures** — at
//!   least one promoted `Producer` or `Consumer` on it ([BR-55]). Any other value
//!   is [`declared-only`](DeclarationStatus::DeclaredOnly) and is not a topic.
//! - **Net-new.** A declaration is net-new when M has no observed `Producer` or
//!   `Consumer` on T; else it is [already observed](DeclarationStatus::Observed).
//!   A net-new pair carries the B3 [site split](SiteState) — config-only,
//!   site-refused (with the refusals) or site-uncaptured — at **member grain**: a
//!   refused broker row stores no operand, so it is tied to M, never provably to
//!   T.
//! - **Directionless.** Configuration does not prove produce or consume.
//!
//! These rules were measured before they were built: [S-488]'s gate harness
//! (`logos-core/tests/operand_resolvability/declared_topics.rs`) defined
//! `key_shape`, the `src/main` reading, the profile-scoped hop, the classifier
//! and the site split, and recorded 8 net-new pairs on the reference estate. That
//! harness is a closed gate's evidence and is left byte-identical; this module
//! is the product's own statement of the same rules, member-local where the
//! harness read a whole-estate corpus. They differ in two places:
//!
//! - **The hop's scope** — see `resolve`, below: the member, not the build module.
//! - **The configuration population.** This module reads what a member's store
//!   holds, and a store holds the configuration sources of the files its index
//!   walked. No language plugin claims the `.properties` extension, so an
//!   `application.properties` file is never an indexed file and its values
//!   have no row; the harness parsed such files itself. On the reference estate
//!   that is S-488's one `declared-only` value (`clickstream`, in
//!   `pec-services-tools`), and no net-new pair.
//!
//! # Declared, never observed ([BR-54], [ADR-72] decision point 3)
//!
//! The relation has its own types and its own headline,
//! [`DeclaredTopicHeadline::declared_topic_pairs`], beside its denominators
//! ([BR-51]). It is never a [`BridgeEdge`](super::BridgeEdge), a `Producer`, a
//! `Consumer`, a `Publishes` or a `Subscribes`, never a binding candidate and
//! never a gate input: nothing in [`bridge`](super::bridge),
//! [`coverage`](super::coverage), [`topics`](super::topics) or any member's
//! scan reads this module. It reads the coverage tier's broker rows **after**
//! that tier sealed them, and only when a net-new pair needs its site split.
//!
//! # Built in memory, on the first query ([ADR-52] decision point 4)
//!
//! [`DeclaredTopics`] builds the relation on its first query from each member's
//! committed configuration ([`MemberContracts::committed_config`]) and promoted
//! topic surface, and caches it on the members' sync-stamps. Nothing is
//! persisted and no store is migrated.
//!
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
//! [ADR-72]: ../../../docs/specs/architecture/decisions/ADR-72.md
//! [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [BR-54]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [BR-55]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
//! [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
//! [FR-WS-36]: ../../../docs/specs/requirements/FR-WS-36.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [S-488]: ../../../docs/planning/journal.md#s-488-measure-the-net-new-config-declared-topic-memberships-beyond-the-observed-broker-edges

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::Serialize;

use crate::model::BridgeNamespace;
use crate::resolve::binding::{MemberCorpus, Provenance, Resolver, ValueRefusal};
use crate::resolve::broker_identity::{topic_identity, TopicIdentity};

use super::bridge::{current_stamps, read_members, MemberContracts, StampCache, Stamps};
use super::coverage::{cross_service_coverage, CoverageState, CrossServiceCoverage, UnboundReason};
use super::registry::{AnswerScope, EngineRegistry, MemberEngine};
use super::topics::TopicSummary;

/// The provenance token every [`TopicDeclaration`] carries ([ADR-72]).
///
/// [ADR-72]: ../../../docs/specs/architecture/decisions/ADR-72.md
pub const CONFIG_DECLARED: &str = "config-declared";

/// The [`unread`](DeclaredTopicMembers::unread) reason of a member whose engine
/// did not open, or whose configuration or topic surface could not be read
/// ([ADR-53]); the cause is in the log line.
///
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
pub const UNREAD_FAILED: &str = "declared-topic facts could not be read";

// ── What a declaration is ───────────────────────────────────────────────────

/// The three shapes of a **topic-named** key ([CR-157] A1), in the order a key
/// is tested.
///
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeyShape {
    /// `spring.kafka.topics.<k>`.
    SpringKafkaTopics,
    /// The last segment **is** `topic` / `topics` (`filteraggregate.topic`).
    LastSegment,
    /// The last segment **ends in** `topic` / `topics`
    /// (`spring.kafka.template.default-topic`, canonically `defaulttopic`).
    LastSegmentSuffix,
}

/// The shape of a **canonical** configuration key, or `None` when it does not
/// name a topic.
///
/// Applied to the relaxed-binding canonical form the store holds, so
/// `default-topic` arrives as `defaulttopic` and still ends in `topic`. Any
/// other key is not read, whatever its value: a value equal to a topic name
/// under an unrelated key is a coincidence, not a declaration ([ADR-72],
/// "any-key value equality", rejected).
///
/// [ADR-72]: ../../../docs/specs/architecture/decisions/ADR-72.md
pub fn key_shape(canonical: &str) -> Option<KeyShape> {
    if canonical.starts_with("spring.kafka.topics.") {
        return Some(KeyShape::SpringKafkaTopics);
    }
    let last = canonical.rsplit('.').next().unwrap_or(canonical);
    if last == "topic" || last == "topics" {
        Some(KeyShape::LastSegment)
    } else if last.ends_with("topic") || last.ends_with("topics") {
        Some(KeyShape::LastSegmentSuffix)
    } else {
        None
    }
}

/// Whether a member-relative configuration source path is in a `src/main` tree
/// — the member's own root (`src/main/…`) or a module's (`manager/src/main/…`).
///
/// A whole path segment, so `src/maintenance/` is not `src/main/`, and a test
/// tree (`src/test/…`) is never read ([CR-157] B2).
///
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
pub fn is_src_main(path: &str) -> bool {
    path.starts_with("src/main/") || path.contains("/src/main/")
}

/// The topic half of a promoted topic key: a message-schema guard
/// (`orders#com.acme.OrderCreated`) is not part of the topic a configuration
/// value can name.
fn topic_of(key: &str) -> &str {
    key.split('#').next().unwrap_or(key)
}

/// The placeholder hop a declared value took to its topic, and the profile set
/// of the committed composition that proves it ([FR-WS-19] AC2, AC6).
///
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct PlaceholderHop {
    /// The canonical keys the declared value names, in source order.
    pub keys: Vec<String>,
    /// The profiles whose sources prove this composition, sorted.
    pub profiles: Vec<String>,
    /// Whether the unprofiled base proves it.
    pub unprofiled: bool,
}

/// What one committed value under a topic-named key declares.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Resolved {
    /// The topics it names, each with its hop (`None` for a literal value).
    Topics(Vec<(String, Option<PlaceholderHop>)>),
    /// The shipped refusal.
    Refused(ValueRefusal),
}

/// Resolve one declared value as [S-410] resolves a topic operand: the shipped
/// [`topic_identity`] decides literal / committed / refused, over `corpus` — the
/// declaring member's own `src/main` sources ([ADR-64]'s within-reach rule).
///
/// A committed hop is then read **as the declaring file's profile reads it**
/// ([CR-157] A1, "profile-tagged per overlay"). Spring under profile `p` reads
/// `application.yml` and `application-p.yml`, never another profile's file, so
/// a declaration in a `p`-profiled file admits the compositions `p` itself
/// proves (a blank one proves no topic but still overrides the base), else
/// those the unprofiled base proves, else nothing — refused
/// [`ValueRefusal::MissingKey`], the word the shipped resolver uses when no
/// committed composition exists ([NFR-RA-05]). An unprofiled declaration is read
/// under every profile, so every composition is admitted, each tagged with the
/// profiles of **its own** composition, never the union. The per-composition
/// profiles come from the same [`Resolver::resolve_template`] `topic_identity`
/// wraps.
///
/// **Scope, and how it differs from [S-488]'s harness.** The harness resolved
/// over a whole-estate corpus filtered to the declaring source's build module.
/// A member's store carries no module partition, so the member is the scope
/// here — the same rule every member-local resolver in this crate follows
/// ([`ConfigLookup`](crate::resolve::binding::ConfigLookup)). Two modules of one
/// member defining the hopped key differently are therefore both admitted,
/// where the harness admitted only the declaring module's.
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
/// [S-488]: ../../../docs/planning/journal.md#s-488-measure-the-net-new-config-declared-topic-memberships-beyond-the-observed-broker-edges
fn resolve(value: &str, corpus: &MemberCorpus, declaring: Option<&str>) -> Resolved {
    let (topics, bound) = match topic_identity(value, corpus) {
        TopicIdentity::Literal => return Resolved::Topics(vec![(value.trim().to_string(), None)]),
        TopicIdentity::Unresolved { refusal, .. } => return Resolved::Refused(refusal),
        TopicIdentity::Committed { topics, bound } => (topics, bound),
    };
    let keys: Vec<String> = bound.iter().map(|b| b.key.clone()).collect();
    let candidates = Resolver { corpus, module: "" }
        .resolve_template(value)
        .and_then(Result::ok)
        .map(|r| r.candidates)
        .unwrap_or_default();
    let proven: Vec<PlaceholderHop> = topics
        .iter()
        .map(|topic| {
            let mut profiles = BTreeSet::new();
            let mut unprofiled = false;
            for c in candidates.iter().filter(|c| c.template.trim() == topic) {
                profiles.extend(c.profiles.iter().cloned());
                unprofiled |= c.unprofiled;
            }
            PlaceholderHop {
                keys: keys.clone(),
                profiles: profiles.into_iter().collect(),
                unprofiled,
            }
        })
        .collect();
    let hops = topics.into_iter().zip(proven);
    let admitted: Vec<(String, PlaceholderHop)> = match declaring {
        None => hops.collect(),
        // Whether `profile` proves a composition is read off **every**
        // candidate, blank ones included: `topic_identity` drops a blank
        // composition from its topics, but a profile that sets the hopped key
        // to `""` still overrides the base under Spring, so it must not fall
        // back to the base's value — it proves no topic, and is refused.
        Some(profile) if candidates.iter().any(|c| c.profiles.iter().any(|p| p == profile)) => hops
            .filter(|(_, h)| h.profiles.iter().any(|p| p == profile))
            .collect(),
        Some(_) => hops.filter(|(_, h)| h.unprofiled).collect(),
    };
    if admitted.is_empty() {
        return Resolved::Refused(ValueRefusal::MissingKey);
    }
    Resolved::Topics(admitted.into_iter().map(|(t, h)| (t, Some(h))).collect())
}

// ── The relation ────────────────────────────────────────────────────────────

/// What a resolved declaration is, against the workspace's observed broker
/// graph ([CR-157] A2, A3).
///
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeclarationStatus {
    /// T is captured and M already has an observed Producer/Consumer on it.
    Observed,
    /// T is captured and M has none on it — what configuration adds.
    NetNew,
    /// No member read captures T: the value is not a topic.
    DeclaredOnly,
}

/// The decision, in one function: [CR-157] A2, then A3.
///
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
fn classify(
    member: &str,
    topic: &str,
    captured: &BTreeSet<&str>,
    observed: &BTreeMap<String, BTreeSet<String>>,
) -> DeclarationStatus {
    if !captured.contains(topic) {
        DeclarationStatus::DeclaredOnly
    } else if observed.get(member).is_some_and(|on| on.contains(topic)) {
        DeclarationStatus::Observed
    } else {
        DeclarationStatus::NetNew
    }
}

/// One broker row of the coverage tier the product refused, as a net-new
/// member's site split names it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct SiteRefusal {
    /// The row's refusal: its unbound reason (`topic-not-literal`), or
    /// `config-unresolved: <refusal>` for a configuration-bound row that kept
    /// its placeholder.
    pub reason: String,
    /// The declaration the broker site sits in.
    pub symbol: String,
}

/// Where a net-new pair's member stands with respect to broker sites
/// ([CR-157] B3) — **member grain**: a refused row stores no operand, so it is
/// tied to M, never provably to T.
///
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "site", rename_all = "kebab-case")]
pub enum SiteState {
    /// M has no broker row and no promoted topic at all.
    ConfigOnly,
    /// M has at least one broker row the product refused.
    SiteRefused {
        /// Every refused broker row of M, sorted.
        refusals: Vec<SiteRefusal>,
    },
    /// M has broker sites, none refused, none on T.
    SiteUncaptured,
    /// The coverage walk could not read M's broker sites (its contract surface
    /// or its invocation references failed to read), so the split is **not
    /// determined** — never reported as config-only or uncaptured on a read
    /// that did not happen ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    SiteUnread,
}

/// The split for one member, from its broker row count, whether it has any
/// promoted topic, and its refused rows.
fn site_state(
    broker_rows: usize,
    has_promoted_topics: bool,
    refusals: Vec<SiteRefusal>,
) -> SiteState {
    if broker_rows == 0 && !has_promoted_topics {
        SiteState::ConfigOnly
    } else if !refusals.is_empty() {
        SiteState::SiteRefused { refusals }
    } else {
        SiteState::SiteUncaptured
    }
}

/// The refusal one broker coverage row carries, or `None` when the product did
/// not refuse it. A keyed publish with no subscriber is unbound
/// `no-provider-in-workspace`, and that is an observed site, not a refusal; a
/// row whose value provenance is `config-unresolved` is a refusal whatever its
/// bucket, because the site kept its placeholder. [S-488]'s rule, recorded in
/// its finding.
///
/// [S-488]: ../../../docs/planning/journal.md#s-488-measure-the-net-new-config-declared-topic-memberships-beyond-the-observed-broker-edges
pub fn refusal_of(state: &CoverageState, provenance: &Provenance) -> Option<String> {
    if let Provenance::ConfigUnresolved { refusal, .. } = provenance {
        return Some(format!("config-unresolved: {}", refusal.label()));
    }
    match state {
        CoverageState::Unbound { reason } if *reason != UnboundReason::NoProviderInWorkspace => {
            Some(reason.as_str().to_string())
        }
        _ => None,
    }
}

/// One resolved declaration: M's configuration names T ([CR-157] A4).
///
/// One per `(member, topic, profile)` a source proves: the declaring file's
/// [`profile`](Self::profile) and, through a placeholder, the
/// [composition's own profile set](PlaceholderHop::profiles). Two keys or two
/// files of M naming the same T are two declarations of one pair.
///
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TopicDeclaration {
    /// The declaring member.
    pub member: String,
    /// The topic identity the value resolves to.
    pub topic: String,
    /// Always [`CONFIG_DECLARED`].
    pub provenance: &'static str,
    /// The canonical topic-named key.
    pub key: String,
    /// The member-relative configuration source defining it.
    pub file: String,
    /// The profile that source declares; `null` for the unprofiled base, which
    /// every profile reads.
    pub profile: Option<String>,
    /// The placeholder hop, when the value was not a literal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<PlaceholderHop>,
    /// Which topic-named shape the key has.
    pub shape: KeyShape,
    /// Observed, net-new or declared-only.
    pub status: DeclarationStatus,
    /// The member's site split — on a net-new declaration only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<SiteState>,
}

/// One topic-named value the hop refused ([NFR-RA-05]): it declares nothing.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefusedDeclaration {
    /// The declaring member.
    pub member: String,
    /// The canonical topic-named key.
    pub key: String,
    /// The member-relative source defining it.
    pub file: String,
    /// The profile that source declares; `null` for the unprofiled base.
    pub profile: Option<String>,
    /// The committed value as written.
    pub value: String,
    /// The existing refusal word.
    pub reason: ValueRefusal,
}

/// A count at two grains: distinct `(member, topic)` pairs, and the
/// declarations behind them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PairTally {
    /// Distinct `(member, topic)` pairs.
    pub pairs: u64,
    /// Declarations behind them.
    pub declarations: u64,
}

/// The net-new pairs, split by their member's broker sites ([CR-157] B3).
///
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct NetNewTally {
    /// Distinct net-new `(member, topic)` pairs.
    pub pairs: u64,
    /// Declarations behind them.
    pub declarations: u64,
    /// Pairs whose member has no broker row and no promoted topic.
    pub config_only: u64,
    /// Pairs whose member has a refused broker row.
    pub site_refused: u64,
    /// Pairs whose member has broker sites, none refused, none on the topic.
    pub site_uncaptured: u64,
    /// Pairs whose member's broker sites the coverage walk could not read —
    /// their split is not determined ([`SiteState::SiteUnread`]).
    pub site_unread: u64,
    /// Whether the coverage tier the split reads covered every member and read
    /// every net-new member's broker sites. `false` marks the split partial
    /// ([NFR-CC-04]); `true` when there is no net-new pair to split.
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub split_covers_all_members: bool,
}

/// How much of the workspace the relation was read over ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DeclaredTopicMembers {
    /// The roster size.
    pub members: u64,
    /// Members whose configuration and topic surface were read.
    pub read: u64,
    /// Members read with at least one topic-named value considered.
    pub declaring: u64,
    /// Roster members not read, by name, in roster order. Absent when none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unread: Vec<String>,
    /// Why each unread member was not read ([`UNREAD_FAILED`]). Absent when
    /// none.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub unread_reasons: BTreeMap<String, &'static str>,
}

/// The `declared_topics` headline: `declared_topic_pairs` beside its
/// denominators ([CR-157] C, [BR-51]). Nothing in it is a broker figure.
///
/// The accounting closes two ways, and the tests pin both:
///
/// - `declarations_considered` = values resolved + `refused`, where a resolved
///   value yields one declaration per topic it names;
/// - `declared_topic_pairs` = `already_observed.pairs` + `net_new.pairs`
///   (`declared_only` pairs are not topics, so they are apart), and
///   `net_new.pairs` = `config_only` + `site_refused` + `site_uncaptured` +
///   `site_unread`.
///
/// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeclaredTopicHeadline {
    /// Distinct `(member, topic)` pairs where M's configuration declares a
    /// captured topic T — observed and net-new together.
    pub declared_topic_pairs: u64,
    /// Non-empty committed values under a topic-named key in a `src/main`
    /// source of a member read — every one of them resolved or refused.
    pub declarations_considered: u64,
    /// Pairs M already shows on T through an observed Producer/Consumer.
    pub already_observed: PairTally,
    /// Pairs configuration adds, split by site.
    pub net_new: NetNewTally,
    /// Pairs whose value no member's broker site names — not a topic.
    pub declared_only: PairTally,
    /// Values the hop refused.
    pub refused: u64,
    /// The topic identities captured over the members read — what a declared
    /// value is matched against.
    pub captured_topics: u64,
    /// The members the relation was read over.
    pub members: DeclaredTopicMembers,
    /// The headline, its denominators and what is apart, as one line.
    pub summary: String,
}

/// The `declared_topics` read-model: every declaration beside its headline
/// ([FR-WS-36]).
///
/// [FR-WS-36]: ../../../docs/specs/requirements/FR-WS-36.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeclaredTopicRelation {
    /// The headline.
    pub headline: DeclaredTopicHeadline,
    /// Every resolved declaration, sorted by `(member, topic, file, key,
    /// profile)`.
    pub declarations: Vec<TopicDeclaration>,
    /// Every refused value, sorted by `(member, file, key)`.
    pub refused: Vec<RefusedDeclaration>,
    /// The members read, in roster order — what a `--repo` scope is judged
    /// against. Not serialized: the headline's [`DeclaredTopicMembers`] states
    /// it.
    #[serde(skip)]
    roster_read: Vec<String>,
}

impl DeclaredTopicRelation {
    /// Whether no topic-named value was considered at all — the workspace
    /// declares nothing, so a surface omits the section.
    pub fn is_empty(&self) -> bool {
        self.headline.declarations_considered == 0
    }

    /// Whether `member` is one the relation was read over.
    pub fn was_read(&self, member: &str) -> bool {
        self.roster_read.iter().any(|m| m == member)
    }
}

/// One member's facts as read: its committed configuration and its promoted
/// topic surface — what [`derive`] takes, one entry per member read.
pub type MemberTopicFacts = (String, MemberCorpus, Vec<TopicSummary>);

/// Every broker coverage row's member, and its refusal if any.
type BrokerRows = BTreeMap<String, (usize, Vec<SiteRefusal>)>;

/// What the B3 site split reads off one coverage walk: every member's broker
/// rows, whether the walk covered every member, and the members whose reads it
/// needed and could not make.
///
/// Built from a [`CrossServiceCoverage`] the caller already holds, so a surface
/// that walked coverage for its own answer hands it on instead of walking twice.
#[derive(Debug, Clone, Default)]
pub struct SiteEvidence {
    rows: BrokerRows,
    covers_all_members: bool,
    unread: BTreeSet<String>,
}

impl SiteEvidence {
    /// The evidence in `coverage`, beside `unread` — the members the walk
    /// needed and could not read, as the answer scope recorded them
    /// ([`AnswerScope::reads`]).
    pub fn from_coverage(coverage: &CrossServiceCoverage, unread: impl IntoIterator<Item = String>) -> Self {
        Self {
            rows: broker_rows(coverage),
            covers_all_members: coverage.covers_all_members,
            unread: unread.into_iter().collect(),
        }
    }
}

/// The broker rows of `coverage`, per member — the B3 split's input.
fn broker_rows(coverage: &CrossServiceCoverage) -> BrokerRows {
    let mut out: BrokerRows = BTreeMap::new();
    let relation = BridgeNamespace::BrokerTopic.relation();
    for row in coverage
        .references
        .iter()
        .filter(|r| r.relation == relation)
    {
        let entry = out.entry(row.from.member.clone()).or_default();
        entry.0 += 1;
        if let Some(reason) = refusal_of(&row.state, &row.provenance) {
            entry.1.push(SiteRefusal {
                reason,
                symbol: row.from.symbol.as_str().to_string(),
            });
        }
    }
    for (_, refusals) in out.values_mut() {
        refusals.sort();
    }
    out
}

/// What the walk over the members' topic-named values has recorded so far.
#[derive(Default)]
struct Intake {
    considered: u64,
    declaring: BTreeSet<String>,
    declarations: Vec<TopicDeclaration>,
    refused: Vec<RefusedDeclaration>,
}

impl Intake {
    /// Resolve one non-blank value under a topic-named `key` of `member` and
    /// record what it declares — one declaration per topic, each classified by
    /// `status` — or its refusal.
    fn record(
        &mut self,
        member: &str,
        key: &str,
        shape: KeyShape,
        def: &crate::graph_store::ConfigDefinition,
        main: &MemberCorpus,
        status: impl Fn(&str) -> DeclarationStatus,
    ) {
        self.considered += 1;
        self.declaring.insert(member.to_string());
        let topics = match resolve(&def.value, main, def.profile.as_deref()) {
            Resolved::Topics(topics) => topics,
            Resolved::Refused(reason) => {
                self.refused.push(RefusedDeclaration {
                    member: member.to_string(),
                    key: key.to_string(),
                    file: def.path.clone(),
                    profile: def.profile.clone(),
                    value: def.value.clone(),
                    reason,
                });
                return;
            }
        };
        for (topic, via) in topics {
            self.declarations.push(TopicDeclaration {
                member: member.to_string(),
                status: status(&topic),
                topic,
                provenance: CONFIG_DECLARED,
                key: key.to_string(),
                file: def.path.clone(),
                profile: def.profile.clone(),
                via,
                shape,
                site: None,
            });
        }
    }
}

/// Derive the relation from member-local facts ([ADR-72]). Pure but for
/// `sites`, which is called **only** when a net-new pair needs its site
/// split — a workspace whose declarations are all observed or declared-only
/// never pays for the coverage walk.
///
/// `roster` is the workspace's member list in manifest order; a roster member
/// absent from `facts` is reported unread.
///
/// [ADR-72]: ../../../docs/specs/architecture/decisions/ADR-72.md
pub fn derive(
    roster: &[String],
    facts: &[MemberTopicFacts],
    sites: impl FnOnce() -> SiteEvidence,
) -> DeclaredTopicRelation {
    // A2's population: every topic a member read has a Producer or Consumer on.
    let observed: BTreeMap<String, BTreeSet<String>> = facts
        .iter()
        .map(|(member, _, topics)| {
            let on = topics
                .iter()
                .filter(|t| t.producers + t.consumers > 0)
                .map(|t| topic_of(&t.topic).to_string())
                .collect();
            (member.clone(), on)
        })
        .collect();
    let captured: BTreeSet<&str> = observed.values().flatten().map(String::as_str).collect();

    let mut read = Intake::default();
    for (member, corpus, _) in facts {
        // The hop reads the member's `src/main` sources only, so a `src/main`
        // declaration never resolves through a test fixture's value.
        let main: MemberCorpus = corpus
            .iter()
            .filter_map(|(key, defs)| {
                let defs: Vec<_> = defs
                    .iter()
                    .filter(|d| is_src_main(&d.path))
                    .cloned()
                    .collect();
                (!defs.is_empty()).then(|| (key.clone(), defs))
            })
            .collect();
        for (key, defs) in &main {
            let Some(shape) = key_shape(key) else {
                continue;
            };
            for def in defs.iter().filter(|d| !d.value.trim().is_empty()) {
                read.record(member, key, shape, def, &main, |topic| {
                    classify(member, topic, &captured, &observed)
                });
            }
        }
    }
    let Intake {
        considered,
        declaring,
        mut declarations,
        mut refused,
    } = read;

    // B3: the site split, read off the coverage tier's sealed broker rows —
    // only for the members a net-new pair names, and only when there is one.
    let net_new_members: BTreeSet<&str> = declarations
        .iter()
        .filter(|d| d.status == DeclarationStatus::NetNew)
        .map(|d| d.member.as_str())
        .collect();
    let mut split_covers_all_members = true;
    let mut site_of: BTreeMap<String, SiteState> = BTreeMap::new();
    if !net_new_members.is_empty() {
        let mut evidence = sites();
        split_covers_all_members = evidence.covers_all_members;
        for member in net_new_members {
            let site = if evidence.unread.contains(member) {
                // No row was read for M, so "no row" would be a fabrication.
                split_covers_all_members = false;
                SiteState::SiteUnread
            } else {
                let (count, refusals) = evidence.rows.remove(member).unwrap_or_default();
                let promoted = observed.get(member).is_some_and(|on| !on.is_empty());
                site_state(count, promoted, refusals)
            };
            site_of.insert(member.to_string(), site);
        }
    }
    for d in declarations
        .iter_mut()
        .filter(|d| d.status == DeclarationStatus::NetNew)
    {
        d.site = site_of.get(&d.member).cloned();
    }

    declarations.sort_by(|a, b| {
        (&a.member, &a.topic, &a.file, &a.key, &a.profile, &a.via)
            .cmp(&(&b.member, &b.topic, &b.file, &b.key, &b.profile, &b.via))
    });
    refused.sort_by(|a, b| {
        (&a.member, &a.file, &a.key, &a.value).cmp(&(&b.member, &b.file, &b.key, &b.value))
    });

    let read: BTreeSet<&str> = facts.iter().map(|(m, _, _)| m.as_str()).collect();
    let unread: Vec<String> = roster
        .iter()
        .filter(|m| !read.contains(m.as_str()))
        .cloned()
        .collect();
    let members = DeclaredTopicMembers {
        members: roster.len() as u64,
        read: facts.len() as u64,
        declaring: declaring.len() as u64,
        unread_reasons: unread.iter().map(|m| (m.clone(), UNREAD_FAILED)).collect(),
        unread,
    };
    let headline = headline(
        &declarations,
        considered,
        refused.len() as u64,
        captured.len() as u64,
        &site_of,
        split_covers_all_members,
        members,
    );
    DeclaredTopicRelation {
        headline,
        declarations,
        refused,
        roster_read: roster
            .iter()
            .filter(|m| read.contains(m.as_str()))
            .cloned()
            .collect(),
    }
}

/// Count the declarations into the headline, and state it as one line.
fn headline(
    declarations: &[TopicDeclaration],
    considered: u64,
    refused: u64,
    captured_topics: u64,
    sites: &BTreeMap<String, SiteState>,
    split_covers_all_members: bool,
    members: DeclaredTopicMembers,
) -> DeclaredTopicHeadline {
    let tally = |status: DeclarationStatus| {
        let rows: Vec<&TopicDeclaration> =
            declarations.iter().filter(|d| d.status == status).collect();
        let pairs: BTreeSet<(&str, &str)> = rows
            .iter()
            .map(|d| (d.member.as_str(), d.topic.as_str()))
            .collect();
        (rows.len() as u64, pairs)
    };
    let (observed_rows, observed_pairs) = tally(DeclarationStatus::Observed);
    let (net_new_rows, net_new_pairs) = tally(DeclarationStatus::NetNew);
    let (declared_only_rows, declared_only_pairs) = tally(DeclarationStatus::DeclaredOnly);

    let mut net_new = NetNewTally {
        pairs: net_new_pairs.len() as u64,
        declarations: net_new_rows,
        split_covers_all_members,
        ..NetNewTally::default()
    };
    for (member, _) in &net_new_pairs {
        match sites.get(*member) {
            Some(SiteState::ConfigOnly) => net_new.config_only += 1,
            Some(SiteState::SiteRefused { .. }) => net_new.site_refused += 1,
            Some(SiteState::SiteUncaptured) => net_new.site_uncaptured += 1,
            Some(SiteState::SiteUnread) | None => net_new.site_unread += 1,
        }
    }
    let already_observed = PairTally {
        pairs: observed_pairs.len() as u64,
        declarations: observed_rows,
    };
    let declared_only = PairTally {
        pairs: declared_only_pairs.len() as u64,
        declarations: declared_only_rows,
    };
    let declared_topic_pairs = already_observed.pairs + net_new.pairs;
    let partial = if split_covers_all_members {
        ""
    } else {
        "; site split partial: coverage did not read every member"
    };
    let summary = format!(
        "declared_topic_pairs {declared_topic_pairs} ({} already observed, {} net-new: {} config-only, \
         {} site-refused, {} site-uncaptured, {} site-unread{partial}) from {considered} declarations considered over \
         {} of {} members read, against {captured_topics} captured topics; apart: {} declared-only \
         (not a topic), {refused} refused — declared in configuration, never an observed broker edge",
        already_observed.pairs,
        net_new.pairs,
        net_new.config_only,
        net_new.site_refused,
        net_new.site_uncaptured,
        net_new.site_unread,
        members.read,
        members.members,
        declared_only.pairs,
    );
    DeclaredTopicHeadline {
        declared_topic_pairs,
        declarations_considered: considered,
        already_observed,
        net_new,
        declared_only,
        refused,
        captured_topics,
        members,
        summary,
    }
}

// ── Built on the first query ────────────────────────────────────────────────

/// The relation for one workspace, derived on the **first** query and cached on
/// the members' sync-stamps ([ADR-52], [ADR-72] decision point 4) — the
/// [`BuildDependencies`](super::BuildDependencies) shape.
///
/// Empty at construction — building one reads nothing, so holding it on a
/// serve surface costs nothing at startup.
///
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
/// [ADR-72]: ../../../docs/specs/architecture/decisions/ADR-72.md
#[derive(Debug, Default)]
pub struct DeclaredTopics {
    cache: StampCache<DeclaredTopicRelation>,
}

impl DeclaredTopics {
    /// A holder with nothing derived yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The relation over `registry`'s members: the stamps are read first, and
    /// the facts are read and derived only on a miss — the first call, or any
    /// call after a member re-synced.
    ///
    /// A member whose engine will not open or whose reads fail is skipped with
    /// a warning and named [unread](DeclaredTopicMembers::unread) ([ADR-53]).
    ///
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    pub fn relation<E>(&self, registry: &EngineRegistry<E>) -> Arc<DeclaredTopicRelation>
    where
        E: MemberEngine + MemberContracts,
    {
        let answer = registry.answer();
        let stamps = current_stamps(&answer);
        self.relation_in(registry, &answer, stamps, None)
    }

    /// [`relation`](Self::relation) inside a caller's [`AnswerScope`], keyed on
    /// the stamps it already read — so a surface building it beside another
    /// read-model shares one open attempt per member and one stamp snapshot
    /// with it ([FR-WS-16]).
    ///
    /// `coverage` is the coverage payload the caller already computed **in this
    /// answer**, if any, handed on so a net-new pair's site split does not walk
    /// it a second time ([NFR-PE-10]). `None` walks it here, and only on a miss
    /// with a net-new pair. `workspace status` does not come through here: its
    /// all-member walks already read every member's configuration and topic
    /// surface, so it calls [`derive`] over those reads — an all-member read of
    /// its own would add a resident-engine reconstruction per member under the
    /// connection budget.
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    pub(super) fn relation_in<E>(
        &self,
        registry: &EngineRegistry<E>,
        answer: &AnswerScope<'_, E>,
        stamps: Stamps,
        coverage: Option<&CrossServiceCoverage>,
    ) -> Arc<DeclaredTopicRelation>
    where
        E: MemberEngine + MemberContracts,
    {
        self.cache.get_or_compute(stamps, || {
            let facts: Vec<MemberTopicFacts> =
                read_members(answer, "declared-topic facts", |engine| {
                    Ok((engine.committed_config()?, engine.topic_surface()?))
                })
                .into_iter()
                .map(|(member, (config, topics))| (member, config, topics))
                .collect();
            let roster: Vec<String> = registry
                .federation()
                .members
                .iter()
                .map(|m| m.name.clone())
                .collect();
            derive(&roster, &facts, || {
                let walked;
                let coverage = match coverage {
                    Some(coverage) => coverage,
                    None => {
                        walked = cross_service_coverage(answer);
                        &walked
                    }
                };
                // Read after the walk, so its own read failures are in it.
                SiteEvidence::from_coverage(coverage, answer.reads().unread.into_keys())
            })
        })
    }
}

#[cfg(test)]
#[path = "declared_topics_tests.rs"]
mod tests;
