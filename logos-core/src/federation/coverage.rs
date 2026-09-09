//! The **3-state cross-service coverage** read-model ([FR-WS-05], [ADR-53]).
//!
//! Extends the [bridge](super::bridge)'s binary bound/not-bound match outcome
//! into a reason-annotated advisory tier: every cross-boundary reference — an
//! `ApiOperation` HTTP consumer node, an outbound HTTP client-call site, a
//! broker publish/subscribe, and a `GrpcCall` stub-call ledger consumer
//! (S-253, [FR-WS-09]) — is classified **bound**, **ambiguous**, or **unbound**,
//! and each non-bound reference carries a [`CoverageState`] naming *why*. The
//! `GrpcCall` classifier below is real and tested; naming it here is not a
//! claim that a `grpc-call` row can occur in production today — it cannot, no
//! plugin captures one, [S-379] records why the arm is retained regardless.
//!
//! [FR-WS-09]: ../../../docs/specs/requirements/FR-WS-09.md
//! [S-379]: ../../../docs/planning/journal.md#s-379-the-grpc-invocation-arm-is-marked-honestly-absent
//!
//! # Advisory only ([ADR-53])
//! This module is **never** called from `scan`, `gate`, or `check_rules`
//! ([`crate::governance`]) — those operate on a single [`crate::Engine`] and
//! have no dependency on `federation` at all, so the coverage tier is
//! structurally incapable of moving the gate. [`cross_service_coverage`] is
//! reachable only through an [`EngineRegistry`], which itself exists only when
//! a workspace manifest is present ([`Backing::Federated`](super::Backing)) —
//! the single-root path never constructs one, so this tier is inert with no
//! manifest ([FR-WS-05]).
//!
//! # `no-provider-in-workspace` is bucketed separately ([ADR-53])
//! A reference whose key has no provider anywhere in the workspace is not a
//! defect — the provider repo simply isn't a member of this workspace. It is
//! reported in its own bucket, excluded from the `spec_conformance_ratio`
//! denominator, so a sparse workspace reads as *measured*, not broken.
//!
//! [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
//! [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md


use serde::{Deserialize, Serialize};

use crate::model::{BridgeNamespace, BridgeRole, MatchDiscipline, NodeKind};
use crate::resolve::http_client_call::ClientCallRefusal;

use super::bridge::{
    bucket_candidates, classify, consumer_portable_key, index_provider, read_members,
    sort_buckets, BridgeEndpoint, BridgeIntake, MemberContracts, PortableKey, ProviderIndex,
    Role,
};
use super::registry::{EngineRegistry, MemberEngine};

/// Why one cross-boundary reference did not bind ([FR-WS-05], [ADR-53]).
///
/// **Every variant is a reason some arm reaches from an index run.**
/// `BaseUrlRuntime` was the one exception until S-374: it is the coverage word
/// for [ADR-54]'s base-URL-composition accuracy ceiling, and the HTTP
/// client-call arm now records one keyless ledger row per declining declaration,
/// which this tier reports under it, via [`client_call_refusal`] and the
/// `From<ClientCallRefusal>` impl below. It is no longer forward-declared
/// vocabulary.
///
/// A `SchemaMismatch` variant used to sit here, described as forward-declared
/// vocabulary for "the gRPC/broker/GraphQL invocation arms ([ADR-54])". S-378
/// removed it: [ADR-54] defines no GraphQL arm and no schema check anywhere —
/// it enumerates exactly three accuracy ceilings (base-URL composition,
/// un-joined route prefixes, dynamic topics), each of which already has its
/// own variant above. So the variant was not deferred vocabulary awaiting its
/// arm; it was vocabulary no decision in this repository ever anticipated, and
/// a reason the payload can never carry is the advertised-but-empty capability
/// [NFR-CC-04] disfavours. Re-adding it is additive and needs no migration
/// ([CR-120] §7), so the schema arm that one day wants it loses nothing.
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
/// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnboundReason {
    /// No member in this workspace exposes a matching provider — outside this
    /// workspace's boundary, not a broken binding ([ADR-53]).
    NoProviderInWorkspace,
    /// The reference's own template could not be reduced to a portable key
    /// (e.g. a catch-all route) — never approximately matched ([NFR-RA-05]).
    PathNotComposed,
    /// The provider's address is resolved only at runtime (a dynamic base
    /// URL) — [ADR-54]'s base-URL-composition accuracy ceiling.
    ///
    /// **The HTTP client-call arm's recorded refusal** (S-374, [CR-120]): a call
    /// site the arm captured whose path is not a static absolute literal — a bare
    /// variable, a base-URL join, an interpolated string, a builder lambda, a
    /// helper-method return — leaves one keyless `unresolved_refs` row, and an
    /// empty HTTP target is exactly what [`client_call_refusal`] reads as this
    /// reason. Before S-374 such a site left nothing at all, so an estate whose
    /// client paths are all composed at runtime was indistinguishable from one
    /// with no outbound calls — the same sparsity-as-absence dishonesty
    /// [CR-107] corrected on the broker arm ([NFR-CC-04]).
    ///
    /// **What it does not cover, so the count is not over-read.** A call the
    /// arm's per-language query never matched — a stated capture ceiling, a
    /// receiver the S-375 rule declines, a language shipping no `invocations`
    /// query — is refused before any site exists, so it carries no reason at all
    /// and is absent from this bucket rather than counted in it. The population
    /// is enumerated once, on `extract::capture_http_client_call_arm`.
    ///
    /// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    BaseUrlRuntime,
    /// Two or more providers expose the same key across the workspace — never
    /// fabricated ([NFR-RA-05]).
    Ambiguous,
    /// A broker site's topic operand is **not a static string literal** — a
    /// constant reference, a variable, a concatenation — so no topic identity
    /// exists to match on and none is fabricated ([NFR-RA-05], [FR-WS-10] AC3).
    ///
    /// The broker arm's counterpart to [`PathNotComposed`](UnboundReason::PathNotComposed),
    /// and the reason that makes the refusal *visible*: before [CR-107] a refused
    /// topic left no reference, no ledger row and no coverage entry, so a Spring
    /// estate whose topics are all externalised read exactly like one with no broker
    /// wiring at all — the sparsity-indistinguishable-from-absence dishonesty
    /// [NFR-CC-04] forbids. What a captured literal *contains* is not a refusal
    /// ground: `"${spring.kafka.topics.orders}"` is a static literal and binds.
    ///
    /// **Both roles of the arm report under it, for different capture reasons.**
    /// [CR-107] reached this reason from the arm's *provider* role only — the
    /// `@KafkaListener(topics = TOPIC)` subscribe. S-370 / [CR-117] adds the
    /// *consumer* role: a `MessageBuilder…setHeader(KafkaHeaders.TOPIC, …)` publish
    /// site, which the arm did not recognise at all until then because it looked for
    /// a topic in argument position and idiomatic Spring puts it in a header. That
    /// half is where the reason does most of its work in practice — measured on the
    /// 84-member reference estate, **all 54** header-form sites carry a non-literal
    /// operand (16 a method parameter, 35 a `@ConfigurationProperties` getter), so
    /// the arm's honest output there is 54 refusals and zero producers. Reading a
    /// `topic-not-literal` count as a capture defect would therefore be a
    /// misreading: it is the estate's own configuration style, reported rather than
    /// hidden.
    ///
    /// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
    /// [CR-117]: ../../../docs/requests/CR-117-broker-publish-capture-and-the-topic-key-namespace.md
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    TopicNotLiteral,
}

/// The reason an **unkeyable** invocation reference is reported under, chosen by the
/// arm's own [`BridgeNamespace`] rather than by a hardcoded default ([ADR-54]).
///
/// One stored ledger row that does not reduce to a portable key means different
/// things per arm, and reporting one arm's word for it across all of them is the
/// classifier drift this module exists to prevent: an HTTP consumer's template did
/// not *compose*, a broker site's topic was not a *literal*.
///
/// **Two of the three arms have their own word, and the HTTP arm has two of its
/// own.** A broker site's topic was not a *literal*; an HTTP consumer's path was
/// either never a static literal (`base-url-runtime`) or was one that did not
/// *compose* (`path-not-composed`), which is why this function also reads the
/// stored `target` — see [`client_call_refusal`]. An unkeyable gRPC row still
/// reads `path-not-composed`, and a `package.Service/Method` FQN is not a path —
/// but that is the pre-[CR-107] behaviour for every arm, and giving gRPC its own
/// reason is a change to the [FR-WS-05] reason set that no acceptance criterion
/// here asks for. Stated rather than left for a reader to discover from the `_`
/// arm. [S-379] confirms this branch needs no change: the honest-absence gap it
/// closes is the capability advertisement (a `GrpcCall` row that can never
/// exist), not this reason mapping (which is already correct for a `GrpcCall`
/// row *if* one ever did).
///
/// Every arm's normalizer refuses before the ledger, so a row reaching here is
/// either a refusal the arm deliberately recorded (the broker arm's keyless
/// row, [CR-107]; the HTTP arm's, [CR-120]) or a target that stopped
/// normalizing — both honestly unbound, neither fabricated.
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
/// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
/// [S-379]: ../../../docs/planning/journal.md#s-379-the-grpc-invocation-arm-is-marked-honestly-absent
fn unkeyable_reason(
    relation: crate::model::ArtifactRelation,
    target: &str,
) -> UnboundReason {
    match relation.bridge_namespace() {
        Some(BridgeNamespace::BrokerTopic) => UnboundReason::TopicNotLiteral,
        // The HTTP arm has TWO words, and the stored target says which. It is
        // the only arm whose normalizer distinguishes "no static path was
        // present" from "the path was present and would not normalize", so it is
        // the only one whose reason is read from the row rather than from the
        // relation alone — through the arm's own vocabulary, never a second
        // mapping (S-374, [CR-120]).
        Some(BridgeNamespace::Http) => UnboundReason::from(client_call_refusal(target)),
        _ => UnboundReason::PathNotComposed,
    }
}

/// Which [`ClientCallRefusal`] a stored, unkeyable HTTP client-call row records
/// (S-374, [CR-120]).
///
/// The arm's two refusals are distinguished in the ledger by **whether a target
/// was stored at all**, which is the same distinction the normalizer makes:
///
/// - an **empty** target is the arm's recorded keyless refusal — the path was not
///   a static absolute literal, so no `"METHOD /template"` candidate ever
///   existed to store ([`ClientCallRefusal::BaseUrlRuntime`]);
/// - a **non-empty** target that nonetheless does not key is a
///   `"METHOD /template"` the arm accepted and `route_key` later declined
///   ([`ClientCallRefusal::PathNotComposed`]) — the word every unkeyable HTTP row
///   read before S-374, now expressed through the arm's own vocabulary instead
///   of a hardcoded default.
///
/// **The second branch is defensive, not a live population.** The arm stores a
/// target only after `classify_client_call` accepted it, and it accepts one only
/// when `route_key` succeeds — which is the same test
/// [`consumer_portable_key`](super::bridge::consumer_portable_key) applies here.
/// So no store this binary writes can hold a non-empty HTTP target that fails to
/// key; the branch is reachable only from a row written by an older binary whose
/// target no longer normalizes. It is kept because that is exactly the row it
/// should label, and because collapsing it into the keyless case would report a
/// stored template as `base-url-runtime`.
///
/// Trimmed, because an all-whitespace target is no more of a candidate than an
/// absent one — the same test the bridge's keyless-broker guard applies.
///
/// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
fn client_call_refusal(target: &str) -> ClientCallRefusal {
    if target.trim().is_empty() {
        ClientCallRefusal::BaseUrlRuntime
    } else {
        ClientCallRefusal::PathNotComposed
    }
}

/// The wire word one coverage row is filed under — the arm's own
/// [`BridgeNamespace`] relation name where it has one, else the relation's own
/// token ([ADR-54]).
///
/// One home, so a *consumer* row and a *provider* row on the same arm can never be
/// filed under two different words: this module's `Tally` exists for the same
/// reason on the counters ("one `record` point, so a state and its counter can
/// never drift apart"), and the reason vocabulary already has one in
/// [`unkeyable_reason`].
///
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
fn arm_relation(relation: crate::model::ArtifactRelation) -> String {
    relation
        .bridge_namespace()
        .map(|ns| ns.relation().to_string())
        .unwrap_or_else(|| relation.as_str().to_string())
}

// ── the route-composition refusal has no mapping here, by construction ──────
//
// An `impl From<RouteRefusal> for UnboundReason` used to sit at this point,
// mapping the framework pass's composition refusal onto `PathNotComposed`. It
// had no runtime caller and its own doc comment said so; S-378 removed it, and
// the removal is structural rather than tidying.
//
// A `RouteRefusal` is a provider-side event: a route *registration* the
// framework pass declined to promote. This tier classifies consumer-side
// references, so a refused registration produces no row here for any reason to
// label — a consumer naming that endpoint reads `no-provider-in-workspace`, the
// bucket ADR-53 deliberately holds outside the ratio denominator. That is what
// makes it unlike the `ClientCallRefusal` mapping below, which S-374 made
// live: a declined *call site* is consumer-side, so it has a row.
// `a_registration_that_promoted_no_route_reads_no_provider_not_path_not_composed`
// in this file's tests pins that classification.
//
// What the framework pass reports instead — the per-run
// `FrameworkStats::routes_not_composed` count — and why the comment removed here
// misquoted FR-FW-05 to claim otherwise, is recorded once, on the `RouteRefusal`
// enum in `resolve/framework.rs`. That is where the statistic is produced, and
// this file no longer imports the type; a second copy of the argument here is
// how the two would drift.

impl From<ClientCallRefusal> for UnboundReason {
    /// Map the HTTP client-call arm's refusal ([`ClientCallRefusal`], S-252) onto
    /// the shared coverage vocabulary — so a call the arm's normalizer refused
    /// (contributing no reference, [FR-WS-08]) surfaces under the *same* reason
    /// bucket the read-model reports for the composable cases. This is where
    /// "the normalizer returned `None`" becomes an advisory coverage reason.
    ///
    /// **Live from an index run since S-374 ([CR-120]).** The refusal had to
    /// reach a ledger row before this tier could label it, and the arm now writes
    /// one — a keyless `unresolved_refs` row per declining declaration. The
    /// runtime path is [`unkeyable_reason`] → [`client_call_refusal`] → here,
    /// reached for every unkeyable row of the
    /// [`Http`](BridgeNamespace::Http) namespace, so [FR-WS-08] AC2's surfacing
    /// promise is met. This impl's previous doc comment disclosed the opposite
    /// ("nothing calls this at runtime today"); the disclosure is removed rather
    /// than softened, because it is no longer true.
    ///
    /// **Precisely which arm is live:** the `BaseUrlRuntime` one. The
    /// `PathNotComposed` arm is reached only from a stored target that fails
    /// `route_key`, and the arm never stores one that would — see
    /// [`client_call_refusal`], which states the bound. Saying "both arms are
    /// production paths" would repeat, for a variant rather than a reason, the
    /// over-claim S-378 removed from the sibling impl this comment goes on to
    /// describe.
    ///
    /// The same qualifier once covered a sibling `From<RouteRefusal>` impl, which
    /// S-378 removed for having no producer at all. The two were never the same
    /// case: a `RouteRefusal` is a **provider**-side event and this tier
    /// classifies consumer-side references, so it had no row here to label —
    /// whereas a declined *call site* is consumer-side and does. The comment
    /// above the removal states that asymmetry once.
    ///
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    /// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
    fn from(refusal: ClientCallRefusal) -> Self {
        match refusal {
            ClientCallRefusal::BaseUrlRuntime => UnboundReason::BaseUrlRuntime,
            ClientCallRefusal::PathNotComposed => UnboundReason::PathNotComposed,
        }
    }
}

/// The 3-state classification of one cross-boundary reference ([FR-WS-05],
/// [ADR-53]).
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum CoverageState {
    /// Bound to exactly one provider in another member.
    Bound,
    /// Not bound, with a [`reason`](UnboundReason) attributed.
    Unbound {
        /// Why this reference did not bind.
        reason: UnboundReason,
    },
}

impl CoverageState {
    /// The human-facing 3-state bucket this classification renders as:
    /// `"bound"`, `"ambiguous"`, or `"unbound"` ([FR-WS-05]).
    ///
    /// `Unbound { reason: Ambiguous }` renders as its own `"ambiguous"`
    /// bucket rather than folding into `"unbound"` — the 2+-providers case is
    /// visually distinct from every other non-binding reason ([ADR-53]).
    pub fn bucket(&self) -> &'static str {
        match self {
            CoverageState::Bound => "bound",
            CoverageState::Unbound {
                reason: UnboundReason::Ambiguous,
            } => "ambiguous",
            CoverageState::Unbound { .. } => "unbound",
        }
    }
}

/// How many providers one coverage row lists before it truncates ([CR-118],
/// [NFR-CC-04]).
///
/// The measured ceiling on the 84-member reference workspace is **four** members
/// co-serving one normalized template — three aggregators re-exposing the paths
/// they proxy, plus the origin service — so eight is double the observed worst
/// case. That is headroom enough that the aggregator pattern this field exists to
/// make legible is never itself truncated, while a pathological bucket still
/// cannot grow the payload without limit. Truncation is never silent:
/// [`ProviderCandidates::omitted`] states the remainder and
/// [`ProviderCandidates::summary`] says it in words.
///
/// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
const CANDIDATE_LIMIT: usize = 8;

/// What the providers listed on a coverage row *are* to that row ([CR-118]).
///
/// A bound set and a tied set are the same shape on the wire and mean opposite
/// things, so the object that carries them carries its own meaning: a consumer
/// holding a [`ProviderCandidates`] alone — as the web wire type declares it —
/// need not reach up to the row's [`bucket`](ReferenceCoverage::bucket) to know
/// whether anything was reached. The two cannot disagree; both come from one
/// [`tier`] match arm.
///
/// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderDisposition {
    /// **Every** listed provider is bound — the fan-out discipline's shape
    /// ([FR-WS-10]): one publish reaches every cross-member subscriber, so a
    /// bound broker row names a *set* rather than a single
    /// [`to`](ReferenceCoverage::to).
    ///
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    BoundTo,
    /// The listed providers **tied** at the exactly-one test: not one of them is
    /// bound, no edge exists, and the row's `state` stays `unbound`. Naming a
    /// candidate is not binding to it ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    TiedBetween,
}

/// The providers named on one coverage row — bounded, and never silently trimmed
/// ([CR-118], [NFR-CC-04]).
///
/// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderCandidates {
    /// Whether these providers are bound or merely tied ([`ProviderDisposition`]).
    /// Declared first because it is the field that decides what every other field
    /// here means.
    pub disposition: ProviderDisposition,
    /// The providers themselves, at most [`CANDIDATE_LIMIT`] of them, in the
    /// bridge's own `(member, symbol)` bucket order ([NFR-RA-06]).
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub providers: Vec<BridgeEndpoint>,
    /// How many providers there were **before** truncation.
    pub total: u64,
    /// How many of [`total`](Self::total) are omitted from
    /// [`providers`](Self::providers) — `0` when nothing was truncated, and
    /// stated even then, so a reader never has to infer the absence of truncation
    /// from a list length ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub omitted: u64,
    /// The set, never presented bare: its size, how much of it is listed, and what
    /// it means — e.g. `"4 tied providers, all listed; none bound"`,
    /// `"12 bound providers (fan-out), 8 listed, 4 omitted"`, or, at the ordinary
    /// two-service arity, `"1 bound provider (fan-out), all listed"`.
    ///
    /// The prose is the one thing a reader gets from a pretty-printed read-model
    /// that the sibling fields do not spell out — `disposition` names the meaning
    /// and `total`/`omitted` the arithmetic, but only this line puts "none bound"
    /// beside a list of four members, which is the misreading it exists to
    /// prevent. It is *derived*, so it is also the only field here that can be
    /// wrong on its own; it is asserted verbatim at every arity it can render.
    ///
    /// Kin to [`CrossServiceCoverage::spec_conformance_summary`] ([CR-111]) in shape,
    /// though not in force: that line carries a denominator the payload otherwise
    /// hides, whereas this one restates siblings that are present.
    ///
    /// [CR-111]: ../../../docs/requests/CR-111-bound-ratio-carries-its-denominator.md
    pub summary: String,
}

impl ProviderCandidates {
    /// Bound `providers` to [`CANDIDATE_LIMIT`] and compose the disclosure line.
    ///
    /// `total` is read **before** the truncation, so the remainder is the real
    /// one and not a count of what happened to survive.
    fn new(disposition: ProviderDisposition, mut providers: Vec<BridgeEndpoint>) -> Self {
        let total = providers.len() as u64;
        providers.truncate(CANDIDATE_LIMIT);
        let omitted = total - providers.len() as u64;
        Self {
            disposition,
            providers,
            total,
            omitted,
            summary: summarize_candidates(disposition, total, omitted),
        }
    }
}

/// Compose the [`ProviderCandidates::summary`] line ([CR-118], [NFR-CC-04]).
///
/// The `; none bound` tail is the whole point of the line on a tied set: a reader
/// skimming a list of four members needs the row's refusal restated beside them,
/// not inferred from a `state` field further up the object.
///
/// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
fn summarize_candidates(disposition: ProviderDisposition, total: u64, omitted: u64) -> String {
    // `listed` is not a third independent fact — it is `total - omitted`, and taking
    // it as a parameter would create a consistency obligation nothing enforces.
    let extent = if omitted == 0 {
        "all listed".to_string()
    } else {
        format!("{} listed, {omitted} omitted", total - omitted)
    };
    // A fan-out bound to exactly ONE cross-member subscriber is the ordinary
    // two-service shape, not an edge case, so this line must be grammatical at
    // arity 1 — the idiom the rest of the codebase uses for the same reason.
    let plural = if total == 1 { "" } else { "s" };
    match disposition {
        ProviderDisposition::BoundTo => {
            format!("{total} bound provider{plural} (fan-out), {extent}")
        }
        // A tie is 2-or-more by construction (the sole-candidate and empty-bucket
        // cases are intercepted before it), so this arm never renders arity 1 —
        // it is pluralized alongside its sibling rather than relying on that.
        ProviderDisposition::TiedBetween => {
            format!("{total} tied provider{plural}, {extent}; none bound")
        }
    }
}

/// The provider side [`tier`] resolved for one reference — the half a coverage row
/// never carried before [CR-118], and which the matcher already had in hand
/// ([`bucket_candidates`] returns the whole narrowed bucket, discarding nothing).
///
/// Internal: it is projected onto the row's optional wire fields by
/// [`ReferenceCoverage::new`], which is the single place the projection happens.
///
/// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
enum ProviderEvidence {
    /// No provider to name: nothing in the workspace provides this key, or the
    /// reference's own template never composed into one. The row carries neither
    /// `to` nor `candidates` — absence, not an empty list ([NFR-CC-04]).
    Unnamed,
    /// Exactly one cross-member provider, bound — the row's
    /// [`to`](ReferenceCoverage::to), and the identical `(member, symbol)` pair
    /// `xservice route-providers` reports for this reference.
    Sole(BridgeEndpoint),
    /// Several providers, all bound (fan-out) or all tied (ambiguous) — the row's
    /// [`candidates`](ReferenceCoverage::candidates).
    Several(ProviderDisposition, Vec<BridgeEndpoint>),
}

/// One reference's provider-side provenance: what [`tier`] resolved, plus the
/// intake the consumer arrived through ([CR-118], [CR-083]).
///
/// Bundled so the two adjacent, unrelated values arrive **named** at each
/// `record` call site rather than as a fifth and sixth positional argument.
///
/// It is deliberately not claimed that a later arm benefits: the two successors
/// scheduled into this file (S-339, S-370) add new `record` *call sites* carrying
/// `Unnamed` evidence, not new provenance *fields*, so they gain nothing from the
/// struct beyond the naming.
///
/// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
/// [CR-083]: ../../../docs/requests/CR-083-reachability-invocation-edge-roots.md
struct RowProvenance {
    providers: ProviderEvidence,
    intake: BridgeIntake,
}

/// One cross-boundary reference's coverage classification ([FR-WS-05]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferenceCoverage {
    /// The relation class (`"route"` for the HTTP key, today).
    pub relation: String,
    /// The consumer endpoint this reference belongs to.
    pub from: BridgeEndpoint,
    /// The 3-state display bucket (`"bound"`, `"ambiguous"`, or `"unbound"`,
    /// [`CoverageState::bucket`]) — carried as its own field so a consumer
    /// reads the FR-WS-05 3-state classification directly, without having to
    /// special-case `reason == "ambiguous"` against `state` to recover it.
    pub bucket: &'static str,
    /// This reference's full classification. Flattened so the JSON also
    /// carries a top-level `state` key (`"bound"` or `"unbound"`) plus
    /// `reason` when unbound, rather than nesting the internally-tagged enum
    /// under a second `state` object.
    #[serde(flatten)]
    pub state: CoverageState,
    /// The **provider this reference bound to**, under an exactly-one discipline
    /// (`route`, `grpc-call`) — the same `(member, symbol)` pair
    /// `xservice route-providers` reports for it ([CR-118], [FR-WS-05]).
    ///
    /// **Optional.** Absent on every non-bound row, and absent on a *fan-out*
    /// bound row, which binds a set and reports it in
    /// [`candidates`](Self::candidates) rather than fabricating a single
    /// provider for it. A consumer that does not know this field reads the row
    /// exactly as it did before [CR-118].
    ///
    /// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<BridgeEndpoint>,
    /// The **intake population** this reference arrived through ([CR-083],
    /// [CR-120]).
    ///
    /// **Present on every row, in every state** — bound, ambiguous and unbound
    /// alike — and deliberately not an `Option`, so no state *can* omit it and
    /// no consumer has to carry an absence case ([FR-WS-05]). Every reference
    /// reaches this tier through exactly one of two loops in
    /// [`cross_service_coverage`] — a member's contract surface, or its
    /// invocation ledger — so the value was always known for every row; only the
    /// serialization was conditional.
    ///
    /// # This reverses [S-372]'s decision, and states what that cost
    ///
    /// S-372 emitted `intake` only on a bound row, on the ground that an intake
    /// describes an *edge*. That made the field's **presence** a second, weaker
    /// spelling of `bucket == "bound"`, and it is what hid the defect [CR-120]
    /// was filed for: the reference workspace's 81-row `bound` count is entirely
    /// `contract-surface` with **zero** `invocation` rows, and no reader — human
    /// or machine — could discover that from a payload where the invocation
    /// population's non-bound rows carried no intake at all ([NFR-CC-04]).
    ///
    /// Nothing is lost by the reversal. "Does this row have an edge" is
    /// [`bucket`](Self::bucket) and [`state`](Self::state), which say it
    /// outright, and [`to`](Self::to) / [`candidates`](Self::candidates) name
    /// what it bound. `intake` answers a different question — *which population
    /// is this reference drawn from* — and that question has an answer in every
    /// state. The counts split by it are
    /// [`by_intake`](CrossServiceCoverage::by_intake).
    ///
    /// [CR-083]: ../../../docs/requests/CR-083-reachability-invocation-edge-roots.md
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [S-372]: ../../../docs/planning/journal.md#s-372-coverage-rows-name-the-provider-they-bound-and-the-candidates-they-tied-between
    pub intake: BridgeIntake,
    /// The providers this reference **tied between** (ambiguous) or **fanned out
    /// to** (a bound broker row) — bounded, with any truncation disclosed
    /// ([CR-118], [NFR-CC-04]).
    ///
    /// **Optional.** Absent whenever there is no set to name — a sole bound
    /// provider (that is [`to`](Self::to)), an uncomposable template, or no
    /// provider anywhere in the workspace.
    ///
    /// On an ambiguous row this is the single most actionable thing the payload
    /// carries: it is what turns `ambiguous: 146` from a number into a diagnosis,
    /// because a reader who sees four aggregator members on one template
    /// recognises the architecture instead of suspecting the matcher ([FR-CG-09]
    /// Notes — that ambiguity is call-site-gated, not match-gated). Naming them
    /// creates no edge and moves no bucket ([NFR-RA-05]).
    ///
    /// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
    /// [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidates: Option<ProviderCandidates>,
}

impl ReferenceCoverage {
    /// Build a [`ReferenceCoverage`], deriving [`bucket`](Self::bucket) from
    /// `state` so the two can never disagree — and projecting the tier's
    /// [`ProviderEvidence`] onto the row's optional provider fields here, once,
    /// for the same reason.
    fn new(
        relation: String,
        from: BridgeEndpoint,
        state: CoverageState,
        provenance: RowProvenance,
    ) -> Self {
        let (to, candidates) = match provenance.providers {
            ProviderEvidence::Unnamed => (None, None),
            ProviderEvidence::Sole(endpoint) => (Some(endpoint), None),
            ProviderEvidence::Several(disposition, endpoints) => {
                (None, Some(ProviderCandidates::new(disposition, endpoints)))
            }
        };
        // The three row invariants, at the one place all three are decidable:
        // `to` only on a bound row, a `bound-to` set only on a bound row, and a
        // `tied-between` set never on one. `tier` is the sole producer today and
        // pairs them atomically, so these are cheap guards on the successors this
        // file is scheduled to receive rather than on any live defect.
        debug_assert!(
            to.is_none() || matches!(state, CoverageState::Bound),
            "a non-bound row must name no provider it bound to"
        );
        debug_assert!(
            !matches!(
                candidates.as_ref().map(|c| c.disposition),
                Some(ProviderDisposition::BoundTo)
            ) || matches!(state, CoverageState::Bound),
            "a `bound-to` set claims every listed provider is reached"
        );
        debug_assert!(
            !matches!(
                candidates.as_ref().map(|c| c.disposition),
                Some(ProviderDisposition::TiedBetween)
            ) || !matches!(state, CoverageState::Bound),
            "a `tied-between` set claims NONE is reached (NFR-RA-05)"
        );
        Self {
            relation,
            from,
            bucket: state.bucket(),
            state,
            to,
            intake: provenance.intake,
            candidates,
        }
    }
}

/// The four classification counts over **one** intake population ([FR-WS-05],
/// [CR-120]).
///
/// The same four buckets [`CrossServiceCoverage`] reports workspace-wide, with
/// the same meanings — this is a projection of that partition onto one
/// population, never a second classifier. There is exactly one place a state
/// becomes a count ([`ClassificationCounts::record`]), reached from exactly one
/// place a row is filed ([`Tally::record`]), so a split figure and the headline
/// beside it cannot disagree: the headline is *derived* from the split
/// ([`IntakeSplit::total`]) rather than tallied a second time.
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ClassificationCounts {
    /// References bound to a provider in another member.
    pub bound: u64,
    /// References with 2+ providers across the workspace (no edge).
    pub ambiguous: u64,
    /// References unbound for a reason other than ambiguity or
    /// no-provider-in-workspace.
    pub unbound: u64,
    /// References with no provider anywhere in the workspace — bucketed
    /// separately, exactly as workspace-wide ([ADR-53]).
    ///
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    pub no_provider_in_workspace: u64,
}

impl ClassificationCounts {
    /// Count one classified reference. The single place a [`CoverageState`]
    /// becomes a number in this module.
    fn record(&mut self, state: &CoverageState) {
        match state {
            CoverageState::Bound => self.bound += 1,
            CoverageState::Unbound { reason } => match reason {
                UnboundReason::Ambiguous => self.ambiguous += 1,
                // `no-provider-in-workspace` is its own bucket, deliberately OUTSIDE
                // the `spec_conformance_ratio` denominator: a reference to a service
                // outside this workspace is not a *broken* binding ([ADR-53]).
                UnboundReason::NoProviderInWorkspace => self.no_provider_in_workspace += 1,
                _ => self.unbound += 1,
            },
        }
    }

    /// Field-wise sum — how [`IntakeSplit::total`] recovers the workspace-wide
    /// counts from the split.
    fn plus(self, other: Self) -> Self {
        Self {
            bound: self.bound + other.bound,
            ambiguous: self.ambiguous + other.ambiguous,
            unbound: self.unbound + other.unbound,
            no_provider_in_workspace: self.no_provider_in_workspace
                + other.no_provider_in_workspace,
        }
    }
}

/// The classification counts **split by intake population** ([FR-WS-05],
/// [CR-120]).
///
/// The headline `bound`/`ambiguous`/`unbound`/`no_provider_in_workspace` counts
/// two populations as one: a `contract-surface` reference is a *declared*
/// endpoint matched to a controller, an `invocation` reference is a captured
/// *call site*. Reporting only their sum is how a workspace whose invocation
/// half binds **nothing at all** presented as one binding 81 references
/// ([CR-120] §3.1). Split here, so the two are separable without inference and
/// without a consumer re-classifying [`CrossServiceCoverage::references`]
/// itself ([NFR-CC-04]).
///
/// One field per [`BridgeIntake`] variant, and the mapping is exhaustive in both
/// directions — [`population_mut`](Self::population_mut) matches on the enum and
/// [`total`](Self::total) destructures the struct — so a third intake fails to
/// compile until it is reported here.
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct IntakeSplit {
    /// References that arrived through a member's **contract surface** — an
    /// OpenAPI operation, a declared endpoint.
    pub contract_surface: ClassificationCounts,
    /// References that arrived through a member's **invocation ledger** — a
    /// captured HTTP client call, gRPC stub call, or broker publish/subscribe
    /// ([FR-WS-08]–[FR-WS-10]), including the arms' recorded refusals
    /// ([CR-107], [CR-120]).
    ///
    /// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    /// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
    pub invocation: ClassificationCounts,
}

impl IntakeSplit {
    /// The population `intake` belongs to. Exhaustive on the enum on purpose: a
    /// new [`BridgeIntake`] variant does not compile until it has a field here,
    /// so no population can be silently folded into another's counts.
    fn population_mut(&mut self, intake: BridgeIntake) -> &mut ClassificationCounts {
        match intake {
            BridgeIntake::ContractSurface => &mut self.contract_surface,
            BridgeIntake::Invocation => &mut self.invocation,
        }
    }

    /// Both populations summed — the workspace-wide counts.
    ///
    /// Destructured rather than field-accessed, which is the other half of the
    /// exhaustiveness guard: a struct pattern must name every field, so a third
    /// population cannot be added and then left out of the headline. That is the
    /// failure this reconciliation exists to prevent — a split that reports less
    /// than the sum it sits beside is worse than no split at all ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    fn total(&self) -> ClassificationCounts {
        let Self { contract_surface, invocation } = self;
        contract_surface.plus(*invocation)
    }
}

/// The non-gated 3-state cross-service coverage summary over a workspace
/// ([FR-WS-05], [ADR-53]) — advisory only, never a gate input (see module
/// docs).
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
/// Deliberately **not** `Default`. Every field but one could default harmlessly,
/// and [`covers_all_members`](Self::covers_all_members) could not: `false` is the
/// derived default and is a *lie* over the empty workspace a defaulted value
/// describes (0 of 0 members is covered), while `true` would be a lie over any
/// other. A caller must state the coverage it measured — which is the whole point
/// of the marker ([FR-WS-16], [NFR-CC-04]). Nothing constructs this by default
/// today; [`Tally::finish`] is the one constructor.
///
/// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Serialize)]
pub struct CrossServiceCoverage {
    /// Every classified cross-boundary reference, sorted by endpoint for
    /// deterministic output ([NFR-RA-06]).
    pub references: Vec<ReferenceCoverage>,
    /// References bound to a provider in another member.
    ///
    /// Not "exactly one", which this line said until S-377 put a projection of it
    /// (`ClassificationCounts::bound`) in the same file to disagree with: under a
    /// **fan-out** discipline one broker publish binds *every* cross-member
    /// subscriber and is counted here once ([FR-WS-10]). The exactly-one rule is a
    /// property of the `route`/`grpc-call` namespaces, not of this counter.
    ///
    /// **Unchanged in meaning and in value** by [CR-120]'s split: it is now the
    /// *sum* of [`by_intake`](Self::by_intake)'s two `bound` counts rather than a
    /// counter of its own, which is the same arithmetic reached one step later.
    ///
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    pub bound: u64,
    /// References with 2+ providers across the workspace (no edge).
    pub ambiguous: u64,
    /// References unbound for a reason other than ambiguity or
    /// no-provider-in-workspace (today: an uncomposable template).
    pub unbound: u64,
    /// References with no provider anywhere in the workspace — bucketed
    /// separately so they never depress
    /// [`spec_conformance_ratio`](Self::spec_conformance_ratio) ([ADR-53]).
    pub no_provider_in_workspace: u64,
    /// The same four counts **split by intake population** — `contract-surface`
    /// (a declared endpoint) apart from `invocation` (a captured call site)
    /// ([FR-WS-05], [CR-120]).
    ///
    /// The four counters above are the sum of these two, and are computed from
    /// them ([`IntakeSplit::total`]), so the split can never report less than the
    /// headline it sits beside. Reading `by_intake.invocation.bound` is the
    /// question no earlier payload could answer: on the reference workspace it is
    /// **0** against a headline `bound` of 81 ([CR-120] §3.1, [NFR-CC-04]).
    ///
    /// Every row carries the discriminator these counts group on
    /// ([`ReferenceCoverage::intake`]), so the split is auditable from
    /// [`references`](Self::references) rather than merely asserted.
    ///
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub by_intake: IntakeSplit,
    /// **The workspace headline** ([FR-WS-05], [CR-120], [BR-51]): how many
    /// cross-service edges the bridge actually resolved from a captured
    /// **invocation** — a caller→callee HTTP client call, a producer→consumer
    /// broker publish, a gRPC stub call.
    ///
    /// Deliberately **not** [`bound`](Self::bound). A `contract-surface` bound row
    /// is an OpenAPI operation matched to a controller route — documentation
    /// conformance, which [ADR-52] itself distinguishes from reachability — and
    /// pooling it with resolved call sites is what let the retired bound-ratio
    /// read `0.287` over a workspace with **zero** caller→callee edges ([CR-120]
    /// §2). A count of resolved edges cannot improve while the capability it
    /// describes stays constant, which is the property the ratio lacked: the
    /// ratio moved *downward* as [CR-107] and [CR-117] made invisible losses
    /// visible.
    ///
    /// # It counts edges, not sites — and here the two differ
    ///
    /// Under an exactly-one discipline one resolved reference is one edge. Under
    /// **fan-out** ([FR-WS-10]) one broker publish binds *every* cross-member
    /// subscriber and the bridge emits one edge per subscriber, so that single
    /// reference contributes its whole bound set. So this figure is computed from
    /// the rows' own named providers — [`to`](ReferenceCoverage::to) for a sole
    /// provider, the bound set's pre-truncation
    /// [`ProviderCandidates::total`] for a fan-out — which also means a reader can
    /// reconcile it against [`references`](Self::references) rather than take it
    /// on trust, exactly as [`by_intake`](Self::by_intake) can be.
    ///
    /// It is therefore **not** [`egress_resolution`](Self::egress_resolution)'s
    /// numerator: that rate is over resolved *sites*. Publishing a count of sites
    /// under the word "edges" would be the same name/meaning mismatch [CR-120]
    /// exists to remove, so the two are computed separately and each says what it
    /// counts. They coincide at every arity except fan-out.
    ///
    /// Never published without the rate beside it ([BR-51]) — the structural form
    /// of that duty is
    /// [`resolved_edges_summary`](Self::resolved_edges_summary).
    ///
    /// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
    /// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
    /// [CR-117]: ../../../docs/requests/CR-117-broker-publish-capture-and-the-topic-key-namespace.md
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    pub resolved_cross_service_edges: u64,
    /// The rate at which captured **egress sites resolve at all**:
    /// `invocation.bound / (invocation.bound + invocation.ambiguous +
    /// invocation.unbound)`, over [`by_intake`](Self::by_intake)'s invocation
    /// population alone ([FR-WS-05], [CR-120]).
    ///
    /// *Egress* is that population: an outbound call site an invocation arm
    /// captured. The contract-surface population is not egress — an OpenAPI
    /// operation is a declaration, not a call — which is exactly why this rate
    /// cannot be read off [`spec_conformance_ratio`](Self::spec_conformance_ratio).
    ///
    /// **Absent** (`null` in `--json`) when that denominator is zero, never a
    /// perfect score: [CR-100]'s rule applied to the successor figure, which
    /// [BR-51] states rather than leaving to be rediscovered. A workspace where no
    /// egress site was captured has *no measurement*, and `1.0` there would say
    /// every outbound call resolves.
    ///
    /// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [CR-100]: ../../../docs/requests/CR-100-workspace-resource-budget.md
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub egress_resolution: Option<f64>,
    /// The denominator [`egress_resolution`](Self::egress_resolution) was computed
    /// over — the invocation population's `bound + ambiguous + unbound`, stated
    /// explicitly so a consumer never re-implements it. [CR-111]'s
    /// denominator-disclosure duty, applied to the new figure from its first
    /// release rather than after an incident.
    ///
    /// Present even when the rate is absent: `0` here *is* the fact that no egress
    /// site was captured, which is what an absent rate means and what a reader
    /// would otherwise have to guess ([NFR-CC-04]).
    ///
    /// [CR-111]: ../../../docs/requests/CR-111-bound-ratio-carries-its-denominator.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub egress_resolution_measured: u64,
    /// The resolved-edge count and its resolution rate **as one line** — the
    /// structural form of [BR-51]: a surface that renders the headline renders the
    /// rate, because there is one field and it carries both.
    ///
    /// e.g. `"0 resolved cross-service edges; egress resolution 0.000 (0 of 54
    /// egress sites resolved)"`, or, with nothing captured, `"0 resolved
    /// cross-service edges; egress resolution not measured (0 of 0 egress
    /// sites)"`.
    ///
    /// Modelled on [`spec_conformance_summary`](Self::spec_conformance_summary)
    /// ([CR-111] §4.4): one composed field that both the human and the `--json`
    /// rendering serialize, so neither can regress while the other stays honest.
    /// [BR-51] exists *because* the retired ratio acquired that duty only after an
    /// incident; its successor carries it on release one.
    ///
    /// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [CR-111]: ../../../docs/requests/CR-111-bound-ratio-carries-its-denominator.md
    pub resolved_edges_summary: String,
    /// `bound / (bound + ambiguous + unbound)`, excluding
    /// `no_provider_in_workspace` from the denominator ([ADR-53]).
    ///
    /// **The retired `bound_ratio`'s formula, unchanged, under the name of what it
    /// always measured** ([CR-120] §5.2, [FR-WS-05]). It is dominated by
    /// `contract-surface` intake — on the 84-member reference estate all 81 of its
    /// bound rows are declared-contract matches — so it reports how far this
    /// workspace's *declarations* line up with its controllers, and it is **never**
    /// a measure of cross-service coupling. That headline is
    /// [`resolved_cross_service_edges`](Self::resolved_cross_service_edges), and
    /// [`by_intake`](Self::by_intake) says exactly how this numerator divides.
    ///
    /// The formula stays over the **pooled** population rather than being re-based
    /// onto contract-surface alone. [CR-120] §5.2 puts the change as "name change
    /// only", and re-basing the denominator in the same increment would make every
    /// recorded figure — the 2026-09-08 pre-change baseline included —
    /// incomparable with its successor for two reasons at once, which is precisely
    /// what makes a delta unattributable.
    ///
    /// **Absent** (`null` in `--json`) when that denominator is zero, never a
    /// perfect score ([FR-WS-05], [NFR-CC-04]). `0 / 0` is not full coverage; it
    /// is *no measurement*, and reporting it as `1.0` is how a workspace with 63
    /// of 72 members unopened presented as a healthy one — `bound: 0`,
    /// `bound_ratio: 1.0` ([CR-100]). An `Option` rather than a sentinel so a
    /// consumer that ignores absence fails to compile instead of lying.
    ///
    /// The old key is **not emitted**: an external reader of `--json` gets a
    /// missing field and fails loudly rather than silently reading a figure that
    /// no longer means what it did ([CR-120] §7). Its deprecated
    /// **deserialization** alias, retained for one release, is
    /// [`SpecConformanceReading`].
    ///
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    /// [CR-100]: ../../../docs/requests/CR-100-workspace-resource-budget.md
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_conformance_ratio: Option<f64>,
    /// The denominator
    /// [`spec_conformance_ratio`](Self::spec_conformance_ratio) was computed
    /// over — `bound + ambiguous + unbound`, explicitly, so a `--json` consumer
    /// reads the ratio's scale without re-implementing the sum itself ([CR-111]).
    ///
    /// Present even when the ratio is absent (a zero denominator serializes
    /// this as `0`): the ratio's own scale is exactly the fact a bare `0.857`
    /// hides, and [`no_provider_in_workspace`](Self::no_provider_in_workspace)
    /// beside it is the excluded count the ratio never carried before this
    /// field existed ([FR-WS-05], [ADR-53]).
    ///
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    /// [CR-111]: ../../../docs/requests/CR-111-bound-ratio-carries-its-denominator.md
    pub spec_conformance_measured: u64,
    /// The spec-conformance ratio, never presented bare ([FR-WS-05], [CR-111]):
    /// the ratio's own value (when present) followed by its denominator and the
    /// count excluded as `no-provider-in-workspace` — e.g. `"0.857 (6 of 7
    /// measured; 899 excluded as no-provider-in-workspace)"`, or, on an absent
    /// ratio, `"0 of 0 measured; 899 excluded as no-provider-in-workspace"`
    /// (S-327: the excluded count is reported regardless of whether anything was
    /// measured).
    ///
    /// The line's **wording is unchanged** across the rename — it names figures,
    /// not fields — so a recorded pre-change capture and a post-change one compare
    /// directly, which is what keeps the [CR-120] deltas attributable.
    ///
    /// Both `workspace status`'s human and `--json` renderings serialize this
    /// same field — one line, in both outputs, that can never regress on one
    /// surface while the other stays honest ([CR-111] §4.4).
    ///
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    /// [CR-111]: ../../../docs/requests/CR-111-bound-ratio-carries-its-denominator.md
    pub spec_conformance_summary: String,
    /// Members whose contract surface this summary actually read.
    pub members_read: u64,
    /// Members declared in the workspace — the roster this summary was computed
    /// over ([FR-WS-16]).
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    pub members_total: u64,
    /// Whether every member in the roster contributed to the counts above.
    ///
    /// `false` marks every figure in this summary as covering fewer than all
    /// members, so a partial workspace never reads as a whole one
    /// ([FR-WS-16], [NFR-CC-04]). Stated as its own field rather than left for a
    /// reader to compute from the two counts, because the counts are easy to
    /// ignore and this is the claim that matters.
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub covers_all_members: bool,
}

/// The spec-conformance figures read back out of a **serialized** coverage
/// payload of either vintage — the home of `bound_ratio`'s deprecated
/// deserialization alias, retained for one release ([CR-120] §7,
/// [FR-WS-05]).
///
/// # Why the alias lives here and not on [`CrossServiceCoverage`]
///
/// It would be on the read-model itself if the read-model could derive
/// `Deserialize`, and it cannot: [`ReferenceCoverage::bucket`] is a
/// `&'static str` (a display token derived from the state, deliberately not an
/// owned copy of it), so no borrowed-data deserializer can produce one. Changing
/// that field's type to buy an alias would be a wire-shape change made for a
/// compatibility shim, which is a worse trade than three fields read back
/// explicitly.
///
/// # Why this is not a hand-mirrored twin of the read-model
///
/// Sprint 66's risk register names a hand-mirrored twin in this file as the
/// recorded failure mode, so the distinction is stated rather than assumed. This
/// type **produces no figure**: it computes nothing, classifies nothing, and its
/// three fields are the exact three whose *spelling* changed. It is a reader, and
/// the twin failure was a second producer that drifted from the first. It is also
/// pinned against the real read-model rather than against a fixture — 
/// `the_deprecated_bound_ratio_alias_reads_both_vintages` serializes an actual
/// [`CrossServiceCoverage`] and asserts this type reads the same three values
/// back — so drift fails a test rather than going unnoticed.
///
/// # What it is for
///
/// Comparing a recorded pre-change capture with a post-change one. The reference
/// workspace's 2026-09-08 baseline is a pre-change payload spelling these three
/// figures `bound_ratio*`; the [CR-120] deltas are stated against it, so reading
/// both vintages through one type is what makes that comparison a single
/// operation rather than a fork on vintage at every call site.
///
/// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SpecConformanceReading {
    /// [`CrossServiceCoverage::spec_conformance_ratio`], under either spelling.
    ///
    /// `default` as well as `alias`, because the key is **absent** from both
    /// vintages on a zero denominator — the [CR-100] guarantee — and a reader
    /// that treated absence as a parse failure would reject exactly the payloads
    /// that guarantee produces.
    ///
    /// [CR-100]: ../../../docs/requests/CR-100-workspace-resource-budget.md
    #[serde(default, alias = "bound_ratio")]
    pub spec_conformance_ratio: Option<f64>,
    /// [`CrossServiceCoverage::spec_conformance_measured`], under either
    /// spelling. Not `default`: it is present in both vintages even at zero, so
    /// its absence means the payload is neither, and that must fail loudly.
    #[serde(alias = "bound_ratio_measured")]
    pub spec_conformance_measured: u64,
    /// [`CrossServiceCoverage::spec_conformance_summary`], under either
    /// spelling. The composed line's wording did not change across the rename, so
    /// two vintages' summaries compare directly.
    #[serde(alias = "bound_ratio_summary")]
    pub spec_conformance_summary: String,
}

/// Classify every cross-boundary reference over `registry`'s members
/// ([FR-WS-05], [ADR-53]).
///
/// Reads each member's contract surface through the same
/// [`MemberContracts::contract_surface`] the bridge uses, indexes providers on
/// the shared [`PortableKey`], then classifies **every** `ApiOperation`
/// consumer node — including ones the bridge's edge computation silently
/// drops (an uncomposable template, no candidate, an intra-repo-only match) —
/// so no cross-boundary reference goes unaccounted for.
///
/// A member that fails to start or whose surface read fails is skipped
/// (degraded, not fatal), exactly as [`ContractBridge::edges`](super::bridge::ContractBridge::edges)
/// — and the shortfall is **reported**:
/// [`members_read`](CrossServiceCoverage::members_read) against
/// [`members_total`](CrossServiceCoverage::members_total), with
/// [`covers_all_members`](CrossServiceCoverage::covers_all_members) marking every
/// figure in the summary as partial ([FR-WS-16], [NFR-CC-04]).
///
/// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub fn cross_service_coverage<E>(registry: &EngineRegistry<E>) -> CrossServiceCoverage
where
    E: MemberEngine + MemberContracts,
{
    let mut providers: ProviderIndex = ProviderIndex::new();
    let mut consumer_refs: Vec<(String, String, crate::model::LogosSymbol)> = Vec::new();
    // Arm-tagged invocation consumers (HTTP client calls, S-252, and later arms):
    // `(member, consumer)` pairs read from each member's ledger, classified below
    // through the same provider index as the contract-surface consumers.
    let mut inv_consumers: Vec<(String, super::bridge::InvocationRef)> = Vec::new();
    // Provider-role ledger rows that do not reduce to a portable key. They index no
    // provider, but they are captured sites and are reported — the broker arm's
    // recorded `topic-not-literal` refusals arrive here ([CR-107], [NFR-CC-04]).
    let mut unkeyable_providers: Vec<(String, super::bridge::InvocationRef)> = Vec::new();
    // Ledger endpoints already filed, so one endpoint is filed once — the collapse
    // `broker_edges` performs before its own fan-out ([NFR-RA-05]).
    //
    // Keyed on `(key, is_provider, member, symbol)`, which is
    // [`super::broker::broker_edges`]'s own dedup key **including the role**.
    // Before S-376 this set covered the provider arm only, and the consumer arm
    // went uncollapsed: a publish endpoint captured twice on one topic (two ledger
    // rows differing in `form`) produced two bound rows where the bridge emits one
    // edge per subscriber from *one* publish. That inflated the pooled `bound` —
    // already wrong, but invisible — and S-376 promoted it into a published
    // headline, where `resolved_cross_service_edges` would report 2xN against the
    // bridge's N and break the reconcile-against-`references` contract that field
    // states ([NFR-RA-05], [CR-118]). The comment below already claimed this tier
    // applied "the SAME collapse"; it now does.
    let mut ledger_endpoints: std::collections::HashSet<(PortableKey, bool, String, String)> =
        std::collections::HashSet::new();

    let surfaces = read_members(registry, "contract surface", |e| e.contract_surface());
    // The members that actually contributed — the numerator of the coverage
    // marker below. A member whose engine failed to start or whose surface read
    // failed is skipped here, and without recording that shortfall the summary
    // would present a partial workspace as a whole one ([FR-WS-16]).
    let members_read = surfaces.len();

    for (member, surface) in surfaces {
        for node in &surface {
            if node.kind == NodeKind::ApiOperation {
                consumer_refs.push((member.clone(), node.name.clone(), node.symbol.clone()));
            }
            if let Some((key, Role::Provider)) = classify(node.kind, &node.name) {
                index_provider(
                    &mut providers,
                    key,
                    BridgeEndpoint {
                        member: member.clone(),
                        symbol: node.symbol.clone(),
                    },
                );
            }
        }
    }
    // Arm-tagged ledger refs from each member (HTTP client calls, gRPC stub calls,
    // broker publishes **and subscribes**) — degrade-don't-abort via the same
    // `read_members` the bridge uses ([ADR-53]). Read through the **same seam** the
    // bridge reads (`invocation_refs`, both roles), then split by the arm's own
    // `bridge_role` — so the coverage tier and the bridge see one ledger, not two.
    //
    // Indexing the **provider**-role rows is what keeps this tier honest after S-256
    // ([FR-WS-11]): a broker subscribe has no contract-surface node behind it, so a
    // provider index built from `contract_surface` alone contains no broker provider
    // at all — and every publish, *including the ones the bridge now binds*, would be
    // reported `no-provider-in-workspace`. The service map would draw the coupling
    // while the coverage board next to it denied that any provider existed
    // ([NFR-CC-04]). The bridge's own contract says it: "one classifier, no drift
    // between 'why did this bind' and 'why didn't this bind'".
    //
    // [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
    // [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    for (member, refs) in read_members(registry, "invocation references", |e| e.invocation_refs()) {
        for reference in refs {
            match reference.relation.bridge_role() {
                Some(BridgeRole::Consumer) => {
                    // The consumer half of the same collapse. A consumer row that
                    // does not reduce to a portable key is still a captured site and
                    // is reported (the tier classifies it below), so an unkeyable row
                    // bypasses the dedup rather than being dropped here — dropping it
                    // would lose the refusal the broker arm files.
                    if let Some(key) =
                        consumer_portable_key(reference.relation, &reference.target)
                    {
                        if !ledger_endpoints.insert((
                            key,
                            false,
                            member.clone(),
                            reference.symbol.as_str().to_string(),
                        )) {
                            continue; // a repeat of this exact endpoint on this key
                        }
                    }
                    inv_consumers.push((member.clone(), reference));
                }
                Some(BridgeRole::Provider) => {
                    // A ledger-only provider (a broker subscribe) keys on exactly the
                    // string its consumer side keys on, so the two meet in this index
                    // the same way they meet in the bridge's.
                    let Some(key) = consumer_portable_key(reference.relation, &reference.target)
                    else {
                        // An unkeyable provider contributes no provider — but it is a
                        // captured site, so it is *reported* rather than dropped. This
                        // is where the broker arm's recorded refusal (a keyless
                        // `@KafkaListener(topics = TOPIC)` row) becomes a
                        // `topic-not-literal` coverage row: the listener side is the
                        // provider role, so before [CR-107] it fell out of the tier
                        // here and the loss was invisible ([NFR-CC-04]).
                        unkeyable_providers.push((member.clone(), reference));
                        continue;
                    };
                    // One endpoint per (key, member, symbol) — the SAME collapse
                    // [`super::broker::broker_edges`] applies before its fan-out, and
                    // for the same reason: a ledger can hold two rows for one endpoint
                    // (they differ in `form`, which is outside the ledger's effective
                    // identity — `idx_unresolved_refs_identity` over
                    // `(source_symbol, target, form, kind, COALESCE(payload, ''))`
                    // since migration 18, so `payload` is *inside* it, unlike what
                    // this comment claimed before [CR-107] reviewed it), and the
                    // fan-out treats each as a separate provider. The bridge de-duplicates, so before [CR-118]
                    // this tier could differ only in a boolean nobody could see. Now
                    // the set is NAMED and COUNTED, so a duplicate would report "3
                    // bound providers (fan-out)" beside two bridge edges — a
                    // fabricated count ([NFR-RA-05]) and exactly the classifier drift
                    // this module exists to prevent.
                    if !ledger_endpoints.insert((
                        key.clone(),
                        true,
                        member.clone(),
                        reference.symbol.as_str().to_string(),
                    )) {
                        continue; // a repeat of this exact endpoint on this key
                    }
                    index_provider(
                        &mut providers,
                        key,
                        BridgeEndpoint {
                            member: member.clone(),
                            symbol: reference.symbol,
                        },
                    );
                }
                None => {} // not an invocation arm — not this tier's business
            }
        }
    }

    sort_buckets(&mut providers);

    let mut tally = Tally::default();

    for (member, name, symbol) in consumer_refs {
        let from = BridgeEndpoint {
            member: member.clone(),
            symbol,
        };

        let Some((key, _role)) = classify(NodeKind::ApiOperation, &name) else {
            tally.record(
                "route".to_string(),
                from,
                CoverageState::Unbound {
                    reason: UnboundReason::PathNotComposed,
                },
                // A template that never composed has no provider to name.
                RowProvenance {
                    providers: ProviderEvidence::Unnamed,
                    intake: BridgeIntake::ContractSurface,
                },
            );
            continue;
        };
        let relation = key.relation().to_string();

        if let Some((state, evidence)) = tier(&key, &member, &providers) {
            // A contract-surface consumer *declares* an endpoint rather than
            // calling one — the same intake the bridge stamps on its edge
            // ([CR-083]), read here from the loop the consumer arrived in.
            tally.record(
                relation,
                from,
                state,
                RowProvenance {
                    providers: evidence,
                    intake: BridgeIntake::ContractSurface,
                },
            );
        }
    }

    // Classify the arm-tagged invocation consumers against the same provider index
    // (S-252 HTTP, S-253 gRPC, S-254/S-256 broker). A stored consumer target
    // normalizes by construction (the arm's normalizer refused the rest before the
    // ledger), so it keys; a target that nonetheless does not compose is
    // `path-not-composed` (a gRPC or broker key, already normalized, never hits that
    // arm).
    for (member, consumer) in inv_consumers {
        let from = BridgeEndpoint {
            member: member.clone(),
            symbol: consumer.symbol,
        };
        let relation = arm_relation(consumer.relation);

        let Some(key) = consumer_portable_key(consumer.relation, &consumer.target) else {
            tally.record(
                relation,
                from,
                CoverageState::Unbound {
                    // The arm's own word for "this did not key" — `path-not-composed`
                    // for a template, `topic-not-literal` for a broker topic ([CR-107]).
                    reason: unkeyable_reason(consumer.relation, &consumer.target),
                },
                RowProvenance {
                    providers: ProviderEvidence::Unnamed,
                    intake: BridgeIntake::Invocation,
                },
            );
            continue;
        };

        if let Some((state, evidence)) = tier(&key, &member, &providers) {
            // A ledger reference is a captured call site, so its binding carries
            // the invocation intake — exactly as the bridge stamps it ([CR-083]).
            tally.record(
                relation,
                from,
                state,
                RowProvenance {
                    providers: evidence,
                    intake: BridgeIntake::Invocation,
                },
            );
        }
    }

    // The recorded refusals on the provider side of an arm ([CR-107]). Reported
    // after the classified rows so the tally's own ordering is untouched;
    // `finish` sorts the references by endpoint regardless.
    for (member, provider) in unkeyable_providers {
        let relation = arm_relation(provider.relation);
        tally.record(
            relation,
            BridgeEndpoint {
                member,
                symbol: provider.symbol,
            },
            CoverageState::Unbound {
                // The target is passed for symmetry with the consumer loop, and
                // reaches nothing new: this loop is fed only from the
                // `BridgeRole::Provider` arm, and `BrokerSubscribe` is the one
                // relation with that role — so `unkeyable_reason`'s `Http` branch
                // (the only one that reads the target) is unreachable from here.
                // A future `Http`-namespaced provider arm must revisit this,
                // because `client_call_refusal`'s empty-target rule is stated for
                // a *consumer* row.
                reason: unkeyable_reason(provider.relation, &provider.target),
            },
            // A refusal has no provider to name — it never had a key to look one up
            // with ([CR-118]'s `Unnamed` shape, which is what a `path-not-composed`
            // row already carries).
            RowProvenance {
                providers: ProviderEvidence::Unnamed,
                intake: BridgeIntake::Invocation,
            },
        );
    }

    tally.finish(members_read, registry.members().len())
}

/// The running coverage tally — one `record` point, so a state and its counter can
/// never drift apart (the `bound`/`ambiguous`/`unbound`/`no-provider` buckets were
/// previously incremented by hand at eight separate call sites).
///
/// Since [CR-120] the tally holds **only** the intake-split counts: the
/// workspace-wide four are derived from them in [`finish`](Tally::finish). One
/// number cannot be maintained in two places without a way for the two to
/// disagree, and the split is the finer of the two — so it is the one that is
/// counted.
///
/// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
#[derive(Default)]
struct Tally {
    references: Vec<ReferenceCoverage>,
    by_intake: IntakeSplit,
}

impl Tally {
    fn record(
        &mut self,
        relation: String,
        from: BridgeEndpoint,
        state: CoverageState,
        provenance: RowProvenance,
    ) {
        // The row's own intake selects the population, so the count and the
        // discriminator the row publishes are read from one value — a split a
        // consumer could not reproduce from `references` would be worse than none.
        self.by_intake.population_mut(provenance.intake).record(&state);
        self.references
            .push(ReferenceCoverage::new(relation, from, state, provenance));
    }

    /// Seal the tally into the read-model, over a workspace of `members_total`
    /// members of which `members_read` contributed ([FR-WS-05], [FR-WS-16]).
    ///
    /// # A broker subscribe can only ever depress the ratio ([CR-107], open)
    ///
    /// Stated because it is asymmetric and the asymmetry is not obvious. A *bound*
    /// broker subscribe is a **provider**, and a provider is not a reference — it
    /// contributes no [`ReferenceCoverage`] row and so no numerator. A *refused*
    /// one now contributes an `unbound` row, which **is** inside this denominator.
    /// So a member whose listeners are half captured reads
    /// `spec_conformance_ratio: 0.000`, and one whose listeners are all captured
    /// reads no ratio at all.
    ///
    /// The `path-not-composed` precedent this arm was asked to match is not exact:
    /// that reason sits on the *consumer* side, where its bound counterpart **is**
    /// counted, so its ratio is over one population. Whether `topic-not-literal`
    /// should instead get its own bucket outside the denominator — the
    /// [`NoProviderInWorkspace`](UnboundReason::NoProviderInWorkspace) treatment,
    /// whose stated ground in [ADR-53] ("not a *broken* binding") applies verbatim
    /// to a capture refusal — is a change to [FR-WS-05]'s ratio semantics that no
    /// acceptance criterion settles.
    ///
    /// **DECIDED at the Sprint 65 human review (2026-09-07): `topic-not-literal`
    /// stays INSIDE the denominator. No change.** The
    /// [`NoProviderInWorkspace`](UnboundReason::NoProviderInWorkspace) analogy does
    /// not hold: that reason means the coupling genuinely *leaves* the workspace, so
    /// there was never anything here to bind, whereas a capture refusal means the
    /// coupling is *inside* the workspace and this extractor could not resolve it.
    /// Excluding it would make the ratio improve precisely because [CR-107] and
    /// [CR-117] made previously-invisible losses visible — a measure that rewards
    /// better instrumentation by reading better is the wrong shape, and the opposite
    /// of what [NFR-CC-04] asks for. The count stays legible either way: the
    /// [CR-111] summary line always prints the denominator and the excluded count,
    /// so a reader can see what the ratio was computed over. On the reference estate
    /// this decision holds ~54 rows inside the denominator rather than outside it.
    ///
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    /// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
    ///
    /// The zero-denominator ratio is reported **absent**, never `1.0`: nothing
    /// to bind is not full coverage, it is no measurement, and a fabricated
    /// perfect score over a partially-opened workspace is exactly what
    /// [CR-100] observed ([NFR-CC-04]).
    ///
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [CR-100]: ../../../docs/requests/CR-100-workspace-resource-budget.md
    fn finish(mut self, members_read: usize, members_total: usize) -> CrossServiceCoverage {
        self.references.sort_by(|a, b| a.from.cmp(&b.from));
        // The workspace-wide counts, derived from the split rather than tallied
        // beside it ([CR-120]): the ratio and its denominator are then computed
        // over the same numbers a reader can reconcile against `by_intake`.
        let total = self.by_intake.total();
        let denom = total.bound + total.ambiguous + total.unbound;
        let spec_conformance_ratio = (denom > 0).then(|| total.bound as f64 / denom as f64);
        let spec_conformance_summary = summarize_spec_conformance(
            total.bound,
            denom,
            total.no_provider_in_workspace,
            spec_conformance_ratio,
        );

        // The successor headline ([CR-120], [BR-51]). Its two figures have
        // different numerators on purpose — resolved *edges* against resolved
        // *sites* — so each is computed from the thing it names, and they are
        // composed into one line so neither can be rendered without the other.
        let egress = self.by_intake.invocation;
        let egress_denom = egress.bound + egress.ambiguous + egress.unbound;
        let egress_resolution =
            (egress_denom > 0).then(|| egress.bound as f64 / egress_denom as f64);
        let resolved_cross_service_edges = resolved_edges(&self.references);
        let resolved_edges_summary = summarize_resolved_edges(
            resolved_cross_service_edges,
            egress.bound,
            egress_denom,
            egress_resolution,
        );

        CrossServiceCoverage {
            references: self.references,
            bound: total.bound,
            ambiguous: total.ambiguous,
            unbound: total.unbound,
            no_provider_in_workspace: total.no_provider_in_workspace,
            by_intake: self.by_intake,
            resolved_cross_service_edges,
            egress_resolution,
            egress_resolution_measured: egress_denom,
            resolved_edges_summary,
            spec_conformance_ratio,
            spec_conformance_measured: denom,
            spec_conformance_summary,
            members_read: members_read as u64,
            members_total: members_total as u64,
            covers_all_members: members_read == members_total,
        }
    }
}

/// Count the cross-service edges the bridge resolved from captured invocations —
/// [`CrossServiceCoverage::resolved_cross_service_edges`] ([CR-120], [FR-WS-10]).
///
/// Read off the rows' own named providers rather than tallied beside them, for
/// the reason the fan-out case makes concrete: a bound fan-out reference is **one
/// row and many edges**, and the multiplicity lives in the row's bound set, not in
/// a counter. Deriving it here means a reader can reconcile this figure against
/// `references` with the same arithmetic — which is what stops a headline from
/// being a number nobody can check, the whole subject of [CR-120].
///
/// [`ProviderCandidates::total`] is the **pre-truncation** count, so a fan-out
/// wider than [`CANDIDATE_LIMIT`] contributes all of its edges and not the eight
/// that happen to be listed ([NFR-CC-04]).
///
/// Contract-surface rows are excluded by construction: an OpenAPI operation
/// matched to a controller route is documentation conformance, not a resolved
/// call ([ADR-52], [CR-120] §2).
///
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
/// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
fn resolved_edges(references: &[ReferenceCoverage]) -> u64 {
    references
        .iter()
        .filter(|r| {
            r.intake == BridgeIntake::Invocation && matches!(r.state, CoverageState::Bound)
        })
        .map(|r| match (&r.to, &r.candidates) {
            // An exactly-one discipline: one provider, one edge.
            (Some(_), _) => 1,
            // Fan-out: the bridge emits one edge per cross-member subscriber, and
            // the row names that whole set.
            (None, Some(c)) => c.total,
            // Structurally unreachable — a bound row always names what it bound
            // to — and counted as one rather than dropped, because under-counting
            // an edge that exists is the direction that flatters.
            (None, None) => 1,
        })
        .sum()
}

/// Compose the [`CrossServiceCoverage::spec_conformance_summary`] line: the
/// ratio's own value — when present — followed by the denominator it was computed
/// over and the count excluded as `no-provider-in-workspace`, so the ratio is
/// never presented bare at any presentation site ([FR-WS-05], [CR-111]).
///
/// The excluded count is reported even when `ratio` is `None` (a zero
/// denominator, [S-327]): "0 of 0 measured, N excluded" is the informative
/// statement, and suppressing both leaves a reader with nothing.
///
/// **The wording is unchanged from the retired bound-ratio's line** ([CR-120]
/// renamed the field, not the sentence). That is deliberate: the sentence names
/// figures rather than fields, so a pre-change capture and a post-change one
/// compare directly and each [CR-120] delta stays attributable to a cause rather
/// than to a reformatting.
///
/// [CR-111]: ../../../docs/requests/CR-111-bound-ratio-carries-its-denominator.md
/// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [S-327]: ../../../docs/planning/journal.md#s-327-absent-bound-ratio-on-a-zero-denominator
fn summarize_spec_conformance(bound: u64, denom: u64, excluded: u64, ratio: Option<f64>) -> String {
    let measured = format!("{bound} of {denom} measured");
    match ratio {
        Some(r) => format!("{r:.3} ({measured}; {excluded} excluded as no-provider-in-workspace)"),
        None => format!("{measured}; {excluded} excluded as no-provider-in-workspace"),
    }
}

/// Compose the [`CrossServiceCoverage::resolved_edges_summary`] line — the
/// **structural** form of [BR-51]: the resolved-edge count and the egress
/// resolution rate in one string, so a surface physically cannot render the count
/// without the rate ([FR-WS-05], [CR-120]).
///
/// `resolved` is the edge count and `bound_sites` the rate's numerator; they
/// differ only under fan-out, where one resolved site is several edges. Both are
/// stated rather than one being derived from the other at a presentation site,
/// because that derivation is exactly where a figure acquires a meaning it does
/// not have.
///
/// An absent rate reads "not measured", never `0.000` and never `1.000` — [CR-100]'s
/// rule, which [BR-51] restates for this figure: no egress site captured is *no
/// measurement*, and both a zero and a full reading of it would be claims about
/// outbound calls that nothing supports.
///
/// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [CR-100]: ../../../docs/requests/CR-100-workspace-resource-budget.md
/// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
fn summarize_resolved_edges(
    resolved: u64,
    bound_sites: u64,
    egress_denom: u64,
    rate: Option<f64>,
) -> String {
    match rate {
        Some(r) => format!(
            "{resolved} resolved cross-service edges; egress resolution {r:.3} \
             ({bound_sites} of {egress_denom} egress sites resolved)"
        ),
        None => format!(
            "{resolved} resolved cross-service edges; egress resolution not measured \
             ({bound_sites} of {egress_denom} egress sites)"
        ),
    }
}

/// The coverage verdict for one consumer reference against the workspace provider
/// index — applying the key's **own namespace discipline** ([ADR-54]) rather than a
/// hardcoded arity, so this tier reaches the same verdict
/// [`match_indexed`](super::bridge::match_indexed) reaches when it decides whether to
/// emit the edge. One classifier, no drift between "why did this bind" and "why
/// didn't this bind".
///
/// `None` means the reference is not a **cross-boundary** one at all — its only
/// provider is in its own member. That is an intra-repo fact the per-repo graph
/// already owns, excluded from the tier entirely, exactly as the bridge emits no edge
/// for it ([FR-WS-04]).
///
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
/// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
fn tier(
    key: &PortableKey,
    member: &str,
    providers: &ProviderIndex,
) -> Option<(CoverageState, ProviderEvidence)> {
    // The bridge's own bucket reduction ([CR-109]): a wildcard provider serves
    // every verb, an exact-method provider outranks it. Running the *same*
    // helper is what stops the coverage board from reporting a reference the
    // service map has drawn an edge for — or the reverse ([ADR-52]).
    let candidates = bucket_candidates(providers, key);
    if candidates.is_empty() {
        // Nothing in the workspace provides this endpoint — discipline-independent.
        // An absent bucket and a bucket whose methods do not serve this consumer
        // are the same answer, deliberately: it is the reading a
        // `(method, template)`-keyed index gave for a method mismatch before
        // wildcards existed.
        return Some((
            CoverageState::Unbound {
                reason: UnboundReason::NoProviderInWorkspace,
            },
            ProviderEvidence::Unnamed,
        ));
    }

    match key.namespace().match_discipline() {
        MatchDiscipline::ExactlyOne => match candidates.as_slice() {
            // A sole same-member provider is intra-repo — not a cross-boundary
            // reference (unchanged from the pre-S-256 tier).
            [only] if only.member == member => None,
            [only] => Some((
                CoverageState::Bound,
                // The provider was in hand the whole time: `bucket_candidates`
                // returns the narrowed bucket and discards nothing, so naming the
                // bound end costs one clone and no re-computation ([CR-118]
                // CRA-01, confirmed here rather than assumed).
                ProviderEvidence::Sole((*only).clone()),
            )),
            // Two or more surviving candidates for one key: the sole-provider rule
            // fails, so the bridge fabricates no edge and this is honestly
            // ambiguous. The wildcard rule narrows the bucket before this point but
            // never breaks a tie among equally-specific providers ([NFR-RA-05]).
            //
            // Every tied candidate is listed, **including a same-member one**: the
            // tie is what refused the binding, and dropping a participant from it
            // would misreport why ([CR-118] CRA-02 — the losers are not discarded,
            // they are right here).
            _ => Some((
                CoverageState::Unbound {
                    reason: UnboundReason::Ambiguous,
                },
                ProviderEvidence::Several(
                    ProviderDisposition::TiedBetween,
                    candidates.iter().map(|e| (*e).clone()).collect(),
                ),
            )),
        },
        // Fan-out (a broker topic): one publish binds EVERY cross-member subscriber,
        // so any cross-member provider means bound — many subscribers is the arm
        // working as designed, never an ambiguity ([FR-WS-10]). All-same-member
        // subscribers are the intra-repo fan-out the per-repo graph owns.
        //
        // The bound row therefore names a **set**, and names exactly the set
        // `xservice route-providers` reports: the bridge emits one edge per
        // cross-member subscriber and skips the same-member ones, so the filter
        // here is the same filter there ([FR-WS-10], [FR-WS-11]).
        MatchDiscipline::FanOut => {
            let bound: Vec<BridgeEndpoint> = candidates
                .iter()
                .filter(|p| p.member != member)
                .map(|p| (*p).clone())
                .collect();
            (!bound.is_empty()).then_some((
                CoverageState::Bound,
                ProviderEvidence::Several(ProviderDisposition::BoundTo, bound),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use anyhow::Result;

    use super::super::bridge::ContractNode;
    use super::super::registry::RegistryMode;
    use super::super::{Federation, Member};
    use crate::model::LogosSymbol;

    // Per-test-thread fixtures, mirroring `bridge::tests` — each `#[test]`
    // runs on its own thread, so the thread-local member surfaces are
    // test-isolated.
    thread_local! {
        static FIXTURES: RefCell<HashMap<String, Vec<ContractNode>>> = RefCell::new(HashMap::new());
        static CONSUMERS: RefCell<HashMap<String, Vec<super::super::InvocationRef>>> =
            RefCell::new(HashMap::new());
    }

    fn reset() {
        FIXTURES.with(|f| f.borrow_mut().clear());
        CONSUMERS.with(|c| c.borrow_mut().clear());
    }
    fn set_member(name: &str, nodes: Vec<ContractNode>) {
        FIXTURES.with(|f| {
            f.borrow_mut().insert(name.to_string(), nodes);
        });
    }
    fn set_consumers(name: &str, consumers: Vec<super::super::InvocationRef>) {
        CONSUMERS.with(|c| {
            c.borrow_mut().insert(name.to_string(), consumers);
        });
    }
    /// An HTTP client-call consumer at `symbol` calling `target` (`"METHOD /path"`).
    fn http_call(target: &str, symbol: &str) -> super::super::InvocationRef {
        super::super::InvocationRef {
            relation: crate::model::ArtifactRelation::HttpClientCall,
            target: target.to_string(),
            symbol: LogosSymbol::parse(symbol).unwrap(),
        }
    }
    /// A gRPC stub-call consumer at `symbol` invoking `key` (`package.Service/Method`).
    fn grpc_consumer(key: &str, symbol: &str) -> super::super::InvocationRef {
        super::super::InvocationRef {
            relation: crate::model::ArtifactRelation::GrpcCall,
            target: key.to_string(),
            symbol: LogosSymbol::parse(symbol).unwrap(),
        }
    }
    /// A broker **publish** consumer at `symbol` emitting on `topic` (S-254).
    fn broker_publish(topic: &str, symbol: &str) -> super::super::InvocationRef {
        super::super::InvocationRef {
            relation: crate::model::ArtifactRelation::BrokerPublish,
            target: topic.to_string(),
            symbol: LogosSymbol::parse(symbol).unwrap(),
        }
    }
    /// A broker **subscribe** — a `Provider`-role ledger row with no contract-surface
    /// node behind it, which is why the coverage tier must index the ledger's provider
    /// side to see it at all (S-256, [FR-WS-11]).
    ///
    /// [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
    fn broker_subscribe(topic: &str, symbol: &str) -> super::super::InvocationRef {
        super::super::InvocationRef {
            relation: crate::model::ArtifactRelation::BrokerSubscribe,
            target: topic.to_string(),
            symbol: LogosSymbol::parse(symbol).unwrap(),
        }
    }
    fn proto_service(fqn: &str, symbol: &str) -> ContractNode {
        ContractNode {
            kind: NodeKind::ProtoService,
            name: fqn.to_string(),
            symbol: LogosSymbol::parse(symbol).unwrap(),
        }
    }

    #[derive(Debug)]
    struct FakeEngine {
        member: String,
    }

    fn member_of(root: &Path) -> String {
        root.file_name().unwrap().to_string_lossy().into_owned()
    }

    impl MemberEngine for FakeEngine {
        type Watcher = ();
        fn start(
            root: &Path,
            _read_connections: usize,
            _worker_pool: crate::SharedWorkerPool,
        ) -> Result<Arc<Self>> {
            let member = member_of(root);
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
            if self.member == "unreadable" {
                anyhow::bail!("store read failed");
            }
            Ok(FIXTURES.with(|f| f.borrow().get(&self.member).cloned().unwrap_or_default()))
        }
        fn contract_stamp(&self) -> u64 {
            0
        }
        // The single ledger seam: the coverage tier reads it and applies the role filter
        // itself (every fixture here is consumer-role, plus the S-256 broker subscribes).
        fn invocation_refs(&self) -> Result<Vec<super::super::InvocationRef>> {
            if self.member == "unreadable" {
                anyhow::bail!("store read failed");
            }
            Ok(CONSUMERS.with(|c| c.borrow().get(&self.member).cloned().unwrap_or_default()))
        }
    }

    fn fed(names: &[&str]) -> Federation {
        let root = PathBuf::from("/ws");
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
        }
    }

    fn registry(names: &[&str]) -> EngineRegistry<FakeEngine> {
        EngineRegistry::new(fed(names), RegistryMode::Lazy)
    }

    fn op(name: &str, symbol: &str) -> ContractNode {
        ContractNode {
            kind: NodeKind::ApiOperation,
            name: name.to_string(),
            symbol: LogosSymbol::parse(symbol).unwrap(),
        }
    }
    fn route(name: &str, symbol: &str) -> ContractNode {
        ContractNode {
            kind: NodeKind::Route,
            name: name.to_string(),
            symbol: LogosSymbol::parse(symbol).unwrap(),
        }
    }

    // ── FR-WS-16 / NFR-CC-04: the summary states how much of the workspace it
    //    actually covers, so a partial answer never reads as a whole one ──────

    /// A fully-read workspace reports `covers_all_members` and the two counts
    /// agreeing — the baseline the partial cases below are a departure from.
    #[test]
    fn a_fully_read_workspace_covers_all_members() {
        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_get")]);

        let cov = cross_service_coverage(&registry(&["api", "web"]));

        assert_eq!(cov.members_read, 2);
        assert_eq!(cov.members_total, 2);
        assert!(cov.covers_all_members);
    }

    /// **[FR-WS-16] AC5, the [CR-100] failure exactly.** A member whose engine
    /// cannot be opened contributes nothing, and the summary says so: the
    /// figures cover 1 of 2 members and `covers_all_members` is `false`.
    ///
    /// The observed run reported `bound: 0` with `bound_ratio: 1.0` over the 9
    /// members of 72 that opened — sound-looking numbers over a workspace that
    /// was three-quarters missing. Both halves of that are asserted here: the
    /// ratio is absent, and the shortfall is stated.
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [CR-100]: ../../../docs/requests/CR-100-workspace-resource-budget.md
    #[test]
    fn a_partially_opened_workspace_is_marked_as_covering_fewer_than_all_members() {
        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        // `broken`'s engine start fails; it would have provided the route.
        set_member("broken", vec![route("GET /users/{id}", "local route_get")]);

        let cov = cross_service_coverage(&registry(&["api", "broken"]));

        assert_eq!(cov.members_read, 1, "only `api` contributed");
        assert_eq!(cov.members_total, 2);
        assert!(
            !cov.covers_all_members,
            "every figure below covers 1 of 2 members and must be marked so"
        );
        assert_eq!(
            cov.no_provider_in_workspace, 1,
            "the provider was in the member that never opened"
        );
        assert_eq!(cov.bound, 0);
        assert_eq!(
            cov.spec_conformance_ratio, None,
            "0 bound of an empty denominator is NOT a perfect score — the exact \
             `bound: 0, bound_ratio: 1.0` CR-100 observed"
        );

        let value = serde_json::to_value(&cov).unwrap();
        assert!(
            value.get("spec_conformance_ratio").is_none(),
            "and it reaches the wire absent, not as a number: {value}"
        );
        assert_eq!(value["covers_all_members"], false);
    }

    /// A member that *opened* but whose surface **read** failed is the second
    /// shortfall channel, and it is marked identically — the summary's claim is
    /// about what contributed, not about why it did not.
    #[test]
    fn a_member_whose_surface_read_failed_also_reduces_the_stated_coverage() {
        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        set_member("unreadable", vec![route("GET /users/{id}", "local route_get")]);

        let cov = cross_service_coverage(&registry(&["api", "unreadable"]));

        assert_eq!(cov.members_read, 1);
        assert_eq!(cov.members_total, 2);
        assert!(!cov.covers_all_members);
        // The same payload consequences as its engine-start sibling above — the
        // marker is about what CONTRIBUTED, not about why it did not.
        assert_eq!(
            cov.no_provider_in_workspace, 1,
            "the provider was in the member whose surface could not be read"
        );
        assert_eq!(cov.bound, 0);
        assert_eq!(cov.spec_conformance_ratio, None);
    }

    /// **[CR-100]'s shape at its extreme**: no member opens at all, so the
    /// summary is computed over **zero** members of a non-empty roster.
    ///
    /// The partial cases above test 1-of-2. This is the case that generalises the
    /// observed 9-of-72 all the way down, and it is where an absent ratio and a
    /// stated shortfall matter most: every count is 0, which is exactly the state
    /// a fabricated `bound_ratio: 1.0` made look healthy.
    ///
    /// [CR-100]: ../../../docs/requests/CR-100-workspace-resource-budget.md
    #[test]
    fn a_workspace_where_no_member_opens_measures_nothing_and_says_so() {
        reset();
        set_member("broken", vec![route("GET /users/{id}", "local route_get")]);

        let cov = cross_service_coverage(&registry(&["broken"]));

        assert_eq!(cov.members_read, 0, "not one member contributed");
        assert_eq!(cov.members_total, 1);
        assert!(!cov.covers_all_members);
        assert_eq!(cov.bound, 0);
        assert_eq!(cov.no_provider_in_workspace, 0, "no consumer was even read");
        assert!(cov.references.is_empty());
        assert_eq!(
            cov.spec_conformance_ratio, None,
            "a summary over zero members measures NOTHING — the 1.0 this replaces \
             is what made the observed partial workspace look healthy"
        );

        let value = serde_json::to_value(&cov).unwrap();
        assert!(value.get("spec_conformance_ratio").is_none(), "{value}");
        assert_eq!(value["covers_all_members"], false);
    }

    /// An empty workspace covers all of nothing — `0 == 0` is honestly complete,
    /// and the ratio is still absent because nothing was measured.
    #[test]
    fn an_empty_workspace_covers_all_members_and_measures_nothing() {
        reset();
        let cov = cross_service_coverage(&registry(&[]));

        assert_eq!(cov.members_read, 0);
        assert_eq!(cov.members_total, 0);
        assert!(cov.covers_all_members, "0 of 0 members is covered");
        assert_eq!(cov.spec_conformance_ratio, None, "and nothing is measured");
    }

    /// A consumer with exactly one cross-member provider classifies `Bound`.
    #[test]
    fn a_sole_cross_member_provider_is_bound() {
        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_get")]);

        let cov = cross_service_coverage(&registry(&["api", "web"]));

        assert_eq!(cov.bound, 1);
        assert_eq!(cov.ambiguous, 0);
        assert_eq!(cov.unbound, 0);
        assert_eq!(cov.no_provider_in_workspace, 0);
        assert_eq!(cov.references.len(), 1);
        assert_eq!(cov.references[0].state, CoverageState::Bound);
        assert_eq!(cov.references[0].relation, "route");
        assert_eq!(cov.spec_conformance_ratio, Some(1.0));
    }

    /// A row built with no provider evidence at all — the shape every row had
    /// before [CR-118], and the shape a `path-not-composed` row still has.
    ///
    /// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
    fn unnamed() -> RowProvenance {
        RowProvenance {
            providers: ProviderEvidence::Unnamed,
            intake: BridgeIntake::ContractSurface,
        }
    }

    /// The flattened `state` field serializes as one top-level `"state"` key
    /// (never a nested `state.state`), with `reason` present only when
    /// unbound, and `bucket` always present as the direct 3-state label
    /// (`"bound"`/`"ambiguous"`/`"unbound"`) — a per-reference consumer reads
    /// `bucket` without special-casing `reason == "ambiguous"` ([FR-WS-05]).
    #[test]
    fn state_serializes_flat_not_double_nested() {
        let bound = ReferenceCoverage::new(
            "route".to_string(),
            BridgeEndpoint {
                member: "api".to_string(),
                symbol: LogosSymbol::parse("local op_get").unwrap(),
            },
            CoverageState::Bound,
            unnamed(),
        );
        let bound_json = serde_json::to_value(&bound).unwrap();
        assert_eq!(bound_json["state"], "bound");
        assert_eq!(bound_json["bucket"], "bound");
        assert!(bound_json.get("reason").is_none());

        let unbound = ReferenceCoverage::new(
            bound.relation.clone(),
            bound.from.clone(),
            CoverageState::Unbound {
                reason: UnboundReason::NoProviderInWorkspace,
            },
            unnamed(),
        );
        let unbound_json = serde_json::to_value(&unbound).unwrap();
        assert_eq!(unbound_json["state"], "unbound");
        assert_eq!(unbound_json["bucket"], "unbound");
        assert_eq!(unbound_json["reason"], "no-provider-in-workspace");

        let ambiguous = ReferenceCoverage::new(
            bound.relation.clone(),
            bound.from.clone(),
            CoverageState::Unbound {
                reason: UnboundReason::Ambiguous,
            },
            unnamed(),
        );
        let ambiguous_json = serde_json::to_value(&ambiguous).unwrap();
        assert_eq!(
            ambiguous_json["bucket"], "ambiguous",
            "an ambiguous reason gets its own bucket, distinct from the generic unbound bucket"
        );
    }

    // ── CR-118 / FR-WS-05: the row names the other end ────────────────────────

    /// **[CR-118] CRA-01, confirmed rather than assumed.** A bound row carries the
    /// provider it bound to — member *and* symbol — plus the intake the binding
    /// entered through.
    ///
    /// The `to` key was absent on all 875 rows of the 84-member reference
    /// workspace while `xservice route-providers` reported the identical set with
    /// both ends named. This is that gap closed at the point the row is emitted.
    ///
    /// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
    #[test]
    fn a_bound_row_names_the_provider_it_bound_to() {
        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_get")]);

        let cov = cross_service_coverage(&registry(&["api", "web"]));

        let row = &cov.references[0];
        assert_eq!(row.state, CoverageState::Bound);
        let to = row.to.as_ref().expect("a bound row names its provider");
        assert_eq!(to.member, "web");
        assert_eq!(to.symbol, LogosSymbol::parse("local route_get").unwrap());
        assert_eq!(
            row.intake,
            BridgeIntake::ContractSurface,
            "an OpenAPI operation DECLARES an endpoint; the intake says so, exactly \
             as the bridge stamps it on the same binding (CR-083)"
        );
        assert!(
            row.candidates.is_none(),
            "a sole provider is `to`, not a one-element candidate set"
        );

        let value = serde_json::to_value(row).unwrap();
        assert_eq!(value["to"]["member"], "web");
        assert_eq!(value["intake"], "contract-surface");
    }

    /// The bound row's `to` is the **same** `(member, symbol)` pair the bridge
    /// puts on its edge — the two surfaces computed from one pass agree, which is
    /// the whole premise of [CR-118] §2.1 (the answer already existed; it was the
    /// surface framing the question that lacked it).
    ///
    /// Asserted here against the bridge itself, over one registry, so a drift
    /// between the coverage tier's provider and `xservice route-providers`'
    /// provider fails at the unit layer rather than only end-to-end.
    ///
    /// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
    #[test]
    fn the_bound_rows_provider_is_the_one_route_providers_reports() {
        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_get")]);

        let reg = registry(&["api", "web"]);
        let cov = cross_service_coverage(&reg);
        let edges = super::super::bridge::ContractBridge::new().edges(&reg);

        assert_eq!(edges.len(), 1, "one cross-service binding: {edges:?}");
        let bound: Vec<_> = cov.references.iter().filter(|r| r.bucket == "bound").collect();
        assert_eq!(bound.len(), 1);
        assert_eq!(
            bound[0].to.as_ref(),
            Some(&edges[0].to),
            "the coverage row and the bridge edge name the SAME provider"
        );
        assert_eq!(bound[0].from, edges[0].from);
        assert_eq!(bound[0].intake, edges[0].intake);
    }

    /// **[CR-118] CRA-02, confirmed.** An ambiguous row carries the providers it
    /// tied between — the matcher did not discard the losers, they are the very
    /// list the exactly-one test refused.
    ///
    /// **The aggregator ceiling, demonstrated** ([FR-CG-09] Notes). Three members
    /// serve one normalized template, every provider wildcard-method (`ANY`), so
    /// [CR-109]'s exact-method precedence has nothing to prefer — the corpus
    /// shape measured on the reference workspace, where 15 of 86 templates are
    /// multi-provider because the aggregators re-expose the paths they proxy. The
    /// rule refuses correctly; the row now says *between what*, so a reader
    /// recognises the architecture instead of filing a matching defect.
    ///
    /// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
    /// [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
    /// [CR-109]: ../../../docs/requests/CR-109-wildcard-method-route-matching.md
    #[test]
    fn an_ambiguous_row_names_the_three_aggregators_it_tied_between() {
        reset();
        let template = "/v1/users/{userId}/mailboxes/{mailboxId}";
        set_member("mailbox-api", vec![op(&format!("GET {template}"), "local op_mailbox")]);
        // Three providers of one template, every one of them wildcard-method —
        // the reference workspace's exact shape.
        set_member("mailbox-core", vec![route(&format!("ANY {template}"), "local route_core")]);
        set_member(
            "mailbox-aggregator-api",
            vec![route(&format!("ANY {template}"), "local route_mailbox_agg")],
        );
        set_member(
            "funnel-aggregator-api",
            vec![route(&format!("ANY {template}"), "local route_funnel_agg")],
        );

        let cov = cross_service_coverage(&registry(&[
            "mailbox-api",
            "mailbox-core",
            "mailbox-aggregator-api",
            "funnel-aggregator-api",
        ]));

        assert_eq!(cov.ambiguous, 1);
        assert_eq!(cov.bound, 0, "the exactly-one rule refuses, correctly");
        let row = &cov.references[0];
        assert_eq!(row.bucket, "ambiguous");
        assert_eq!(
            row.state,
            CoverageState::Unbound {
                reason: UnboundReason::Ambiguous
            },
            "naming the candidates does NOT bind to them — the state stays unbound \
             and no edge exists (NFR-RA-05)"
        );
        assert!(
            row.to.is_none(),
            "nothing bound, so there is no `to` to name"
        );
        assert_eq!(
            row.intake,
            BridgeIntake::ContractSurface,
            "S-377/[CR-120]: the intake is the POPULATION this reference came from, \
             not provenance for a binding — so a tie carries it too, and it is the \
             contract surface these three aggregators' consumer arrived on"
        );

        let tied = row.candidates.as_ref().expect("the tie is named");
        assert_eq!(tied.disposition, ProviderDisposition::TiedBetween);
        assert_eq!(tied.total, 3);
        assert_eq!(tied.omitted, 0);
        // The (member, symbol) PAIRS, not each half separately: the AC says "each
        // with member and symbol", and an implementation that carried the consumer's
        // symbol — or cloned one candidate's symbol across all three — would satisfy
        // a members-only assertion. Order is the bridge's own deterministic bucket
        // order ([NFR-RA-06]).
        let pairs: Vec<(&str, String)> = tied
            .providers
            .iter()
            .map(|p| (p.member.as_str(), p.symbol.to_string()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("funnel-aggregator-api", "local route_funnel_agg".to_string()),
                ("mailbox-aggregator-api", "local route_mailbox_agg".to_string()),
                ("mailbox-core", "local route_core".to_string()),
            ]
        );
        assert_eq!(tied.summary, "3 tied providers, all listed; none bound");

        // The WIRE shape, pinned here because `web/ui/src/api/types.ts` and the
        // `docs/howto/commands.md` payload block are hand-written mirrors of it: a
        // `rename_all` change or a field rename would otherwise ship a broken UI
        // contract with a fully green suite.
        let value = serde_json::to_value(row).unwrap();
        assert_eq!(value["candidates"]["disposition"], "tied-between");
        assert_eq!(value["candidates"]["total"], 3);
        assert_eq!(
            value["candidates"]["omitted"], 0,
            "present even at zero — absence of truncation is stated, not inferred (NFR-CC-04)"
        );
        assert_eq!(
            value["candidates"]["providers"][0]["member"],
            "funnel-aggregator-api"
        );
        assert_eq!(
            value["candidates"]["summary"],
            "3 tied providers, all listed; none bound"
        );
        assert!(
            value.get("to").is_none(),
            "a tie names no `to` — nothing bound: {value}"
        );
        // Its `intake` IS carried, since S-377: the field names the population the
        // reference came from, not provenance for a binding ([CR-120]).
        assert_eq!(value["intake"], "contract-surface", "{value}");
    }

    /// The **fan-out** truncation branch — the fourth `summarize_candidates` string,
    /// and the one the [`ProviderCandidates::summary`] doc uses as its worked
    /// example. Truncation must disclose its remainder on *both* dispositions, not
    /// only on a tie ([NFR-CC-04]).
    ///
    /// This branch is also where the arity-1 pluralization defect lived unseen: the
    /// only fan-out test bound two subscribers, so no test ever rendered a fan-out
    /// summary at any other arity.
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn a_truncated_fan_out_set_states_how_many_subscribers_it_omits() {
        reset();
        set_consumers("orders", vec![broker_publish("orders.created", "local publish")]);
        let subs: Vec<String> = (0..12).map(|i| format!("s{i:02}")).collect();
        for name in &subs {
            set_consumers(
                name,
                vec![broker_subscribe("orders.created", &format!("local sub_{name}"))],
            );
        }
        let mut names: Vec<&str> = subs.iter().map(String::as_str).collect();
        names.push("orders");

        let cov = cross_service_coverage(&registry(&names));

        let publish = cov
            .references
            .iter()
            .find(|r| r.relation == "broker-topic" && r.bucket == "bound")
            .expect("the publish binds its cross-member subscribers");
        let bound = publish.candidates.as_ref().expect("the bound set is named");
        assert_eq!(bound.disposition, ProviderDisposition::BoundTo);
        assert_eq!(bound.total, 12, "the total is the set BEFORE truncation");
        assert_eq!(bound.providers.len(), CANDIDATE_LIMIT);
        assert_eq!(bound.omitted, 4);
        assert_eq!(bound.summary, "12 bound providers (fan-out), 8 listed, 4 omitted");
        assert_eq!(
            cov.bound, 1,
            "truncating the NAMED set moves no reference between buckets"
        );
        // The `bound-to` wire token, pinned for the same hand-written-mirror reason
        // as its `tied-between` sibling above.
        let value = serde_json::to_value(publish).unwrap();
        assert_eq!(value["candidates"]["disposition"], "bound-to");
        assert_eq!(value["candidates"]["omitted"], 4);
    }

    /// A tie larger than [`CANDIDATE_LIMIT`] is truncated with the remainder
    /// **stated** — in a machine field and in the composed line. No silent trim
    /// ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn a_truncated_candidate_set_states_how_many_it_omits() {
        reset();
        set_member("consumer", vec![op("GET /users/{id}", "local op_get")]);
        let providers: Vec<String> = (0..12).map(|i| format!("p{i:02}")).collect();
        for name in &providers {
            set_member(name, vec![route("ANY /users/{id}", &format!("local route_{name}"))]);
        }
        let mut names: Vec<&str> = providers.iter().map(String::as_str).collect();
        names.push("consumer");

        let cov = cross_service_coverage(&registry(&names));

        assert_eq!(cov.ambiguous, 1);
        let tied = cov.references[0]
            .candidates
            .as_ref()
            .expect("the tie is named");
        assert_eq!(tied.total, 12, "the total is the tie BEFORE truncation");
        assert_eq!(tied.providers.len(), CANDIDATE_LIMIT);
        assert_eq!(tied.omitted, 4);
        // WHICH eight survive, not merely how many: the `providers` doc claims the
        // bridge's own deterministic bucket order ([NFR-RA-06]), and truncation is
        // where that claim earns its keep — an unordered bucket would drop an
        // arbitrary four.
        let kept: Vec<&str> = tied.providers.iter().map(|p| p.member.as_str()).collect();
        assert_eq!(
            kept,
            ["p00", "p01", "p02", "p03", "p04", "p05", "p06", "p07"],
            "the first eight in bucket order, deterministically"
        );
        assert_eq!(
            tied.summary, "12 tied providers, 8 listed, 4 omitted; none bound",
            "the remainder is disclosed in words as well as in a field"
        );
    }

    /// A **fan-out** bound row names the set it fanned out to, not a fabricated
    /// single provider: one publish reaches every cross-member subscriber
    /// ([FR-WS-10]), which is exactly the set `xservice route-providers` reports
    /// as one edge per subscriber.
    ///
    /// The same-member subscriber is excluded from the set for the same reason
    /// the bridge emits no edge for it — it is the intra-repo fan-out the
    /// per-repo graph already owns.
    ///
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    #[test]
    fn a_fan_out_bound_row_names_every_subscriber_it_binds() {
        reset();
        set_consumers(
            "orders",
            vec![
                broker_publish("orders.created", "local publish"),
                // A same-member subscribe: intra-repo fan-out, never a bound end.
                broker_subscribe("orders.created", "local self_sub"),
            ],
        );
        set_consumers("billing", vec![broker_subscribe("orders.created", "local bill_sub")]);
        set_consumers("shipping", vec![broker_subscribe("orders.created", "local ship_sub")]);

        let cov = cross_service_coverage(&registry(&["orders", "billing", "shipping"]));

        let publish = cov
            .references
            .iter()
            .find(|r| r.relation == "broker-topic" && r.bucket == "bound")
            .expect("the publish binds its cross-member subscribers");
        assert!(
            publish.to.is_none(),
            "a fan-out binding has no single `to` — naming one would fabricate a \
             sole provider where the arm binds a set"
        );
        assert_eq!(publish.intake, BridgeIntake::Invocation);
        let bound = publish.candidates.as_ref().expect("the bound set is named");
        assert_eq!(bound.disposition, ProviderDisposition::BoundTo);
        let members: Vec<&str> = bound.providers.iter().map(|p| p.member.as_str()).collect();
        assert_eq!(
            members,
            ["billing", "shipping"],
            "every cross-member subscriber, and only those — the same filter the \
             bridge applies when it emits one edge per subscriber"
        );
        assert_eq!(bound.total, 2);
        assert_eq!(bound.summary, "2 bound providers (fan-out), all listed");
    }

    /// **The bridge de-duplicates fan-out endpoints; so does this tier.** A ledger
    /// holding two rows for one subscribe endpoint must name that subscriber
    /// **once** — and the coverage row's count must equal the number of edges the
    /// bridge emits, asserted here against the real bridge rather than assumed.
    ///
    /// Before [CR-118] this tier's fan-out arm asked only `.any(|p| …)`, so a
    /// duplicate changed a boolean nobody could observe. Now the set is named and
    /// counted, so the same duplicate would publish "2 bound providers (fan-out)"
    /// beside a single bridge edge — a fabricated count ([NFR-RA-05]) and the
    /// classifier drift this module's contract forbids.
    ///
    /// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn a_repeated_ledger_endpoint_is_named_once_and_counted_once() {
        reset();
        set_consumers("orders", vec![broker_publish("orders.created", "local publish")]);
        // The SAME subscribe endpoint twice — two ledger rows, one subscriber.
        set_consumers(
            "billing",
            vec![
                broker_subscribe("orders.created", "local bill_sub"),
                broker_subscribe("orders.created", "local bill_sub"),
            ],
        );

        let reg = registry(&["orders", "billing"]);
        let cov = cross_service_coverage(&reg);
        let edges = super::super::bridge::ContractBridge::new().edges(&reg);

        let publish = cov
            .references
            .iter()
            .find(|r| r.relation == "broker-topic" && r.bucket == "bound")
            .expect("the publish binds its cross-member subscriber");
        let bound = publish.candidates.as_ref().expect("the bound set is named");
        assert_eq!(
            bound.total, 1,
            "one subscriber, named once — not once per ledger row"
        );
        assert_eq!(bound.providers.len(), 1);
        assert_eq!(bound.summary, "1 bound provider (fan-out), all listed");
        assert_eq!(
            bound.total as usize,
            edges.len(),
            "the named count equals the edges the bridge emits — one classifier, no drift"
        );
    }

    /// The new fields are **optional**, and a row with nothing to name carries
    /// none of them — absence, never an empty object or a null. This is the shape
    /// an older store's rows have, and the shape the web coverage view must keep
    /// rendering ([CR-118] §4.5).
    ///
    /// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
    #[test]
    fn a_row_with_no_provider_to_name_omits_every_provider_field() {
        reset();
        set_member("api", vec![op("GET /orphans/{id}", "local op_orphan")]);
        set_member("web", vec![]);

        let cov = cross_service_coverage(&registry(&["api", "web"]));

        let value = serde_json::to_value(&cov.references[0]).unwrap();
        for field in ["to", "candidates"] {
            assert!(
                value.get(field).is_none(),
                "`{field}` is absent, not null: {value}"
            );
        }
        // `intake` was the third field in this list — and in this test's own name —
        // until S-377 made it unconditional: it is not a *provider* field, and a
        // row with no provider to name still has a population it came from
        // ([CR-120]). The name says `provider_field` now, so a later reader
        // extending the loop does not re-add it.
        assert_eq!(value["intake"], "contract-surface", "{value}");
        // And the pre-CR-118 fields are untouched, so a consumer that ignores the
        // new ones reads this row exactly as it read it before.
        assert_eq!(value["state"], "unbound");
        assert_eq!(value["bucket"], "unbound");
        assert_eq!(value["reason"], "no-provider-in-workspace");
    }

    // ── S-377 / CR-120: intake on every row, and the counts split by it ──────

    /// **AC1 — `intake` is present on every row, in every state.**
    ///
    /// The field is no longer an `Option`, so the *type* forbids a missing value
    /// and this test's job is the other half: that serde still emits it. A
    /// re-introduced `skip_serializing_if`, a `#[serde(skip)]`, or a rename are
    /// each a one-line change that the type system would wave through and that
    /// would restore exactly the blind spot [CR-120] §3.1 describes.
    ///
    /// Asserted over the wire form of every row of a fixture carrying all three
    /// display buckets, and the bucket census is asserted first so the loop can
    /// never pass by iterating over a population that lost a state.
    ///
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    #[test]
    fn every_row_reports_its_intake_in_every_state() {
        let cov = every_bucket_in_both_populations();

        // Guard the guard: all three display buckets must actually be present, or
        // "every row carries it" would be a claim about a subset of the states.
        let mut buckets: Vec<&str> = cov.references.iter().map(|r| r.bucket).collect();
        buckets.sort_unstable();
        buckets.dedup();
        assert_eq!(
            buckets,
            ["ambiguous", "bound", "unbound"],
            "the fixture must exercise all three states: {:?}",
            cov.references
        );

        let mut seen: Vec<String> = Vec::new();
        for row in &cov.references {
            let value = serde_json::to_value(row).expect("a row serialises");
            let intake = value
                .get("intake")
                .and_then(|i| i.as_str())
                .unwrap_or_else(|| {
                    panic!(
                        "every coverage row carries `intake` on the wire, in every \
                         state — this {} row does not: {value}",
                        row.bucket
                    )
                })
                .to_string();
            assert!(
                intake == "contract-surface" || intake == "invocation",
                "`intake` is one of the two documented tokens, not {intake}: {value}"
            );
            seen.push(intake);
        }
        seen.sort();
        seen.dedup();
        assert_eq!(
            seen,
            ["contract-surface", "invocation"],
            "and both populations appear, so the token assertion is not vacuous"
        );
    }

    /// **AC2 — the classification counts are split by intake, and the split
    /// reconciles with the headline it sits beside.**
    ///
    /// Three claims, because a split can be wrong in three different ways:
    ///
    /// 1. each population's four counts are the ones the fixture's rows imply;
    /// 2. the two populations sum to the four headline counters — the headline is
    ///    *derived* from the split ([`IntakeSplit::total`]), so this is the
    ///    property that would break if a third population were added and left out
    ///    of the sum;
    /// 3. the split is **recomputable from `references`** by grouping on each
    ///    row's own `intake` and `bucket`. That is what makes it auditable rather
    ///    than merely asserted: a summary counter that disagreed with the rows it
    ///    summarises is the failure mode [CR-111] and [CR-120] both describe, from
    ///    opposite ends.
    ///
    /// [CR-111]: ../../../docs/requests/CR-111-bound-ratio-carries-its-denominator.md
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    #[test]
    fn the_classification_counts_split_by_intake_and_sum_to_the_headline() {
        let cov = every_bucket_in_both_populations();

        // (1) Per-population counts. The contract surface contributes one row to
        // each of the four buckets; the invocation ledger binds its fan-out publish
        // and refuses two topics.
        assert_eq!(
            cov.by_intake.contract_surface,
            ClassificationCounts {
                bound: 1,
                ambiguous: 1,
                unbound: 1,
                no_provider_in_workspace: 1,
            },
            "{:?}",
            cov.references
        );
        assert_eq!(
            cov.by_intake.invocation,
            ClassificationCounts {
                bound: 1,
                ambiguous: 0,
                unbound: 2,
                no_provider_in_workspace: 0,
            },
            "the fan-out publish binds; both broker refusals are unbound: {:?}",
            cov.references
        );

        // (2) The headline is the sum — the same four numbers the pre-S-377 tally
        // produced, so no existing field changed meaning or value.
        assert_eq!(cov.bound, 2, "the HTTP route and the broker fan-out");
        assert_eq!(cov.ambiguous, 1);
        assert_eq!(cov.unbound, 3);
        assert_eq!(cov.no_provider_in_workspace, 1);
        let total = cov.by_intake.total();
        assert_eq!(
            (total.bound, total.ambiguous, total.unbound, total.no_provider_in_workspace),
            (cov.bound, cov.ambiguous, cov.unbound, cov.no_provider_in_workspace),
            "the split's two populations sum to the headline they sit beside"
        );

        // (3) Recomputed from the rows themselves, exactly as a `--json` consumer
        // would — and **without calling the two functions under audit**. Routing
        // this through `population_mut` / `record`, as it first did, made the
        // assertion tautological: swap both arms of `population_mut`, or point them
        // at one field, and the recount swaps with the thing it is checking. So the
        // grouping is hand-written here, over the row's PUBLISHED `intake` and
        // `bucket` — the two strings a consumer actually reads.
        let mut recounted: std::collections::BTreeMap<(&str, &str), u64> =
            std::collections::BTreeMap::new();
        for row in &cov.references {
            let population = match row.intake {
                BridgeIntake::ContractSurface => "contract_surface",
                BridgeIntake::Invocation => "invocation",
            };
            // `no-provider-in-workspace` arrives inside the `unbound` display
            // bucket while its counter sits outside `unbound` ([ADR-53]) — the same
            // split-back-out the web coverage model performs.
            let bucket = match row.state {
                CoverageState::Bound => "bound",
                CoverageState::Unbound {
                    reason: UnboundReason::Ambiguous,
                } => "ambiguous",
                CoverageState::Unbound {
                    reason: UnboundReason::NoProviderInWorkspace,
                } => "no_provider_in_workspace",
                CoverageState::Unbound { .. } => "unbound",
            };
            *recounted.entry((population, bucket)).or_default() += 1;
        }
        for (population, counts) in [
            ("contract_surface", cov.by_intake.contract_surface),
            ("invocation", cov.by_intake.invocation),
        ] {
            for (bucket, reported) in [
                ("bound", counts.bound),
                ("ambiguous", counts.ambiguous),
                ("unbound", counts.unbound),
                ("no_provider_in_workspace", counts.no_provider_in_workspace),
            ] {
                assert_eq!(
                    recounted.get(&(population, bucket)).copied().unwrap_or(0),
                    reported,
                    "{population}/{bucket}: the split must be reproducible from \
                     `references`, not merely reported"
                );
            }
        }
    }

    /// **Every [`BridgeIntake`] variant has a population of its own** — the
    /// table-driven half of the exhaustiveness guard, mirroring
    /// `federation::reach::tests`' own intake table.
    ///
    /// The compiler already refuses a third variant twice over
    /// ([`IntakeSplit::population_mut`] matches on the enum,
    /// [`IntakeSplit::total`] destructures the struct). What that cannot catch is
    /// **two variants sharing one field** — `population_mut` returning
    /// `contract_surface` for both arms compiles, and would silently report an
    /// invocation row as a declared one, which is the misreading the split exists
    /// to end. So each variant is filed and read back separately.
    #[test]
    fn every_bridge_intake_variant_is_counted_under_its_own_population() {
        // Every variant must appear here AND in `population_mut`'s match; the fixed
        // length is what makes adding one to the enum a compile error here too.
        const ALL: [BridgeIntake; 2] = [BridgeIntake::ContractSurface, BridgeIntake::Invocation];

        for (i, intake) in ALL.into_iter().enumerate() {
            let mut split = IntakeSplit::default();
            // A distinct count per variant, so a shared field shows up as a sum
            // rather than as an equal-looking value.
            for _ in 0..=i {
                split.population_mut(intake).record(&CoverageState::Bound);
            }
            let expected = (i + 1) as u64;
            let filed = match intake {
                BridgeIntake::ContractSurface => split.contract_surface,
                BridgeIntake::Invocation => split.invocation,
            };
            assert_eq!(
                filed.bound, expected,
                "{intake:?} is counted under its own population"
            );
            assert_eq!(
                split.total().bound, expected,
                "{intake:?} reaches the derived total, and nothing else does"
            );
        }

        // And the two never collide: filing one of each leaves 1 in both fields.
        let mut split = IntakeSplit::default();
        for intake in ALL {
            split.population_mut(intake).record(&CoverageState::Bound);
        }
        assert_eq!(split.contract_surface.bound, 1);
        assert_eq!(split.invocation.bound, 1);
        assert_eq!(split.total().bound, 2);
    }

    /// The wire tokens the split's two keys are read under, pinned against the
    /// [`BridgeIntake`] tokens the rows carry.
    ///
    /// A consumer joins `by_intake.invocation` to the rows whose `intake` is
    /// `"invocation"`; if the field name and the row token ever spelt the
    /// population differently the join would silently return nothing, and the
    /// split would read as "no invocation references" over a workspace full of
    /// them — the exact reading [CR-120] §3.1 was filed against.
    ///
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    #[test]
    fn the_split_keys_are_spelt_the_way_the_rows_spell_their_intake() {
        let split = serde_json::to_value(IntakeSplit::default()).unwrap();
        for intake in [BridgeIntake::ContractSurface, BridgeIntake::Invocation] {
            let token = serde_json::to_value(intake).unwrap();
            let token = token.as_str().expect("an intake serialises as a string");
            // The row token is kebab-case; the struct key is the same word in the
            // snake_case a Rust field carries. One transformation, stated.
            let key = token.replace('-', "_");
            assert!(
                split.get(&key).is_some(),
                "the split reports a `{key}` population for rows whose intake is \
                 `{token}`: {split}"
            );
        }
        // Only those two, so a stray key cannot be read as a third population.
        assert_eq!(
            split.as_object().map(|o| o.len()),
            Some(2),
            "exactly one population per intake: {split}"
        );
    }

    /// The one fixture that puts **every classification bucket in both intake
    /// populations** — the shape several guards below need, built once.
    ///
    /// Shared rather than copied. Sprint 66's own risk register names a
    /// hand-mirrored twin in this file as the recorded failure mode, and a second
    /// copy of a fixture whose whole value is its bucket/population coverage is
    /// exactly that shape: the copy that grew a fifth row would leave the other
    /// guard silently measuring four.
    ///
    /// | | bound | ambiguous | unbound | no-provider |
    /// |---|---|---|---|---|
    /// | `contract-surface` | 1 | 1 | 1 | 1 |
    /// | `invocation` | 1 | 0 | 2 | 0 |
    ///
    /// The rows also reach [`Tally::record`] down **three** different paths — the
    /// HTTP rows through the contract-surface loop, the broker publishes through
    /// the invocation-consumer loop, and the keyless broker subscribe through the
    /// `unkeyable_providers` loop — so a row filed under the wrong population, like
    /// one filed under the wrong bucket, is visible from here.
    fn every_bucket_in_both_populations() -> CrossServiceCoverage {
        reset();
        set_member(
            "api",
            vec![
                op("GET /users/{id}", "local op_bound"),    // → bound
                op("GET /tied/{id}", "local op_tied"),      // → ambiguous
                op("GET /orphans/{id}", "local op_orphan"), // → no-provider
                op("nonsense", "local op_nonsense"),        // → path-not-composed
            ],
        );
        set_member(
            "web",
            vec![
                route("GET /users/{id}", "local route_users"),
                route("GET /tied/{id}", "local route_tied_web"),
            ],
        );
        set_member("admin", vec![route("GET /tied/{userId}", "local route_tied_admin")]);
        // The broker arm, in the same pass as the four HTTP rows above.
        set_consumers(
            "api",
            vec![
                broker_publish("orders", "local emitOrder"), // → bound (fan-out)
                broker_publish("", "local emitDynamic"),     // → topic-not-literal
            ],
        );
        set_consumers(
            "web",
            vec![
                // The subscriber that makes the publish above bind. A bound
                // subscribe is a provider, so it contributes no row of its own.
                broker_subscribe("orders", "local onOrder"),
                // A refused subscribe — the provider-role refusal, which reaches
                // the tally through `unkeyable_providers`, not the consumer loop.
                broker_subscribe("", "local onByConstant"), // → topic-not-literal
            ],
        );

        cross_service_coverage(&registry(&["api", "web", "admin"]))
    }

    /// **The [CR-118] invariant: no reference changes bucket.** Every bucket, in one
    /// run: the mixed fixture's counts match the classification the unchanged
    /// single-bucket tests above pin individually.
    ///
    /// **What this test does and does not prove.** It cannot itself be a
    /// before/after — its fixture was written in the same commit as the change, so
    /// its golden counts are this author's expectation, not a recorded prior. The
    /// real before/after evidence is that the ~40 pre-existing classification tests
    /// in this module pass with **zero assertion edits**; the only pre-existing test
    /// touched is `state_serializes_flat_not_double_nested`, and only to pass the
    /// new constructor argument. What this test adds is the *mixed* fixture — all
    /// four buckets in a single pass, which no single-bucket test gives — so a
    /// change that shifted one bucket into another at the boundaries would show up
    /// here.
    ///
    /// # Both arms, in the same pass — extended at the sprint-65 sprint review
    ///
    /// The fixture was originally HTTP-only, written in the iteration *before*
    /// [CR-107]/[CR-117] gave the broker arm rows in this same tally. That left the
    /// sprint's own risk — "S-372's no-reference-changes-bucket assertion runs
    /// pre-merge, before the broker refusal rows exist" — resting on the argument
    /// that the two arms are counted separately rather than on a test. It now rests
    /// on a test: the broker arm's three row shapes sit here beside the four HTTP
    /// ones, and the four bucket counters are asserted over the union.
    ///
    /// The two arms reach `Tally::record` down *different* paths — the HTTP rows
    /// through the consumer loop, the keyless broker subscribe through the
    /// `unkeyable_providers` loop — so a refusal mis-filed into `ambiguous` or
    /// `no-provider-in-workspace` (both of which sit outside `unbound`, and one of
    /// which sits outside the ratio denominator entirely) is exactly the shape this
    /// guard has to be able to see.
    ///
    /// It also pins the one place the two stories' shapes genuinely meet: a
    /// *fan-out* bound broker row carries [`ProviderCandidates`] with
    /// [`ProviderDisposition::BoundTo`], the same container the tied HTTP row
    /// carries with [`TiedBetween`](ProviderDisposition::TiedBetween) — the same
    /// shape meaning opposite things, which is why `disposition` exists.
    ///
    /// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
    /// [CR-117]: ../../../docs/requests/CR-117-broker-publish-capture-and-the-topic-key-namespace.md
    /// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
    #[test]
    fn naming_providers_moves_no_reference_between_buckets() {
        let cov = every_bucket_in_both_populations();

        assert_eq!(cov.bound, 2, "the HTTP route and the broker fan-out");
        assert_eq!(cov.ambiguous, 1);
        assert_eq!(
            cov.unbound, 3,
            "the uncomposable template plus both broker refusals: {:?}",
            cov.references
        );
        assert_eq!(cov.no_provider_in_workspace, 1);
        assert_eq!(cov.spec_conformance_ratio, Some(2.0 / 6.0));
        assert_eq!(cov.spec_conformance_measured, 6);

        // Each arm's contribution, so a count that moved between the arms cannot
        // hide inside a total that happens to reconcile.
        let broker_refusals = cov
            .references
            .iter()
            .filter(|r| {
                r.state
                    == CoverageState::Unbound {
                        reason: UnboundReason::TopicNotLiteral,
                    }
            })
            .count();
        assert_eq!(broker_refusals, 2, "{:?}", cov.references);
        assert_eq!(
            cov.references
                .iter()
                .filter(|r| r.relation == "broker-topic")
                .count(),
            3,
            "one bound fan-out plus the two refusals: {:?}",
            cov.references
        );

        // Guard the guard: without this, the loop below is vacuous — if `candidates`
        // stopped being populated at all, its body would never execute and the test
        // whose whole subject is "naming is not binding" would pass by naming
        // nothing. Two rows name a set now, and they mean opposite things.
        assert_eq!(
            cov.references.iter().filter(|r| r.candidates.is_some()).count(),
            2,
            "the tied HTTP row and the fanned-out broker row: {:?}",
            cov.references
        );
        // Every named candidate belongs to a row whose bucket matches its
        // disposition — a tied set is STILL unbound, so naming is not binding and no
        // ArtifactBinding is emitted for any of it (NFR-RA-05); a fanned-out set is
        // bound, and reads that way. This tier writes no edges at all either way, it
        // only classifies.
        let mut seen = Vec::new();
        for row in &cov.references {
            if let Some(candidates) = &row.candidates {
                seen.push(candidates.disposition);
                match candidates.disposition {
                    ProviderDisposition::TiedBetween => {
                        assert_eq!(row.bucket, "ambiguous");
                        assert_eq!(
                            row.state,
                            CoverageState::Unbound {
                                reason: UnboundReason::Ambiguous
                            }
                        );
                    }
                    ProviderDisposition::BoundTo => {
                        assert_eq!(row.bucket, "bound");
                        assert_eq!(row.state, CoverageState::Bound);
                        assert_eq!(row.relation, "broker-topic");
                    }
                }
            }
        }
        seen.sort_by_key(|d| format!("{d:?}"));
        assert_eq!(
            seen,
            vec![
                ProviderDisposition::BoundTo,
                ProviderDisposition::TiedBetween
            ],
            "both dispositions appear, so neither arm's branch is vacuous"
        );
    }

    /// **Payload growth, measured against the reference workspace's shape**
    /// ([CR-118] §7, [NFR-CC-04]).
    ///
    /// The 84-member `pec-services` workspace is un-enrolled, so this reproduces
    /// its *measured shape* rather than re-running it: 875 references — 81 bound,
    /// 146 ambiguous, the rest with no provider in the workspace — every tie
    /// four-way (the measured aggregator ceiling: three aggregators plus the
    /// origin), over SCIP symbols of realistic length. The before/after figures
    /// are printed, so the growth is a number, not an assurance.
    ///
    /// The `--json` baseline there is ~260 KB; this fixture's own baseline is
    /// printed beside its grown size, and the ratio is what carries over — the
    /// absolute figure scales with symbol length, which is a property of the
    /// corpus, not of this change.
    ///
    /// # Two riders, measured apart (S-377)
    ///
    /// Once `intake` rides every row this test measures two changes at once, and a
    /// growth figure that cannot be attributed to a cause is not a measurement. So
    /// **both** baselines are reconstructed and **both** are pinned to the figures
    /// [S-372] recorded for this same fixture, to the byte:
    ///
    /// - pre-CR-118 — no `to`/`intake`/`candidates` on any row, no `by_intake`
    ///   block — is asserted at **195 044 bytes**;
    /// - pre-S-377 — `intake` on the bound rows only, no `by_intake` — is asserted
    ///   at **311 202 bytes**.
    ///
    /// Those two assertions are what make the riders separable rather than merely
    /// printed side by side: a reconstruction that drifted would otherwise stay
    /// inside the ratio bands below while the attribution quietly stopped holding.
    ///
    /// [S-372]: ../../../docs/planning/journal.md#s-372-coverage-rows-name-the-provider-they-bound-and-the-candidates-they-tied-between
    ///
    /// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn payload_growth_is_measured_against_the_reference_workspace_shape() {
        reset();
        // A SCIP symbol of the length this codebase's own graph carries.
        let sym = |member: &str, name: &str| {
            format!("logos . . . {member}/src/main/java/com/example/`{name}.java`/{name}#handle().")
        };
        let mut consumers = Vec::new();
        for i in 0..81 {
            consumers.push(op(&format!("GET /v1/bound/{i}/{{id}}"), &sym("consumer", &format!("BoundClient{i}"))));
        }
        for i in 0..146 {
            consumers.push(op(&format!("GET /v1/tied/{i}/{{id}}"), &sym("consumer", &format!("TiedClient{i}"))));
        }
        for i in 0..648 {
            consumers.push(op(&format!("GET /v1/orphan/{i}/{{id}}"), &sym("consumer", &format!("OrphanClient{i}"))));
        }
        set_member("consumer", consumers);

        // One sole provider per bound reference, and a FOUR-way tie per ambiguous
        // one — the measured ceiling, spread over four aggregator members.
        set_member(
            "origin-api",
            (0..81)
                .map(|i| route(&format!("GET /v1/bound/{i}/{{id}}"), &sym("origin-api", &format!("BoundRoute{i}"))))
                .chain((0..146).map(|i| {
                    route(&format!("ANY /v1/tied/{i}/{{id}}"), &sym("origin-api", &format!("TiedRoute{i}")))
                }))
                .collect(),
        );
        for agg in ["mailbox-aggregator-api", "funnel-aggregator-api", "deprecated-mailbox-core"] {
            set_member(
                agg,
                (0..146)
                    .map(|i| route(&format!("ANY /v1/tied/{i}/{{id}}"), &sym(agg, &format!("TiedRoute{i}"))))
                    .collect(),
            );
        }

        let cov = cross_service_coverage(&registry(&[
            "consumer",
            "origin-api",
            "mailbox-aggregator-api",
            "funnel-aggregator-api",
            "deprecated-mailbox-core",
        ]));

        // The shape is the reference workspace's, so the growth figure describes it.
        assert_eq!(cov.references.len(), 875);
        assert_eq!(cov.bound, 81);
        assert_eq!(cov.ambiguous, 146);
        assert_eq!(cov.no_provider_in_workspace, 648);

        // **The reference workspace's BOUND split, reproduced** (S-377, [CR-120]
        // CRA-02): the 81-row numerator is 81 `contract-surface` and **0**
        // `invocation` — the fact no payload could express before this story.
        //
        // The fixture reproduces that split, not the corpus's whole population,
        // and the difference is stated so the numbers are not over-read. This
        // shape carries no `invocation` rows at all; the live corpus carries 55,
        // of which 0 bind — 54 broker `topic-not-literal` refusals plus the one
        // workspace-wide `http-client-call` reference, which is bucketed
        // `no-provider-in-workspace` rather than `unbound` (the term of art here
        // excludes that bucket, so [CR-120] CRA-02's colloquial "unbound" would
        // mislead in this file). The live figures are measured by
        // `coverage_intake_split::measure_the_intake_split_over_the_reference_workspace_when_one_is_configured`
        // and recorded in `tests/coverage_intake_split/intake_split_finding.txt`.
        assert_eq!(
            cov.by_intake.contract_surface,
            ClassificationCounts {
                bound: 81,
                ambiguous: 146,
                unbound: 0,
                no_provider_in_workspace: 648,
            }
        );
        assert_eq!(
            cov.by_intake.invocation,
            ClassificationCounts::default(),
            "this synthetic shape carries no invocation rows at all; what it \
             reproduces is the corpus's 0 invocation BOUND rows — the half an \
             81-row `bound` count hid"
        );

        let after = serde_json::to_string(&cov).unwrap().len();

        /// Rewind [S-376]'s summary keys on a serialized copy: drop the four the
        /// story adds and restore the three it renamed to their retired
        /// spellings.
        ///
        /// The same discipline as the `by_intake` strip below and for the same
        /// recorded reason — a baseline that claims to predate a change must not
        /// carry any of it. Without this the pre-CR-118 figure would carry
        /// S-376's key renames (+21 bytes) and its four new summary keys, and
        /// **both** byte-exact reconstructions below would fail against figures
        /// [S-372] recorded, which is exactly how this rewind was found.
        ///
        /// [S-376]: ../../../docs/planning/journal.md#s-376-retire-the-bound-ratio-the-headline-is-a-resolved-edge-count
        fn rewind_s376(value: &mut serde_json::Value) {
            let obj = value.as_object_mut().unwrap();
            for added in [
                "resolved_cross_service_edges",
                "egress_resolution",
                "egress_resolution_measured",
                "resolved_edges_summary",
            ] {
                obj.remove(added);
            }
            for (now, retired) in [
                ("spec_conformance_ratio", "bound_ratio"),
                ("spec_conformance_measured", "bound_ratio_measured"),
                ("spec_conformance_summary", "bound_ratio_summary"),
            ] {
                if let Some(v) = obj.remove(now) {
                    obj.insert(retired.to_string(), v);
                }
            }
        }

        // The same payload as it was before CR-118: strip [CR-118]'s three row keys,
        // S-377's summary block AND S-376's headline rename, changing nothing else.
        //
        // The `by_intake` removal is load-bearing, not tidiness. Without it this
        // baseline carries 185 bytes of S-377 (the block plus its comma) while
        // claiming to predate CR-118, so the CR-118 rider would be measured against
        // a payload that already contains part of the change measured below it —
        // and the S-372 comparison two paragraphs down would be off by exactly
        // those 185 bytes. Stripped, `before` reproduces S-372's recorded 195 044
        // to the byte, which is what makes the two riders separable at all.
        let mut value = serde_json::to_value(&cov).unwrap();
        rewind_s376(&mut value);
        value.as_object_mut().unwrap().remove("by_intake");
        for row in value["references"].as_array_mut().unwrap() {
            let row = row.as_object_mut().unwrap();
            for field in ["to", "intake", "candidates"] {
                row.remove(field);
            }
        }
        let before = serde_json::to_string(&value).unwrap().len();
        // The pre-CR-118 baseline, checked against the figure [S-372] recorded for
        // this same fixture — the other half of the attribution, and the reason the
        // two riders below can be reported apart rather than as one number.
        assert_eq!(
            before, 195_044,
            "the pre-CR-118 baseline must reproduce S-372's recorded 195 044 bytes \
             to the byte; got {before}"
        );
        let growth = (after - before) as f64 / before as f64;
        // The HUMAN rendering too, because `workspace status` has no formatter of
        // its own — `Output::print` pretty-prints this same read-model, so the
        // "stays readable on an 84-member workspace" criterion is a claim about
        // THIS figure and its line count, not about a layout. Measured rather than
        // asserted: the bound that keeps it readable is `CANDIDATE_LIMIT`, and a
        // reviewer is entitled to the number it produces.
        let pretty = serde_json::to_string_pretty(&cov).unwrap();
        let pretty_lines = pretty.lines().count();
        println!(
            "CR-118 payload growth at the reference workspace's shape \
             (875 refs: 81 bound / 146 ambiguous / 648 no-provider, ties four-way): \
             compact {before} → {after} bytes (+{:.1}%); \
             human (pretty) {} bytes over {pretty_lines} lines",
            growth * 100.0,
            pretty.len()
        );
        // Measured at **+71.0%** on this shape (195 044 → 333 619 bytes), which
        // projects the ~260 KB reference baseline to roughly 445 KB. With [CR-118]
        // alone as the rider it is **+59.6%** (195 044 → 311 202) — [S-372]'s
        // recorded figure, reproduced to the byte at both ends. S-377 accounts for
        // the rest and is measured on its own below. Almost all of the total is the
        // 146 four-way ties:
        // naming what a reference tied between IS the payload, so the cost is the
        // feature, and the ceiling guards against a blow-up — a doubling, a
        // per-row string, an unbounded set — not against the intended rider.
        //
        // The **floor** is the half that matters more. Stripping the three keys is
        // how `before` is computed, so if the fields silently stopped being emitted
        // `after == before`, growth would be 0.0, and a one-sided `< 0.75` would
        // pass green over a completely dead feature — in the very test named for
        // measuring it. A range fails in both directions.
        assert!(
            (0.40..0.75).contains(&growth),
            "provider identity must stay a rider on the payload — present, and not a \
             rewrite of it: {before} → {after} bytes (+{:.1}%)",
            growth * 100.0
        );

        // **S-377's own increment, separated from CR-118's.** The figure above
        // conflates the two once `intake` rides every row, and a growth number that
        // cannot be attributed is not a measurement. So the pre-S-377 payload is
        // reconstructed exactly: `intake` on the bound rows only (where CR-118 put
        // it), and no `by_intake` block on the summary.
        let mut prior = serde_json::to_value(&cov).unwrap();
        rewind_s376(&mut prior);
        prior.as_object_mut().unwrap().remove("by_intake");
        for row in prior["references"].as_array_mut().unwrap() {
            let row = row.as_object_mut().unwrap();
            if row["bucket"] != "bound" {
                row.remove("intake");
            }
        }
        let pre_s377 = serde_json::to_string(&prior).unwrap().len();
        // The reconstruction's own check, and the reason the S-377 figure below can
        // be attributed at all: this fixture is deterministic, so reproducing
        // [S-372]'s recorded byte count proves the payload was rewound to exactly
        // the state that figure describes. Without it the ratio range below would
        // pass over a reconstruction that had silently drifted — the doc would keep
        // claiming a byte-exact match to a figure no longer reproduced.
        assert_eq!(
            pre_s377, 311_202,
            "the pre-S-377 reconstruction must reproduce S-372's recorded 311 202 \
             bytes to the byte; got {pre_s377}"
        );
        let universal_intake = (after - pre_s377) as f64 / pre_s377 as f64;
        println!(
            "S-377 increment on the same shape: {pre_s377} → {after} bytes \
             (+{:.1}%) — `intake` on the {} non-bound rows plus the `by_intake` block",
            universal_intake * 100.0,
            cov.references.len() - cov.bound as usize
        );
        // A one-line key on 794 rows: a few percent of a payload dominated by the
        // 146 four-way candidate sets. Ranged in both directions for the same
        // reason as above — a zero would mean the field stopped being emitted, in
        // the test named for measuring it.
        assert!(
            (0.02..0.20).contains(&universal_intake),
            "universal `intake` is a per-row key, not a payload rewrite: \
             {pre_s377} → {after} bytes (+{:.1}%)",
            universal_intake * 100.0
        );

        // **S-376's own increment, separated from the two above.** Four summary
        // keys and three renames, on the summary object only — nothing per-row —
        // so on a payload dominated by 875 reference rows it must be a rounding
        // error. That is the claim, and it is measured rather than asserted: a
        // headline that cost a per-row key would be a different design decision,
        // and this is where it would show up.
        let mut pre_s376_value = serde_json::to_value(&cov).unwrap();
        rewind_s376(&mut pre_s376_value);
        let pre_s376 = serde_json::to_string(&pre_s376_value).unwrap().len();
        let headline = (after - pre_s376) as f64 / pre_s376 as f64;
        println!(
            "S-376 increment on the same shape: {pre_s376} → {after} bytes (+{:.3}%) \
             — four summary keys plus three renames, none of them per-row",
            headline * 100.0
        );
        // Ranged in both directions, like its two siblings: a zero would mean the
        // headline stopped being emitted at all, in the test named for measuring
        // it, and an upper bound catches a per-row key added by mistake.
        assert!(
            (0.0001..0.01).contains(&headline),
            "the S-376 headline is a summary-object rider, not a per-row cost: \
             {pre_s376} → {after} bytes (+{:.3}%)",
            headline * 100.0
        );
    }

    /// No provider anywhere in the workspace classifies as its own bucket,
    /// separate from `unbound`, and never enters the `spec_conformance_ratio`
    /// denominator ([ADR-53] acceptance — the rule is unchanged by [CR-120]'s
    /// rename, which is why this test is unchanged too).
    #[test]
    fn no_provider_in_workspace_is_bucketed_separately_and_excluded_from_ratio() {
        reset();
        set_member("api", vec![op("GET /orphans/{id}", "local op_orphan")]);
        set_member("web", vec![]); // no route anywhere

        let cov = cross_service_coverage(&registry(&["api", "web"]));

        assert_eq!(cov.no_provider_in_workspace, 1);
        assert_eq!(cov.bound, 0);
        assert_eq!(cov.ambiguous, 0);
        assert_eq!(cov.unbound, 0);
        assert_eq!(
            cov.references[0].state,
            CoverageState::Unbound {
                reason: UnboundReason::NoProviderInWorkspace
            }
        );
        assert_eq!(
            cov.spec_conformance_ratio, None,
            "the no-provider bucket is excluded from the denominator, which leaves it \
             EMPTY — and 0/0 is reported absent, never as the perfect score it used to \
             fabricate (FR-WS-05, NFR-CC-04)"
        );
    }

    /// Two providers of the same key classify `Ambiguous`, distinct from a
    /// plain `unbound` reason bucket.
    #[test]
    fn two_providers_classify_ambiguous() {
        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_web")]);
        set_member(
            "admin",
            vec![route("GET /users/{userId}", "local route_admin")],
        );

        let cov = cross_service_coverage(&registry(&["api", "web", "admin"]));

        assert_eq!(cov.ambiguous, 1);
        assert_eq!(cov.bound, 0);
        assert_eq!(
            cov.references[0].state,
            CoverageState::Unbound {
                reason: UnboundReason::Ambiguous
            }
        );
        assert_eq!(cov.references[0].state.bucket(), "ambiguous");
    }

    /// A local-plus-remote provider pair is still ambiguous (2 providers),
    /// mirroring the bridge's own classification.
    #[test]
    fn a_local_plus_remote_provider_pair_is_ambiguous() {
        reset();
        set_member(
            "api",
            vec![
                op("GET /users/{id}", "local op_get"),
                route("GET /users/{id}", "local route_local"),
            ],
        );
        set_member("web", vec![route("GET /users/{id}", "local route_web")]);

        let reg = registry(&["api", "web"]);
        let cov = cross_service_coverage(&reg);
        assert_eq!(cov.ambiguous, 1);
        assert_eq!(cov.bound, 0);

        // **[CR-118] CRA-02: the tie lists its same-member participant too.** The
        // consumer's own member holds one of the two tied routes, and it is named:
        // the tie is what refused the binding, so dropping a participant would
        // misreport *why*. This is the one place the tied set deliberately DIVERGES
        // from the fan-out set, which excludes same-member subscribers because the
        // bridge emits no edge for them — an invariant stated emphatically in
        // `tier`, and therefore asserted here rather than left to the comment.
        let tied = cov.references[0]
            .candidates
            .as_ref()
            .expect("the tie is named");
        assert_eq!(tied.total, 2);
        let members: Vec<&str> = tied.providers.iter().map(|p| p.member.as_str()).collect();
        assert_eq!(members, ["api", "web"], "including the consumer's own member");

        // **AC: naming candidates creates no edge** ([NFR-RA-05]). Asserted against
        // the real bridge, mirroring the bound twin's `edges.len() == 1` — this is
        // the story's one claim that was otherwise carried by argument alone.
        assert!(
            super::super::bridge::ContractBridge::new().edges(&reg).is_empty(),
            "a tie fabricates no edge, so there is no ArtifactBinding to emit"
        );
    }

    /// A tie whose candidates are **all in the consumer's own member** — pinned, not
    /// changed.
    ///
    /// Only the *sole* same-member provider is excluded as intra-repo
    /// (`[only] if only.member == member => None`); a two-or-more tie applies no
    /// member filter, so this classifies `ambiguous` on the cross-service board with
    /// no cross-boundary participant at all. That is **pre-existing** tier
    /// behaviour — the bridge agrees, emitting no edge either way — and [CR-118] is
    /// only what made it legible, by naming the participants. Changing it would move
    /// a reference between buckets, which this story's own acceptance criterion
    /// forbids.
    ///
    /// Recorded as a test so the next reader meets it as a known property rather
    /// than as a [CR-118] regression.
    ///
    /// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
    #[test]
    fn an_all_same_member_tie_is_pre_existing_behaviour_and_names_only_intra_repo_candidates() {
        reset();
        set_member(
            "api",
            vec![
                op("GET /users/{id}", "local op_get"),
                route("GET /users/{id}", "local route_one"),
                route("GET /users/{userId}", "local route_two"),
            ],
        );

        let reg = registry(&["api"]);
        let cov = cross_service_coverage(&reg);

        assert_eq!(cov.ambiguous, 1, "pre-existing: a 2+ tie applies no member filter");
        let tied = cov.references[0]
            .candidates
            .as_ref()
            .expect("the tie is named");
        let members: Vec<&str> = tied.providers.iter().map(|p| p.member.as_str()).collect();
        assert_eq!(
            members,
            ["api", "api"],
            "every named participant is the consumer's own member — the property \
             CR-118 makes visible, not one it introduces"
        );
        assert!(
            super::super::bridge::ContractBridge::new().edges(&reg).is_empty(),
            "and the bridge agrees: no edge either way"
        );
    }

    /// A route whose template does not normalize is never a provider
    /// candidate; the consumer classifies `no-provider-in-workspace`, not
    /// silently dropped.
    #[test]
    fn a_non_normalizing_provider_leaves_the_consumer_unbound() {
        reset();
        set_member("api", vec![op("GET /files/{id}", "local op_files")]);
        set_member(
            "web",
            vec![route("GET /files/{*rest}", "local route_catchall")],
        );

        let cov = cross_service_coverage(&registry(&["api", "web"]));
        assert_eq!(cov.no_provider_in_workspace, 1);
        assert_eq!(cov.bound, 0);
    }

    /// A consumer whose own template does not normalize classifies
    /// `path-not-composed` — it is still an accounted-for reference, not
    /// silently dropped the way the bridge's edge computation drops it.
    #[test]
    fn an_uncomposable_consumer_template_is_path_not_composed() {
        reset();
        set_member(
            "api",
            vec![op("GET /files/{*rest}", "local op_catchall")],
        );
        set_member("web", vec![route("GET /files/{id}", "local route_get")]);

        let cov = cross_service_coverage(&registry(&["api", "web"]));

        assert_eq!(cov.unbound, 1);
        assert_eq!(cov.bound, 0);
        assert_eq!(cov.no_provider_in_workspace, 0);
        assert_eq!(
            cov.references[0].state,
            CoverageState::Unbound {
                reason: UnboundReason::PathNotComposed
            }
        );
        assert_eq!(cov.references[0].state.bucket(), "unbound");
    }

    /// A sole provider in the consumer's own member is excluded from the
    /// coverage tier entirely — it is an intra-repo fact, not a cross-boundary
    /// reference, mirroring the bridge's own exclusion.
    #[test]
    fn a_sole_same_member_provider_is_excluded_not_unbound() {
        reset();
        set_member(
            "api",
            vec![
                op("GET /users/{id}", "local op_get"),
                route("GET /users/{id}", "local route_local"),
            ],
        );

        let cov = cross_service_coverage(&registry(&["api"]));

        assert!(
            cov.references.is_empty(),
            "an intra-repo-only match is not a cross-boundary reference: {:?}",
            cov.references
        );
        assert_eq!(cov.bound + cov.ambiguous + cov.unbound + cov.no_provider_in_workspace, 0);
    }

    /// A degraded member (fails to start, or starts but its surface read
    /// fails) is skipped, not fatal — the healthy members still classify
    /// ([ADR-53] degrade-don't-abort, mirroring the bridge).
    #[test]
    fn a_degraded_member_is_skipped_not_fatal() {
        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_get")]);

        let cov = cross_service_coverage(&registry(&["api", "web", "broken"]));
        assert_eq!(cov.bound, 1);

        let cov2 = cross_service_coverage(&registry(&["api", "web", "unreadable"]));
        assert_eq!(cov2.bound, 1);
    }

    /// `spec_conformance_ratio` is computed over bound+ambiguous+unbound only —
    /// the retired bound-ratio's formula, carried forward unchanged in meaning
    /// ([CR-120] §5.2 "name change only").
    #[test]
    fn spec_conformance_ratio_excludes_no_provider_but_includes_ambiguous_and_unbound() {
        reset();
        set_member(
            "api",
            vec![
                op("GET /a/{id}", "local op_a"),  // bound
                op("GET /b/{id}", "local op_b"),  // ambiguous
                op("GET /c/{id}", "local op_c"),  // no-provider
                op("GET /d/{*r}", "local op_d"),  // path-not-composed
            ],
        );
        set_member(
            "web",
            vec![
                route("GET /a/{id}", "local route_a"),
                route("GET /b/{id}", "local route_b1"),
            ],
        );
        set_member("svc", vec![route("GET /b/{id}", "local route_b2")]);

        let cov = cross_service_coverage(&registry(&["api", "web", "svc"]));
        assert_eq!(cov.bound, 1);
        assert_eq!(cov.ambiguous, 1);
        assert_eq!(cov.unbound, 1);
        assert_eq!(cov.no_provider_in_workspace, 1);
        assert_eq!(cov.spec_conformance_ratio, Some(1.0 / 3.0), "1 bound of 3 counted (bound+ambiguous+unbound)");
        assert_eq!(
            cov.spec_conformance_measured, 3,
            "the explicit denominator field agrees with the ratio it was computed over (CR-111)"
        );
        assert_eq!(
            cov.spec_conformance_summary, "0.333 (1 of 3 measured; 1 excluded as no-provider-in-workspace)",
            "the ratio is never presented without its denominator and excluded count (CR-111)"
        );
    }

    // ── CR-111 / FR-WS-05: the ratio never travels without its scale ─────────

    /// The motivating near-degenerate case itself, at the `Tally` level (a full
    /// 906-reference fixture is impractical to construct here — this pins the
    /// exact `pec-services` figures [CR-111] observed: `bound: 6, ambiguous: 0,
    /// unbound: 1, no_provider_in_workspace: 899`).
    ///
    /// A denominator of 7 against 899 exclusions is not `0/0`, so it slipped
    /// past the [S-327] zero-denominator guard while misleading just as
    /// effectively — this is the exact shape that guard does not catch.
    ///
    /// Filed under `contract_surface` because that is what the observed run was:
    /// every bound row on that workspace arrived through a contract surface
    /// ([CR-120] CRA-02). The headline the assertions read is *derived* from the
    /// split ([`IntakeSplit::total`]), so this fixture also exercises that
    /// derivation on the exact figures it was written for.
    ///
    /// [S-327]: ../../../docs/planning/journal.md#s-327-absent-bound-ratio-on-a-zero-denominator
    #[test]
    fn spec_conformance_summary_states_the_pec_services_near_degenerate_case() {
        let tally = Tally {
            by_intake: IntakeSplit {
                contract_surface: ClassificationCounts {
                    bound: 6,
                    ambiguous: 0,
                    unbound: 1,
                    no_provider_in_workspace: 899,
                },
                invocation: ClassificationCounts::default(),
            },
            references: Vec::new(),
        };
        let cov = tally.finish(83, 83);

        assert_eq!(cov.spec_conformance_ratio, Some(6.0 / 7.0));
        assert_eq!(cov.spec_conformance_measured, 7);
        assert_eq!(cov.no_provider_in_workspace, 899);
        assert_eq!(
            cov.spec_conformance_summary,
            "0.857 (6 of 7 measured; 899 excluded as no-provider-in-workspace)",
            "the exact CR-111 headline: correct AND legible, describing 7 of 906 references"
        );
    }

    /// [S-327]'s zero-denominator case still reports the excluded count: "0 of 0
    /// measured, N excluded" is the informative statement, and suppressing both
    /// leaves a reader with nothing (CR-111 §4.4).
    ///
    /// [S-327]: ../../../docs/planning/journal.md#s-327-absent-bound-ratio-on-a-zero-denominator
    #[test]
    fn spec_conformance_summary_reports_the_excluded_count_when_the_ratio_is_absent() {
        let tally = Tally {
            by_intake: IntakeSplit {
                contract_surface: ClassificationCounts {
                    bound: 0,
                    ambiguous: 0,
                    unbound: 0,
                    no_provider_in_workspace: 899,
                },
                invocation: ClassificationCounts::default(),
            },
            references: Vec::new(),
        };
        let cov = tally.finish(1, 1);

        assert_eq!(cov.spec_conformance_ratio, None, "a zero denominator is absent, never a fabricated score");
        assert_eq!(cov.spec_conformance_measured, 0);
        assert_eq!(
            cov.spec_conformance_summary, "0 of 0 measured; 899 excluded as no-provider-in-workspace",
            "the excluded count is STILL reported when the ratio itself is absent"
        );
    }

    /// A `ProtoService`/`GqlType` surface node carries no portable HTTP key in
    /// this story ([FR-WS-07]+ deferred) — mixed alongside genuine
    /// `ApiOperation`/`Route` nodes, it must be silently excluded from both
    /// `consumer_refs` and `providers`, not misclassified or double-counted.
    #[test]
    fn proto_and_graphql_nodes_are_silently_excluded_not_misclassified() {
        reset();
        set_member(
            "api",
            vec![
                op("GET /users/{id}", "local op_get"),
                ContractNode {
                    kind: NodeKind::ProtoService,
                    name: "user.UserService".to_string(),
                    symbol: LogosSymbol::parse("local svc").unwrap(),
                },
            ],
        );
        set_member(
            "web",
            vec![
                route("GET /users/{id}", "local route_get"),
                ContractNode {
                    kind: NodeKind::GqlType,
                    name: "User".to_string(),
                    symbol: LogosSymbol::parse("local gqltype").unwrap(),
                },
            ],
        );

        let with_proto_graphql = cross_service_coverage(&registry(&["api", "web"]));

        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_get")]);
        let without = cross_service_coverage(&registry(&["api", "web"]));

        assert_eq!(
            with_proto_graphql.references, without.references,
            "a ProtoService/GqlType node must not add, remove, or alter any classified reference"
        );
        assert_eq!(with_proto_graphql.bound, 1);
        assert_eq!(with_proto_graphql.ambiguous, 0);
        assert_eq!(with_proto_graphql.unbound, 0);
        assert_eq!(with_proto_graphql.no_provider_in_workspace, 0);
    }

    /// The emitted reference set is deterministic (sorted by endpoint)
    /// regardless of member fan-out order ([NFR-RA-06]).
    #[test]
    fn references_are_deterministic_across_member_order() {
        reset();
        set_member(
            "api",
            vec![op("GET /a/{id}", "local op_a"), op("GET /b/{id}", "local op_b")],
        );
        set_member("web", vec![route("GET /a/{id}", "local route_a")]);
        set_member("svc", vec![route("GET /b/{id}", "local route_b")]);

        let one = cross_service_coverage(&registry(&["api", "web", "svc"]));
        let two = cross_service_coverage(&registry(&["svc", "web", "api"]));
        assert_eq!(one.references, two.references);
    }

    // ── S-252 / FR-WS-08: HTTP client-call consumers in the coverage tier ─────

    /// A static client call with exactly one cross-member route classifies
    /// `Bound` under the `route` relation — the same 3-state model an operation
    /// consumer gets, now driven by a ledger-side invocation consumer.
    #[test]
    fn a_client_call_with_a_sole_cross_member_route_is_bound() {
        reset();
        set_consumers("web", vec![http_call("GET /users/{id}", "local get_user_call")]);
        set_member("api", vec![route("GET /users/{userId}", "local route_get")]);

        let cov = cross_service_coverage(&registry(&["web", "api"]));

        assert_eq!(cov.bound, 1);
        assert_eq!(cov.ambiguous, 0);
        assert_eq!(cov.unbound, 0);
        assert_eq!(cov.no_provider_in_workspace, 0);
        assert_eq!(cov.references.len(), 1);
        assert_eq!(cov.references[0].state, CoverageState::Bound);
        assert_eq!(cov.references[0].relation, "route");
        assert_eq!(cov.references[0].from.member, "web");
    }

    /// Two matching routes make the client call `Ambiguous` (its own bucket).
    #[test]
    fn a_client_call_with_two_routes_is_ambiguous() {
        reset();
        set_consumers("web", vec![http_call("GET /users/{id}", "local get_user_call")]);
        set_member("api", vec![route("GET /users/{id}", "local route_api")]);
        set_member("admin", vec![route("GET /users/{userId}", "local route_admin")]);

        let cov = cross_service_coverage(&registry(&["web", "api", "admin"]));

        assert_eq!(cov.ambiguous, 1);
        assert_eq!(cov.bound, 0);
        assert_eq!(cov.references[0].state.bucket(), "ambiguous");
    }

    /// A client call with no matching route anywhere is bucketed
    /// `no-provider-in-workspace` (outside the boundary, not a defect), excluded
    /// from the `spec_conformance_ratio` denominator.
    #[test]
    fn a_client_call_with_no_route_is_no_provider_in_workspace() {
        reset();
        set_consumers("web", vec![http_call("GET /orphans/{id}", "local orphan_call")]);
        set_member("api", vec![]);

        let cov = cross_service_coverage(&registry(&["web", "api"]));

        assert_eq!(cov.no_provider_in_workspace, 1);
        assert_eq!(cov.bound, 0);
        assert_eq!(
            cov.spec_conformance_ratio, None,
            "the only reference is bucketed out of the denominator, so there is nothing \
             measured — absent, not 1.0 (NFR-CC-04)"
        );
    }

    /// A client call whose only matching route is in its own member is an
    /// intra-repo fact — excluded from the coverage tier, mirroring operations.
    #[test]
    fn a_same_member_client_call_route_pair_is_excluded() {
        reset();
        set_consumers("web", vec![http_call("GET /users/{id}", "local get_user_call")]);
        set_member("web", vec![route("GET /users/{id}", "local route_local")]);

        let cov = cross_service_coverage(&registry(&["web"]));
        assert!(
            cov.references.is_empty(),
            "an intra-repo client call→route pair is not a cross-boundary reference: {:?}",
            cov.references
        );
    }

    /// Acceptance (2): the HTTP arm's refusals map onto the coverage vocabulary —
    /// a base-URL-composed call is `base-url-runtime`, a non-normalizable one is
    /// `path-not-composed`. This ties the arm's refusal reason to the coverage
    /// reason enum.
    ///
    /// Since S-374 both arms of the mapping are on a production path
    /// ([`unkeyable_reason`] → [`client_call_refusal`] → here); the row-level
    /// proof that a *recorded* refusal actually arrives under `base-url-runtime`
    /// is [`a_recorded_client_call_refusal_is_reported_base_url_runtime`], which
    /// is what stops this test being the only caller of the code it covers.
    #[test]
    fn client_call_refusals_map_to_the_coverage_reasons() {
        assert_eq!(
            UnboundReason::from(ClientCallRefusal::BaseUrlRuntime),
            UnboundReason::BaseUrlRuntime
        );
        assert_eq!(
            UnboundReason::from(ClientCallRefusal::PathNotComposed),
            UnboundReason::PathNotComposed
        );
        // And they render as the FR-WS-08 wire tokens.
        assert_eq!(
            serde_json::to_value(UnboundReason::BaseUrlRuntime).unwrap(),
            "base-url-runtime"
        );
        assert_eq!(
            serde_json::to_value(UnboundReason::PathNotComposed).unwrap(),
            "path-not-composed"
        );
    }

    /// **S-374 acceptance: a recorded client-call refusal reaches the [FR-WS-05]
    /// payload as `base-url-runtime`, and reaches it as an unbound row rather
    /// than as nothing.**
    ///
    /// The consumer-side twin of
    /// [`a_refused_broker_topic_is_reported_topic_not_literal`]. Three refusing
    /// call sites in three declarations — the keyless rows
    /// `extract::capture_http_client_call_arm` writes — beside one call that
    /// binds a provider in another member, so the refusals cannot be an artefact
    /// of a fixture in which nothing binds.
    ///
    /// Two things are asserted that the mapping test above cannot reach: the
    /// **reason** is `base-url-runtime` and specifically not `path-not-composed`
    /// (the word this tier gave every unkeyable HTTP row before S-374, and
    /// reporting a keyless row under it would be exactly the classifier drift
    /// this module exists to prevent), and the **row shape** a refusal carries —
    /// no provider named, no candidate set, no intake, filed under `route`,
    /// bucketed `unbound`.
    ///
    /// A `path-not-composed` HTTP row is asserted alongside, because the two are
    /// told apart by one thing only — whether a target was stored — and a test
    /// that pinned only the keyless half would pass if the function returned
    /// `base-url-runtime` unconditionally.
    ///
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    #[test]
    fn a_recorded_client_call_refusal_is_reported_base_url_runtime() {
        reset();
        set_consumers(
            "web",
            vec![
                // The bound control: a static absolute literal that keys.
                http_call("GET /users/{id}", "local get_user"),
                // The recorded refusals — keyless rows, one per declaration.
                http_call("", "local bare_variable"),
                http_call("", "local base_url_join"),
                http_call("", "local builder_lambda"),
                // A stored target that does not normalize: the OTHER HTTP word.
                http_call("GET /files/{*rest}", "local catch_all"),
            ],
        );
        set_member("web", vec![]);
        set_member("api", vec![route("GET /users/{id}", "local users_route")]);

        let cov = cross_service_coverage(&registry(&["api", "web"]));

        let mut refused: Vec<&str> = cov
            .references
            .iter()
            .filter(|r| {
                r.state
                    == CoverageState::Unbound {
                        reason: UnboundReason::BaseUrlRuntime,
                    }
            })
            .map(|r| r.from.symbol.as_str())
            .collect();
        refused.sort();
        assert_eq!(
            refused,
            vec!["local bare_variable", "local base_url_join", "local builder_lambda"],
            "each keyless row is one base-url-runtime reference: {:?}",
            cov.references
        );

        // The other HTTP word is still reachable and still distinct.
        let not_composed: Vec<&str> = cov
            .references
            .iter()
            .filter(|r| {
                r.state
                    == CoverageState::Unbound {
                        reason: UnboundReason::PathNotComposed,
                    }
            })
            .map(|r| r.from.symbol.as_str())
            .collect();
        assert_eq!(not_composed, vec!["local catch_all"]);

        // The row shape of a refusal: nothing to point at, but it DOES name the
        // population it came from (S-377, [CR-120]) — a recorded refusal is an
        // invocation-intake row, and reporting that is the whole reason the arm
        // records it.
        let row = cov
            .references
            .iter()
            .find(|r| r.from.symbol.as_str() == "local bare_variable")
            .expect("the refusal row");
        assert_eq!(row.relation, "route");
        assert_eq!(row.from.member, "web");
        assert!(row.to.is_none(), "{row:?}");
        assert!(row.candidates.is_none(), "{row:?}");
        assert_eq!(row.intake, BridgeIntake::Invocation, "{row:?}");
        assert_eq!(row.bucket, "unbound");

        // The control bound, so the refusals sit beside a real binding.
        assert_eq!(cov.bound, 1);
        assert_eq!(cov.unbound, 4);
        assert_eq!(cov.ambiguous, 0);
    }

    /// **The route-composition refusal has no mapping onto this vocabulary, and
    /// that is the assertion.**
    ///
    /// This test replaces `route_composition_refusals_map_to_the_same_coverage_reason`,
    /// which pinned an `impl From<RouteRefusal> for UnboundReason` that had no
    /// runtime caller (S-378, [CR-120]) — the test itself was the only caller of the
    /// code it covered, which is how dead code stays compiling and reads as coverage.
    ///
    /// **What replaces it is the fact that made the mapping unnecessary**, not an
    /// assertion that the impl is absent. Rust has no stable way to say a trait is
    /// *not* implemented, and a test dressed up to look like it could would be the
    /// same class of defect as the doc comment this story corrects. So nothing here
    /// mechanically prevents the mapping being re-added; what is recorded is the
    /// reason re-adding it would be wrong, in a form that fails if the reason ever
    /// stops being true.
    ///
    /// A refused registration promotes **no `route` node**, so the provider member
    /// carries nothing at that key. That is reproduced here exactly: `web` names
    /// `GET /v1/users` and `api` holds an unrelated route, which is
    /// indistinguishable at this tier from `api` having refused to compose
    /// `/v1/users`. The consumer is reported `no-provider-in-workspace` — the
    /// bucket [ADR-53] holds outside the ratio denominator — and **not**
    /// `path-not-composed`. There is therefore no row a `RouteRefusal` mapping
    /// could ever have labelled, which is why removing it changes no output.
    ///
    /// The grain [FR-FW-05] actually asks for — `FrameworkStats::routes_not_composed`
    /// — is pinned from a real index run by
    /// `spring_non_literal_prefix_promotes_no_route_and_is_counted` in
    /// `logos-core/tests/multilang.rs` (and its Kotlin twin). The consumer-side
    /// mapping that survives is pinned by
    /// `client_call_refusals_map_to_the_coverage_reasons` two functions above.
    /// Neither is re-asserted here: mirroring an existing assertion into a second
    /// place is how the two copies later disagree.
    ///
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    /// [FR-FW-05]: ../../../docs/specs/requirements/FR-FW-05.md
    #[test]
    fn a_registration_that_promoted_no_route_reads_no_provider_not_path_not_composed() {
        reset();
        set_member("web", vec![op("GET /v1/users", "local op_list")]);
        // What a refused registration leaves behind: the member is present and
        // indexed, and simply has no `route` node at the consumer's key.
        set_member("api", vec![route("GET /v1/health", "local route_health")]);

        let cov = cross_service_coverage(&registry(&["web", "api"]));

        assert_eq!(cov.references.len(), 1, "{:?}", cov.references);
        assert_eq!(
            cov.references[0].state,
            CoverageState::Unbound {
                reason: UnboundReason::NoProviderInWorkspace
            },
            "a refused registration leaves no provider to label, so this tier \
             reports no-provider-in-workspace and never path-not-composed"
        );
        assert_eq!(cov.no_provider_in_workspace, 1);
        assert_eq!(cov.bound, 0);
    }

    // ── CR-109 / S-349: wildcard-method matching with exact-method precedence ──
    //
    // The read-model half of the shared fixture matrix. The intra-repo binder and
    // the bridge can only observe *whether* a reference bound; this site observes
    // *why* it did not, which is what pins the ambiguous-vs-no-provider
    // distinction the acceptance criteria require to be reported rather than
    // absorbed ([FR-WS-05], [NFR-CC-04]).

    /// The coverage state every matrix outcome must be reported as.
    fn expected_state(expect: crate::resolve::route_method::matrix::Expect) -> CoverageState {
        use crate::resolve::route_method::matrix::Expect;
        match expect {
            Expect::Binds(_) => CoverageState::Bound,
            Expect::Ambiguous => CoverageState::Unbound {
                reason: UnboundReason::Ambiguous,
            },
            Expect::NoProvider => CoverageState::Unbound {
                reason: UnboundReason::NoProviderInWorkspace,
            },
            Expect::NotComposed => CoverageState::Unbound {
                reason: UnboundReason::PathNotComposed,
            },
        }
    }

    /// Every matrix case, driven through the coverage read-model: the consumer
    /// operation alone in `spec`, each provider alone in its own `p{i}`.
    #[test]
    fn the_wildcard_method_matrix_holds_at_the_coverage_read_model() {
        for case in crate::resolve::route_method::matrix::MATRIX {
            reset();
            set_member("spec", vec![op(case.consumer, "local operation")]);
            let mut members = vec!["spec".to_string()];
            for (i, name) in case.providers.iter().enumerate() {
                let member = format!("p{i}");
                set_member(&member, vec![route(name, &format!("local route_{i}"))]);
                members.push(member);
            }
            let names: Vec<&str> = members.iter().map(String::as_str).collect();

            let cov = cross_service_coverage(&registry(&names));

            assert_eq!(
                cov.references.len(),
                1,
                "{}: the operation is the one classified reference",
                case.name
            );
            assert_eq!(
                cov.references[0].state,
                expected_state(case.expect),
                "{}: `{}` against {:?} must be reported this way",
                case.name,
                case.consumer,
                case.providers
            );
        }
    }

    /// The ambiguity a wildcard introduces is **reported**, not absorbed: a
    /// template genuinely owned by two members counts in the `ambiguous` bucket
    /// and never in `unbound`, so a rising ambiguity is legible as the correct
    /// refusal it is rather than as a silent loss ([FR-WS-05], [NFR-RA-05]).
    #[test]
    fn a_wildcard_ambiguity_is_counted_as_ambiguous_not_absorbed() {
        reset();
        set_member("spec", vec![op("GET /v1/users/{id}/mailboxes/{mid}", "local op")]);
        set_member("a", vec![route("ANY /v1/users/{uid}/mailboxes/{m}", "local route_a")]);
        set_member("b", vec![route("ANY /v1/users/{u}/mailboxes/{box}", "local route_b")]);

        let cov = cross_service_coverage(&registry(&["spec", "a", "b"]));

        assert_eq!(cov.ambiguous, 1, "the two-owner template is ambiguous");
        assert_eq!(cov.bound, 0, "no provider is guessed (NFR-RA-05)");
        assert_eq!(cov.unbound, 0, "ambiguity is its own bucket, never folded into unbound");
        assert_eq!(cov.references[0].bucket, "ambiguous");
    }

    /// Operation consumers and client-call consumers coexist: an `ApiOperation`
    /// and an HTTP client call both binding cross-member each count once, proving
    /// the two intake paths compose without double-counting or interfering.
    #[test]
    fn operation_and_client_call_consumers_coexist() {
        reset();
        // An operation in `spec` bound by a route in `web`; a client call in `web`
        // bound by a route in `api` — both cross-member, no same-member collision.
        set_member("spec", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_users")]);
        set_consumers("web", vec![http_call("GET /orders/{id}", "local orders_call")]);
        set_member("api", vec![route("GET /orders/{id}", "local route_orders")]);

        let cov = cross_service_coverage(&registry(&["spec", "web", "api"]));
        assert_eq!(cov.bound, 2, "the operation and the client call each bind once");
        assert_eq!(cov.references.len(), 2);
        assert!(cov.references.iter().all(|r| r.relation == "route"));
    }

    // ── S-253 / FR-WS-09: gRPC stub-call coverage ────────────────────────

    /// A gRPC stub call whose `package.Service/Method` provider lives in another
    /// member classifies `Bound`, under the `grpc-call` relation.
    #[test]
    fn a_grpc_stub_call_binds_its_cross_member_proto_service() {
        reset();
        set_member(
            "svc",
            vec![proto_service("example.v1.UserService/GetUser", "local svc")],
        );
        set_consumers(
            "api",
            vec![grpc_consumer("example.v1.UserService/GetUser", "local stub")],
        );

        let cov = cross_service_coverage(&registry(&["api", "svc"]));
        assert_eq!(cov.bound, 1);
        assert_eq!(cov.references.len(), 1);
        assert_eq!(cov.references[0].state, CoverageState::Bound);
        assert_eq!(cov.references[0].relation, "grpc-call");
        // [CR-118] on the SECOND exactly-one arm. The `to` doc names `route` and
        // `grpc-call` together; only `route` was covered. This is also the only
        // `Sole` + `Invocation` pairing in the suite — a captured call site binding
        // a single cross-member provider, which is the commonest bound row shape on
        // a real workspace.
        let to = cov.references[0]
            .to
            .as_ref()
            .expect("a bound gRPC row names its provider");
        assert_eq!(to.member, "svc");
        assert_eq!(to.symbol, LogosSymbol::parse("local svc").unwrap());
        assert_eq!(
            cov.references[0].intake,
            BridgeIntake::Invocation,
            "a stub call is a captured call site, not a declared contract"
        );
        assert!(cov.references[0].candidates.is_none());
    }

    /// Acceptance (3): a qualifiable gRPC stub call with no provider anywhere in
    /// the workspace stays honestly unbound with a coverage reason
    /// (`no-provider-in-workspace`) — never silently dropped, never fabricated.
    #[test]
    fn a_grpc_stub_call_with_no_provider_is_unbound_with_a_reason() {
        reset();
        set_consumers(
            "api",
            vec![grpc_consumer("example.v1.UserService/GetUser", "local stub")],
        );
        // A second member with no matching proto service provider.
        set_member("svc", vec![proto_service("example.v1.Other/Do", "local other")]);

        let cov = cross_service_coverage(&registry(&["api", "svc"]));
        assert_eq!(cov.no_provider_in_workspace, 1);
        assert_eq!(cov.bound, 0);
        assert_eq!(
            cov.references[0].state,
            CoverageState::Unbound {
                reason: UnboundReason::NoProviderInWorkspace
            }
        );
        assert_eq!(cov.references[0].relation, "grpc-call");
    }

    /// A gRPC stub call whose sole provider is in its own member is an intra-repo
    /// fact the per-repo graph owns — excluded from the coverage tier entirely,
    /// exactly as the HTTP path and the bridge (never counted as `Bound`).
    #[test]
    fn an_intra_repo_grpc_call_is_excluded_not_bound() {
        reset();
        set_member(
            "svc",
            vec![proto_service("example.v1.UserService/GetUser", "local svc")],
        );
        set_consumers(
            "svc",
            vec![grpc_consumer("example.v1.UserService/GetUser", "local stub")],
        );

        let cov = cross_service_coverage(&registry(&["svc"]));
        assert!(
            cov.references.is_empty(),
            "a same-member stub→provider pair is intra-repo, not a cross-boundary reference: {:?}",
            cov.references
        );
        assert_eq!(cov.bound + cov.ambiguous + cov.unbound + cov.no_provider_in_workspace, 0);
    }

    /// Two members exposing the identical `package.Service/Method` provider make
    /// the stub call ambiguous — its own bucket, distinct from a plain unbound.
    #[test]
    fn two_grpc_providers_classify_ambiguous() {
        reset();
        set_member(
            "svc1",
            vec![proto_service("example.v1.UserService/GetUser", "local a")],
        );
        set_member(
            "svc2",
            vec![proto_service("example.v1.UserService/GetUser", "local b")],
        );
        set_consumers(
            "api",
            vec![grpc_consumer("example.v1.UserService/GetUser", "local stub")],
        );

        let cov = cross_service_coverage(&registry(&["api", "svc1", "svc2"]));
        assert_eq!(cov.ambiguous, 1);
        assert_eq!(cov.bound, 0);
        assert_eq!(cov.references[0].state.bucket(), "ambiguous");
        // [CR-118]: the tie is named on the gRPC arm too, not only on `route`.
        let tied = cov.references[0]
            .candidates
            .as_ref()
            .expect("the gRPC tie is named");
        assert_eq!(tied.disposition, ProviderDisposition::TiedBetween);
        assert_eq!(tied.total, 2);
        let members: Vec<&str> = tied.providers.iter().map(|p| p.member.as_str()).collect();
        assert_eq!(members, ["svc1", "svc2"]);
        assert_eq!(tied.summary, "2 tied providers, all listed; none bound");
    }

    // ── S-254 / FR-WS-10: broker-topic consumers in the coverage tier ─────────

    /// Integration guard (S-INT): a broker **publish** is a `Consumer`-role arm, so
    /// it reaches the coverage tier through the same generic ledger intake the HTTP
    /// and gRPC arms use. Its topic key *composes* — so it must never be reported
    /// as `path-not-composed` (the HTTP arm's refusal reason) and must never land
    /// in the `unbound` defect bucket that drags the bound ratio down.
    ///
    /// With **no subscriber anywhere in the workspace** (this fixture), the publish is
    /// honestly `no-provider-in-workspace` — outside the boundary, not a defect. Once a
    /// subscriber exists the verdict flips to `bound`, which is
    /// [`a_bound_broker_publish_is_reported_bound_not_no_provider`] — the two together
    /// pin both halves.
    #[test]
    fn a_broker_publish_topic_keys_and_is_never_path_not_composed() {
        reset();
        set_consumers("api", vec![broker_publish("orders", "local emit_order")]);
        set_member("svc", vec![]);

        let cov = cross_service_coverage(&registry(&["api", "svc"]));

        assert_eq!(
            cov.unbound, 0,
            "a statically-composed topic is never `path-not-composed`: {:?}",
            cov.references
        );
        assert_eq!(cov.no_provider_in_workspace, 1);
        assert_eq!(cov.references.len(), 1);
        assert_eq!(cov.references[0].relation, "broker-topic");
        assert_eq!(
            cov.references[0].state,
            CoverageState::Unbound {
                reason: UnboundReason::NoProviderInWorkspace
            }
        );
        // The non-defect bucket does not drag the bound ratio down — it empties
        // the denominator entirely, and 0/0 is reported absent rather than as a
        // perfect score ([NFR-CC-04]).
        assert_eq!(cov.spec_conformance_ratio, None);
    }

    /// **[CR-107] acceptance: a refused topic reaches the payload.** The
    /// `@KafkaListener(topics = TOPIC)` case. The listener side is the arm's
    /// **provider** role, so its recorded keyless row used to fall out of this tier
    /// entirely — no reference, no reason, nothing to distinguish "every topic here
    /// is externalised" from "there is no broker wiring here" ([NFR-CC-04]). It is
    /// now one `topic-not-literal` row, and specifically **not**
    /// `path-not-composed`: that is the HTTP arm's word and reporting it here would
    /// be the classifier drift this module exists to prevent.
    ///
    /// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
    #[test]
    fn a_refused_broker_topic_is_reported_topic_not_literal() {
        reset();
        // One listener whose topic is a literal, one whose topic was refused. The
        // keyless row is what `extract::broker` records for the refusal.
        set_consumers(
            "svc",
            vec![
                broker_subscribe("orders", "local onOrder"),
                broker_subscribe("", "local byConstant"),
            ],
        );
        set_member("api", vec![]);

        let cov = cross_service_coverage(&registry(&["api", "svc"]));

        let refused: Vec<&ReferenceCoverage> = cov
            .references
            .iter()
            .filter(|r| {
                r.state
                    == CoverageState::Unbound {
                        reason: UnboundReason::TopicNotLiteral,
                    }
            })
            .collect();
        assert_eq!(
            refused.len(),
            1,
            "the refused listener is one row, the bound one is not: {:?}",
            cov.references
        );
        assert_eq!(refused[0].relation, "broker-topic");
        assert_eq!(refused[0].from.member, "svc");
        assert_eq!(refused[0].from.symbol.as_str(), "local byConstant");
        // It has no provider to name — it never had a key to look one up with, and
        // it bound no edge ([CR-118]'s `Unnamed` shape, which is exactly what a
        // `path-not-composed` row already carries). Its INTAKE is nonetheless
        // reported: the row is a captured broker site, and which population a
        // refusal belongs to is the fact S-377 makes universal ([CR-120]).
        assert!(refused[0].to.is_none(), "{:?}", refused[0]);
        assert!(refused[0].candidates.is_none(), "{:?}", refused[0]);
        assert_eq!(refused[0].intake, BridgeIntake::Invocation, "{:?}", refused[0]);
        assert_eq!(refused[0].bucket, "unbound");
        // The keyed subscribe indexed a provider and reported nothing itself (a
        // provider is not a reference), so the refusal is the only row.
        assert_eq!(cov.references.len(), 1, "{:?}", cov.references);
        assert_eq!(cov.unbound, 1);
        assert_eq!(cov.no_provider_in_workspace, 0);
        assert_eq!(cov.ambiguous, 0);
        assert_eq!(cov.bound, 0);
    }

    /// **S-370 / [CR-117] acceptance: a refused header-form publish reaches the
    /// [FR-WS-05] payload, once per site.**
    ///
    /// **What is genuinely new here, stated precisely — the path itself is already
    /// covered.** [CR-107] proved the reason for the arm's *provider* role (the
    /// listener), and the consumer role does travel a different path through this
    /// function — the `inv_consumers` loop and [`unkeyable_reason`] rather than the
    /// `unkeyable_providers` bucket. But that consumer path is **not** untested:
    /// [`a_refused_topic_indexes_no_provider_and_binds_no_publish`] already drives a
    /// keyless `broker_publish` through `cross_service_coverage` and asserts
    /// `TopicNotLiteral`, and [`an_unkeyable_row_is_reported_under_its_own_arms_reason`]
    /// asserts the relation→reason map directly.
    ///
    /// What this test adds is two things neither of those covers: a **bound publish
    /// coexisting with refusals** (so the refusals cannot be an artefact of a
    /// fixture in which nothing binds), and the **row shape** a consumer-side
    /// refusal carries — `to`, `candidates` and `intake` all absent, filed under
    /// `broker-topic`, bucketed `unbound`. S-370 is what makes the shape reachable
    /// from real source: before it the arm did not recognise a header-form publish
    /// at all, so no such row existed to classify.
    ///
    /// Three sites, one keyless each: a method parameter, a configuration-bound
    /// getter and a `@Value`-injected field — the three operand shapes the reference
    /// estate actually writes. Each is its own row, each reads `topic-not-literal`
    /// and not `path-not-composed`, and the bound publish beside them is unaffected.
    ///
    /// Be exact about the grain, because it is easy to overstate: three refused
    /// publishes reach this tier as three rows when they sit in three **methods**,
    /// which is what this fixture's three distinct symbols model. Three in ONE
    /// method reach the ledger as ONE row — `dedup_sort_refs` keys on
    /// `(source, target, form, kind, relation)` and every refusal shares an empty
    /// target — so the "at most once per site" discipline is enforced upstream in
    /// [`crate::extract::broker`], not here. This test asserts what this layer
    /// actually owns: that whatever rows arrive are classified under the arm's own
    /// reason.
    ///
    /// [CR-117]: ../../../docs/requests/CR-117-broker-publish-capture-and-the-topic-key-namespace.md
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    #[test]
    fn refused_header_form_publishes_are_reported_topic_not_literal_once_per_site() {
        reset();
        // The producer member: one publish that keyed, three that were refused. A
        // refusal is a keyless row, which is what `extract::broker` records for a
        // recognised publish site whose topic operand is not a literal.
        set_consumers(
            "producer",
            vec![
                broker_publish("orders", "local sendKept"),
                broker_publish("", "local sendByParameter"),
                broker_publish("", "local sendByConfiguredGetter"),
                broker_publish("", "local sendByInjectedField"),
            ],
        );
        // A subscriber on the keyed topic, so the bound row really binds and the
        // refusals are not the only thing this fixture can produce.
        set_consumers("consumer", vec![broker_subscribe("orders", "local onOrder")]);
        set_member("producer", vec![]);
        set_member("consumer", vec![]);

        let cov = cross_service_coverage(&registry(&["consumer", "producer"]));

        let mut refused: Vec<&str> = cov
            .references
            .iter()
            .filter(|r| {
                r.state
                    == CoverageState::Unbound {
                        reason: UnboundReason::TopicNotLiteral,
                    }
            })
            .map(|r| r.from.symbol.as_str())
            .collect();
        refused.sort();
        assert_eq!(
            refused,
            vec![
                "local sendByConfiguredGetter",
                "local sendByInjectedField",
                "local sendByParameter",
            ],
            "each refused publish site is its own row, named by the method that \
             refused: {:?}",
            cov.references
        );

        // Filed under the arm's own word, with nothing to name — it never had a key
        // to look a provider up with.
        for reference in cov.references.iter().filter(|r| {
            r.state
                == CoverageState::Unbound {
                    reason: UnboundReason::TopicNotLiteral,
                }
        }) {
            assert_eq!(reference.relation, "broker-topic", "{reference:?}");
            assert_eq!(reference.from.member, "producer", "{reference:?}");
            assert_eq!(reference.bucket, "unbound", "{reference:?}");
            assert!(reference.to.is_none(), "{reference:?}");
            assert!(reference.candidates.is_none(), "{reference:?}");
            assert_eq!(reference.intake, BridgeIntake::Invocation, "{reference:?}");
        }

        // The keyed publish is untouched by the refusals beside it: it binds to the
        // subscriber's provider row — the property this fixture exists for, since a
        // refusal-only fixture cannot show that the two do not interfere.
        assert_eq!(cov.bound, 1, "{:?}", cov.references);
        assert_eq!(cov.unbound, 3, "{:?}", cov.references);
        assert_eq!(cov.no_provider_in_workspace, 0);
        assert_eq!(cov.ambiguous, 0);
    }

    /// A refused topic indexes **no provider**, so a cross-member publish on a real
    /// topic cannot bind to it — the refusal is reported, never matched
    /// ([NFR-RA-05]). Guards the keyless-row gate in `consumer_portable_key`: if a
    /// keyless row keyed as the empty topic it would sit in the provider index and
    /// a publish on the empty key would bind to it.
    #[test]
    fn a_refused_topic_indexes_no_provider_and_binds_no_publish() {
        reset();
        set_consumers("svc", vec![broker_subscribe("", "local byConstant")]);
        set_consumers("api", vec![broker_publish("", "local emitDynamic")]);
        set_member("api", vec![]);
        set_member("svc", vec![]);

        let cov = cross_service_coverage(&registry(&["api", "svc"]));

        assert_eq!(cov.bound, 0, "a refusal binds nothing: {:?}", cov.references);
        assert_eq!(cov.unbound, 2, "both refusals are reported: {:?}", cov.references);
        for reference in &cov.references {
            assert_eq!(
                reference.state,
                CoverageState::Unbound {
                    reason: UnboundReason::TopicNotLiteral
                },
                "{reference:?}"
            );
        }
    }

    /// A placeholder topic is a **static literal**, so it keys and binds like any
    /// other — the coverage half of [CR-107]'s capture fix. A publish and a subscribe
    /// on `"${spring.kafka.topics.orders}"` meet on that key, and the row is `bound`,
    /// never `topic-not-literal`.
    ///
    /// The key here is the placeholder text as written: reducing it to the committed
    /// configured value is [CR-117](
    /// ../../../docs/requests/CR-117-broker-publish-capture-and-the-topic-key-namespace.md)
    /// §3.2's canonical-identity rule, which this story does not own.
    #[test]
    fn a_placeholder_topic_keys_and_binds_like_any_other_literal() {
        reset();
        let topic = "${spring.kafka.topics.orders}";
        set_consumers("api", vec![broker_publish(topic, "local emit_order")]);
        set_consumers("svc", vec![broker_subscribe(topic, "local onOrder")]);
        set_member("api", vec![]);
        set_member("svc", vec![]);

        let cov = cross_service_coverage(&registry(&["api", "svc"]));

        assert_eq!(
            cov.bound, 1,
            "a placeholder literal is a topic identity: {:?}",
            cov.references
        );
        assert_eq!(cov.unbound, 0);
        assert_eq!(cov.references.len(), 1);
        assert_eq!(cov.references[0].state, CoverageState::Bound);
    }

    /// **The cross-surface guard, inverted so a closed list cannot rot.**
    /// [CR-107] added one `UnboundReason` variant and it had to be mirrored by hand
    /// onto five other surfaces — this enum, the operator manual, the frontend
    /// design's enumerated list, the TypeScript union, and its label map. The first
    /// four were updated and `docs/howto/commands.md` was missed, because nothing
    /// checked it: this module's own comment concedes those mirrors are hand-written,
    /// and the wire-shape test beside it pins field names, not the reason set.
    ///
    /// So the guard is written the way a guard over an enumerated surface has to be:
    /// it enumerates the **variants** and requires each to be documented, rather
    /// than listing the documented tokens and checking them off. `wire` is an
    /// exhaustive `match`, and `ALL` is a fixed-length array — so the next arm's
    /// reason fails to compile here until it is classified, and then fails this
    /// assertion until it is documented. Appending one entry cannot satisfy it.
    ///
    /// Which files count as surfaces, and how a missing `docs/specs` symlink is
    /// treated, live in [`readable_reason_surfaces`] — shared with the removal
    /// proof beside it so the two halves cannot cover different file sets.
    ///
    /// **This guard covers only one direction.** It cannot notice a token a surface
    /// still enumerates after the variant behind it is *removed*, because a closed
    /// list stops mentioning what it no longer contains — hence
    /// [`the_removed_schema_mismatch_reason_is_absent_from_every_surface`].
    ///
    /// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
    #[test]
    fn every_unbound_reason_is_documented_on_every_surface_that_enumerates_them() {
        /// The wire token of one reason. Exhaustive on purpose: a new variant does
        /// not compile until it is named here.
        fn wire(reason: UnboundReason) -> &'static str {
            match reason {
                UnboundReason::NoProviderInWorkspace => "no-provider-in-workspace",
                UnboundReason::PathNotComposed => "path-not-composed",
                UnboundReason::BaseUrlRuntime => "base-url-runtime",
                UnboundReason::Ambiguous => "ambiguous",
                UnboundReason::TopicNotLiteral => "topic-not-literal",
            }
        }
        /// Every variant. The fixed length is the second half of the guard: adding a
        /// variant without extending this fails to compile.
        const ALL: [UnboundReason; 5] = [
            UnboundReason::NoProviderInWorkspace,
            UnboundReason::PathNotComposed,
            UnboundReason::BaseUrlRuntime,
            UnboundReason::Ambiguous,
            UnboundReason::TopicNotLiteral,
        ];

        // The wire token really is what serde emits — otherwise this guard would
        // check a string the payload never carries.
        for reason in ALL {
            assert_eq!(
                serde_json::to_value(reason).unwrap(),
                wire(reason),
                "{reason:?} must serialise as its documented token"
            );
        }

        /// Is `token` present as a *reason token* rather than as an ordinary word?
        ///
        /// Bare substring matching made one arm of this guard unfalsifiable.
        /// `ambiguous` is not only a reason: it is also the `CoverageState` tag, the
        /// TypeScript `CoverageBucket` member, and an English word these surfaces
        /// use freely ("bound/ambiguous/unbound", `ambiguous-candidate`). So
        /// `contains("ambiguous")` could not fail, and dropping `Ambiguous` from a
        /// surface's enumeration would have gone unnoticed — the one token of the
        /// five with that problem.
        ///
        /// So the form required is an *enumeration* shape, not merely a delimited
        /// one: a markdown backtick, a union arm `| "token"`, or a map key
        /// `token:` / `"token":`. A plain `"token"` is deliberately **not**
        /// enough — `coverageModel.ts` contains `ref.bucket === "ambiguous"`, a
        /// bucket comparison, and accepting that left the arm exactly as
        /// unfalsifiable as the bare substring it replaced.
        ///
        /// # `ambiguous` on the two `web/ui` surfaces is still not falsifiable
        ///
        /// Stated rather than left implied, because a guard that looks stronger
        /// than it is, is the defect this story exists to remove. `ambiguous` is
        /// the one wire word shared by two enums: `UnboundReason::Ambiguous` *and*
        /// the 3-state `CoverageBucket`/`CoverageState` tag. So `types.ts` supplies
        /// `| "ambiguous"` from `CoverageBucket` (`"bound" | "ambiguous" |
        /// "unbound"`) and `coverageModel.ts` supplies `ambiguous:` from its
        /// counter fields, whatever the reason union and the label map say. Deleting
        /// the real reason entry from either file was tried and the guard still
        /// passed. No textual check can separate the two meanings there; only
        /// parsing the declarations could, which is more machinery than an
        /// enumeration mirror is worth.
        ///
        /// The tightening still earns its place: the other four tokens are now
        /// genuinely checked on all six surfaces (each fails when removed), and the
        /// pure-prose loophole is closed on the markdown surfaces — where
        /// `ambiguous` *is* falsifiable, since they write it in backticks.
        fn enumerated(text: &str, token: &str) -> bool {
            text.contains(&format!("`{token}`"))
                || text.contains(&format!("| \"{token}\""))
                || text.contains(&format!("\"{token}\":"))
                || text.contains(&format!("\n  {token}:"))
        }

        for (rel, text, enumerates_all) in readable_reason_surfaces() {
            if !enumerates_all {
                continue;
            }
            for reason in ALL {
                assert!(
                    enumerated(&text, wire(reason)),
                    "{rel} enumerates the unbound reasons but does not mention \
                     `{}` as a reason token — a reason the payload can carry that \
                     this surface cannot explain ([NFR-CC-04])",
                    wire(reason)
                );
            }
        }
    }

    /// **Every intake token and every split key is explained on every surface that
    /// documents the payload** (S-377, [CR-120], [NFR-CC-04]).
    ///
    /// The sibling of
    /// [`every_unbound_reason_is_documented_on_every_surface_that_enumerates_them`],
    /// and it exists for the same recorded reason. That guard was written because
    /// [CR-107] added one reason token and `docs/howto/commands.md` was missed for
    /// releases — a closed list of surfaces, hand-mirrored, that nothing checked.
    /// S-377 has just created the same shape for a *second* vocabulary: `intake`'s
    /// two tokens and `by_intake`'s two keys are now hand-written into the operator
    /// manual, the TypeScript wire type, the dashboard model and the MCP tool
    /// description. Only the MCP one was guarded (by
    /// `mcp/tests/workspace_status_intake_parity.rs`), which is exactly the
    /// asymmetry that let the reason token rot.
    ///
    /// # What it checks, and the one thing it cannot
    ///
    /// Both directions of the join, because either alone leaves a way in: the row
    /// token (`contract-surface`) and the split key (`contract_surface`) are the
    /// same population spelt two ways, and a rename of one without the other makes
    /// a consumer's join silently return nothing — reading as "no invocation
    /// references" over a workspace full of them, which is [CR-120] §3.1's defect
    /// restored by a typo.
    ///
    /// It cannot notice a token a surface still documents after the variant behind
    /// it is *removed* — a closed list stops mentioning what it no longer contains.
    /// [`BridgeIntake`] has never lost a variant, so there is no removal proof to
    /// write yet; the pattern to copy when there is one is
    /// [`the_removed_schema_mismatch_reason_is_absent_from_every_surface`].
    ///
    /// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn every_intake_token_and_split_key_is_documented_on_every_payload_surface() {
        /// The wire token of one intake. Exhaustive on purpose: a new variant does
        /// not compile until it is named here.
        fn wire(intake: BridgeIntake) -> &'static str {
            match intake {
                BridgeIntake::ContractSurface => "contract-surface",
                BridgeIntake::Invocation => "invocation",
            }
        }
        /// Every variant. The fixed length is the second half of the guard: adding
        /// one without extending this fails to compile.
        const ALL: [BridgeIntake; 2] = [BridgeIntake::ContractSurface, BridgeIntake::Invocation];

        // The tokens really are what serde emits, and the keys really are what the
        // split serializes — otherwise this guard would check strings the payload
        // never carries, which is how a green guard covers nothing.
        let split = serde_json::to_value(IntakeSplit::default()).unwrap();
        for intake in ALL {
            assert_eq!(
                serde_json::to_value(intake).unwrap(),
                wire(intake),
                "{intake:?} must serialise as its documented token"
            );
            assert!(
                split.get(wire(intake).replace('-', "_")).is_some(),
                "the split must carry a population key for {intake:?}: {split}"
            );
        }

        /// The surfaces that document this payload's fields. Tracked in **this**
        /// repository, all three — unlike the reason vocabulary, the intake
        /// vocabulary is not mirrored into `docs/specs`, so there is no
        /// tolerated-absence case and a read failure is a guard defect.
        ///
        /// `mcp/src/server.rs` is deliberately **not** here: its description is
        /// guarded at the MCP boundary against the *shipped* tool
        /// (`LogosMcp::list_tools()`), which is stronger than reading the source,
        /// and duplicating it would create the second copy this guard exists to
        /// prevent.
        const SURFACES: [&str; 3] = [
            "docs/howto/commands.md",
            "web/ui/src/api/types.ts",
            "web/ui/src/views/workspace/coverageModel.ts",
        ];
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("logos-core sits under the repository root");

        for rel in SURFACES {
            let text = std::fs::read_to_string(repo.join(rel))
                .unwrap_or_else(|e| panic!("{rel} must be readable to be guarded: {e}"));
            for intake in ALL {
                let token = wire(intake);
                // The row token, in an enumeration shape rather than as a bare
                // word: `invocation` is also an English word and the name of a
                // whole subsystem here, so a plain `contains` could not fail.
                assert!(
                    text.contains(&format!("`{token}`")) || text.contains(&format!("| \"{token}\"")),
                    "{rel} documents the coverage payload but does not name \
                     `{token}` as an intake token — a value the payload carries \
                     that this surface cannot explain ([NFR-CC-04])"
                );
                // And the split key it joins to.
                let key = token.replace('-', "_");
                assert!(
                    text.contains(&format!("`{key}`"))
                        || text.contains(&format!("{key}:"))
                        || text.contains(&format!("\"{key}\":")),
                    "{rel} names the `{token}` intake but not the `{key}` population \
                     key the split reports it under — a consumer joining the two \
                     would silently match nothing ([CR-120] §3.1)"
                );
            }
            assert!(
                text.contains("by_intake"),
                "{rel} documents the coverage payload but never names `by_intake`, \
                 the field the split is reported in"
            );
        }
    }

    /// **Every headline field is documented on every surface that documents the
    /// payload, and the count is never documented without its rate** (S-376,
    /// [CR-120], [BR-51], [NFR-CC-04]).
    ///
    /// The third member of the family
    /// [`every_unbound_reason_is_documented_on_every_surface_that_enumerates_them`]
    /// and
    /// [`every_intake_token_and_split_key_is_documented_on_every_payload_surface`]
    /// belong to, and it exists for the recorded reason both of them do: a closed
    /// list of hand-mirrored surfaces with nothing checking it rots, and
    /// `docs/howto/commands.md` was missed for releases the last time a vocabulary
    /// grew. S-376 renames three payload fields and adds four, across the operator
    /// manual, the TypeScript wire type and the dashboard model — the same shape,
    /// for a third vocabulary.
    ///
    /// # The [BR-51] half is the one that is not merely bookkeeping
    ///
    /// Naming both fields is not enough: a surface must state the **duty that
    /// binds them**, by citing [BR-51]. That is deliberately a check on the rule
    /// rather than on the pair, because a check on the pair would be unfalsifiable
    /// here — the positive list above already requires both names, so "mentions
    /// the count but not the rate" is a state no surface can reach. An arm that
    /// cannot fail is the defect S-378 spent a story removing, so it is not
    /// written.
    ///
    /// What the rule-citation arm catches is the case that actually happens: a
    /// surface that lists the two fields as two independent figures, leaving a
    /// reader free to quote the count alone. That is how a resolved-edge count
    /// becomes read as a coverage percentage — the misreading [CR-120] was filed
    /// about. It sits beside the payload-layer guard
    /// [`the_resolved_edge_count_is_never_published_without_its_egress_rate`],
    /// which stops the *server* emitting a bare count; neither implies the other.
    ///
    /// # A plain `contains` is enough here, and that is a property of the tokens
    ///
    /// Its siblings need an enumeration shape (a backtick, a union arm, a map key)
    /// because `ambiguous` and `invocation` are also ordinary English words, so a
    /// bare substring test could not fail. These tokens are unique snake_case
    /// identifiers that occur nowhere in prose, so a plain `contains` is already
    /// falsifiable — each was checked by deleting it from each surface. Requiring
    /// a shape on top would only reject the honest mention `coverage.egress_resolution`
    /// that the dashboard model actually makes.
    ///
    /// The retired `bound_ratio` is deliberately **not** asserted absent from these
    /// surfaces: all three document the retirement in prose, and that is correct
    /// documentation rather than rot. Its absence is asserted where it is a fact
    /// about behaviour rather than about wording — on the emitted payload, at all
    /// four renderings.
    ///
    /// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn every_headline_field_is_documented_on_every_payload_surface() {
        /// The fields S-376 introduces or renames, as the payload spells them.
        /// Read off a real serialization below rather than trusted, so this guard
        /// cannot check strings the payload never carries.
        const HEADLINE: [&str; 7] = [
            "resolved_cross_service_edges",
            "egress_resolution",
            "egress_resolution_measured",
            "resolved_edges_summary",
            "spec_conformance_ratio",
            "spec_conformance_measured",
            "spec_conformance_summary",
        ];

        // The keys really are what the payload emits. `egress_resolution` is
        // absent on a zero denominator, so the fixture must be one where it is
        // present — otherwise this guard would tolerate a key that never ships.
        let cov = every_bucket_in_both_populations();
        let wire = serde_json::to_value(&cov).unwrap();
        for key in HEADLINE {
            assert!(
                wire.get(key).is_some(),
                "`{key}` must be a key the payload actually carries, or this guard \
                 checks a string nothing emits: {wire}"
            );
        }

        /// The surfaces that document this payload's fields. The same tracked trio
        /// the intake guard reads, and for the same reason: this vocabulary is not
        /// mirrored into `docs/specs`, so there is no tolerated-absence case and a
        /// read failure is a guard defect.
        ///
        /// `mcp/src/server.rs` is deliberately absent: its description is guarded
        /// at the MCP boundary against the *shipped* tool
        /// (`mcp/tests/workspace_status_intake_parity.rs`), which is stronger than
        /// reading the source, and duplicating it here would create the second copy
        /// this family of guards exists to prevent.
        const SURFACES: [&str; 3] = [
            "docs/howto/commands.md",
            "web/ui/src/api/types.ts",
            "web/ui/src/views/workspace/coverageModel.ts",
        ];
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("logos-core sits under the repository root");

        for rel in SURFACES {
            let text = std::fs::read_to_string(repo.join(rel))
                .unwrap_or_else(|e| panic!("{rel} must be readable to be guarded: {e}"));
            for key in HEADLINE {
                assert!(
                    text.contains(key),
                    "{rel} documents the coverage payload but never names `{key}` — \
                     a field the payload carries that this surface cannot explain \
                     ([NFR-CC-04])"
                );
            }
            // [BR-51], at the documentation layer: the rule, not just the pair.
            assert!(
                text.contains("BR-51"),
                "{rel} names the resolved-edge count and its egress rate but never \
                 cites BR-51, the rule that binds them — a reader told they are two \
                 fields, rather than a figure and the rate it must never be quoted \
                 without, will quote the count alone"
            );
        }
    }

    /// Every surface that enumerates the reason vocabulary, with its text — the
    /// **one** list, read by the positive guard above and by the removal proof
    /// below.
    ///
    /// Shared deliberately. Sprint 66's own risk register names a hand-mirrored
    /// twin in this file as the recorded failure mode, and a second copy of this
    /// list is exactly that shape: the copy that keeps a *removed* token out
    /// would silently stop covering a surface the copy that keeps *present*
    /// tokens in had already grown.
    ///
    /// `docs/howto/commands.md` and the two `web/ui` files are tracked in this
    /// repository and must be readable. The four `docs/specs` entries reach a
    /// symlink into the separate docs repository, so their absence is tolerated
    /// (a vendored or packaged checkout has no `docs/specs`) while their presence
    /// is checked — the one thing never done is passing silently on a file that
    /// *is* readable.
    ///
    /// **So the enforcement floor is the tracked trio, not all seven.** In CI and
    /// in any fresh clone `docs/specs` does not exist, and those four entries are
    /// skipped; the spec surfaces are enforced only on a developer checkout that
    /// carries the `logos-docs` symlink. Stated because the negative direction
    /// makes this load-bearing in a way the old positive-only guard never was: a
    /// *stale* `logos-docs` checkout is present-but-wrong, so it fails the removal
    /// proof for a reason outside this repository. That is the intended signal —
    /// the two repositories disagree — but it means a branch touching this
    /// vocabulary must land with its `logos-docs` commit, not after it.
    fn readable_reason_surfaces() -> Vec<(&'static str, String, bool)> {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("logos-core sits under the repository root");
        /// One surface the reason vocabulary is mirrored onto.
        ///
        /// `enumerates_all` is the distinction between the two guards, and it is
        /// not cosmetic. A surface that lists the *whole* vocabulary owes every
        /// surviving reason an entry — that is the parity guard. A surface that
        /// merely *names* a reason in passing owes nothing positive, but still must
        /// not offer a reader a retired one — that is the removal proof. Collapsing
        /// the two made the parity guard demand a full enumeration from
        /// `FR-WS-10`, the broker-arm requirement, which has no business carrying
        /// one.
        struct Surface {
            rel: &'static str,
            /// Tracked in THIS repository, so a read failure is a guard defect.
            tracked_here: bool,
            /// Keeps revision history under a trailing `## Notes` heading.
            has_notes_history: bool,
            /// Enumerates the complete reason vocabulary (⇒ subject to the parity
            /// guard, not only the removal proof).
            enumerates_all: bool,
        }
        const fn s(
            rel: &'static str,
            tracked_here: bool,
            has_notes_history: bool,
            enumerates_all: bool,
        ) -> Surface {
            Surface { rel, tracked_here, has_notes_history, enumerates_all }
        }
        let surfaces = [
            s("docs/howto/commands.md", true, false, true),
            s("docs/specs/frontend-design.md", false, false, true),
            s("docs/specs/requirements/FR-WS-05.md", false, true, true),
            s("docs/specs/architecture/decisions/ADR-53.md", false, true, true),
            // Names a reason in passing; does not enumerate the vocabulary.
            s("docs/specs/requirements/FR-WS-10.md", false, true, false),
            s("web/ui/src/api/types.ts", true, false, true),
            s("web/ui/src/views/workspace/coverageModel.ts", true, false, true),
        ];
        /// The **normative** part of a surface: everything before its trailing
        /// `## Notes` heading.
        ///
        /// A requirement's or ADR's Notes section is its revision history, and this
        /// repository's amendment convention requires a superseded value to be
        /// *shown as superseded* rather than deleted — so a Notes paragraph
        /// legitimately quotes a retired reason token by name. Checking the
        /// normative body is what keeps the removal guard below from forbidding the
        /// very record that explains the removal, without weakening it: the
        /// enumeration a reader acts on is in the Statement or the Decision, not
        /// the history.
        ///
        /// Applied **only** to the surfaces that declare the convention. Blanket
        /// truncation would be a silent hole in the other four: nothing stops the
        /// 96 KB operator manual or a `.ts` doc block growing a `## Notes` line,
        /// and every enumeration after it would stop being checked by both guards
        /// with no failure — the same closed-list blind spot the removal proof
        /// exists to cover.
        fn normative(text: String) -> String {
            match text.find("\n## Notes") {
                Some(at) => text[..at].to_string(),
                None => text,
            }
        }

        let mut out = Vec::new();
        for Surface { rel, tracked_here, has_notes_history, enumerates_all } in surfaces {
            match std::fs::read_to_string(repo.join(rel)) {
                Ok(text) => out.push((
                    rel,
                    if has_notes_history { normative(text) } else { text },
                    enumerates_all,
                )),
                // Only a `docs/specs` file that is genuinely *not present* may be
                // skipped. A tracked surface, or one that exists but cannot be read
                // (a dangling symlink, a permission error, invalid UTF-8), is the
                // guard silently covering nothing — which is the failure mode it is
                // here to prevent, so it fails loudly instead.
                Err(e) => assert!(
                    !tracked_here && e.kind() == std::io::ErrorKind::NotFound,
                    "{rel} must be readable to be guarded, but failed: {e}"
                ),
            }
        }
        out
    }

    /// **The removal proof for `schema-mismatch` (S-378, [CR-120] §7).**
    ///
    /// The variant is gone from the enum, so the positive guard above can no longer
    /// say anything about it — a closed list stops mentioning what it no longer
    /// contains, which is precisely how a retired token survives on four other
    /// surfaces for releases. This asserts the other half: no surface still offers
    /// a reader a reason the payload can never carry.
    ///
    /// `docs/specs/requirements/FR-WS-05.md` is in the shared list for this reason.
    /// Its Statement enumerates the reason vocabulary, so leaving `schema-mismatch`
    /// there would have moved the dishonesty from the code into the specification
    /// rather than removing it. Only its normative body is read — its Notes section
    /// records the removal and must name the token to do so; see
    /// [`readable_reason_surfaces`].
    ///
    /// Why an absent-token guard is not written for *every* conceivable retired
    /// name: this one is named because it was really removed and the surfaces
    /// really did carry it. A general "no unknown token" guard would need the
    /// closed vocabulary these surfaces deliberately do not have — the TypeScript
    /// union's own comment requires readers to treat it as **open**, so a later
    /// arm's reason can reach a build that predates it.
    ///
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    #[test]
    fn the_removed_schema_mismatch_reason_is_absent_from_every_surface() {
        for (rel, text, _) in readable_reason_surfaces() {
            assert!(
                !text.contains("schema-mismatch"),
                "{rel} still enumerates `schema-mismatch`, a reason S-378 removed \
                 because no producer for it exists anywhere in the tree — a surface \
                 offering a reader a reason the payload can never carry ([NFR-CC-04])"
            );
        }
    }

    /// `topic-not-literal` is the **broker** arm's word for an unkeyable row —
    /// chosen by the arm's own namespace, not by a shared default — and the HTTP
    /// arm has **two**, told apart by whether a target was stored at all
    /// (S-374): a keyless row is its recorded `base-url-runtime` refusal, a
    /// stored target that will not key is `path-not-composed`. Both render as
    /// their FR-WS-05 wire tokens.
    ///
    /// The broker rows are asserted on **both** target shapes precisely because
    /// the reason must NOT depend on the target there: the arm has one word, and
    /// reading the row would be the drift this function exists to prevent.
    #[test]
    fn an_unkeyable_row_is_reported_under_its_own_arms_reason() {
        use crate::model::ArtifactRelation;

        for target in ["", "orders"] {
            assert_eq!(
                unkeyable_reason(ArtifactRelation::BrokerSubscribe, target),
                UnboundReason::TopicNotLiteral,
                "the broker arm has one word whatever the row carries ({target:?})"
            );
            assert_eq!(
                unkeyable_reason(ArtifactRelation::BrokerPublish, target),
                UnboundReason::TopicNotLiteral
            );
            assert_eq!(
                unkeyable_reason(ArtifactRelation::GrpcCall, target),
                UnboundReason::PathNotComposed
            );
        }

        // The HTTP arm's two words, and the empty/blank equivalence the ledger's
        // keyless row relies on.
        assert_eq!(
            unkeyable_reason(ArtifactRelation::HttpClientCall, "GET /files/{*rest}"),
            UnboundReason::PathNotComposed
        );
        for keyless in ["", "   "] {
            assert_eq!(
                unkeyable_reason(ArtifactRelation::HttpClientCall, keyless),
                UnboundReason::BaseUrlRuntime,
                "a keyless HTTP row is the arm's recorded base-url-runtime refusal"
            );
            assert_eq!(
                client_call_refusal(keyless),
                ClientCallRefusal::BaseUrlRuntime
            );
        }
        assert_eq!(
            client_call_refusal("GET /users/{id}"),
            ClientCallRefusal::PathNotComposed
        );
        assert_eq!(
            serde_json::to_value(UnboundReason::TopicNotLiteral).unwrap(),
            "topic-not-literal"
        );
        assert_eq!(
            serde_json::to_value(UnboundReason::BaseUrlRuntime).unwrap(),
            "base-url-runtime"
        );
    }

    // ── S-256 / FR-WS-11: the coverage tier and the bridge must not disagree ──

    /// **Regression for the S-256 cross-surface contradiction.** A publish that the
    /// bridge BINDS to a cross-member subscribe must be reported `bound` here.
    ///
    /// Before the fix, the coverage tier built its provider index from
    /// `contract_surface` alone. A broker subscribe has no contract-surface node — it
    /// exists only in the ledger — so no broker provider was ever indexed and this
    /// publish was reported `no-provider-in-workspace` *while the service map drew the
    /// very coupling it denied*. The board and the map, side by side in the same tab,
    /// disagreed about the same fact ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn a_bound_broker_publish_is_reported_bound_not_no_provider() {
        reset();
        set_member("api", vec![]);
        set_consumers("api", vec![broker_publish("orders", "local emit_order")]);
        set_member("billing", vec![]);
        set_consumers("billing", vec![broker_subscribe("orders", "local on_order")]);

        let cov = cross_service_coverage(&registry(&["api", "billing"]));

        assert_eq!(
            cov.bound, 1,
            "the publish binds its cross-member subscribe — the tier must say so: {:?}",
            cov.references
        );
        assert_eq!(
            cov.no_provider_in_workspace, 0,
            "a provider EXISTS in this workspace — it is a ledger-only one"
        );
        assert_eq!(cov.unbound, 0);
        assert_eq!(cov.ambiguous, 0);
        assert_eq!(cov.references.len(), 1, "the subscribe is a provider, not a second reference");
        assert_eq!(cov.references[0].state, CoverageState::Bound);
        assert_eq!(cov.references[0].relation, "broker-topic");
        assert_eq!(cov.spec_conformance_ratio, Some(1.0));
    }

    /// A topic with **two** cross-member subscribers is `bound`, never `ambiguous`.
    /// The broker namespace's discipline is fan-out ([ADR-54]): one publish reaches
    /// every subscriber, so a second subscriber is the arm working as designed. The
    /// tier now reads the discipline off the key's namespace instead of hardcoding
    /// exactly-one arity — the same decision the bridge's `match_indexed` makes, which
    /// is what keeps the two from drifting.
    ///
    /// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
    #[test]
    fn a_topic_with_two_subscribers_is_bound_not_ambiguous() {
        reset();
        set_member("api", vec![]);
        set_consumers("api", vec![broker_publish("orders", "local emit_order")]);
        set_member("billing", vec![]);
        set_consumers("billing", vec![broker_subscribe("orders", "local bill_on_order")]);
        set_member("shipping", vec![]);
        set_consumers("shipping", vec![broker_subscribe("orders", "local ship_on_order")]);

        let cov = cross_service_coverage(&registry(&["api", "billing", "shipping"]));

        assert_eq!(
            cov.ambiguous, 0,
            "two subscribers is fan-out, not ambiguity: {:?}",
            cov.references
        );
        assert_eq!(cov.bound, 1);
    }

    /// A publish whose ONLY subscriber is in its own member is the intra-repo fan-out
    /// the per-repo graph already owns — the bridge emits no edge for it, so the tier
    /// reports no cross-boundary reference either (it is excluded, not `unbound`).
    #[test]
    fn a_same_member_only_subscriber_is_not_a_cross_boundary_reference() {
        reset();
        set_member("api", vec![]);
        set_consumers(
            "api",
            vec![
                broker_publish("orders", "local emit_order"),
                broker_subscribe("orders", "local api_local_listener"),
            ],
        );
        set_member("billing", vec![]);

        let cov = cross_service_coverage(&registry(&["api", "billing"]));

        assert!(
            cov.references.is_empty(),
            "an intra-repo publish→subscribe pair is not a cross-boundary reference: {:?}",
            cov.references
        );
        assert_eq!((cov.bound, cov.ambiguous, cov.unbound, cov.no_provider_in_workspace), (0, 0, 0, 0));
    }

    /// The HTTP arm's exactly-one discipline is **unchanged** by the discipline-aware
    /// tiering: two providers of one route key are still `ambiguous`, not "bound
    /// because a cross-member one exists". Guards against the fan-out rule leaking
    /// across namespaces.
    #[test]
    fn the_http_arm_stays_exactly_one_under_the_discipline_aware_tier() {
        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_a")]);
        set_member("svc", vec![route("GET /users/{id}", "local route_b")]);

        let cov = cross_service_coverage(&registry(&["api", "web", "svc"]));

        assert_eq!(
            cov.ambiguous, 1,
            "two route providers remain ambiguous — HTTP is exactly-one: {:?}",
            cov.references
        );
        assert_eq!(cov.bound, 0);
    }

    /// **One endpoint captured twice is one site and one set of edges** — the
    /// consumer half of the collapse [`super::broker::broker_edges`] performs
    /// ([NFR-RA-05], [CR-118]).
    ///
    /// A ledger can hold two rows for one endpoint (they differ in `form`, which is
    /// inside the ledger's identity but outside the endpoint's), and the provider
    /// arm of this tier has collapsed them since [CR-107]. The consumer arm did
    /// not. Before S-376 that inflated the pooled `bound` invisibly; S-376 promoted
    /// the same rows into `resolved_cross_service_edges`, where the duplicate would
    /// publish **twice the bridge's edge count** under a field whose own contract
    /// says a reader can reconcile it against `references`.
    ///
    /// Asserted on the count, the split AND the edge total, because the three fail
    /// differently: a missing collapse shows up as two rows, two bound sites and
    /// double the edges, and pinning only the last would pass over a tier that
    /// double-counted the sites and happened to halve the fan-out.
    ///
    /// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
    /// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn a_publish_endpoint_captured_twice_is_one_site_and_one_set_of_edges() {
        reset();
        set_member("api", vec![]);
        set_member("web", vec![]);
        set_member("audit", vec![]);
        // The SAME endpoint, filed twice — the shape a ledger holds when one site
        // is captured under two `form`s.
        set_consumers(
            "api",
            vec![
                broker_publish("orders", "local emitOrder"),
                broker_publish("orders", "local emitOrder"),
            ],
        );
        for member in ["web", "audit"] {
            set_consumers(
                member,
                vec![broker_subscribe("orders", &format!("local on_{member}"))],
            );
        }

        let cov = cross_service_coverage(&registry(&["api", "web", "audit"]));

        assert_eq!(
            cov.references.len(),
            1,
            "one endpoint is one row however many times the ledger holds it: {:?}",
            cov.references
        );
        assert_eq!(cov.by_intake.invocation.bound, 1, "…and one egress site");
        assert_eq!(
            cov.resolved_cross_service_edges, 2,
            "…binding two cross-member subscribers: two edges, not four"
        );
        assert_eq!(cov.egress_resolution_measured, 1);
    }

    /// **A duplicated SUBSCRIBE endpoint still indexes one provider**, so the
    /// provider arm's own collapse is unchanged by the consumer arm gaining one.
    ///
    /// The regression this pins is the role key: collapsing on `(key, member,
    /// symbol)` without the role would make a publish and a subscribe on the same
    /// topic from the same symbol collide, and the second would be dropped.
    #[test]
    fn the_collapse_is_per_role_so_a_publish_and_a_subscribe_never_collide() {
        reset();
        set_member("api", vec![]);
        set_member("web", vec![]);
        // One symbol that BOTH publishes and subscribes on the same topic — the
        // shape a role-blind dedup key would silently collapse to one row.
        set_consumers(
            "api",
            vec![
                broker_publish("orders", "local relay"),
                broker_subscribe("orders", "local relay"),
            ],
        );
        set_consumers("web", vec![broker_subscribe("orders", "local on_web")]);

        let cov = cross_service_coverage(&registry(&["api", "web"]));

        assert_eq!(
            cov.by_intake.invocation.bound, 1,
            "the publish is a consumer row and binds: {:?}",
            cov.references
        );
        assert_eq!(
            cov.resolved_cross_service_edges, 1,
            "…to the one CROSS-member subscriber; api's own subscribe is intra-repo"
        );
    }

    // ── S-376 / CR-120: the headline is a resolved-edge count ────────────────

    /// **The retired `bound_ratio` is gone from the wire and lives only as a
    /// deserialization alias** ([CR-120] AC1, §7).
    ///
    /// Both halves, because either alone leaves the retirement half-done:
    ///
    /// 1. **Nothing emits the old keys.** An external reader of `--json` gets a
    ///    missing field and fails loudly, which is the whole point of retiring a
    ///    figure rather than quietly re-pointing it — a silently-renamed
    ///    `bound_ratio` would go on being read as coupling.
    /// 2. **A stored pre-change payload still reads back**, through
    ///    [`SpecConformanceReading`]'s alias, and yields the *same three values*
    ///    the post-change payload does. Asserted against a real serialized
    ///    [`CrossServiceCoverage`] rather than a hand-written fixture, so the
    ///    compatibility type cannot drift away from the read-model it reads.
    ///
    /// The pre-change vintage is built by rewinding the live payload's key names,
    /// which is exactly the transformation the reference workspace's 2026-09-08
    /// baseline is on the other side of.
    ///
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    #[test]
    fn the_deprecated_bound_ratio_alias_reads_both_vintages() {
        let cov = every_bucket_in_both_populations();
        let post = serde_json::to_value(&cov).unwrap();

        // (1) Retired on the emit side, under every one of its three spellings.
        for retired in ["bound_ratio", "bound_ratio_measured", "bound_ratio_summary"] {
            assert!(
                post.get(retired).is_none(),
                "`{retired}` must be absent from the emitted payload — an external \
                 reader is meant to fail loudly, not read a renamed figure: {post}"
            );
        }
        assert!(
            post.get("spec_conformance_ratio").is_some()
                && post.get("spec_conformance_measured").is_some()
                && post.get("spec_conformance_summary").is_some(),
            "and the successors ARE emitted, or this test would pass over a payload \
             that simply lost the figures: {post}"
        );

        // The pre-CR-120 vintage of the very same payload.
        let mut pre = post.clone();
        {
            let obj = pre.as_object_mut().unwrap();
            for (now, retired) in [
                ("spec_conformance_ratio", "bound_ratio"),
                ("spec_conformance_measured", "bound_ratio_measured"),
                ("spec_conformance_summary", "bound_ratio_summary"),
            ] {
                if let Some(v) = obj.remove(now) {
                    obj.insert(retired.to_string(), v);
                }
            }
        }

        // (2) Both vintages read through one type, to the same three values.
        let from_post: SpecConformanceReading = serde_json::from_value(post).unwrap();
        let from_pre: SpecConformanceReading =
            serde_json::from_value(pre).expect("the deprecated alias reads a pre-CR-120 payload");
        assert_eq!(
            from_pre, from_post,
            "the alias must yield the same reading as the current spelling — that \
             equality is what lets a pre-change baseline be compared with a \
             post-change one in a single operation"
        );
        // …and the reading really is the read-model's, not a coincidence of two
        // defaults: pinned against the struct fields themselves.
        assert_eq!(from_post.spec_conformance_ratio, cov.spec_conformance_ratio);
        assert_eq!(from_post.spec_conformance_measured, cov.spec_conformance_measured);
        assert_eq!(from_post.spec_conformance_summary, cov.spec_conformance_summary);
    }

    /// **An absent ratio survives the alias too** — the [CR-100] guarantee, which
    /// is the case a `Option<f64>` compatibility field is most likely to get wrong.
    ///
    /// A zero denominator omits the key in *both* vintages, so a reader without
    /// `#[serde(default)]` would reject exactly the payloads that guarantee
    /// produces. Asserted rather than left to the attribute, because the failure is
    /// a parse error on the one shape [CR-100] exists to make representable.
    ///
    /// [CR-100]: ../../../docs/requests/CR-100-workspace-resource-budget.md
    #[test]
    fn the_alias_reads_an_absent_ratio_as_absent_rather_than_failing() {
        reset();
        set_member("api", vec![op("GET /orphans/{id}", "local op_orphan")]);
        set_member("web", vec![]);
        let cov = cross_service_coverage(&registry(&["api", "web"]));
        assert_eq!(cov.spec_conformance_ratio, None, "the fixture must be degenerate");

        let post = serde_json::to_value(&cov).unwrap();
        let pre = serde_json::json!({
            "bound_ratio_measured": post["spec_conformance_measured"],
            "bound_ratio_summary": post["spec_conformance_summary"],
        });
        for (label, value) in [("post-change", post), ("pre-change", pre)] {
            let reading: SpecConformanceReading = serde_json::from_value(value)
                .unwrap_or_else(|e| panic!("{label} payload with an absent ratio must parse: {e}"));
            assert_eq!(reading.spec_conformance_ratio, None, "{label}");
            assert_eq!(reading.spec_conformance_measured, 0, "{label}");
            assert_eq!(
                reading.spec_conformance_summary,
                "0 of 0 measured; 1 excluded as no-provider-in-workspace",
                "{label}: and the summary's wording is unchanged across the rename, \
                 which is what makes two vintages comparable"
            );
        }
    }

    /// **The headline counts EDGES, and the rate counts SITES** ([CR-120],
    /// [FR-WS-10]).
    ///
    /// The one shape where the two differ, made to differ: a broker publish that
    /// binds **three** cross-member subscribers is *one* resolved egress site and
    /// *three* cross-service edges — one per subscriber, which is exactly what the
    /// bridge emits and what `xservice route-providers` reports.
    ///
    /// Written because publishing a count of sites under the word "edges" would be
    /// the same name/meaning mismatch [CR-120] exists to remove. Without this
    /// fixture both figures read `1` and either implementation would pass.
    ///
    /// The same-member subscriber is in the fixture deliberately: the bridge skips
    /// it (an intra-repo fan-out is the per-repo graph's own fact), so counting the
    /// row's bound *set* rather than the topic's subscriber count is what keeps
    /// this figure equal to the bridge's edge count rather than above it.
    ///
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    #[test]
    fn one_fan_out_publish_is_one_egress_site_and_several_resolved_edges() {
        reset();
        set_member("api", vec![]);
        set_member("web", vec![]);
        set_member("audit", vec![]);
        set_member("billing", vec![]);
        set_consumers("api", vec![broker_publish("orders", "local emitOrder")]);
        for member in ["web", "audit", "billing"] {
            set_consumers(
                member,
                vec![broker_subscribe("orders", &format!("local on_{member}"))],
            );
        }
        // A same-member subscriber: the bridge emits no edge for it, so it must not
        // reach the count either.
        set_consumers(
            "api",
            vec![
                broker_publish("orders", "local emitOrder"),
                broker_subscribe("orders", "local onOrderLocally"),
            ],
        );

        let cov = cross_service_coverage(&registry(&["api", "web", "audit", "billing"]));

        assert_eq!(cov.by_intake.invocation.bound, 1, "one publish, one egress site");
        assert_eq!(
            cov.resolved_cross_service_edges, 3,
            "…and three cross-member subscribers, so three edges: {:?}",
            cov.references
        );
        assert_eq!(
            cov.egress_resolution,
            Some(1.0),
            "the rate is over SITES: the one captured egress site resolved"
        );
        assert_eq!(cov.egress_resolution_measured, 1);
        assert_eq!(
            cov.resolved_edges_summary,
            "3 resolved cross-service edges; egress resolution 1.000 (1 of 1 egress sites resolved)",
            "and the composed line states both, so neither can be read as the other"
        );

        // The figure is reconcilable from the rows, which is what makes it
        // checkable rather than merely reported ([CR-120]'s whole subject).
        let from_rows: u64 = cov
            .references
            .iter()
            .filter(|r| {
                r.intake == BridgeIntake::Invocation && matches!(r.state, CoverageState::Bound)
            })
            .map(|r| r.candidates.as_ref().map_or(1, |c| c.total))
            .sum();
        assert_eq!(from_rows, cov.resolved_cross_service_edges);
    }

    /// **`egress_resolution` inherits [CR-100]'s absent-on-zero-denominator rule,
    /// and [BR-51] states it** — a workspace that captured no egress site reports
    /// the rate absent, never `1.0`, and still discloses the denominator.
    ///
    /// This is the near-exact shape [CR-100] observed, moved onto the successor
    /// figure: a workspace whose contract surface binds perfectly and whose
    /// invocation population is empty. The old ratio reads `1.000` here — correctly,
    /// it is a spec-conformance figure — and a successor that inherited that
    /// reading would say every outbound call in the workspace resolves, over a
    /// workspace with no outbound call at all.
    ///
    /// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [CR-100]: ../../../docs/requests/CR-100-workspace-resource-budget.md
    #[test]
    fn egress_resolution_is_absent_when_no_egress_site_was_captured() {
        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_users")]);
        set_member("web", vec![route("GET /users/{id}", "local route_users")]);

        let cov = cross_service_coverage(&registry(&["api", "web"]));

        assert_eq!(
            cov.spec_conformance_ratio,
            Some(1.0),
            "the declarations do line up with the controllers — that reading is correct"
        );
        assert_eq!(
            cov.egress_resolution, None,
            "…and it says nothing about outbound calls: no egress site was captured, \
             so the rate is NO MEASUREMENT, never a perfect score (CR-100, BR-51)"
        );
        assert_eq!(cov.resolved_cross_service_edges, 0);
        assert_eq!(
            cov.egress_resolution_measured, 0,
            "the denominator rides regardless — `0` IS the finding a reader needs"
        );
        assert_eq!(
            cov.resolved_edges_summary,
            "0 resolved cross-service edges; egress resolution not measured \
             (0 of 0 egress sites)",
            "and the line says 'not measured', not '0.000' — a zero rate would claim \
             that captured calls failed to resolve"
        );

        let wire = serde_json::to_value(&cov).unwrap();
        assert!(
            wire.get("egress_resolution").is_none(),
            "absence reaches the wire as an omitted key, never as a number: {wire}"
        );
    }

    /// **[BR-51] on the payload: the resolved-edge count never travels without its
    /// rate.**
    ///
    /// The structural guard, asserted over every bucket-and-population shape this
    /// module can produce: wherever `resolved_cross_service_edges` appears, the
    /// denominator and the composed line appear with it, and the line names both
    /// figures. A surface can then render the line and be correct by construction —
    /// which is the point of composing it server-side rather than trusting four
    /// renderings to remember the pairing, the way [CR-111] had to learn for the
    /// figure this one replaces.
    ///
    /// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [CR-111]: ../../../docs/requests/CR-111-bound-ratio-carries-its-denominator.md
    #[test]
    fn the_resolved_edge_count_is_never_published_without_its_egress_rate() {
        for (label, cov) in [
            ("mixed", every_bucket_in_both_populations()),
            ("no egress captured", {
                reset();
                set_member("api", vec![op("GET /users/{id}", "local op_users")]);
                set_member("web", vec![route("GET /users/{id}", "local route_users")]);
                cross_service_coverage(&registry(&["api", "web"]))
            }),
            ("empty workspace", {
                reset();
                cross_service_coverage(&registry(&[]))
            }),
        ] {
            let wire = serde_json::to_value(&cov).unwrap();
            assert!(
                wire.get("resolved_cross_service_edges").is_some(),
                "{label}: the headline is unconditional — an absent count and a \
                 count of zero are different claims: {wire}"
            );
            for companion in ["egress_resolution_measured", "resolved_edges_summary"] {
                assert!(
                    wire.get(companion).is_some(),
                    "{label}: `{companion}` must ride beside the count (BR-51): {wire}"
                );
            }
            let line = cov.resolved_edges_summary.as_str();
            assert!(
                line.starts_with(&format!(
                    "{} resolved cross-service edges;",
                    cov.resolved_cross_service_edges
                )),
                "{label}: the line must open with the count it summarises: {line:?}"
            );
            assert!(
                line.contains("egress resolution"),
                "{label}: …and carry the rate beside it: {line:?}"
            );
            assert!(
                line.contains("egress sites"),
                "{label}: …over a stated denominator: {line:?}"
            );
            match cov.egress_resolution {
                Some(r) => assert!(
                    line.contains(&format!("egress resolution {r:.3} ")),
                    "{label}: a measured rate is stated as a figure: {line:?}"
                ),
                None => assert!(
                    line.contains("egress resolution not measured"),
                    "{label}: an absent rate is stated as absent, never as 0.000 \
                     (CR-100): {line:?}"
                ),
            }
        }
    }

    // ── advisory isolation (ADR-53 / sprint-55 risk register) ─────────────

    /// The coverage tier must never leak into the gated `scan`/`gate`/
    /// `check_rules` surfaces: their serialized output carries none of the
    /// coverage vocabulary, computing coverage first or not at all yields
    /// byte-identical gated output, and (structurally) none of those Engine
    /// methods ever import or call into this module.
    #[test]
    fn coverage_never_flows_into_scan_gate_or_check_rules() {
        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_get")]);

        // Computing the coverage tier must not be a prerequisite for, nor
        // side-effect into, the gated surfaces below.
        let _ = cross_service_coverage(&registry(&["api", "web"]));

        let tmp = tempfile::TempDir::new().expect("temp root");
        let engine = crate::Engine::start(tmp.path()).expect("engine starts");

        let scan = engine.scan(true).expect("scan runs");
        let gate = engine.gate(None, false, true).expect("gate runs");
        let check = engine.check_rules(None, true).expect("check_rules runs");

        let scan_json = serde_json::to_string(&scan).expect("scan serializes");
        let gate_json = serde_json::to_string(&gate).expect("gate serializes");
        let check_json = serde_json::to_string(&check).expect("check_rules serializes");

        for (label, json) in [("scan", &scan_json), ("gate", &gate_json), ("check_rules", &check_json)] {
            for token in [
                // The retired key, still checked: the gated surfaces must not carry
                // it under either spelling, and a leak of the OLD name would be
                // a stale copy of this module rather than a live one.
                "bound_ratio",
                "spec_conformance_ratio",
                "resolved_cross_service_edges",
                "egress_resolution",
                "no_provider_in_workspace",
                "ambiguous",
                "path-not-composed",
                "base-url-runtime",
                "topic-not-literal",
                "no-provider-in-workspace",
            ] {
                assert!(
                    !json.contains(token),
                    "coverage vocabulary {token:?} leaked into gated {label} output: {json}"
                );
            }
        }
    }

    /// Single-root regression ([FR-WS-05]): with no workspace manifest there
    /// is no `EngineRegistry` to call this module's entry point on at all —
    /// the tier is structurally inert, and the existing per-repo `scan` output
    /// is unaffected by this module's mere presence in the crate.
    #[test]
    fn inert_without_a_workspace_manifest() {
        let tmp = tempfile::TempDir::new().expect("temp root");
        let engine = crate::Engine::start(tmp.path()).expect("engine starts");

        let federation = super::super::discover(tmp.path()).expect("discovery succeeds");
        assert!(
            federation.is_none(),
            "no manifest anywhere up-tree must discover as single-root"
        );

        // With no `Federation`, no `Backing::Federated` registry is ever
        // built, so `cross_service_coverage` is never reachable on this path.
        let scan = engine.scan(true).expect("scan runs on the single-root path");
        assert!(scan.warnings.iter().all(|w| !w.contains("workspace")));
    }
}
