//! The **repo-qualified cross-service query read-models** ([FR-WS-05], [ADR-53]).
//!
//! This module is the thick-core home of the `xservice` surface the
//! [mcp-surface] and [cli-surface] adapters expose: `route-providers`,
//! `callers`, `impact`, and `search`, plus [`workspace_status`]. Every answer
//! is **repo-qualified** — each per-member value is tagged with the
//! [`Member::name`](super::Member::name) that produced it ([FR-WS-03]) — and an
//! optional `repo` filter scopes any fan-out to a single member.
//!
//! # Advisory only, never a gate input ([ADR-53])
//! These read-models are reachable only through an [`EngineRegistry`], which
//! itself exists only when a workspace manifest is present
//! ([`Backing::Federated`](super::Backing)). `scan`/`gate`/`check_rules` operate
//! on a single [`Engine`] and never construct a registry, so nothing here can
//! move a member's gated signal — the single-root path is byte-for-byte
//! unchanged ([FR-WS-05]).
//!
//! # Cross-service impact ([FR-WS-05])
//! [`xservice_impact`] fans per-member [`Engine::impact`] across the
//! [bridge](super::bridge) edges: the seed member's impact, plus — for every
//! [`BridgeEdge`] whose near endpoint is the queried `(member, symbol)` pair
//! ([BR-73]) — the far member's impact of the opposite endpoint, tagged with
//! the edge it was reached through. A member whose engine fails to start surfaces as a per-member error
//! rather than aborting the whole answer ([ADR-53]).
//!
//! [mcp-surface]: ../../../docs/specs/architecture/components/mcp-surface.md
//! [cli-surface]: ../../../docs/specs/architecture/components/cli-surface.md
//! [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
//! [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
//! [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
//! [BR-73]: ../../../docs/specs/software-spec.md#327-workspace-federation

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::Serialize;

use crate::history::security::OwaspEdition;
use crate::model::{LogosSymbol, NodeKind};
use crate::models::{AffectedResult, CallersResult, ImpactResult, SearchResult, StatusInfo, SymbolRef};
use crate::security::{Excluded, MemberScan, MemberSecurity, SeverityCounts};
use crate::Engine;

use super::bridge::{BridgeEdge, BridgeEndpoint, BridgeIntake, MemberContracts};
use crate::graph_store::BuildManifestRow;

use super::build_deps::{self, BuildDependencyHeadline, MemberBuildFacts};
use super::coverage::{cross_service_coverage, CrossServiceCoverage, ReferenceCoverage};
use super::declared_contracts::DeclaredContractRelation;
use super::external_join::BoundExternal;
use super::links::{self, LinkStatus};
use super::manifest::MemberKind;
use super::open_state::{self, DegradedRollup, MemberOpenState};
use super::registry::{AnswerScope, EngineRegistry, MemberReads, MemberScoped};
use super::residue::{AnswerReach, EgressResidue, MemberResolved};
use super::topics::{workspace_topics, MemberTopics};
use super::Exclusion;
use super::type_refs::{
    self, MemberTypeFacts, MemberTypeFactsRead, MemberTypeReferences, PairEvidence, TypeImporter, TypeNaming, TypeOwner, TypeRefForm,
    TypeReference, TypeReferenceHeadline, TypeReferenceIndex,
};
use super::warm_state::{self, MemberWarmState, WarmEvidence, WarmRollup};
use super::Member;

/// One member's outcome for a repo-qualified fan-out query ([FR-WS-03]).
///
/// The per-member value rides `result`; a member whose engine failed to start
/// carries a human-readable `error` instead — a partly-degraded workspace still
/// answers for its healthy members ([ADR-53]). Exactly one of the two is
/// populated.
///
/// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
#[derive(Debug, Serialize)]
pub struct MemberResult<T> {
    /// The owning member's name (its workspace-relative path).
    pub member: String,
    /// The per-member read-model, when the member's engine started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    /// Why this member produced no result (engine start / read failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T> MemberResult<T> {
    /// Record this row in `reads`: its member read when it carries a result,
    /// unread with its error when it does not ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    fn note_in(&self, reads: &mut MemberReads) {
        match &self.error {
            Some(error) => reads.note_unread(&self.member, error.clone()),
            None => reads.note_read(&self.member),
        }
    }

    /// Project a fan-out [`MemberScoped<Result<T>>`] onto the wire shape,
    /// flattening the start-failure `Err` into the `error` channel.
    fn from_scoped(scoped: MemberScoped<anyhow::Result<T>>) -> Self {
        match scoped.value {
            Ok(value) => Self {
                member: scoped.member,
                result: Some(value),
                error: None,
            },
            Err(err) => Self {
                member: scoped.member,
                result: None,
                error: Some(format!("{err:#}")),
            },
        }
    }
}

/// Run `f` over the members in scope, tagged repo-qualified ([FR-WS-03]).
///
/// `repo = Some(name)` scopes to that one member (constructing only its engine,
/// [NFR-PE-10]); `repo = None` fans over every member in discovery order. An
/// unknown `repo` surfaces as a single per-member error, not a panic.
///
/// One walk, so this is the whole answer: it mints its own [`AnswerScope`]
/// rather than taking one, and a caller that composes several of these must mint
/// the scope itself and walk through it ([`workspace_status`] is the one that
/// does).
fn fan<T>(
    registry: &EngineRegistry<Engine>,
    repo: Option<&str>,
    f: impl Fn(&Engine) -> T,
) -> Vec<MemberResult<T>> {
    match repo {
        Some(member) => vec![MemberResult::from_scoped(MemberScoped {
            member: member.to_string(),
            value: registry.engine_for(member).map(|engine| f(&engine)),
        })],
        None => registry
            .answer()
            .fan_out(|_, engine| f(engine))
            .into_iter()
            .map(MemberResult::from_scoped)
            .collect(),
    }
}

/// Per-member index freshness with **both** failure channels folded into
/// [`MemberResult::error`] ([FR-WS-05], [BR-44]).
///
/// [`fan`] cannot serve this: it flattens only the engine-*start* `Err`, and the
/// freshness read has a second failure mode of its own. [`Engine::status`] hides
/// that one — it degrades to a defaulted [`StatusInfo`] whose `indexed: false`
/// is indistinguishable from an honestly empty graph, so a member whose read
/// failed would be labeled `deferred` ("never attempted") instead of `degraded`
/// ([BR-44]). Fanning [`Engine::try_status`] and flattening both `Result`s keeps
/// `MemberResult`'s exactly-one-channel contract while making the second failure
/// visible to [`MemberStatus`].
///
/// # The same walk reads each member's build-manifest and declared-type facts
/// The `build_dependency` headline ([FR-WS-33]) and the `type_reference`
/// section ([FR-WS-35]) are built from facts read **here**, on the engine this
/// walk already opened, so they cost no walk of their own and
/// `WALKS_PER_STATUS` stays at 4 ([NFR-PE-10]). A member whose facts cannot be
/// read is left out of them with a warning — the join names it unread — and
/// its freshness row is untouched. A member whose store holds no extracted
/// facts yet is returned by name, apart, so the join names it unread with that
/// reason rather than as a member with no manifests or no types.
///
/// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
/// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
fn fan_status(answer: &AnswerScope<'_, Engine>) -> (Vec<MemberResult<StatusInfo>>, FactsWalked) {
    let mut walked = FactsWalked::default();
    let freshness = answer
        .fan_out(|_, engine| (engine.try_status(), engine.build_manifests(), engine.type_facts()))
        .into_iter()
        .map(|scoped| split_status_and_facts(scoped, &mut walked))
        .collect();
    (freshness, walked)
}

/// What the freshness walk reads off one member's engine: its freshness read,
/// its build-manifest facts and its declared-type facts (`None`: not yet
/// extracted), each with its own failure channel.
type StatusAndFacts = (
    anyhow::Result<StatusInfo>,
    anyhow::Result<Option<Vec<BuildManifestRow>>>,
    anyhow::Result<Option<MemberTypeFacts>>,
);

/// The facts the freshness walk read, as [`build_deps::join`] and
/// [`type_refs::build_index`] take them: the members read, and apart from them
/// the members whose store holds no extracted facts yet.
#[derive(Debug, Default)]
struct FactsWalked {
    facts: Vec<MemberBuildFacts>,
    not_extracted: Vec<String>,
    types: Vec<MemberTypeFactsRead>,
    types_not_extracted: Vec<String>,
}

/// One member of the freshness walk: its freshness row, with its build and
/// declared-type facts each pushed onto `walked` only when they were read
/// ([FR-WS-33], [FR-WS-35]).
///
/// A facts read that failed — or an engine that never started — pushes
/// nothing, so the join names the member unread instead of counting it read
/// with no manifests; a store holding no extracted facts yet is pushed by name
/// onto `not_extracted`, so the join names it unread with that reason. The
/// freshness row is exactly what [`flatten_read`] makes of the status half
/// in every case. Split out from [`fan_status`] so all four channels are
/// assertable without a corrupt on-disk store, as [`flatten_read`] is.
///
/// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
/// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
fn split_status_and_facts(
    scoped: MemberScoped<anyhow::Result<StatusAndFacts>>,
    walked: &mut FactsWalked,
) -> MemberResult<StatusInfo> {
    let member = scoped.member;
    let value = scoped.value.map(|(status, facts, types)| {
        match types {
            Ok(read) => build_deps::sort_read(
                member.clone(),
                read,
                &mut walked.types,
                &mut walked.types_not_extracted,
            ),
            Err(err) => tracing::warn!(
                member = %member,
                "reading a workspace member's declared-type facts failed; \
                 the type-reference section reports it unread: {err:#}"
            ),
        }
        match facts {
            Ok(read) => build_deps::sort_read(
                member.clone(),
                read,
                &mut walked.facts,
                &mut walked.not_extracted,
            ),
            Err(err) => tracing::warn!(
                member = %member,
                "reading a workspace member's build-manifest facts failed; \
                 the build-dependency headline reports it unread: {err:#}"
            ),
        }
        status
    });
    flatten_read(MemberScoped { member, value })
}

/// Collapse the two nested failure channels of a fanned fallible read —
/// [`Engine::try_status`], or [`workspace_security`]'s member read — onto
/// [`MemberResult`]'s single `error` ([BR-44]).
///
/// Outer `Err`: the member's engine failed to start. Inner `Err`: it started and
/// the read itself failed. Split out from [`fan_status`] so both are assertable
/// without a corrupt on-disk store — the inner channel is the one S-323 added for
/// the freshness read, and the point of it is that it must NOT arrive as a
/// defaulted `StatusInfo`.
///
/// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
fn flatten_read<T>(scoped: MemberScoped<anyhow::Result<anyhow::Result<T>>>) -> MemberResult<T> {
    MemberResult::from_scoped(MemberScoped {
        member: scoped.member,
        value: scoped.value.and_then(|read| read),
    })
}

/// Repo-qualified cross-service full-text search ([FR-WS-05]): [`Engine::search`]
/// fanned across the members in scope.
#[derive(Debug, Serialize)]
pub struct XserviceSearch {
    /// The search text as given.
    pub query: String,
    /// The `--repo` scope, if one was applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Per-member search results, repo-qualified.
    pub members: Vec<MemberResult<SearchResult>>,
}

/// Search every member (or the `repo`-scoped one) for `query` ([FR-WS-05]).
pub fn xservice_search(
    registry: &EngineRegistry<Engine>,
    query: &str,
    kind: Option<NodeKind>,
    limit: Option<usize>,
    repo: Option<&str>,
) -> XserviceSearch {
    XserviceSearch {
        query: query.to_string(),
        scope: repo.map(str::to_string),
        members: fan(registry, repo, |engine| engine.search(query, kind, limit)),
    }
}

/// Repo-qualified cross-service callers ([FR-WS-05]): each member's intra-repo
/// [`Engine::callers`], plus the cross-service consumers that reach the symbol
/// over a [bridge](super::bridge) edge.
#[derive(Debug, Serialize)]
pub struct XserviceCallers {
    /// The symbol text as given.
    pub query: String,
    /// The `--repo` scope, if one was applied: the member whose symbol is meant
    /// ([BR-73]).
    ///
    /// [BR-73]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Set when no scope was applied and two or more members hold the queried
    /// symbol ([BR-73]): it names them and `--repo` as the way to choose one, and
    /// says the answer is grouped per member — each row names its provider
    /// member and [`resolved_by_member`](Self::resolved_by_member) carries each
    /// one's share. Absent otherwise, so a symbol one member holds serializes as
    /// it did before.
    ///
    /// [BR-73]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambiguity_warning: Option<String>,
    /// Per-member intra-repo callers, repo-qualified.
    pub members: Vec<MemberResult<CallersResult>>,
    /// Cross-service callers: bridge edges whose provider endpoint is the
    /// queried `(member, symbol)` pair — under a scope, that member's symbol
    /// alone ([BR-73]) — and whose consumer endpoint (`from`), the
    /// cross-boundary caller, lies in any member. Never fabricated (exactly-one,
    /// [NFR-RA-05]).
    ///
    /// [BR-73]: ../../../docs/specs/software-spec.md#327-workspace-federation
    pub cross_service: Vec<BridgeEdge>,
    /// The per-member breakdown of [`cross_service`](Self::cross_service) under
    /// [`ambiguity_warning`](Self::ambiguity_warning): one entry per member
    /// holding the symbol, in name order, with the rows whose provider endpoint
    /// is its own — zero for a member that holds the symbol and provides
    /// nothing. Absent exactly when the warning is.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub resolved_by_member: Vec<MemberResolved>,
    /// Cross-service callers a valid `[[links]]` entry **asserts** ([FR-WS-41],
    /// [BR-75]): each asserted edge whose provider endpoint is the queried
    /// `(member, symbol)` pair — matched exactly as
    /// [`cross_service`](Self::cross_service) is, under the same scope — with
    /// `intake: "asserted"` and the `reached` marker
    /// [`ASSERTED_MARKER`](links::ASSERTED_MARKER).
    ///
    /// **Apart from** [`cross_service`](Self::cross_service) and never merged
    /// with it: a declared link is a claim, so it is in neither the resolved
    /// count nor [`resolved_by_member`](Self::resolved_by_member), nor the
    /// residue line. Absent when no valid link names the symbol, so an answer
    /// without `[[links]]` serializes as before.
    ///
    /// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
    /// [BR-75]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub via_asserted_link: Vec<AssertedCaller>,
    /// Cross-member callers reached across a [`TypeReference`]: each importing
    /// declaration a bound reference names, with its file and line — **apart
    /// from** [`cross_service`](Self::cross_service) and never merged with it
    /// ([BR-60]); filled by [`with_type_references`](Self::with_type_references).
    /// Absent when no bound type reference names the symbol, so such an answer
    /// serializes exactly as before the overlay existed.
    ///
    /// [BR-60]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub via_type_reference: Vec<TypeReferenceCaller>,
    /// Type references reaching the symbol whose owner names no member of this
    /// workspace, so no scope can claim or exclude them ([BR-73]): reported here,
    /// under any scope, never dropped. Absent when every reference reaching the
    /// symbol is attributed.
    ///
    /// [BR-73]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub type_reference_unattributed: Vec<TypeReferenceCaller>,
    /// The members whose declared types the type-reference overlay could not
    /// read, each with its reason ([NFR-CC-04]): an importer there cannot be
    /// reached, so [`via_type_reference`](Self::via_type_reference) is stated
    /// over the members read and its absence is not "no importer". Filled by
    /// [`with_type_references`](Self::with_type_references); absent when every
    /// member was read.
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub type_reference_unread: BTreeMap<String, &'static str>,
    /// What this answer **could not reach**: the unresolved egress residue of the
    /// members in scope ([FR-WS-05], [BR-53], [CR-125]).
    ///
    /// **Absent exactly when the residue is zero**, which is the one case in
    /// which silence is honest — so an answer over a fully-resolved scope
    /// serializes byte-for-byte as it did before [CR-125]. Present otherwise,
    /// including (and especially) when [`cross_service`](Self::cross_service) is
    /// empty: an empty answer over a non-zero residue is the reading a developer
    /// acts on, and it is the whole reason this field exists.
    ///
    /// [BR-53]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [CR-125]: ../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unresolved_egress: Option<EgressResidue>,
    /// The members this answer read, and every member it needed and could not
    /// read, with the reason ([NFR-CC-04], [NFR-PE-10]): the bridge's stamp
    /// check — every member's contract surface too, when its cache missed —
    /// and the intra-repo fan-out.
    ///
    /// A tier stitched on beside them states its own coverage and is not
    /// counted here: the type-reference overlay in
    /// [`type_reference_unread`](Self::type_reference_unread), as the declared
    /// relations of `route-providers` do in their headlines' denominators.
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    pub member_reads: MemberReads,
}

/// Callers of `symbol` across the workspace, with what the answer could not
/// reach ([FR-WS-05], [BR-53]).
///
/// `inputs.residue` is the workspace's unresolved egress, assembled once by
/// [`residue`](self::residue) and merely **scoped** here — the surfaces hand it
/// in exactly as they hand in the edges, so none of them composes a figure of
/// its own and the MCP, CLI and web renderings cannot disagree ([CR-125] §4.4).
///
/// [BR-53]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [CR-125]: ../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
pub fn xservice_callers(
    registry: &EngineRegistry<Engine>,
    inputs: &ReachabilityInputs,
    symbol: &str,
    limit: Option<usize>,
    repo: Option<&str>,
) -> XserviceCallers {
    // The cross-service tier matches the queried `(member, symbol)` pair, never
    // the symbol string alone ([BR-73]): a symbol written without a repository
    // coordinate ([FR-EX-36]) answers in every member declaring that
    // repo-relative declaration, each under its own coordinate. `repo` pins the
    // provider side; the consumer (`from`) may lie in any member.
    let cross_service: Vec<BridgeEdge> =
        inputs.edges.iter().filter(|edge| is_queried(&edge.to, symbol, repo)).cloned().collect();
    // The asserted tier, by the same `(member, symbol)` match ([FR-WS-41]).
    let via_asserted_link = inputs
        .asserted()
        .filter(|edge| is_queried(&edge.to, symbol, repo))
        .map(|edge| AssertedCaller {
            reached: links::ASSERTED_MARKER,
            edge: edge.clone(),
        })
        .collect();
    let members = fan(registry, repo, |engine| engine.callers(symbol, limit));
    let mut member_reads = inputs.reads.clone();
    for member in &members {
        member.note_in(&mut member_reads);
    }
    let holding = members
        .iter()
        .filter(|m| m.result.as_ref().is_some_and(|r| holds(r.resolved.as_ref(), symbol)))
        .map(|m| m.member.as_str());
    let near: Vec<&str> = cross_service.iter().map(|edge| edge.to.member.as_str()).collect();
    let grouped = QueriedSide::group(symbol, repo, holding, &near);
    XserviceCallers {
        query: symbol.to_string(),
        scope: repo.map(str::to_string),
        // Read off the answer that was just built, never recounted from the
        // inputs: the residue's "no resolved cross-service callers" and the
        // `cross_service` list are then two renderings of one fact.
        unresolved_egress: inputs.residue.beside(
            repo,
            AnswerReach {
                resolved: cross_service.len(),
                noun: "cross-service caller",
                by_member: &grouped.by_member,
            },
        ),
        ambiguity_warning: grouped.warning,
        members,
        cross_service,
        resolved_by_member: grouped.by_member,
        via_asserted_link,
        via_type_reference: Vec::new(),
        type_reference_unattributed: Vec::new(),
        type_reference_unread: BTreeMap::new(),
        member_reads,
    }
}

/// One cross-service caller a valid `[[links]]` entry asserts ([FR-WS-41]): the
/// asserted edge, flattened — so the row carries `intake: "asserted"` where an
/// evidence row carries its own intake — beside the `reached` marker a human
/// reads it by.
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
#[derive(Debug, Serialize)]
pub struct AssertedCaller {
    /// Always [`ASSERTED_MARKER`](links::ASSERTED_MARKER).
    pub reached: &'static str,
    /// The asserted edge: the consumer endpoint (`from`) is the caller.
    #[serde(flatten)]
    pub edge: BridgeEdge,
}

/// One far-side impact reached across an asserted edge ([FR-WS-41]): the
/// stitched impact — its `via` edge carrying `intake: "asserted"` — beside the
/// `reached` marker.
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
#[derive(Debug, Serialize)]
pub struct AssertedImpact {
    /// Always [`ASSERTED_MARKER`](links::ASSERTED_MARKER).
    pub reached: &'static str,
    /// The far member's impact, stitched as an evidence edge's is.
    #[serde(flatten)]
    pub impact: CrossServiceImpact,
}

/// Whether `endpoint` is the queried side of a bridge edge: the queried symbol
/// in the scoped member, or in any member when unscoped ([BR-73]). The one
/// match both reachability queries apply, as
/// [`xservice_route_providers`] scopes its provider endpoints.
///
/// [BR-73]: ../../../docs/specs/software-spec.md#327-workspace-federation
fn is_queried(endpoint: &BridgeEndpoint, symbol: &str, repo: Option<&str>) -> bool {
    answers_to(&endpoint.symbol, symbol) && repo.is_none_or(|member| endpoint.member == member)
}

/// Whether a member's own fan-out row resolved to exactly the queried symbol —
/// the member holds it. A bare name resolving to some other node does not
/// count: [BR-73] is about one symbol string held by several members.
///
/// [BR-73]: ../../../docs/specs/software-spec.md#327-workspace-federation
fn holds(resolved: Option<&SymbolRef>, symbol: &str) -> bool {
    resolved.is_some_and(|node| held_answers_to(&node.symbol, symbol))
}

/// [`answers_to`] over a held symbol still in its stored string form — a
/// resolved row's, a type reference owner's: equal, or parsed and matched.
fn held_answers_to(held: &str, symbol: &str) -> bool {
    held == symbol || LogosSymbol::parse(held).is_ok_and(|held| answers_to(&held, symbol))
}

/// Whether `held`, a symbol a member holds, is the queried `symbol`: equal, or
/// — for a symbol written without a repository coordinate ([FR-EX-36]) — equal
/// once `held`'s coordinate is removed. Each member's own fan-out row upgrades
/// such a query to that member's coordinate, so the cross-service tier must
/// match the same per-member upgrade, or a member would answer the symbol while
/// its bridge edges and its [BR-73] share went missing.
///
/// [FR-EX-36]: ../../../docs/specs/requirements/FR-EX-36.md
/// [BR-73]: ../../../docs/specs/software-spec.md#327-workspace-federation
fn answers_to(held: &LogosSymbol, symbol: &str) -> bool {
    held.as_str() == symbol
        || (held.coordinate().is_some()
            && held.descriptor_tail().is_some_and(|tail| symbol.ends_with(tail))
            && held.with_coordinate(None).is_ok_and(|bare| bare.as_str() == symbol))
}

/// How an unscoped cross-service answer groups a symbol that two or more
/// members hold ([BR-73]): each member's share of the resolved count and the
/// notice naming them. Both empty under a scope, and when one member or none
/// holds the symbol — the answer then serializes as it did before.
///
/// [BR-73]: ../../../docs/specs/software-spec.md#327-workspace-federation
struct QueriedSide {
    by_member: Vec<MemberResolved>,
    warning: Option<String>,
}

impl QueriedSide {
    /// Group over the members `holding` the symbol — each one whose own store
    /// resolves it, read off the fan-out the answer already made — and the
    /// queried endpoint's member of every resolved row, `near`, which also
    /// holds it.
    fn group<'a>(symbol: &str, repo: Option<&str>, holding: impl Iterator<Item = &'a str>, near: &[&'a str]) -> Self {
        let mut shares: BTreeMap<&str, usize> = BTreeMap::new();
        if repo.is_none() {
            shares.extend(holding.map(|member| (member, 0)));
            for member in near {
                *shares.entry(member).or_default() += 1;
            }
        }
        if shares.len() < 2 {
            return Self { by_member: Vec::new(), warning: None };
        }
        let names: Vec<&str> = shares.keys().copied().collect();
        let warning = format!(
            "{symbol:?} is held by {} members ({}); the answer is grouped per member, never pooled — \
             pass --repo <member> (`repo` over MCP) to choose one",
            names.len(),
            names.join(", ")
        );
        Self {
            by_member: shares
                .into_iter()
                .map(|(member, resolved)| MemberResolved { member: member.to_string(), resolved })
                .collect(),
            warning: Some(warning),
        }
    }
}

impl XserviceCallers {
    /// Stitch the type-reference tier on ([FR-WS-35], [BR-60]): every bound
    /// [`TypeReference`] reaching the queried type, its importer the caller.
    ///
    /// At class grain the importer **is** the caller — a reference binds the
    /// import of a type, not a call of one of its methods — as the bridge
    /// tier's consumer endpoint is, so no engine is opened. See
    /// [`references_reaching`] for what the query matches and how the answer's
    /// scope narrows it; a reference no member owns lands in
    /// `type_reference_unattributed`. A member the overlay could not read is
    /// named in `type_reference_unread`. Nothing else in the answer moves:
    /// `cross_service` and the residue's resolved count are the bridge's alone.
    ///
    /// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
    /// [BR-60]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[must_use]
    pub fn with_type_references(mut self, registry: &EngineRegistry<Engine>, index: &TypeReferenceIndex) -> Self {
        let reaching =
            references_reaching(&registry.federation().members, &index.references, &self.query, self.scope.as_deref());
        let caller = |reference: &TypeReference| TypeReferenceCaller {
            reached: VIA_TYPE_REFERENCE,
            via: reference.clone(),
        };
        self.via_type_reference = reaching.attributed.into_iter().map(caller).collect();
        self.type_reference_unattributed = reaching.unattributed.into_iter().map(caller).collect();
        self.type_reference_unread = index.headline.members.unread_reasons.clone();
        self
    }
}

/// One far-side impact reached by stitching across a [`BridgeEdge`] ([FR-WS-05]).
#[derive(Debug, Serialize)]
pub struct CrossServiceImpact {
    /// The bridge edge the far member was reached through.
    pub via: BridgeEdge,
    /// The far member whose impact this is.
    pub member: String,
    /// The far endpoint's transitive impact within its own member.
    pub impact: ImpactResult,
}

/// Repo-qualified cross-service impact ([FR-WS-05]): the seed member(s)' impact
/// plus each far-side impact stitched across a bridge edge.
#[derive(Debug, Serialize)]
pub struct XserviceImpact {
    /// The symbol text as given.
    pub query: String,
    /// The `--repo` scope, if one was applied: the member whose symbol is meant
    /// ([BR-73]).
    ///
    /// [BR-73]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// As [`XserviceCallers::ambiguity_warning`]: set when no scope was applied
    /// and two or more members hold the queried symbol.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambiguity_warning: Option<String>,
    /// The seed impact, per member in scope, repo-qualified.
    pub seed: Vec<MemberResult<ImpactResult>>,
    /// Far-side impacts reached by fanning across the bridge edges whose near
    /// endpoint is the queried `(member, symbol)` pair — under a scope, that
    /// member's symbol alone ([BR-73]); the far side lies in any member.
    ///
    /// [BR-73]: ../../../docs/specs/software-spec.md#327-workspace-federation
    pub cross_service: Vec<CrossServiceImpact>,
    /// As [`XserviceCallers::resolved_by_member`]: each member's share of
    /// [`cross_service`](Self::cross_service), counted on the near endpoint.
    /// Absent exactly when [`ambiguity_warning`](Self::ambiguity_warning) is.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub resolved_by_member: Vec<MemberResolved>,
    /// Far-side impacts reached across an edge a valid `[[links]]` entry
    /// **asserts** ([FR-WS-41], [BR-75]) — stitched exactly as
    /// [`cross_service`](Self::cross_service) is, from each end that is the
    /// queried pair, each with the `reached` marker. Apart from it, and counted
    /// in no figure; see [`XserviceCallers::via_asserted_link`]. Absent when no
    /// valid link names the symbol.
    ///
    /// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
    /// [BR-75]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub via_asserted_link: Vec<AssertedImpact>,
    /// One entry per bound [`TypeReference`] reaching the symbol — per import
    /// row, so a file importing the type and a static member of it is reached
    /// twice — each with what depends on the importing file in its member —
    /// **apart from**
    /// [`cross_service`](Self::cross_service) and never merged with it
    /// ([BR-60]); filled by [`with_type_references`](Self::with_type_references).
    /// Absent when no bound type reference names the symbol, so such an answer
    /// serializes exactly as before the overlay existed.
    ///
    /// [BR-60]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub via_type_reference: Vec<TypeReferenceImpact>,
    /// As [`XserviceCallers::type_reference_unattributed`], each entry with its
    /// importer's reach as in [`via_type_reference`](Self::via_type_reference).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub type_reference_unattributed: Vec<TypeReferenceImpact>,
    /// The members whose declared types the type-reference overlay could not
    /// read, each with its reason ([NFR-CC-04]): an importer there cannot be
    /// reached, so [`via_type_reference`](Self::via_type_reference) is stated
    /// over the members read and its absence is not "no importer". Filled by
    /// [`with_type_references`](Self::with_type_references); absent when every
    /// member was read.
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub type_reference_unread: BTreeMap<String, &'static str>,
    /// What this answer **could not reach**: the unresolved egress residue of the
    /// members in scope ([FR-WS-05], [BR-53], [CR-125]).
    ///
    /// Absent exactly when that residue is zero, so a fully-resolved scope
    /// renders as it did before [CR-125]; see
    /// [`XserviceCallers::unresolved_egress`].
    ///
    /// [BR-53]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [CR-125]: ../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unresolved_egress: Option<EgressResidue>,
    /// The members this answer read — the bridge's, the seed fan-out's and
    /// each far member's — and every member it needed and could not read, with
    /// the reason; see [`XserviceCallers::member_reads`]. A far member that
    /// will not start is skipped in [`cross_service`](Self::cross_service), as
    /// it always was, and named here.
    pub member_reads: MemberReads,
}

/// Transitive impact of `symbol`, stitched across bridge edges ([FR-WS-05]).
///
/// The seed is `symbol`'s impact in each member in scope; the cross-service tier
/// follows every [`BridgeEdge`] whose near endpoint is the queried
/// `(member, symbol)` pair ([BR-73]) — `repo` pins the member — to the opposite
/// endpoint's member, in any member, and computes *its* impact — literally
/// fanning per-member impact across the bridge. A far member whose engine fails to start is
/// skipped (degraded, not fatal, [ADR-53]) and named unread in
/// [`member_reads`](XserviceImpact::member_reads) ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [BR-73]: ../../../docs/specs/software-spec.md#327-workspace-federation
pub fn xservice_impact(
    registry: &EngineRegistry<Engine>,
    inputs: &ReachabilityInputs,
    symbol: &str,
    depth: Option<usize>,
    repo: Option<&str>,
) -> XserviceImpact {
    let mut member_reads = inputs.reads.clone();
    let queried = Queried { symbol, repo, depth };
    let stitched = queried.stitch(registry, inputs.edges.iter(), &mut member_reads);
    // The near member of each stitched row, for the per-member breakdown.
    let near: Vec<&str> = stitched.iter().map(|(here, _)| *here).collect();
    let cross_service: Vec<CrossServiceImpact> = stitched.into_iter().map(|(_, impact)| impact).collect();
    // The asserted tier, stitched by the same code ([FR-WS-41]) and counted in
    // no share: a claim is not a resolved row.
    let via_asserted_link = queried
        .stitch(registry, inputs.asserted(), &mut member_reads)
        .into_iter()
        .map(|(_, impact)| AssertedImpact {
            reached: links::ASSERTED_MARKER,
            impact,
        })
        .collect();
    let seed = fan(registry, repo, |engine| engine.impact(symbol, depth));
    for member in &seed {
        member.note_in(&mut member_reads);
    }
    let holding = seed
        .iter()
        .filter(|m| m.result.as_ref().is_some_and(|r| holds(r.resolved.as_ref(), symbol)))
        .map(|m| m.member.as_str());
    let grouped = QueriedSide::group(symbol, repo, holding, &near);

    XserviceImpact {
        query: symbol.to_string(),
        scope: repo.map(str::to_string),
        unresolved_egress: inputs.residue.beside(
            repo,
            AnswerReach {
                resolved: cross_service.len(),
                noun: "cross-service impact",
                by_member: &grouped.by_member,
            },
        ),
        ambiguity_warning: grouped.warning,
        seed,
        cross_service,
        resolved_by_member: grouped.by_member,
        via_asserted_link,
        via_type_reference: Vec::new(),
        type_reference_unattributed: Vec::new(),
        type_reference_unread: BTreeMap::new(),
        member_reads,
    }
}

/// The queried side of an `xservice impact` answer, for stitching edges across.
struct Queried<'q> {
    symbol: &'q str,
    repo: Option<&'q str>,
    depth: Option<usize>,
}

impl Queried<'_> {
    /// One far-side impact per end of each of `edges` that is the queried
    /// `(member, symbol)` pair ([BR-73]), with that end's member — the one
    /// stitch both the evidence and the asserted tier take.
    ///
    /// Each end that is the queried pair is a near side of its own: an edge
    /// between two members holding the symbol — a fork calling its original —
    /// is stitched from both, so each holder's share is its own scoped answer.
    /// Under a scope at most one end is the scoped member's. A far member that
    /// fails to start is skipped, not fatal ([ADR-53]), and named unread in
    /// `member_reads` ([NFR-CC-04]).
    ///
    /// [BR-73]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    fn stitch<'e>(
        &self,
        registry: &EngineRegistry<Engine>,
        edges: impl Iterator<Item = &'e BridgeEdge>,
        member_reads: &mut MemberReads,
    ) -> Vec<(&'e str, CrossServiceImpact)> {
        edges
            .flat_map(|edge| {
                [(&edge.from, &edge.to), (&edge.to, &edge.from)]
                    .into_iter()
                    .filter(|(here, _)| is_queried(here, self.symbol, self.repo))
                    .map(move |(here, far)| (edge, here, far))
            })
            .filter_map(|(edge, here, far)| {
                let engine = match registry.engine_for(&far.member) {
                    Ok(engine) => engine,
                    Err(err) => {
                        member_reads.note_unread(&far.member, format!("{err:#}"));
                        return None;
                    }
                };
                member_reads.note_read(&far.member);
                Some((
                    here.member.as_str(),
                    CrossServiceImpact {
                        via: edge.clone(),
                        member: far.member.clone(),
                        impact: engine.impact(far.symbol.as_str(), self.depth),
                    },
                ))
            })
            .collect()
    }
}

impl XserviceImpact {
    /// Stitch the type-reference tier on ([FR-WS-35], [BR-60]): every bound
    /// [`TypeReference`] reaching the queried type, with the importing file's
    /// reach in its member — the files depending on it, directly or
    /// transitively ([`Engine::affected`]), within `depth` hops (the surfaces
    /// pass the `depth` they passed [`xservice_impact`]; its default when
    /// `None`), so the bound reads the same in every tier of the answer.
    ///
    /// The reach is **file-grain**, not the importing declaration's
    /// [`Engine::impact`]: a Java/Kotlin import row is held by the importing
    /// file's module node, whose symbol impact is empty however much depends on
    /// the file's classes, while the file closure follows the member's own
    /// calls, imports and references from it. A reference binds the import of a
    /// type, not a call of one of its methods, so every importer of the type is
    /// reached ([CR-152] CRA-04). See [`references_reaching`] for what the
    /// query matches and how the answer's scope narrows it; a reference no
    /// member owns lands in `type_reference_unattributed`, reached alike. A
    /// member the overlay could not read is named in
    /// `type_reference_unread`; an importing member read when the overlay was
    /// built whose engine will not start now yields an entry carrying its
    /// error, never an abort ([ADR-53]). Nothing else in the answer moves.
    ///
    /// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
    /// [BR-60]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    /// [CR-152]: ../../../docs/requests/CR-152-cross-member-type-references-overlay.md
    #[must_use]
    pub fn with_type_references(
        mut self,
        registry: &EngineRegistry<Engine>,
        index: &TypeReferenceIndex,
        depth: Option<usize>,
    ) -> Self {
        let hops = depth.unwrap_or(crate::navigate::DEFAULT_IMPACT_DEPTH);
        let within = |mut affected: AffectedResult| {
            affected.affected.retain(|file| usize::try_from(file.distance).is_ok_and(|d| d <= hops));
            affected
        };
        // One open per importing member per answer: a member that will not
        // start is attempted — and logged, and counted — once, and its error
        // replayed on every entry reaching it, as an `AnswerScope` replays a
        // failed open within one walk ([ADR-53]).
        let mut engines: BTreeMap<String, Result<Arc<Engine>, String>> = BTreeMap::new();
        // One closure per importing file: several rows of one file (an import
        // and a static-member import of one type) share it.
        let mut closures: BTreeMap<(String, String), AffectedResult> = BTreeMap::new();
        let reaching =
            references_reaching(&registry.federation().members, &index.references, &self.query, self.scope.as_deref());
        let mut reach = |reference: &TypeReference| {
            let importer = &reference.importer;
            let engine = engines
                .entry(importer.member.clone())
                .or_insert_with(|| registry.engine_for(&importer.member).map_err(|err| format!("{err:#}")));
            TypeReferenceImpact {
                reached: VIA_TYPE_REFERENCE,
                via: reference.clone(),
                reach: MemberResult::from_scoped(MemberScoped {
                    member: importer.member.clone(),
                    value: engine
                        .as_ref()
                        .map(|engine| {
                            closures
                                .entry((importer.member.clone(), importer.file.clone()))
                                .or_insert_with(|| {
                                    within(engine.affected(std::slice::from_ref(&importer.file), false))
                                })
                                .clone()
                        })
                        .map_err(|err| anyhow::anyhow!("{err}")),
                }),
            }
        };
        self.via_type_reference = reaching.attributed.into_iter().map(&mut reach).collect();
        self.type_reference_unattributed = reaching.unattributed.into_iter().map(&mut reach).collect();
        self.type_reference_unread = index.headline.members.unread_reasons.clone();
        self
    }
}

/// The tag every entry of a `via_type_reference` section carries: the entry
/// was reached through an advisory type reference, never a bridge edge
/// ([BR-60]).
///
/// [BR-60]: ../../../docs/specs/software-spec.md#327-workspace-federation
pub const VIA_TYPE_REFERENCE: &str = "via type reference";

/// One cross-member caller reached across a [`TypeReference`] ([FR-WS-35]):
/// the reference, whose importer — member, file, line and declaration — is
/// the caller.
///
/// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
#[derive(Debug, Serialize)]
pub struct TypeReferenceCaller {
    /// Always [`VIA_TYPE_REFERENCE`].
    pub reached: &'static str,
    /// The bound reference: the type, the importer's file and line, the owner
    /// and the pair evidence.
    pub via: TypeReference,
}

/// One bound [`TypeReference`] reached across ([FR-WS-35]): the reference,
/// and the importing member's affected-file closure of the importing file — or, when that member's engine will not start, its error
/// ([ADR-53]).
///
/// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
#[derive(Debug, Serialize)]
pub struct TypeReferenceImpact {
    /// Always [`VIA_TYPE_REFERENCE`].
    pub reached: &'static str,
    /// The bound reference: the type, the importer's file and line, the owner
    /// and the pair evidence.
    pub via: TypeReference,
    /// The importing member, and its affected-file closure of the importing
    /// file (`changed` is that file) or its error.
    #[serde(flatten)]
    pub reach: MemberResult<AffectedResult>,
}

/// The bound [`TypeReference`]s reaching `symbol` ([FR-WS-35]), split by
/// whether their owner is a member of `roster`.
struct ReachingReferences<'a> {
    /// Owned by a roster member — the scoped one under a scope — grouped by
    /// owner member in roster order.
    attributed: Vec<&'a TypeReference>,
    /// Owned by no roster member: no scope can claim or exclude them, so they
    /// are reported apart, under any scope, never dropped ([BR-73]).
    ///
    /// [BR-73]: ../../../docs/specs/software-spec.md#327-workspace-federation
    unattributed: Vec<&'a TypeReference>,
}

/// Every bound [`TypeReference`] reaching `symbol` ([FR-WS-35]), keyed on the
/// owner's `(member, symbol)` pair ([BR-73]).
///
/// `symbol` reaches a reference of `references` — the index's bound ones —
/// when it is the owner's node — the node path, the owner pair
/// [`references_to`](TypeReferenceIndex::references_to) answers, or, for a
/// symbol written without a repository coordinate, the owner's node under its
/// own coordinate ([FR-EX-36]), as [`holds`] matches a member's row — or the
/// type's dotted name — the path [`importers`](TypeReferenceIndex::importers)
/// answers, matched here in the same pass, which is how an
/// Avro-declared type (no node in any store) is reached, and which a source
/// type answers as well. The two never overlap: a `LogosSymbol` carries spaces
/// and a dotted name none. A dotted name, or a symbol written without a
/// coordinate, can reach owners in two members at once, so `repo` keeps the
/// references whose **owner member** is the scope, on both paths, while the
/// importer may lie in any member. Unscoped, every owner
/// is kept, grouped by owner member in roster order. Every owner the index
/// builds names the member whose declarations it read, Avro included, so the
/// unattributed group is empty unless the index and `roster` disagree.
///
/// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
/// [FR-EX-36]: ../../../docs/specs/requirements/FR-EX-36.md
/// [BR-73]: ../../../docs/specs/software-spec.md#327-workspace-federation
fn references_reaching<'a>(
    roster: &[Member],
    references: &'a [TypeReference],
    symbol: &str,
    repo: Option<&str>,
) -> ReachingReferences<'a> {
    // Only a symbol written without a coordinate can answer to another
    // spelling: any other query is matched as written, parsing no owner.
    let old_form = LogosSymbol::parse(symbol).is_ok_and(|query| query.is_coordinate_less());
    let owned_by = |held: &str| held == symbol || (old_form && held_answers_to(held, symbol));
    let reaching =
        references.iter().filter(|r| r.owner.symbol.as_deref().is_some_and(owned_by) || r.fqn == symbol);
    let mut attributed: Vec<(usize, &TypeReference)> = Vec::new();
    let mut unattributed = Vec::new();
    for reference in reaching {
        let owner = reference.owner.member.as_str();
        match roster.iter().position(|member| member.name == owner) {
            Some(at) if repo.is_none_or(|scope| scope == owner) => attributed.push((at, reference)),
            Some(_) => {}
            None => unattributed.push(reference),
        }
    }
    // Stable: within one owner the index's own order is kept.
    attributed.sort_by_key(|(at, _)| *at);
    ReachingReferences {
        attributed: attributed.into_iter().map(|(_, reference)| reference).collect(),
        unattributed,
    }
}

/// The `xservice type-refs` read-model ([FR-WS-05], [FR-WS-35]) — one per
/// surface: the CLI and the MCP twin serialize it alike.
///
/// Per **provider** member, the types other members import from it, each with
/// its importers' file and line; the overlay's headline beside them, with its
/// denominators ([BR-51]). An advisory type reference, never a coupling
/// ([BR-60]).
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
/// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [BR-60]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XserviceTypeRefs {
    /// The `--repo` scope, when one was applied: the provider member.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Set when the scope names no member the overlay was built over — so the
    /// empty [`providers`](Self::providers) is never read as "nothing imports
    /// its types" ([NFR-CC-04]). It states why: the member's unread reason, or
    /// "not in the workspace".
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_note: Option<String>,
    /// The workspace-wide headline, whatever the scope: `type_reference_pairs`
    /// beside the rows considered and the members read — the denominator the
    /// listing is read against.
    pub headline: TypeReferenceHeadline,
    /// Unscoped: every member read whose types another member imports, in
    /// roster order. Scoped: the scoped member alone, listed even when nothing
    /// imports its types.
    pub providers: Vec<ProviderTypeRefs>,
}

/// One provider member's imported types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderTypeRefs {
    /// The declaring member.
    pub member: String,
    /// Its types other members import, sorted by name.
    pub types: Vec<ImportedType>,
}

/// One type another member imports, with every importer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportedType {
    /// The type, dotted.
    pub fqn: String,
    /// Where it is declared: source file and node, or Avro schema.
    pub owner: TypeOwner,
    /// Each bound reference to it, sorted by importer.
    pub importers: Vec<TypeImport>,
}

/// One importer of an [`ImportedType`]: the importing member, file and line,
/// and how the reference bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeImport {
    /// The importing member, file, line and declaration.
    #[serde(flatten)]
    pub importer: TypeImporter,
    /// Exactly, or through its enclosing name.
    pub naming: TypeNaming,
    /// The row's form.
    pub form: TypeRefForm,
    /// What admits the pair.
    pub evidence: PairEvidence,
}

/// The `xservice type-refs` answer over `index`, scoped to one provider member
/// when `repo` is given ([FR-WS-35]).
///
/// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
#[must_use]
pub fn xservice_type_refs(index: &TypeReferenceIndex, repo: Option<&str>) -> XserviceTypeRefs {
    let read: Vec<MemberTypeReferences> = match repo {
        Some(member) => index.member(member).into_iter().collect(),
        None => index.per_member().into_iter().filter(|m| !m.imported_by.is_empty()).collect(),
    };
    let providers: Vec<ProviderTypeRefs> = read
        .into_iter()
        .map(|member| ProviderTypeRefs {
            member: member.member,
            types: imported_types(member.imported_by),
        })
        .collect();
    let scope_note = build_deps::scope_note(
        repo,
        !providers.is_empty(),
        &index.headline.members.unread_reasons,
        "type-reference overlay was built over",
    );
    XserviceTypeRefs {
        scope: repo.map(str::to_string),
        scope_note,
        headline: index.headline.clone(),
        providers,
    }
}

/// Group one provider's bound references by the type they name.
fn imported_types(references: Vec<TypeReference>) -> Vec<ImportedType> {
    let mut by_type: BTreeMap<String, ImportedType> = BTreeMap::new();
    for reference in references {
        let import = TypeImport {
            importer: reference.importer,
            naming: reference.naming,
            form: reference.form,
            evidence: reference.evidence,
        };
        by_type
            .entry(reference.fqn.clone())
            .or_insert_with(|| ImportedType {
                fqn: reference.fqn,
                owner: reference.owner,
                importers: Vec::new(),
            })
            .importers
            .push(import);
    }
    by_type.into_values().collect()
}

/// The resolved cross-service route bindings ([FR-WS-05]): the [bridge](super::bridge)
/// edges, optionally scoped to routes a single member *provides*.
///
/// # Declared relations ride beside, never inside ([BR-57], S-461)
/// [`with_declared`](Self::with_declared) adds the coverage tier's
/// `declared_contracts` ([FR-WS-31]) and `bound_external` ([ADR-68] point 3)
/// verbatim, each with its own headline and denominator. Neither is a binding:
/// no declared contract or bound external ever enters `providers`, and both
/// keys are absent when the workspace has nothing to declare, so such a
/// workspace's answer is byte-for-byte what it was before them.
///
/// [BR-57]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [FR-WS-31]: ../../../docs/specs/requirements/FR-WS-31.md
/// [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
#[derive(Debug, Serialize)]
pub struct XserviceRouteProviders {
    /// The `--repo` scope, if one was applied (routes provided by that member).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Each resolved cross-service binding: `from` (the consumer endpoint) →
    /// `to` (the provider endpoint), both repo-qualified `(member, symbol)`.
    pub providers: Vec<BridgeEdge>,
    /// The declared-contract relation, workspace-wide — declared by vendored
    /// specs, never observed calls. Absent when no member holds a vendored or
    /// `mock`-held spec document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_contracts: Option<DeclaredContractRelation>,
    /// The external join, workspace-wide — each `no-provider-in-workspace` REST
    /// row judged against the externals its own member declares. Absent when
    /// no member declares one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound_external: Option<BoundExternal>,
    /// Present only under a `repo` scope with a declared relation beside it:
    /// says the scope narrowed `providers` and not the two relations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_scope_note: Option<String>,
    /// The members the bridge read for these bindings, and every member it
    /// needed and could not read, with the reason; see
    /// [`XserviceCallers::member_reads`]. The declared relations beside them
    /// are workspace-wide and state their own denominators.
    pub member_reads: MemberReads,
}

impl XserviceRouteProviders {
    /// Carry `coverage`'s declared relations beside the bindings, verbatim.
    ///
    /// They are **not** scoped by `repo`: a declared contract is not a route a
    /// member provides, and narrowing the relation would leave its headline
    /// stated over a denominator its rows no longer match ([BR-51]). Under a
    /// scope the answer says so in `declared_scope_note`, so an unscoped
    /// relation is never read as the scoped member's.
    ///
    /// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
    pub fn with_declared(mut self, coverage: CrossServiceCoverage) -> Self {
        self.declared_contracts = coverage.declared_contracts;
        self.bound_external = coverage.bound_external;
        let declared = self.declared_contracts.is_some() || self.bound_external.is_some();
        self.declared_scope_note = self.scope.as_deref().filter(|_| declared).map(|member| {
            format!(
                "`--repo {member}` scopes `providers` to routes {member} provides; \
                 `declared_contracts` and `bound_external` are workspace-wide — a \
                 declared contract is not a provided route"
            )
        });
        self
    }
}

/// The cross-service route bindings, scoped to a provider member when `repo` is
/// given ([FR-WS-05]).
pub fn xservice_route_providers(read: &BridgeRead, repo: Option<&str>) -> XserviceRouteProviders {
    XserviceRouteProviders {
        scope: repo.map(str::to_string),
        providers: read
            .edges
            .iter()
            .filter(|edge| repo.is_none_or(|member| edge.to.member == member))
            .cloned()
            .collect(),
        declared_contracts: None,
        bound_external: None,
        declared_scope_note: None,
        member_reads: read.reads.clone(),
    }
}

/// One member's row in a [`WorkspaceStatus`]: its index freshness, its warm
/// state, and its open state, in **one** record ([FR-WS-05], [FR-WS-15],
/// [FR-WS-16]).
///
/// All three parts are flattened, so a row is the flat table a reader expects —
/// `{"member": "api", "result": {…}, "warm_state": "warm", "open_state":
/// "opened"}` — rather than a freshness list a caller has to join two state
/// lists against by name. [FR-WS-16]'s degraded reporting extended this row
/// rather than adding a competing member table, which is why `open_state` lives
/// here beside `warm_state` and not in a payload of its own.
///
/// # Two axes, not one merged label
/// `warm_state` is about **index presence** and `open_state` about **store
/// openability** (see [`super::open_state`]) — different questions, kept as
/// different keys so neither can be read as the other. A member that could not
/// be opened reads `degraded` on both, which is two independent derivations
/// agreeing, not one value duplicated.
///
/// Only `workspace status` carries these states; the `xservice` fan-outs keep
/// the plain [`MemberResult`], which is why they live here rather than on the
/// generic envelope.
///
/// # `error` is the canonical reason (Sprint 61 review, S-323 deferred #15)
/// A degraded row can carry the same failure under three keys: `error` (this
/// [`MemberResult`]'s verbatim engine diagnostic), `reason` (the *warm* axis'
/// — populated from the durable warm-outcome record when index presence itself
/// does not already settle the state; see [`super::warm_state::derive_state`]
/// and [FR-WS-17]), and `degraded_reason`/`degraded_diagnostic` (the *open*
/// axis' classified and verbatim readings). [FR-WS-16] states the row "keeps
/// its existing `error` field", so `error` is the field a consumer wanting
/// *one* fact should read; the other two are additive, axis-scoped detail, not
/// a competing source of truth. Nothing here is renamed or removed — the exact
/// key set (all three, plus their absence on a healthy row) stays pinned by
/// this module's own tests — this note only settles which one is
/// authoritative.
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
/// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
/// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
#[derive(Debug, Serialize)]
pub struct MemberStatus {
    /// The member's index freshness, or why it could not be read.
    #[serde(flatten)]
    pub status: MemberResult<StatusInfo>,
    /// The repository coordinate this member resolves to — its `[project] id`,
    /// else the identity derived from its remote — the package slot of every
    /// symbol its graph carries ([FR-WS-39], [ADR-85]).
    ///
    /// Always present, one per roster member: a member that could not be opened
    /// still has an identity, and it is what tells two members apart when one is
    /// excluded for sharing the other's (`open_state: degraded`, cause
    /// `duplicate-coordinate`). Read off the registry's own resolution, the same
    /// one that decides the exclusion, so the row and the verdict cannot
    /// disagree.
    ///
    /// [FR-WS-39]: ../../../docs/specs/requirements/FR-WS-39.md
    /// [ADR-85]: ../../../docs/specs/architecture/decisions/ADR-85.md
    pub coordinate: String,
    /// This member's warm state — index presence ([FR-WS-15]).
    ///
    /// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
    #[serde(flatten)]
    pub warm: MemberWarmState,
    /// This member's open state — whether its store was opened, and the cause
    /// when it was attempted and failed ([FR-WS-16]).
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    #[serde(flatten)]
    pub open: MemberOpenState,
}

impl MemberStatus {
    /// Build one labelled row from a member's already-read freshness and the
    /// registry's record of whether its store opened ([FR-WS-15], [FR-WS-16]).
    ///
    /// The warm state is derived by
    /// [`warm_state::derive_state`](super::warm_state::derive_state), which owns
    /// the vocabulary AND its precedence rules; the only work here is projecting
    /// [`MemberResult`]'s two channels onto that function's
    /// `Result<bool, &str>` input. Deliberately not restated — a second copy of
    /// the table would go quietly stale the first time the precedence changed.
    /// The open state is likewise taken as given, from
    /// [`EngineRegistry::open_states`](super::registry::EngineRegistry::open_states):
    /// re-deriving it from `error` here would be a second author of the
    /// eviction/laziness discrimination, and the wrong one — this row cannot
    /// tell an unopenable member from an unreadable index.
    ///
    /// No engine is constructed, opened, or touched here ([NFR-PE-10]).
    ///
    /// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    fn labelled(
        status: MemberResult<StatusInfo>,
        coordinate: &str,
        evidence: &WarmEvidence,
        open: MemberOpenState,
    ) -> Self {
        // `result` is the only channel that can yield `Ok`; absent freshness is
        // never evidence of an un-attempted member, so it reads unreadable
        // rather than silently `deferred` — including the `error`-less case,
        // which `MemberResult::from_scoped` cannot actually produce
        // ([BR-44], [NFR-CC-04]).
        let indexed = match &status.result {
            Some(info) => Ok(info.indexed),
            None => Err(status
                .error
                .as_deref()
                .unwrap_or("the member reported no index freshness")),
        };
        let warm = warm_state::derive_state(&status.member, indexed, evidence);
        Self {
            status,
            coordinate: coordinate.to_string(),
            warm,
            open,
        }
    }
}

/// The `logos workspace status` read-model ([FR-WS-05]): each member's index
/// freshness and warm state, the workspace warm roll-up, the 3-state
/// cross-service coverage summary, and the promoted topic inventory.
#[derive(Debug, Serialize)]
pub struct WorkspaceStatus {
    /// The workspace name (`[workspace] name`).
    pub workspace: String,
    /// Per-member index freshness ([`Engine::status`]) and warm state,
    /// repo-qualified.
    pub members: Vec<MemberStatus>,
    /// The workspace-wide warm roll-up over [`members`](Self::members)
    /// ([FR-WS-15]). Purely derived from the rows above — it can never disagree
    /// with them.
    ///
    /// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
    pub warm_rollup: WarmRollup,
    /// The workspace-wide **degraded** roll-up over the same
    /// [`members`](Self::members) ([FR-WS-16]).
    ///
    /// The second projection of one member table, not a second table: it names
    /// the members whose store could not be opened.
    ///
    /// # Two markers, each governing its own scope ([NFR-CC-04])
    /// [`covers_all_members`](DegradedRollup::covers_all_members) is about the
    /// **open** axis: it is `false` when a member was not opened, so the rows
    /// beside it (and the warm roll-up folded from them) describe fewer than all
    /// members. It does **not** govern [`coverage`](Self::coverage), which has
    /// its own [`covers_all_members`](CrossServiceCoverage::covers_all_members)
    /// computed from a different walk — a member can open perfectly well and
    /// still fail its *contract-surface* read, which reduces the coverage figures
    /// while leaving nothing degraded. The two can therefore legitimately
    /// disagree, and a consumer rendering any figure must read the marker that
    /// belongs to it rather than assuming one implies the other.
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub degraded_rollup: DegradedRollup,
    /// The non-gated 3-state cross-service coverage summary from [S-247]
    /// ([`cross_service_coverage`], [ADR-53]).
    ///
    /// [S-247]: ../coverage/index.html
    pub coverage: CrossServiceCoverage,
    /// Each member's promoted broker topics (S-256, [`workspace_topics`],
    /// [FR-WS-11]).
    ///
    /// Read from the per-repo topic **graph**, not from the cross-member bind, so a
    /// topic one member publishes and nobody consumes yet is still reported — the
    /// service map draws it, and its `consumers: 0` is an honest fact rather than an
    /// absence ([FR-WS-11]).
    ///
    /// [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
    pub topics: Vec<MemberTopics>,
    /// Members that **look like** documentation or mock repositories and
    /// declare no kind (of any kind, `platform` included) — a hint for a human,
    /// never a classification
    /// ([FR-WS-32], [ADR-68] point 5).
    ///
    /// **Absent** when there is none, so a workspace with no such member
    /// serializes exactly as before. Listing a member here moves nothing: its
    /// rows stay in the headline until someone declares its kind.
    ///
    /// [FR-WS-32]: ../../../docs/specs/requirements/FR-WS-32.md
    /// [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind_candidates: Option<KindCandidates>,
    /// The **build-dependency** headline — member pairs joined from build
    /// manifests, by kind, beside the references they were joined from, with
    /// declared platforms' inbound pairs apart and platform candidates listed
    /// ([FR-WS-33], [ADR-69]).
    ///
    /// **Never a runtime figure** ([BR-58]): it sits beside
    /// [`coverage`](Self::coverage), and nothing in coverage reads it. Absent
    /// only when every member was read and none holds a build manifest; a
    /// member whose facts could not be read — or are not yet extracted, as on a
    /// store upgraded across migration 22 — keeps it present, naming the member
    /// under `members.unread` with its reason in `members.unread_reasons`
    /// ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    ///
    /// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
    /// [ADR-69]: ../../../docs/specs/architecture/decisions/ADR-69.md
    /// [BR-58]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_dependency: Option<BuildDependencyHeadline>,
    /// The **cross-member type-reference** headline — member pairs bound by an
    /// import of a type exactly one other member declares, admitted only
    /// between members the build relation relates (or a build collision
    /// backs), beside every row considered and the members read ([FR-WS-35],
    /// [ADR-70]).
    ///
    /// **Advisory, never a coupling** ([BR-60]): it sits beside
    /// [`build_dependency`](Self::build_dependency) and
    /// [`coverage`](Self::coverage), and neither reads it — nor does it enter
    /// the build headline. Absent only when every member was read and none is
    /// Java/Kotlin/Avro; a member whose declared types could not be read, or
    /// are not yet extracted, keeps it present, named under `members.unread`
    /// ([NFR-CC-04]).
    ///
    /// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
    /// [ADR-70]: ../../../docs/specs/architecture/decisions/ADR-70.md
    /// [BR-60]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_reference: Option<TypeReferenceHeadline>,
    /// What the manifest's `[workspace] exclude` removed ([FR-WS-40]): the
    /// declared globs, the directories they dropped, and every explicit member
    /// they matched but did not drop (each with its warning, [NFR-CC-04]) — so
    /// an exclusion is visible rather than silent.
    ///
    /// **Absent** when the manifest declares no `exclude`, so a workspace
    /// without the key serializes exactly as before. Read off the
    /// [`Federation`](super::Federation) the registry already holds; it costs no
    /// walk.
    ///
    /// [FR-WS-40]: ../../../docs/specs/requirements/FR-WS-40.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusion: Option<Exclusion>,
    /// Every declared `[[links]]` entry and what became of it ([FR-WS-41]): a
    /// `valid` link with the one `asserted` edge it yields, an `invalid` one with
    /// its reason (`unknown-member`, `member-degraded`, `no-match`, `ambiguous`
    /// with bounded candidates, `same-member`), or a `redundant` one naming the
    /// edge it duplicates — so a declared coupling is never silently ignored.
    ///
    /// **Absent** when the manifest declares no link, so a workspace without
    /// `[[links]]` serializes exactly as before, and costs no walk. A link is a
    /// claim, never evidence ([BR-75]): nothing here reaches
    /// [`coverage`](Self::coverage) or any figure beside it.
    ///
    /// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
    /// [BR-75]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkStatus>,
}

/// The member-kind **candidate hint** on `workspace status` ([FR-WS-32]).
///
/// A candidate is an undeclared member that **holds API documents** — at least
/// one contract-surface row in [`CrossServiceCoverage::references`], i.e. an
/// OpenAPI operation its own index holds — and **no runnable source**: none of
/// the languages its index tags is a code grammar
/// ([`code_language_names`](crate::plugin::grammars::code_language_names)).
/// Both halves are read off what the status walk already gathered, so the hint
/// costs no walk of its own ([NFR-PE-10]). A member whose index is unreadable
/// is never a candidate: "no runnable source" is a fact about an index, and
/// there is none to state it about.
///
/// It classifies nothing. The logic that decides what a kind *does* reads the
/// manifest alone; this list is only what a reviewer might want to declare.
///
/// [FR-WS-32]: ../../../docs/specs/requirements/FR-WS-32.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KindCandidates {
    /// The candidate members' repo-qualified names, in roster order.
    pub members: Vec<String>,
    /// The roster size the candidates were drawn from.
    pub members_total: u64,
    /// The count, its denominator and what it is, as one line — e.g. `"2 of 84
    /// members hold API documents and no runnable source and declare no kind
    /// (a hint, never a classification: declare `[workspace.member.<name>] kind
    /// = \"documentation\" | \"mock\"` to report a member's rows apart)"`.
    pub summary: String,
}

/// The members holding **API documents**, read off the coverage summary: every
/// member with at least one contract-surface row in the headline's
/// [`references`](CrossServiceCoverage::references).
///
/// A member's operations all produce a row except those resolving to a sole
/// provider in the same member ([`super::coverage`]'s `tier`) — and that
/// provider is a route in the member's own source, which already disqualifies
/// it as a candidate. So no candidate is missed by reading the rows. Declared
/// members' rows sit in `declared_apart` instead, and a declared member is no
/// candidate either way.
///
/// Only a `contract-surface` row is a document: an `invocation` row is a call
/// site, and an `asserted` one a declared claim that holds nothing ([BR-75]).
///
/// [BR-75]: ../../../docs/specs/software-spec.md#327-workspace-federation
fn members_holding_api_documents(references: &[ReferenceCoverage]) -> BTreeSet<&str> {
    references
        .iter()
        .filter(|row| row.intake == BridgeIntake::ContractSurface)
        .map(|row| row.from.member.as_str())
        .collect()
}

/// The [`KindCandidates`] over one status walk's freshness rows, or `None` when
/// no member qualifies ([FR-WS-32]).
///
/// [FR-WS-32]: ../../../docs/specs/requirements/FR-WS-32.md
fn kind_candidates(
    freshness: &[MemberResult<StatusInfo>],
    holds_documents: &BTreeSet<&str>,
    declared: &BTreeMap<String, MemberKind>,
) -> Option<KindCandidates> {
    let code = crate::plugin::grammars::code_language_names();
    let members: Vec<String> = freshness
        .iter()
        .filter(|row| !declared.contains_key(&row.member))
        .filter(|row| holds_documents.contains(row.member.as_str()))
        .filter(|row| {
            row.result.as_ref().is_some_and(|info| {
                !info
                    .resolution_by_language
                    .iter()
                    .any(|language| language.files > 0 && code.contains(&language.language))
            })
        })
        .map(|row| row.member.clone())
        .collect();
    if members.is_empty() {
        return None;
    }
    let members_total = freshness.len() as u64;
    let summary = format!(
        "{} of {members_total} members hold API documents and no runnable source and \
         declare no kind (a hint, never a classification: declare \
         `[workspace.member.<name>] kind = \"documentation\" | \"mock\"` to report a \
         member's rows apart)",
        members.len()
    );
    Some(KindCandidates {
        members,
        members_total,
        summary,
    })
}

/// The workspace **roster** — the manifest, and nothing but the manifest
/// ([FR-WS-06], [NFR-PE-10]).
///
/// Deliberately engine-free. [`workspace_status`] fans out over every member (it
/// reads each one's index freshness and the cross-service coverage), which
/// *constructs and watches every member's engine*. That is right for the coverage
/// dashboard and wrong for the thing the web shell needs on **every page load**:
/// the member names, so it can render its selector. Serving the selector from
/// `workspace_status` would eagerly warm all N members on first paint and undo
/// [NFR-PE-10]'s warm-only-the-default policy.
///
/// This read touches no engine at all: it projects the already-discovered
/// [`Federation`](super::Federation).
#[derive(Debug, Serialize)]
pub struct WorkspaceRoster {
    /// The workspace name (`[workspace] name`).
    pub workspace: String,
    /// The default member's name, when the manifest named one that survived
    /// resolution — the member an unscoped request answers from, so the shell can
    /// open on it rather than guessing at the roster's first entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Every member's repo-qualified name, in manifest order.
    pub members: Vec<String>,
}

/// The engine-free workspace roster ([`WorkspaceRoster`], [FR-WS-06], [NFR-PE-10]).
pub fn workspace_roster<E>(registry: &EngineRegistry<E>) -> WorkspaceRoster
where
    E: super::registry::MemberEngine,
{
    let federation = registry.federation();
    WorkspaceRoster {
        workspace: federation.name.clone(),
        default: federation.default.clone(),
        members: federation.members.iter().map(|m| m.name.clone()).collect(),
    }
}

/// Per-member freshness, warm state and open state, the warm and degraded
/// roll-ups, the 3-state coverage summary, and the promoted topic inventory
/// ([FR-WS-05], [FR-WS-11], [FR-WS-15], [FR-WS-16]).
///
/// # The degraded roll-up costs no walk either
/// [`EngineRegistry::open_states`](super::registry::EngineRegistry::open_states)
/// reads the ledger the fan-outs below already wrote, and the
/// `build_dependency` headline joins facts the freshness walk read
/// ([`fan_status`]), so `WALKS_PER_STATUS` stays at 4 ([NFR-PE-10]) — see
/// `tests/workspace_connection_budget.rs`.
///
/// That figure is a workspace **without `[[links]]`**, the only shape the budget
/// test measures. A manifest that declares links adds the evidence edges a link
/// is judged against ([`links::workspace_links`]): two more all-member walks of
/// the same answer, six in all, plus two lookups per link endpoint — its target,
/// and the routes serving it — in the member it names ([FR-WS-41]).
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
///
/// # A broken member is attempted, and reported, once per answer
/// The first three statements below are **four** all-member fan-outs —
/// `coverage` reads twice (the contract surface and the invocation references),
/// which is why `WALKS_PER_STATUS` above is 4 and not 3; the `links` statement
/// after them walks only when the manifest declares a link. The first of them makes a
/// member's *first* attempt; the other three used to **re**-attempt one whose
/// engine had already failed to start, so a single unopenable member cost three
/// wasted opens and emitted the same `WARN` three times, growing as `3 × N`.
///
/// The four walks (six with `[[links]]`) share one attempt and one announcement
/// because they share one [`AnswerScope`], minted here and dropped when this function returns: the
/// freshness walk makes the real attempt and records the diagnostic on the
/// member row's `error`, while the three coverage and topic reads replay the
/// recorded failure without touching the store
/// ([`AnswerScope::fan_out`](super::registry::AnswerScope::fan_out)), and the
/// first walk that reaches the human channel is the only one to speak
/// (`AnswerScope::announce_open_failure`).
///
/// The scope's lifetime is **one answer**, not the registry's, and that is the
/// whole point of it. A `RegistryMode::Lazy` one-shot and a long-lived
/// `RegistryMode::Serve` registry therefore behave identically here: the CLI
/// command and `GET /api/v1/workspace/status` each pay one attempt and one line
/// per broken member, and the *next* call — a fresh command or the next request
/// against the same serving registry — mints a fresh scope and re-attempts, so a
/// member whose failure was transient is not reported degraded once it recovers
/// ([CR-105]).
///
/// Nothing in this payload moves: the ledger read below is unchanged, so the
/// member rows, [`degraded_rollup`](WorkspaceStatus::degraded_rollup), the
/// coverage marker and the exit code derived from them are what they were. What
/// changes is only how many times the same failure is paid for and repeated
/// ([FR-WS-16], [NFR-CC-04], [NFR-PE-10]).
///
/// [CR-105]: ../../../docs/requests/CR-105-report-a-failed-member-open-once-per-answer.md
/// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
///
/// # The warm labelling costs one file read, and no engine
/// The evidence is the durable warm-outcome sidecar at the workspace root
/// ([FR-WS-17]) — **one** `read` of one small file, whatever N is, made before
/// any fan-out. That once-per-command property is held **structurally**, by the
/// read sitting here rather than inside the per-member labelling below, and is
/// deliberately not asserted: moving it into the loop would produce
/// byte-identical output, so a test could only catch it by counting syscalls,
/// which is a heavier instrument than the invariant is worth. The labels then come entirely from it and from the freshness
/// rows the first fan-out already produced: no all-member walk, no engine
/// construction, no *store* read, and nothing opened per member, which is what
/// keeps the resident-engine ceiling of a `status` exactly what it was
/// ([NFR-PE-10], [NFR-PE-11]) — asserted at N = 72 in
/// `tests/workspace_connection_budget.rs`.
///
/// An absent or unreadable sidecar yields an empty record, which makes this
/// evidence equal to [`WarmEvidence::none`] and the whole payload identical to
/// what it was before [FR-WS-17] ([NFR-RA-02]). `warming` is absent from the
/// roll-up either way: a *finished* outcome is not a live in-flight signal, and
/// no such source exists ([NFR-CC-04]).
///
/// [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
/// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
/// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
/// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
/// [NFR-RA-02]: ../../../docs/specs/requirements/NFR-RA-02.md
pub fn workspace_status(registry: &EngineRegistry<Engine>) -> WorkspaceStatus {
    let evidence =
        WarmEvidence::none().with_outcomes(&warm_state::read_outcomes(&registry.federation().root));
    // One scope over all four walks below (six when `[[links]]` are declared) —
    // the unit "once per answer" is measured in. It is dropped with this call,
    // so the next one re-attempts.
    let answer = registry.answer();
    let (mut freshness, build_facts) = fan_status(&answer);
    split_call_residue_across_members(&mut freshness);
    let coverage = cross_service_coverage(&answer);
    let topics = workspace_topics(&answer);
    // Reads nothing when the manifest declares no link. Otherwise it computes the
    // evidence edges a link must not duplicate over this same answer — two more
    // all-member walks, plus the configuration reads of members whose call sites
    // name a key — and makes two lookups per endpoint (its target, and the routes
    // serving it), in the members the links name.
    let links = links::workspace_links(&answer);
    // The links are reported beside the headline, never inside it ([BR-75]):
    // the tally, and a pointer on each residue row a valid link covers.
    let coverage = coverage.with_links(&links);

    // Read the open-state ledger **last**, after every walk this read-model
    // makes. A member that opened for the freshness walk and then failed under
    // descriptor pressure during the coverage walk is degraded for this answer
    // as a whole, which is the claim the exit code and the coverage marker rest
    // on ([FR-WS-16]).
    let opens = registry.open_states();
    let degraded_rollup = open_state::rollup(&opens);
    let kind_candidates = kind_candidates(
        &freshness,
        &members_holding_api_documents(&coverage.references),
        &registry.federation().member_kinds,
    );
    let federation = registry.federation();
    let relation =
        build_deps::join(&federation.members, &federation.member_kinds, &build_facts.facts, &build_facts.not_extracted);
    let type_reference = type_refs::build_index(
        &federation.members,
        &build_facts.types,
        &build_facts.types_not_extracted,
        &relation,
    )
    .section();
    let build_dependency = build_dependency_section(relation);

    // `zip`, not a name-keyed join: `fan_status` and `open_states` are two
    // projections of the SAME list — both map over `federation.members` in
    // manifest order, one row per roster member — so they are aligned by
    // construction. A join would allocate a map, clone N keys, and need a
    // fallback arm for a mismatch that cannot happen; worse, that arm would
    // silently relabel a healthy member `not-attempted` if the invariant ever
    // broke. The `debug_assert` makes a future divergence a dev-build panic
    // instead of quietly wrong data.
    let members: Vec<MemberStatus> = freshness
        .into_iter()
        .zip(opens)
        .zip(registry.member_coordinates())
        .map(|((status, open), coordinate)| {
            debug_assert_eq!(
                status.member, open.member,
                "one roster order, two projections of it"
            );
            MemberStatus::labelled(status, coordinate, &evidence, open.state)
        })
        .collect();

    WorkspaceStatus {
        workspace: registry.federation().name.clone(),
        warm_rollup: warm_state::rollup(members.iter().map(|m| &m.warm), &evidence),
        degraded_rollup,
        members,
        coverage,
        topics,
        kind_candidates,
        build_dependency,
        type_reference,
        exclusion: (!federation.exclusion.is_unset()).then(|| federation.exclusion.clone()),
        links,
    }
}

/// Sort every member's `external-type` call residue into the rows whose type
/// **another member** declares — `type-in-another-member` — and the rest
/// (S-468, [FR-RS-10], [CR-150] §3.2 C). A member's own graph cannot tell the
/// two apart; the workspace can, from the fully-qualified names each member's
/// residue row states it declares, read in the same freshness walk.
///
/// The union of every member's declared names is enough: a row is
/// `external-type` only when its own member declares none of the names it
/// could be, so any declarer of one of them is another member.
///
/// A member whose status was not read declares nothing here, so a row naming
/// only its types stays `external-type`: the split under-counts rather than
/// guesses ([NFR-RA-05]).
///
/// [FR-RS-10]: ../../../docs/specs/requirements/FR-RS-10.md
/// [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn split_call_residue_across_members(freshness: &mut [MemberResult<StatusInfo>]) {
    let declared: std::collections::HashSet<Vec<String>> = freshness
        .iter()
        .filter_map(|row| row.result.as_ref())
        .flat_map(|info| &info.resolution_by_language)
        .filter_map(|language| language.call_residue.as_ref())
        .flat_map(|residue| residue.declared_types.iter().cloned())
        .collect();
    for residue in freshness
        .iter_mut()
        .filter_map(|row| row.result.as_mut())
        .flat_map(|info| &mut info.resolution_by_language)
        .filter_map(|language| language.call_residue.as_mut())
    {
        residue.split_by_workspace(&|fqn: &[String]| declared.contains(fqn));
    }
}

/// The `build_dependency` headline over the build relation the freshness walk's
/// facts were joined into, or `None` when every member was read and none holds
/// a build manifest — so a workspace without one serializes exactly as before
/// the relation existed ([FR-WS-33]). [`workspace_status`] joins the facts once
/// and hands the same relation to the type-reference overlay.
///
/// A member whose facts could not be read — or whose store holds none extracted
/// yet (`not_extracted`) — keeps the headline present: "unread" is not "no
/// manifests", and dropping the section would leave the failure in a log line
/// only ([NFR-CC-04]).
///
/// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
fn build_dependency_section(relation: build_deps::BuildDependencyRelation) -> Option<BuildDependencyHeadline> {
    let members = &relation.headline.members;
    (members.with_manifests > 0 || !members.unread.is_empty()).then_some(relation.headline)
}

/// The bridge edge set the query surface stitches over, resolved once per call
/// so a CLI one-shot and the serve loop share the same entry point ([FR-WS-04]).
///
/// A thin re-export of [`ContractBridge::edges`](super::bridge::ContractBridge::edges)
/// kept here so callers reach the whole query surface through this module. A
/// read-model that names the members it read takes [`bridge_read`] instead.
///
/// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
pub fn edges(
    bridge: &super::bridge::ContractBridge,
    registry: &EngineRegistry<Engine>,
) -> Arc<Vec<BridgeEdge>> {
    bridge.edges(registry)
}

/// `workspace check` — the workspace governance report ([FR-WS-13]), the one
/// entry point the CLI, MCP and HTTP surfaces share.
///
/// Reads the bridge edge set and, when a `no_cross_service_callers` rule is
/// declared, every provider endpoint the members expose — at one stamp snapshot
/// ([`ContractBridge::governance_read`](super::bridge::ContractBridge::governance_read)) —
/// and evaluates the declared rules over them
/// ([`workspace_governance`](super::governance::workspace_governance)).
///
/// # Errors
/// A rule that could never fire because it is malformed, as
/// [`workspace_governance`](super::governance::workspace_governance) defines.
///
/// [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
pub fn workspace_check(
    bridge: &super::bridge::ContractBridge,
    registry: &EngineRegistry<Engine>,
) -> anyhow::Result<Option<super::governance::WorkspaceGovernance>> {
    let federation = registry.federation();
    let with_providers = !federation.governance.no_cross_service_callers.is_empty();
    let (edges, providers) = bridge.governance_read(registry, with_providers);
    super::governance::workspace_governance(federation, &edges, &providers)
}

/// One read of the bridge edge set, with the members read for it
/// ([`bridge_read`]).
#[derive(Debug, Clone, Default)]
pub struct BridgeRead {
    /// The resolved cross-service bindings.
    pub edges: Arc<Vec<BridgeEdge>>,
    /// The members the read touched, and the ones it could not read.
    pub reads: MemberReads,
}

/// The bridge edge set and the members read for it — what
/// [`xservice_route_providers`] is built from ([FR-WS-05], [NFR-PE-10]).
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
pub fn bridge_read(bridge: &super::bridge::ContractBridge, registry: &EngineRegistry<Engine>) -> BridgeRead {
    let (edges, reads) = bridge.edges_read(registry);
    BridgeRead { edges, reads }
}

/// The inputs one **cross-service reachability** answer is built from — the
/// bridge's [`ReachabilityRead`](super::bridge::ReachabilityRead) under the name
/// the query surface has always used ([`reachability_inputs`]): one struct, so a
/// new input is added once.
pub use super::bridge::ReachabilityRead as ReachabilityInputs;

/// The inputs one **cross-service reachability** answer is built from — the edge
/// set, the unresolved egress residue and the declared `[[links]]` judged against
/// those edges — resolved together, with the members read for them ([CR-125],
/// [FR-WS-05], [FR-WS-41], [NFR-PE-10]).
///
/// The reachability twin of [`bridge_read`], and the entry point
/// `callers`/`impact` use instead of it. All three values come from **one** walk
/// at **one** member sync-stamp snapshot, so the answer, its asserted rows and the
/// residue printed beside it can never describe two different generations of the
/// workspace
/// ([`ContractBridge::reachability_read`](super::bridge::ContractBridge::reachability_read)
/// carries the full reasoning).
///
/// [CR-125]: ../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
pub fn reachability_inputs(
    bridge: &super::bridge::ContractBridge,
    registry: &EngineRegistry<Engine>,
) -> ReachabilityInputs {
    bridge.reachability_read(registry)
}

/// The edge set **app-wide reachability** seeds from ([FR-WS-12], [FR-WS-41]):
/// the evidence edges, then the edges the valid `[[links]]` assert — each still
/// labelled by its intake, so the view roots an invocation edge under
/// `live-via-cross-service` and an asserted call under `live-via-asserted-link`
/// and nothing else. Both read at one stamp snapshot. Without `[[links]]`, the
/// evidence edges alone, as [`edges`] answers them.
///
/// [FR-WS-12]: ../../../docs/specs/requirements/FR-WS-12.md
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
pub fn reach_edges(bridge: &super::bridge::ContractBridge, registry: &EngineRegistry<Engine>) -> Vec<BridgeEdge> {
    let (edges, links) = bridge.edges_and_links(registry);
    let mut all = edges.as_ref().clone();
    all.extend(links::asserted_edges(&links));
    all
}

/// The **app-wide reachability** answer every surface serves ([FR-WS-12],
/// [FR-WS-41]): the union view over [`reach_edges`], so the CLI, MCP and web
/// renderings seed from one edge set and none of them chooses its own. Passing
/// the evidence-only [`edges`] to
/// [`app_wide_reachability`](super::reach::app_wide_reachability) instead would
/// compile and silently drop every `live-via-asserted-link` claim — they are
/// absent, not zero, without an asserted edge — which is why the surfaces call
/// this and not the two halves.
///
/// [FR-WS-12]: ../../../docs/specs/requirements/FR-WS-12.md
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
pub fn workspace_reachability(
    bridge: &super::bridge::ContractBridge,
    registry: &EngineRegistry<Engine>,
) -> super::reach::AppWideReachability {
    super::reach::app_wide_reachability(registry, &reach_edges(bridge, registry))
}

/// The workspace security roll-up ([FR-SC-16]): every member's
/// [`MemberSecurity`], and the totals summed over them, each figure beside the
/// population it was summed over ([NFR-CC-04]).
///
/// Totals count only fresh, unsuppressed findings — fresh per finding, by the
/// one definition every security surface shares ([S-760]) — over the members
/// with a stored report (`members_counted`); what they leave out is in
/// `excluded`, and `reports_fresh` of `reports_stored` states how many of the
/// stored reports are themselves fresh at HEAD. A member never scanned is
/// counted in `members_never_scanned`, never as a member with no findings
/// ([BR-66]). A member that could not be
/// opened or read is a row with its `error`, and every other member still
/// answers ([ADR-53]).
///
/// [FR-SC-16]: ../../../docs/specs/requirements/FR-SC-16.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [BR-66]: ../../../docs/specs/software-spec.md#328-security-findings
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
/// [S-760]: ../../../docs/planning/journal.md#s-760-one-freshness-definition-across-the-security-surfaces
#[derive(Debug, Serialize)]
pub struct WorkspaceSecurity {
    /// One row per member, in roster order.
    pub members: Vec<MemberResult<MemberSecurity>>,
    /// Every member of the roster.
    pub members_total: usize,
    /// The members whose security state was read.
    pub members_read: usize,
    /// Members read with no stored report at all.
    pub members_never_scanned: usize,
    /// Members read with at least one stored report: the members the totals
    /// come from. The category counts come from fresh reports only
    /// (`reports_fresh` of `reports_stored`; each category's `covered_in`).
    pub members_counted: usize,
    /// Stored reports over the members read, a disabled scanner's included.
    pub reports_stored: usize,
    /// Of `reports_stored`, the reports that are fresh at HEAD: "N of M
    /// reports fresh", its own figure beside the totals it qualifies.
    pub reports_fresh: usize,
    /// Fresh, unsuppressed findings by severity, over `members_counted`.
    pub totals: SeverityCounts,
    /// The stored findings of the members read that the totals leave out.
    pub excluded: Excluded,
    /// The categories summed over the members read, per edition.
    pub owasp: Vec<WorkspaceEdition>,
}

/// One OWASP edition summed over the members read.
#[derive(Debug, Serialize)]
pub struct WorkspaceEdition {
    pub edition: OwaspEdition,
    /// Every category of the edition, in OWASP's order.
    pub categories: Vec<WorkspaceCategory>,
    /// Counted findings under no category of the edition, summed over the
    /// members read ([`MemberEdition::unclassified`](crate::security::MemberEdition::unclassified)).
    pub unclassified: usize,
}

/// One category summed over the members read.
#[derive(Debug, Serialize)]
pub struct WorkspaceCategory {
    /// The category id (`A05:2025`).
    pub id: &'static str,
    /// The category name.
    pub name: &'static str,
    /// Members with a fresh report from a scanner that can detect it, of
    /// `members_read`.
    pub covered_in: usize,
    /// Fresh, unsuppressed findings under it in those members; `None` when no
    /// member covers it — not covered, never zero ([BR-66]).
    ///
    /// [BR-66]: ../../../docs/specs/software-spec.md#328-security-findings
    pub findings: Option<usize>,
}

/// The workspace security roll-up ([`WorkspaceSecurity`]): each member's
/// `security status` and stored findings read through its own engine, then
/// summed. Starts no scanner beyond the version probe of a scanner a member's
/// scan consent names, once per process for each binary, as the Security page
/// does; writes nothing, and computes no graph context.
pub fn workspace_security(registry: &EngineRegistry<Engine>) -> WorkspaceSecurity {
    roll_up_security(read_member_security(registry, crate::security::VersionProbes::Consented))
}

/// Every member's [`MemberSecurity`], read through its own engine with the
/// version probes `probes` allows, in roster order. A member that would not
/// open and one whose read failed both land in `error`: either way there is
/// nothing to count.
fn read_member_security(
    registry: &EngineRegistry<Engine>,
    probes: crate::security::VersionProbes,
) -> Vec<MemberResult<MemberSecurity>> {
    registry.answer().fan_out(|_, engine| member_security(engine, probes)).into_iter().map(flatten_read).collect()
}

/// One member's [`MemberSecurity`] off its engine.
fn member_security(engine: &Engine, probes: crate::security::VersionProbes) -> anyhow::Result<MemberSecurity> {
    // The roll-up never reads a row's coverage, so no member's tree is walked
    // for it; only an installed gosec or Bandit with no stored report walks
    // it, to decide whether it is `not_applicable` (S-769).
    let status = engine.security_status_probing(probes, false)?;
    // Test code is classified by this member's own graph (FR-SC-16).
    let listings = crate::security::listings(
        engine.root(),
        &engine.scan_environment(),
        engine.tools_manifest(),
        crate::engine::today_utc(),
        |findings| crate::security::context::in_test_symbols(engine, findings),
    )?;
    Ok(MemberSecurity::from_reads(status, &listings))
}

/// The workspace Dashboard's security widget ([FR-UI-36], [S-790]): one row per
/// member — its scan state and its fresh, unsuppressed production severity
/// counts — and how many members have a fresh scan out of all of them.
///
/// **Stored-only**: each member is read with
/// [`VersionProbes::Never`](crate::security::VersionProbes::Never), so no scanner
/// process starts, not even a version probe, and nothing is written; a
/// Dashboard load is a read a cross-site page can trigger ([ADR-80]). Each row
/// is the [`MemberSecurity`] row [`WorkspaceSecurity`] holds for that member,
/// narrowed to the two things the widget states, so a count and a state cannot
/// differ between the two reads. **No figure is summed, averaged or weighted
/// across members** ([BR-56]): the one workspace figure is a count of members.
/// A member that could not be opened or read is a row with its `error`
/// ([NFR-CC-04]).
///
/// [FR-UI-36]: ../../../docs/specs/requirements/FR-UI-36.md
/// [S-790]: ../../../docs/planning/journal.md#s-790-the-workspace-dashboard-rolls-security-up-per-member
/// [ADR-80]: ../../../docs/specs/architecture/decisions/ADR-80.md
/// [BR-56]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Serialize)]
pub struct WorkspaceSecurityState {
    /// One row per member, in roster order.
    pub members: Vec<MemberResult<MemberSecurityState>>,
    /// Every member of the roster.
    pub members_total: usize,
    /// The members whose security state was read.
    pub members_read: usize,
    /// The members whose scan is fresh at HEAD: "N of `members_total`".
    pub members_fresh: usize,
}

/// One member's row of the [`WorkspaceSecurityState`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberSecurityState {
    /// Fresh, stale or never run.
    pub scan: MemberScan,
    /// Fresh, unsuppressed production findings by severity; `None` only for a
    /// member never scanned — nothing to count from, which is not a zero.
    pub findings: Option<SeverityCounts>,
}

impl From<&MemberSecurity> for MemberSecurityState {
    fn from(row: &MemberSecurity) -> Self {
        Self { scan: row.scan, findings: row.findings }
    }
}

/// The stored-only workspace security read ([`WorkspaceSecurityState`]): each
/// member's `security status` and stored findings read through its own engine
/// with no version probe, narrowed to its row. Starts no scanner, writes
/// nothing, and computes no graph context beyond classifying test code.
pub fn workspace_security_state(registry: &EngineRegistry<Engine>) -> WorkspaceSecurityState {
    roll_up_security_state(read_member_security(registry, crate::security::VersionProbes::Never))
}

/// Narrow the member rows into a [`WorkspaceSecurityState`]. Pure.
fn roll_up_security_state(members: Vec<MemberResult<MemberSecurity>>) -> WorkspaceSecurityState {
    let rows: Vec<MemberResult<MemberSecurityState>> = members
        .into_iter()
        .map(|m| MemberResult {
            member: m.member,
            result: m.result.as_ref().map(MemberSecurityState::from),
            error: m.error,
        })
        .collect();
    let states = || rows.iter().filter_map(|m| m.result.as_ref());
    WorkspaceSecurityState {
        members_total: rows.len(),
        members_read: states().count(),
        members_fresh: states().filter(|s| s.scan == MemberScan::Fresh).count(),
        members: rows,
    }
}

/// Sum the member rows into a [`WorkspaceSecurity`]. Pure, so every
/// combination of rows is assertable without a store.
fn roll_up_security(members: Vec<MemberResult<MemberSecurity>>) -> WorkspaceSecurity {
    let mut totals = SeverityCounts::default();
    let mut excluded = Excluded::default();
    let mut owasp: Vec<WorkspaceEdition> = Vec::new();
    let (mut read, mut never_scanned, mut counted) = (0, 0, 0);
    let (mut reports_stored, mut reports_fresh) = (0, 0);
    for security in members.iter().filter_map(|m| m.result.as_ref()) {
        read += 1;
        match security.scan {
            MemberScan::NeverRun => never_scanned += 1,
            MemberScan::Fresh | MemberScan::Stale => counted += 1,
        }
        reports_stored += security.reports_stored;
        reports_fresh += security.reports_fresh;
        if let Some(findings) = &security.findings {
            totals.absorb(findings);
        }
        excluded.absorb(&security.excluded);
        for member_edition in &security.owasp {
            let at = match owasp.iter().position(|e| e.edition == member_edition.edition) {
                Some(at) => at,
                None => {
                    owasp.push(WorkspaceEdition { edition: member_edition.edition, categories: Vec::new(), unclassified: 0 });
                    owasp.len() - 1
                }
            };
            owasp[at].unclassified += member_edition.unclassified;
            let categories = &mut owasp[at].categories;
            for category in &member_edition.categories {
                let summed = match categories.iter().position(|c| c.id == category.id) {
                    Some(i) => &mut categories[i],
                    None => {
                        categories.push(WorkspaceCategory { id: category.id, name: category.name, covered_in: 0, findings: None });
                        categories.last_mut().expect("just pushed")
                    }
                };
                if let Some(n) = category.findings {
                    summed.covered_in += 1;
                    summed.findings = Some(summed.findings.unwrap_or(0) + n);
                }
            }
        }
    }
    WorkspaceSecurity {
        members_total: members.len(),
        members_read: read,
        members_never_scanned: never_scanned,
        members_counted: counted,
        reports_stored,
        reports_fresh,
        totals,
        excluded,
        owasp,
        members,
    }
}

/// The workspace coverage roll-up ([FR-CV-16]): one row per member — its
/// coverage state, overall line coverage over its fresh files, fresh files of
/// total, its tools' outcomes and when one last ran — and how many members
/// have fresh coverage out of all of them ([NFR-CC-04]).
///
/// Each row is read through the member's own engine with the read its own
/// Coverage page makes ([`Engine::coverage_status_probing`] under
/// [`Probes::Never`](crate::coverage::Probes::Never)), so no tool process
/// starts, nothing is written, and a figure here cannot differ from the
/// member's page. **No figure is summed, averaged or weighted across members**
/// ([BR-56]): each member's line coverage is over its own files, so the one
/// workspace figure is a count of members. A member with nothing ingested reads
/// `never_run` with no percentage — never 0 % — and a member that could not be
/// opened or read is a row with its `error`, which hides no other ([ADR-53]).
///
/// [FR-CV-16]: ../../../docs/specs/requirements/FR-CV-16.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [BR-56]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
#[derive(Debug, Serialize)]
pub struct WorkspaceCoverage {
    /// One row per member, in roster order.
    pub members: Vec<MemberResult<MemberCoverage>>,
    /// Every member of the roster.
    pub members_total: usize,
    /// The members whose coverage state was read.
    pub members_read: usize,
    /// The members with fresh coverage: "m of `members_total`".
    pub members_fresh: usize,
    /// Members read with no coverage ingested at all.
    pub members_never_run: usize,
}

/// Whether a member has coverage evidence, and whether any of it still holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberCoverageState {
    /// Nothing is ingested: no tool's report reached the store ([FR-CV-06]).
    ///
    /// [FR-CV-06]: ../../../docs/specs/requirements/FR-CV-06.md
    NeverRun,
    /// A snapshot is stored, and it covers no indexed file: its reports matched
    /// none of the member's files — nothing changed, and nothing is measured.
    Unmatched,
    /// A snapshot is stored, and every file it covers has changed since.
    Stale,
    /// A snapshot is stored, and at least one file it covers is unchanged.
    Fresh,
}

/// One member's row of the [`WorkspaceCoverage`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberCoverage {
    /// Fresh, stale, unmatched or never run.
    pub coverage: MemberCoverageState,
    /// Covered ÷ instrumented lines over the fresh files, in basis points, as the
    /// member's own `coverage status` states it: `None` (`n/a`, [FR-CV-06]) when
    /// no line of a fresh file is covered, and for a member never run — never 0 %.
    pub overall_coverage_bp: Option<i64>,
    /// Covered files whose content still matches the snapshot.
    pub fresh_files: usize,
    /// Covered files in the snapshot, fresh and stale.
    pub total_files: usize,
    /// Tools whose state reads `ran` or `tests_failed`: a report was produced.
    pub tools_ran: usize,
    /// Tools whose state reads `failed` or `timed_out`.
    pub tools_failed: usize,
    /// Tools that apply to the member and are not disabled — the denominator
    /// of the two counts above.
    pub tools_applicable: usize,
    /// When any tool's last run ended, in Unix seconds; `None` before any run.
    pub last_run: Option<u64>,
}

impl From<&crate::history::CoverageStatus> for MemberCoverage {
    fn from(status: &crate::history::CoverageStatus) -> Self {
        let coverage = if status.report_count == 0 {
            MemberCoverageState::NeverRun
        } else if status.total_files == 0 {
            MemberCoverageState::Unmatched
        } else if status.fresh_files == 0 {
            MemberCoverageState::Stale
        } else {
            MemberCoverageState::Fresh
        };
        let tools = status.runner.as_ref().map_or(&[][..], |r| r.tools.as_slice());
        let in_state = |states: &[&str]| tools.iter().filter(|t| states.contains(&t.state)).count();
        Self {
            coverage,
            overall_coverage_bp: status.overall_coverage_bp,
            fresh_files: status.fresh_files,
            total_files: status.total_files,
            tools_ran: in_state(&["ran", "tests_failed"]),
            tools_failed: in_state(&["failed", "timed_out"]),
            tools_applicable: tools.len() - in_state(&["not_applicable", "disabled"]),
            last_run: tools.iter().filter_map(|t| t.last_run.as_ref().map(|r| r.finished_at)).max(),
        }
    }
}

/// The workspace coverage roll-up ([`WorkspaceCoverage`]): each member's
/// coverage status read through its own engine with no version probe, narrowed
/// to its row. Starts no tool, runs no coverage, writes nothing.
pub fn workspace_coverage(registry: &EngineRegistry<Engine>) -> WorkspaceCoverage {
    roll_up_coverage(
        registry
            .answer()
            .fan_out(|_, engine| engine.coverage_status_probing(crate::coverage::Probes::Never))
            .into_iter()
            .map(flatten_read)
            .collect(),
    )
}

/// Narrow the member reads into a [`WorkspaceCoverage`]. Pure.
fn roll_up_coverage(members: Vec<MemberResult<crate::history::CoverageStatus>>) -> WorkspaceCoverage {
    let rows: Vec<MemberResult<MemberCoverage>> = members
        .into_iter()
        .map(|m| MemberResult { member: m.member, result: m.result.as_ref().map(MemberCoverage::from), error: m.error })
        .collect();
    let states = || rows.iter().filter_map(|m| m.result.as_ref().map(|c| c.coverage));
    WorkspaceCoverage {
        members_total: rows.len(),
        members_read: states().count(),
        members_fresh: states().filter(|s| *s == MemberCoverageState::Fresh).count(),
        members_never_run: states().filter(|s| *s == MemberCoverageState::NeverRun).count(),
        members: rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The coordinate the row-shape fixtures below carry.
    const TEST_COORDINATE: &str = "example.com/test";

    /// A bridge read of `edges` that names no member.
    fn read<const N: usize>(edges: [BridgeEdge; N]) -> BridgeRead {
        BridgeRead { edges: Arc::new(edges.to_vec()), reads: MemberReads::default() }
    }

    // ── the build-dependency headline on the status payload (S-463) ─────

    /// [`build_dependency_section`] over a fresh join of `facts`.
    fn build_dependency_headline(
        federation: &super::super::Federation,
        facts: &[MemberBuildFacts],
        not_extracted: &[String],
    ) -> Option<BuildDependencyHeadline> {
        build_dependency_section(build_deps::join(
            &federation.members,
            &federation.member_kinds,
            facts,
            not_extracted,
        ))
    }

    fn two_member_federation() -> super::super::Federation {
        let root = std::path::PathBuf::from("/ws");
        super::super::Federation {
            name: "w".to_string(),
            members: ["a", "b"]
                .iter()
                .map(|n| super::super::Member { name: (*n).to_string(), root: root.join(n) })
                .collect(),
            root,
            default: None,
            links: Vec::new(),
            governance: Default::default(),
            warm_concurrency: None,
            member_kinds: Default::default(),
            exclusion: Default::default(),
        }
    }

    /// The four channels of one freshness-walk member: facts read (pushed),
    /// facts not yet extracted (named apart, never counted read), facts
    /// unreadable on a live engine (not pushed — the member reads unread — and
    /// its freshness row untouched), and an engine that never started (not
    /// pushed, error row) — for the build facts and the declared-type facts
    /// alike, each on its own channel (S-473).
    #[test]
    fn a_failed_build_facts_read_is_unread_and_leaves_the_freshness_row_alone() {
        let scoped = |value| MemberScoped { member: "api".to_string(), value };
        let mut walked = FactsWalked::default();

        let row = split_status_and_facts(
            scoped(Ok((Ok(StatusInfo::default()), Ok(Some(Vec::new())), Ok(Some(MemberTypeFacts::default()))))),
            &mut walked,
        );
        assert!(row.result.is_some() && row.error.is_none());
        assert_eq!(walked.facts.len(), 1, "facts read are pushed, even when empty");
        assert_eq!(walked.types.len(), 1, "declared types read are pushed, even when empty");

        let row =
            split_status_and_facts(scoped(Ok((Ok(StatusInfo::default()), Ok(None), Ok(None)))), &mut walked);
        assert!(row.result.is_some() && row.error.is_none(), "the freshness row is untouched");
        assert_eq!(walked.facts.len(), 1, "a store with no extracted facts is never counted read");
        assert_eq!(walked.not_extracted, ["api"], "…it is named apart");
        assert_eq!(walked.types.len(), 1, "no extracted declared types is never counted read");
        assert_eq!(walked.types_not_extracted, ["api"], "…it is named apart too");

        let row = split_status_and_facts(
            scoped(Ok((
                Ok(StatusInfo::default()),
                Err(anyhow::anyhow!("store read failed")),
                Err(anyhow::anyhow!("store read failed")),
            ))),
            &mut walked,
        );
        assert!(row.result.is_some() && row.error.is_none(), "the freshness row is untouched");
        assert_eq!(walked.facts.len(), 1, "an unreadable member is never counted read");
        assert_eq!(
            (walked.types.len(), walked.types_not_extracted.len()),
            (1, 1),
            "a failed declared-type read pushes nothing: the member reads unread, failed"
        );

        // The two fact channels are independent: build facts read, type facts failed.
        let row = split_status_and_facts(
            scoped(Ok((Ok(StatusInfo::default()), Ok(Some(Vec::new())), Err(anyhow::anyhow!("read failed"))))),
            &mut walked,
        );
        assert!(row.result.is_some() && row.error.is_none());
        assert_eq!((walked.facts.len(), walked.types.len()), (2, 1));

        let row = split_status_and_facts(scoped(Err(anyhow::anyhow!("store is corrupt"))), &mut walked);
        assert!(row.error.is_some());
        assert_eq!((walked.facts.len(), walked.not_extracted.len()), (2, 1));
        assert_eq!((walked.types.len(), walked.types_not_extracted.len()), (1, 1));

        let headline = build_dependency_headline(&two_member_federation(), &[], &[]).unwrap();
        assert_eq!(headline.members.unread, ["a", "b"], "no fact pushed ⇒ named unread");
    }

    /// **Not yet extracted keeps the section and says so** ([FR-WS-33], S-462
    /// task 2): every member of an upgraded workspace unread, with the reason,
    /// never "every member read, none holds a manifest" — which would drop the
    /// section entirely.
    ///
    /// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
    #[test]
    fn a_member_not_yet_extracted_keeps_the_section_and_names_its_reason() {
        let federation = two_member_federation();
        let upgraded = ["a".to_string(), "b".to_string()];
        let headline = build_dependency_headline(&federation, &[], &upgraded)
            .expect("an unextracted member keeps the section");
        assert_eq!(headline.members.unread, ["a", "b"]);
        assert_eq!(
            headline.members.unread_reasons.values().copied().collect::<Vec<_>>(),
            [build_deps::UNREAD_NOT_EXTRACTED; 2]
        );
        assert_eq!((headline.members.read, headline.members.with_manifests), (0, 0));
    }

    /// Every member read and none holds a manifest: the section is absent. One
    /// member unread: it is present and names that member — an unread member
    /// could hold manifests nobody saw, so it must never read as "none".
    #[test]
    fn an_unread_member_keeps_the_build_dependency_headline_on_the_payload() {
        let federation = two_member_federation();
        let all_read = [("a".to_string(), Vec::new()), ("b".to_string(), Vec::new())];
        assert!(build_dependency_headline(&federation, &all_read, &[]).is_none());

        let headline = build_dependency_headline(&federation, &[("a".to_string(), Vec::new())], &[])
            .expect("an unread member keeps the section");
        assert_eq!(headline.members.unread, ["b"]);
        assert_eq!((headline.members.read, headline.members.with_manifests), (1, 0));

        let none_read = build_dependency_headline(&federation, &[], &[]).expect("present");
        assert_eq!(none_read.members.unread, ["a", "b"]);
        assert!(none_read.platform_candidates.is_empty(), "no member read, no candidate");
        assert!(none_read.summary.contains("over 0 of 2 members read"), "{}", none_read.summary);
    }

    use crate::federation::BridgeEndpoint;
    use crate::model::LogosSymbol;

    fn endpoint(member: &str, symbol: &str) -> BridgeEndpoint {
        // The `local <name>` scheme is the smallest valid SCIP symbol the
        // bridge fixtures use — a bare name is not a parseable symbol.
        BridgeEndpoint {
            member: member.to_string(),
            symbol: LogosSymbol::parse(&format!("local {symbol}")).unwrap(),
        }
    }

    fn edge(from_member: &str, from_symbol: &str, to_member: &str, to_symbol: &str) -> BridgeEdge {
        BridgeEdge {
            relation: "route".to_string(),
            from: endpoint(from_member, from_symbol),
            to: endpoint(to_member, to_symbol),
            intake: crate::federation::bridge::BridgeIntake::Invocation,
            from_value: crate::resolve::binding::Provenance::Literal,
            to_value: crate::resolve::binding::Provenance::Literal,
        }
    }

    // ── the member-kind candidate hint (S-457, FR-WS-32) ─────────────────

    /// A freshness row for `member` whose index tags `languages`, one file each.
    fn indexed(member: &str, languages: &[&str]) -> MemberResult<StatusInfo> {
        use crate::models::{LanguageResolution, RelationResolution};
        MemberResult {
            member: member.to_string(),
            result: Some(StatusInfo {
                indexed: true,
                resolution_by_language: languages
                    .iter()
                    .map(|language| LanguageResolution {
                        language: (*language).to_string(),
                        files: 1,
                        calls: RelationResolution::measured(0, 0, 0, 0),
                        imports: RelationResolution::measured(0, 0, 0, 0),
                        call_residue: None,
                    })
                    .collect(),
                ..StatusInfo::default()
            }),
            error: None,
        }
    }

    /// Each half of the rule, probed with the member one fact away from
    /// qualifying: `docs` qualifies; `svc` holds documents beside Java (runnable
    /// source); `config` has no runnable source but holds no document; `gone`
    /// holds documents but its index could not be read; `mock` and `hub` qualify
    /// but are already declared — `hub` as `platform`, a kind that sets no row
    /// apart, and still a declaration. Roster order is kept, and the denominator
    /// is the roster.
    #[test]
    fn a_candidate_holds_api_documents_and_no_runnable_source_and_declares_no_kind() {
        let freshness = vec![
            indexed("config", &["yaml"]),
            indexed("docs", &["markdown", "yaml"]),
            MemberResult { member: "gone".to_string(), result: None, error: Some("unreadable".into()) },
            indexed("hub", &["yaml"]),
            indexed("mock", &["json"]),
            indexed("svc", &["java", "yaml"]),
        ];
        let holds_documents = BTreeSet::from(["docs", "gone", "hub", "mock", "svc"]);
        let declared = BTreeMap::from([
            ("hub".to_string(), MemberKind::Platform),
            ("mock".to_string(), MemberKind::Mock),
        ]);

        let hint = kind_candidates(&freshness, &holds_documents, &declared).expect("docs qualifies");
        assert_eq!(hint.members, ["docs"]);
        assert_eq!(hint.members_total, 6);
        assert!(
            hint.summary.starts_with("1 of 6 members hold API documents and no runnable source"),
            "{}",
            hint.summary
        );
        assert!(hint.summary.contains("never a classification"), "{}", hint.summary);
    }

    /// Only a **contract-surface** row makes its member a holder of API
    /// documents — never an `invocation` call site, and never an `asserted`
    /// claim ([BR-75], [FR-WS-41]).
    ///
    /// [BR-75]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
    #[test]
    fn only_a_contract_surface_row_holds_api_documents_never_an_asserted_one() {
        let row = |member: &str, intake: BridgeIntake| ReferenceCoverage {
            relation: "route".to_string(),
            from: BridgeEndpoint {
                member: member.to_string(),
                symbol: crate::model::LogosSymbol::parse(&format!("local {member}_op")).unwrap(),
            },
            bucket: crate::federation::CoverageState::Bound.bucket(),
            state: crate::federation::CoverageState::Bound,
            to: None,
            intake,
            candidates: None,
            asserted_by: Vec::new(),
            provenance: crate::resolve::binding::Provenance::Literal,
        };
        let references = [
            row("docs", BridgeIntake::ContractSurface),
            row("svc", BridgeIntake::Invocation),
            row("claims", BridgeIntake::Asserted),
        ];
        assert_eq!(members_holding_api_documents(&references), BTreeSet::from(["docs"]));
    }

    /// No qualifying member means no hint on the wire at all.
    #[test]
    fn no_candidate_is_no_hint() {
        let freshness = vec![indexed("svc", &["rust", "yaml"])];
        assert_eq!(
            kind_candidates(&freshness, &BTreeSet::from(["svc"]), &BTreeMap::new()),
            None
        );
    }

    /// The code set is read off the plugin descriptors: the code grammars are in
    /// it, and every data, configuration and documentation grammar is not —
    /// `shell` included, the near miss: it is executable, but its descriptor
    /// declares it an artifact grammar, and the descriptor is what decides.
    #[test]
    #[cfg(all(
        feature = "lang-rust",
        feature = "lang-java",
        feature = "lang-python",
        feature = "lang-typescript",
        feature = "lang-go",
        feature = "lang-kotlin"
    ))]
    fn runnable_source_is_a_code_grammar_and_nothing_else() {
        let code = crate::plugin::grammars::code_language_names();
        for language in ["rust", "java", "python", "typescript", "tsx", "go", "kotlin"] {
            assert!(code.contains(language), "{language} is a code grammar: {code:?}");
        }
        for language in ["yaml", "json", "toml", "markdown", "sql", "protobuf", "graphql", "shell"] {
            assert!(!code.contains(language), "{language} is not runnable source: {code:?}");
        }
    }

    /// Unscoped, `route-providers` returns every resolved binding verbatim.
    #[test]
    fn route_providers_unscoped_returns_every_edge() {
        let edges = read([edge("api", "op1", "web", "r1"), edge("api", "op2", "svc", "r2")]);
        let out = xservice_route_providers(&edges, None);
        assert_eq!(out.providers.len(), 2);
        assert!(out.scope.is_none(), "no scope when repo is None");
    }

    /// `--repo` scopes to bindings whose PROVIDER (`to`) is that member — routes
    /// that member exposes to the rest of the workspace ([FR-WS-05]).
    #[test]
    fn route_providers_repo_scopes_to_the_provider_member() {
        let edges = read([edge("api", "op1", "web", "r1"), edge("api", "op2", "svc", "r2")]);
        let out = xservice_route_providers(&edges, Some("web"));
        assert_eq!(out.providers.len(), 1, "only web-provided routes survive");
        assert_eq!(out.providers[0].to.member, "web");
        assert_eq!(out.scope.as_deref(), Some("web"));

        // A member that provides nothing (only consumes) scopes to empty.
        assert!(xservice_route_providers(&edges, Some("api")).providers.is_empty());
    }

    // ── declared relations beside the bindings (S-461) ──────────────────

    /// A coverage answer carrying the given declared relations and nothing else
    /// worth reading — `with_declared` takes only those two fields from it.
    fn coverage_declaring(
        declared: Option<super::super::DeclaredContractRelation>,
        bound: Option<super::super::BoundExternal>,
    ) -> CrossServiceCoverage {
        use super::super::{ClassificationCounts, IntakeSplit};
        CrossServiceCoverage {
            references: Vec::new(),
            bound: 0,
            ambiguous: 0,
            unbound: 0,
            no_provider_in_workspace: 1,
            by_intake: IntakeSplit {
                contract_surface: ClassificationCounts::default(),
                invocation: ClassificationCounts { no_provider_in_workspace: 1, ..Default::default() },
            },
            resolved_cross_service_edges: 0,
            egress_resolution: None,
            egress_resolution_measured: 0,
            resolved_edges_summary: String::new(),
            spec_conformance_ratio: None,
            spec_conformance_measured: 0,
            spec_conformance_summary: String::new(),
            members_read: 2,
            members_total: 2,
            covers_all_members: true,
            declared_apart: None,
            declared_contracts: declared,
            bound_external: bound,
            asserted_edges: None,
        }
    }

    /// `webmail` vendors a PSS copy; `facade`'s one call binds it under `/prov`.
    fn pss_relations() -> (super::super::DeclaredContractRelation, super::super::BoundExternal) {
        use super::super::declared_contracts::{
            ContractTarget, DeclaredContract, DeclaredContractHeadline, DeclaredContractRelation,
            DocumentAccounting, ExternalId, NamedExternal,
        };
        use super::super::external_join::{
            BaseOrigin, BasePathEvidence, BaseSource, BoundExternal, BoundExternalHeadline,
            ExternalBinding, ExternalJoinRow, JoinAccounting, JoinOutcome,
        };
        let pss = ExternalId("facade:pss.yaml".to_string());
        let relation = DeclaredContractRelation {
            headline: DeclaredContractHeadline {
                declared_contract_pairs: 1,
                to_member: 0,
                to_external: 1,
                documents: DocumentAccounting { documents: 1, vendored: 1, ..Default::default() },
                named_externals: 1,
                identity_collisions: 0,
                resolved_ties: 0,
                summary: "1 declared contract pair; declared by vendored specs, never observed calls".into(),
            },
            contracts: vec![DeclaredContract {
                holder: "facade".into(),
                document: "pss.yaml".into(),
                provenance: "vendored-spec",
                target: ContractTarget::External { external: pss.clone(), name: "PSS".into() },
                operations: Default::default(),
            }],
            externals: vec![NamedExternal {
                id: pss.clone(),
                name: "PSS".into(),
                copies: Vec::new(),
                declared_by: vec!["facade".into()],
                stand_ins: Vec::new(),
            }],
            collisions: Vec::new(),
            resolved_ties: Vec::new(),
        };
        let join = BoundExternal {
            headline: BoundExternalHeadline {
                bound_external: 1,
                no_provider_rows: 1,
                accounting: JoinAccounting { bound_external: 1, ..Default::default() },
                summary: "1 of 1 bound; never a cross-service edge".into(),
            },
            rows: vec![ExternalJoinRow {
                from: endpoint("facade", "f"),
                target: "GET ${pss.url}/user".into(),
                outcome: JoinOutcome::BoundExternal(ExternalBinding {
                    external: pss,
                    name: "PSS".into(),
                    document: "pss.yaml".into(),
                    operation: "GET /prov/user".into(),
                    base: BasePathEvidence {
                        path: "/prov".into(),
                        origin: BaseOrigin::DeployOverlay,
                        sources: vec![BaseSource { file: "deploy/values.yaml".into(), key: "pss.url".into() }],
                    },
                }),
            }],
        };
        (relation, join)
    }

    /// S-461 ([BR-57]): both relations ride beside `providers` verbatim —
    /// the same bytes the coverage answer carries — and add no binding.
    ///
    /// [BR-57]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[test]
    fn route_providers_carry_both_declared_relations_verbatim_and_beside() {
        let edges = read([edge("api", "op1", "web", "r1")]);
        let (relation, join) = pss_relations();
        let coverage = coverage_declaring(Some(relation), Some(join));
        let expected = serde_json::to_value(&coverage).unwrap();

        let out = xservice_route_providers(&edges, None).with_declared(coverage);
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(json["declared_contracts"], expected["declared_contracts"]);
        assert_eq!(json["bound_external"], expected["bound_external"]);
        assert_eq!(json["providers"].as_array().unwrap().len(), 1, "no declared row became a binding");
        assert!(json.get("declared_scope_note").is_none(), "unscoped, nothing to note: {json}");
    }

    /// A workspace with nothing declared prints `route-providers` byte for
    /// byte as before S-461 — no key, not a `null` one.
    #[test]
    fn route_providers_without_declared_relations_serialize_unchanged() {
        let edges = read([edge("api", "op1", "web", "r1")]);
        let before = serde_json::to_string(&xservice_route_providers(&edges, Some("web"))).unwrap();
        let after = serde_json::to_string(
            &xservice_route_providers(&edges, Some("web")).with_declared(coverage_declaring(None, None)),
        )
        .unwrap();
        assert_eq!(after, before);
        assert!(!after.contains("declared"), "{after}");
    }

    /// Under `--repo` the relations stay workspace-wide, and the answer says so
    /// rather than letting them read as the scoped member's ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn a_repo_scope_narrows_providers_and_states_the_relations_are_workspace_wide() {
        let edges = read([edge("api", "op1", "web", "r1")]);
        let (relation, join) = pss_relations();
        let out = xservice_route_providers(&edges, Some("api"))
            .with_declared(coverage_declaring(Some(relation), Some(join)));
        assert!(out.providers.is_empty(), "api provides nothing");
        assert_eq!(out.bound_external.as_ref().map(|b| b.rows.len()), Some(1), "not narrowed");
        let note = out.declared_scope_note.expect("a scoped answer with a relation states its reach");
        assert!(note.contains("`--repo api`") && note.contains("workspace-wide"), "{note}");

        let bound_only = xservice_route_providers(&edges, Some("api"))
            .with_declared(coverage_declaring(None, Some(pss_relations().1)));
        assert!(bound_only.declared_scope_note.is_some(), "the join alone is also workspace-wide");
    }

    /// [`MemberResult`] serialises the ok and error channels **mutually
    /// exclusively** — a healthy member carries `result` (no `error`), a
    /// degraded one carries `error` (no `result`), so the wire is machine-clean.
    #[test]
    fn member_result_serialises_exactly_one_channel() {
        let ok = MemberResult::from_scoped(MemberScoped {
            member: "api".to_string(),
            value: Ok(7u32),
        });
        let value = serde_json::to_value(&ok).unwrap();
        assert_eq!(value["member"], "api");
        assert_eq!(value["result"], 7);
        assert!(value.get("error").is_none(), "an ok result omits the error key");

        let degraded: MemberResult<u32> = MemberResult::from_scoped(MemberScoped {
            member: "web".to_string(),
            value: Err(anyhow::anyhow!("store is corrupt")),
        });
        let value = serde_json::to_value(&degraded).unwrap();
        assert_eq!(value["member"], "web");
        assert!(value.get("result").is_none(), "an error omits the result key");
        assert_eq!(value["error"], "store is corrupt");
    }

    // ── warm state on the member row (FR-WS-15, BR-44, NFR-CC-04) ──────────

    /// A member row carrying `indexed` freshness.
    fn fresh(member: &str, indexed: bool) -> MemberResult<StatusInfo> {
        MemberResult {
            member: member.to_string(),
            result: Some(StatusInfo {
                indexed,
                ..StatusInfo::default()
            }),
            error: None,
        }
    }

    /// A member row whose engine failed to start.
    fn unopenable(member: &str, error: &str) -> MemberResult<StatusInfo> {
        MemberResult {
            member: member.to_string(),
            result: None,
            error: Some(error.to_string()),
        }
    }

    /// The open state of a member whose store opened — the default for rows
    /// exercising the *warm* axis, which the open axis must not perturb.
    fn opened() -> MemberOpenState {
        MemberOpenState::Opened
    }

    /// The open state of a member whose store was attempted and failed, with a
    /// diagnostic that classifies to no cause (so the row's `degraded_reason` is
    /// the verbatim text and the key set stays minimal).
    fn unopened(diagnostic: &str) -> MemberOpenState {
        MemberOpenState::degraded(diagnostic, super::open_state::StoreFile::Present)
    }

    /// The freshness row and the warm label serialise into **one flat member
    /// row** — the coherent table [FR-WS-16] extends, not two lists to join.
    #[test]
    fn a_member_row_carries_freshness_and_the_warm_label_in_one_record() {
        let row = MemberStatus::labelled(fresh("api", true), TEST_COORDINATE, &WarmEvidence::none(), opened());
        let value = serde_json::to_value(&row).unwrap();

        assert_eq!(value["member"], "api");
        assert_eq!(value["result"]["indexed"], true);
        assert_eq!(value["warm_state"], "warm", "one row, both facts: {value}");
        assert_eq!(value["coordinate"], TEST_COORDINATE, "and its identity: {value}");
    }

    /// The member row's **exact key set**, pinned against literals.
    ///
    /// `MemberStatus` flattens *three* structs into one JSON object, and
    /// `serde_json`'s flatten merge is last-write-wins and **silent**: a key
    /// emitted by two halves produces no compile error, no runtime error and no
    /// failing test — one value simply disappears. The collision surface S-323
    /// flagged as "real and imminent" is now live: the degraded row carries
    /// `error` (the verbatim engine diagnostic), `reason` (the *warm* axis') and
    /// `degraded_reason` (the *open* axis'), three distinct facts under three
    /// distinct keys. Pinning the key set is what turns a collision into a
    /// failing test rather than a dropped field.
    ///
    /// [FR-WS-16]'s reason is deliberately **additive**: `error` is byte-for-byte
    /// what it always was, so the still-open question of whether `error` or
    /// `reason` is the canonical degraded-reason field (S-323 deferred #15) can be
    /// settled either way without touching this row's other keys.
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    #[test]
    fn a_member_row_has_exactly_the_keys_it_is_meant_to() {
        let keys = |row: &MemberStatus| {
            let value = serde_json::to_value(row).unwrap();
            let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };

        let healthy = MemberStatus::labelled(fresh("api", true), TEST_COORDINATE, &WarmEvidence::none(), opened());
        assert_eq!(
            keys(&healthy),
            ["coordinate", "member", "open_state", "result", "warm_state"],
            "a healthy row: its coordinate, no `error`, no `reason`, no `degraded_*`, nothing shadowed"
        );

        let degraded = MemberStatus::labelled(
            unopenable("web", "store is corrupt"), TEST_COORDINATE,
            &WarmEvidence::none(),
            unopened("store is corrupt"),
        );
        assert_eq!(
            keys(&degraded),
            [
                "coordinate",
                "degraded_diagnostic",
                "degraded_reason",
                "error",
                "member",
                "open_state",
                "reason",
                "warm_state"
            ],
            "a degraded row: no `result`, and `error`/`reason`/`degraded_reason` all \
             survive the triple flatten as three separate facts, with the verbatim \
             diagnostic on a fourth"
        );

        // A classified cause adds exactly one more key — and only when there IS
        // one, so an unidentified cause is an absent key rather than a default
        // ([NFR-CC-04]).
        let host_limited = MemberStatus::labelled(
            unopenable("svc", "unable to open database file"), TEST_COORDINATE,
            &WarmEvidence::none(),
            MemberOpenState::degraded(
                "unable to open database file",
                super::open_state::StoreFile::Present,
            ),
        );
        assert_eq!(
            keys(&host_limited),
            [
                "coordinate",
                "degraded_cause",
                "degraded_diagnostic",
                "degraded_reason",
                "error",
                "member",
                "open_state",
                "reason",
                "warm_state"
            ],
            "a classified cause is one extra key, never a rename of another"
        );

        // EXTENDED for FR-WS-17, not relaxed: a member whose store opens and
        // reads perfectly well while the durable record says its warm failed is
        // a row shape that could not exist before the record did — `reason` now
        // rides a row that keeps its `result` and is `opened` on the other
        // axis. It is the *warm* axis' reason and nothing else: no `error`, no
        // `degraded_*`, and the freshness payload untouched.
        let warm_failed = MemberStatus::labelled(
            fresh("api", false), TEST_COORDINATE,
            &WarmEvidence::none().with_failures([("api", "index failed: exit status: 2")]),
            opened(),
        );
        assert_eq!(
            keys(&warm_failed),
            ["coordinate", "member", "open_state", "reason", "result", "warm_state"],
            "a recorded warm failure adds exactly `reason` to a healthy row"
        );
        let value = serde_json::to_value(&warm_failed).unwrap();
        assert_eq!(value["warm_state"], "degraded");
        assert_eq!(value["open_state"], "opened", "the OPEN axis is untouched");
        assert_eq!(value["reason"], "index failed: exit status: 2");
    }

    /// The two axes stay **separable**: a member that opens fine but has no index
    /// is `warm_state: deferred` / `open_state: opened`, and one that cannot be
    /// opened is `degraded` on both — two independent derivations, two keys
    /// ([FR-WS-15], [FR-WS-16]).
    ///
    /// The regression this pins is a merge: folding open-state into the warm
    /// vocabulary (or deriving either from the other) would make a perfectly
    /// healthy un-indexed member indistinguishable from an unopenable one on
    /// whichever axis survived.
    ///
    /// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    #[test]
    fn the_warm_axis_and_the_open_axis_are_independent_fields() {
        let deferred_but_openable =
            MemberStatus::labelled(fresh("cold", false), TEST_COORDINATE, &WarmEvidence::none(), opened());
        let value = serde_json::to_value(&deferred_but_openable).unwrap();
        assert_eq!(value["warm_state"], "deferred");
        assert_eq!(
            value["open_state"], "opened",
            "an un-indexed member opened perfectly well: {value}"
        );

        let unopenable_member = MemberStatus::labelled(
            unopenable("broken", "no such table: nodes"), TEST_COORDINATE,
            &WarmEvidence::none(),
            unopened("no such table: nodes"),
        );
        let value = serde_json::to_value(&unopenable_member).unwrap();
        assert_eq!(value["warm_state"], "degraded");
        assert_eq!(value["open_state"], "degraded");
        assert_eq!(
            value["reason"], value["degraded_reason"],
            "the two axes agree here, and still report it on their own keys: {value}"
        );

        // And a member whose store opened but which was skipped by laziness is
        // `not-attempted` on the open axis with the warm axis untouched — the
        // shape a scoped fan-out produces ([NFR-PE-10]).
        let never_reached = MemberStatus::labelled(
            fresh("lazy", false), TEST_COORDINATE,
            &WarmEvidence::none(),
            MemberOpenState::NotAttempted,
        );
        let value = serde_json::to_value(&never_reached).unwrap();
        assert_eq!(value["open_state"], "not-attempted");
        assert_eq!(value["warm_state"], "deferred");
        assert!(
            value.get("degraded_reason").is_none(),
            "a member nobody tried to open has no failure to report: {value}"
        );
    }

    /// Index presence is the whole derivation for the two derivable states: an
    /// indexed member is `warm`, an empty one `deferred`.
    #[test]
    fn index_presence_decides_warm_versus_deferred() {
        let evidence = WarmEvidence::none();
        assert_eq!(
            MemberStatus::labelled(fresh("api", true), TEST_COORDINATE, &evidence, opened()).warm,
            MemberWarmState::Warm
        );
        assert_eq!(
            MemberStatus::labelled(fresh("web", false), TEST_COORDINATE, &evidence, opened()).warm,
            MemberWarmState::Deferred
        );
    }

    /// [BR-44]: a member whose open was attempted and **failed** reports
    /// `degraded` carrying the fan-out's own reason — never `deferred`, which
    /// would claim it was never attempted.
    ///
    /// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[test]
    fn an_unopenable_member_is_degraded_not_deferred() {
        let row = MemberStatus::labelled(
            unopenable("web", "starting the engine for workspace member \"web\""), TEST_COORDINATE,
            &WarmEvidence::none(),
            unopened("starting the engine for workspace member \"web\""),
        );

        assert!(matches!(row.warm, MemberWarmState::Degraded { .. }));
        assert_ne!(row.warm, MemberWarmState::Deferred);
        let value = serde_json::to_value(&row).unwrap();
        assert_eq!(value["warm_state"], "degraded");
        assert!(
            value["reason"].as_str().unwrap().contains("web"),
            "the fan-out's reason rides the label: {value}"
        );
        // The freshness channel is untouched — the row still reports the error
        // it always did, so no existing reader loses anything.
        assert!(value["error"].as_str().is_some());
    }

    /// The finding this fix closes: a member whose engine STARTED but whose
    /// **freshness read failed** must land in the `error` channel — and so read
    /// `degraded` — not arrive as a defaulted `StatusInfo` whose `indexed:
    /// false` would read `deferred` ("never attempted", [BR-44]).
    ///
    /// [`Engine::status`] hides exactly that case by design (ADR-14 degradation),
    /// which is why [`fan_status`] fans [`Engine::try_status`]. Asserted over
    /// [`flatten_read`] rather than a corrupted on-disk store, so all three
    /// channels are pinned deterministically.
    ///
    /// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[test]
    fn a_failed_freshness_read_becomes_an_error_channel_not_an_empty_graph() {
        let scoped = |value| MemberScoped {
            member: "api".to_string(),
            value,
        };

        // Inner Err — the engine started, `navigate::status` failed. THE case.
        let row = flatten_read(scoped(Ok(Err(anyhow::anyhow!("no such table: edges")))));
        assert!(
            row.result.is_none(),
            "a failed read must not surface as a defaulted, apparently-empty graph"
        );
        assert_eq!(row.error.as_deref(), Some("no such table: edges"));
        assert!(
            matches!(
                MemberStatus::labelled(row, TEST_COORDINATE, &WarmEvidence::none(), opened()).warm,
                MemberWarmState::Degraded { .. }
            ),
            "BR-44: the read was attempted and failed"
        );

        // Outer Err — the engine never started. Still degraded, as before.
        let row = flatten_read(scoped(Err(anyhow::anyhow!("starting the engine"))));
        assert!(row.result.is_none());
        assert_eq!(row.error.as_deref(), Some("starting the engine"));

        // Ok — the read succeeded and reports an honestly empty graph.
        let row = flatten_read(scoped(Ok(Ok(StatusInfo::default()))));
        assert!(row.error.is_none(), "a successful read carries no error");
        assert_eq!(
            MemberStatus::labelled(row, TEST_COORDINATE, &WarmEvidence::none(), opened()).warm,
            MemberWarmState::Deferred,
            "an honestly empty graph is deferred, which is the whole distinction"
        );
    }

    /// A row with neither channel populated (unreachable through
    /// `from_scoped`, but representable) reads `degraded`, not `deferred`:
    /// missing freshness is not evidence that nothing was attempted
    /// ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn a_row_with_no_freshness_at_all_is_degraded_not_deferred() {
        let row = MemberStatus::labelled(
            MemberResult {
                member: "ghost".to_string(),
                result: None,
                error: None,
            }, TEST_COORDINATE,
            &WarmEvidence::none(),
            opened(),
        );
        assert!(matches!(row.warm, MemberWarmState::Degraded { .. }));
    }

    /// The roll-up is derived from the very rows it accompanies, so the two can
    /// never disagree — and with no live signal the `warming` key is absent
    /// from the roll-up entirely ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn the_rollup_agrees_with_the_rows_and_omits_warming() {
        let evidence = WarmEvidence::none();
        let rows: Vec<MemberStatus> = vec![
            MemberStatus::labelled(fresh("api", true), TEST_COORDINATE, &evidence, opened()),
            MemberStatus::labelled(fresh("web", true), TEST_COORDINATE, &evidence, opened()),
            MemberStatus::labelled(fresh("svc", false), TEST_COORDINATE, &evidence, opened()),
            MemberStatus::labelled(
                unopenable("old", "store is corrupt"), TEST_COORDINATE,
                &evidence,
                unopened("store is corrupt"),
            ),
        ];
        let rollup = warm_state::rollup(rows.iter().map(|r| &r.warm), &evidence);

        assert_eq!(rollup.members, rows.len());
        assert_eq!((rollup.warm, rollup.deferred, rollup.degraded), (2, 1, 1));

        let value = serde_json::to_value(&rollup).unwrap();
        assert!(
            value.get("warming").is_none(),
            "no live signal ⇒ no warming key, not a fabricated 0: {value}"
        );

        // The roll-up really is a projection of the rows, not a parallel count.
        let count = |want: &MemberWarmState| {
            rows.iter()
                .filter(|r| std::mem::discriminant(&r.warm) == std::mem::discriminant(want))
                .count()
        };
        assert_eq!(count(&MemberWarmState::Warm), rollup.warm);
        assert_eq!(count(&MemberWarmState::Deferred), rollup.deferred);
        assert_eq!(
            count(&MemberWarmState::Degraded {
                reason: String::new()
            }),
            rollup.degraded
        );
        assert_eq!(
            count(&MemberWarmState::Warming),
            0,
            "the vocabulary is present but `warming` is unreachable today"
        );
    }

    // ── the `xservice type-refs` listing (S-474) ─────────────────────────

    /// A bound reference to `fqn`, owned by `lib`'s source, from `importer`'s
    /// `file`, by import, through the build relation.
    fn type_reference(fqn: &str, importer: &str, file: &str) -> TypeReference {
        TypeReference {
            fqn: fqn.to_string(),
            naming: TypeNaming::Exact,
            form: TypeRefForm::Import,
            importer: TypeImporter {
                member: importer.to_string(),
                file: file.to_string(),
                line: Some(3),
                symbol: format!("{importer} {file}"),
            },
            owner: TypeOwner {
                member: "lib".to_string(),
                origin: type_refs::TypeOrigin::Source,
                declared_in: format!("{fqn}.java"),
                symbol: Some(format!("lib {fqn}#")),
                kind: "class".to_string(),
            },
            evidence: PairEvidence::Build { platform: false },
        }
    }

    /// **One entry per type, its importers under it**: references arrive in
    /// the index's order (owner, type, importer) and leave grouped by type,
    /// types sorted by name and each type's importers in arrival order — so a
    /// type with 131 importers on the estate is one entry, not 131.
    #[test]
    fn imported_types_groups_references_by_type_in_name_then_importer_order() {
        let types = imported_types(vec![
            type_reference("com.acme.lib.Bean", "app", "App.java"),
            type_reference("com.acme.lib.Dto", "app", "Also.java"),
            type_reference("com.acme.lib.Dto", "app", "App.java"),
            type_reference("com.acme.lib.Dto", "web", "Web.java"),
        ]);
        let shape: Vec<(&str, Vec<&str>)> = types
            .iter()
            .map(|t| (t.fqn.as_str(), t.importers.iter().map(|i| i.importer.file.as_str()).collect()))
            .collect();
        assert_eq!(
            shape,
            [
                ("com.acme.lib.Bean", vec!["App.java"]),
                ("com.acme.lib.Dto", vec!["Also.java", "App.java", "Web.java"]),
            ]
        );
        assert_eq!(types[1].owner.declared_in, "com.acme.lib.Dto.java", "the type's one owner, once");
    }

    /// **Each importer row says how it bound**: `naming`, `form` and
    /// `evidence` are the reference's own, copied through — an enclosing
    /// type-use row admitted through a collision is listed as exactly that,
    /// never as the exact build-admitted import the fixtures mostly hold.
    #[test]
    fn imported_types_carries_each_references_naming_form_and_evidence() {
        let mut reference = type_reference("com.acme.lib.Dto", "user", "User.java");
        reference.naming = TypeNaming::Enclosing;
        reference.form = TypeRefForm::TypeUse;
        reference.evidence = PairEvidence::Collision {
            artifacts: vec!["com.acme:dup".to_string()],
        };
        let types = imported_types(vec![reference]);
        let row = serde_json::to_value(&types[0].importers[0]).unwrap();
        assert_eq!(
            (&row["naming"], &row["form"], &row["evidence"]),
            (
                &serde_json::json!("enclosing"),
                &serde_json::json!("type-use"),
                &serde_json::json!({"via": "collision", "artifacts": ["com.acme:dup"]})
            ),
            "{row:#}"
        );
        assert_eq!(row["file"], "User.java", "the importer is flattened beside them: {row:#}");
    }

    // ── the queried side is a (member, symbol) pair (S-701, BR-73) ───────

    /// The byte-identical symbol two members declare — the shape every Spring
    /// archetype and every fork produces.
    const SHARED: &str = "logos . . . src/main/kotlin/`Dto.kt`/Dto#";

    fn roster(names: &[&str]) -> Vec<super::super::Member> {
        names
            .iter()
            .map(|name| super::super::Member { name: (*name).to_string(), root: format!("/ws/{name}").into() })
            .collect()
    }

    /// A bound reference from `importer` to the type `fqn` that `owner` declares
    /// at node `node` (`None` for an Avro type).
    fn reference(owner: &str, node: Option<&str>, fqn: &str, importer: &str) -> TypeReference {
        TypeReference {
            fqn: fqn.to_string(),
            naming: TypeNaming::Exact,
            form: TypeRefForm::Import,
            importer: TypeImporter {
                member: "app".to_string(),
                file: importer.to_string(),
                line: Some(3),
                symbol: format!("file:{importer}"),
            },
            owner: TypeOwner {
                member: owner.to_string(),
                origin: if node.is_some() { type_refs::TypeOrigin::Source } else { type_refs::TypeOrigin::Avro },
                declared_in: "src/main/kotlin/Dto.kt".to_string(),
                symbol: node.map(str::to_string),
                kind: "class".to_string(),
            },
            evidence: PairEvidence::Build { platform: false },
        }
    }

    fn importers(references: &[&TypeReference]) -> Vec<String> {
        references.iter().map(|r| format!("{}:{}", r.owner.member, r.importer.file)).collect()
    }

    /// An endpoint at a full `LogosSymbol`, as the bridge stores one.
    fn at(member: &str, symbol: &str) -> BridgeEndpoint {
        BridgeEndpoint { member: member.to_string(), symbol: crate::model::LogosSymbol::parse(symbol).unwrap() }
    }

    /// **The queried endpoint matches on the member and the symbol** ([BR-73]),
    /// probed with each near miss: the same symbol in another member, and the
    /// scoped member with another symbol.
    #[test]
    fn a_queried_endpoint_is_the_member_and_the_symbol_never_the_symbol_alone() {
        let svc_b = at("svc-b", SHARED);
        assert!(is_queried(&svc_b, SHARED, Some("svc-b")));
        assert!(!is_queried(&svc_b, SHARED, Some("svc-a")), "the same symbol in another member is not the scoped one");
        assert!(
            !is_queried(&at("svc-a", "logos . . . src/main/kotlin/`Dto.kt`/Dtos#"), SHARED, Some("svc-a")),
            "the scoped member's neighbouring symbol is not the queried one"
        );
        assert!(is_queried(&svc_b, SHARED, None), "unscoped, every member holding the symbol is queried");
    }

    /// **A symbol written without a repository coordinate is queried in each
    /// member under that member's own coordinate** ([FR-EX-36]), as each member's
    /// fan-out row resolves it: every holder's endpoint and resolved row answer
    /// to it, under any coordinate. A symbol naming a coordinate stays exact,
    /// and one descriptor off answers to nothing.
    ///
    /// [FR-EX-36]: ../../../docs/specs/requirements/FR-EX-36.md
    #[test]
    fn an_old_form_symbol_is_queried_under_each_members_own_coordinate() {
        let in_a = at("svc-a", "logos . example.com/a . src/main/kotlin/`Dto.kt`/Dto#");
        let in_b = at("svc-b", "logos . example.com/b . src/main/kotlin/`Dto.kt`/Dto#");
        assert!(is_queried(&in_a, SHARED, None) && is_queried(&in_b, SHARED, None));
        assert!(is_queried(&in_b, SHARED, Some("svc-b")) && !is_queried(&in_b, SHARED, Some("svc-a")));
        assert!(!is_queried(&in_a, in_b.symbol.as_str(), None), "another coordinate is never rewritten");
        assert!(!is_queried(&in_a, "logos . . . src/main/kotlin/`Dto.kt`/Dtos#", None), "a near miss");

        let row = |symbol: &LogosSymbol| SymbolRef {
            symbol: symbol.to_scip_string(),
            name: "Dto".to_string(),
            kind: NodeKind::Class,
            file: None,
            line: None,
        };
        assert!(holds(Some(&row(&in_a.symbol)), SHARED), "the member's upgraded row holds it");
        assert!(holds(Some(&row(&in_a.symbol)), in_a.symbol.as_str()));
        assert!(!holds(Some(&row(&in_a.symbol)), in_b.symbol.as_str()));
    }

    /// **Unscoped over a symbol two members hold, the answer is grouped and
    /// flagged** ([BR-73]): every holder with its share — zero for one that
    /// holds the symbol and provides nothing — and a notice naming them and
    /// `--repo`. A scope, or a single holder, adds nothing.
    #[test]
    fn an_unscoped_symbol_two_members_hold_is_grouped_per_member_and_flagged() {
        let grouped = QueriedSide::group(SHARED, None, ["svc-a", "svc-b"].into_iter(), &["svc-b"]);
        assert_eq!(
            grouped.by_member,
            [
                MemberResolved { member: "svc-a".to_string(), resolved: 0 },
                MemberResolved { member: "svc-b".to_string(), resolved: 1 },
            ]
        );
        let warning = grouped.warning.expect("two holders are flagged");
        assert!(
            warning.contains("held by 2 members (svc-a, svc-b)") && warning.contains("--repo"),
            "{warning}"
        );

        // A near endpoint is a holder too, though its fan-out row did not resolve.
        let near_only = QueriedSide::group(SHARED, None, std::iter::empty(), &["svc-b", "svc-c", "svc-b"]);
        assert_eq!(
            near_only.by_member,
            [
                MemberResolved { member: "svc-b".to_string(), resolved: 2 },
                MemberResolved { member: "svc-c".to_string(), resolved: 1 },
            ]
        );

        for (label, quiet) in [
            ("scoped", QueriedSide::group(SHARED, Some("svc-a"), ["svc-a", "svc-b"].into_iter(), &["svc-b"])),
            ("one holder", QueriedSide::group(SHARED, None, ["svc-b"].into_iter(), &["svc-b", "svc-b"])),
            ("no holder", QueriedSide::group(SHARED, None, std::iter::empty(), &[])),
        ] {
            assert!(quiet.by_member.is_empty() && quiet.warning.is_none(), "{label} adds nothing");
        }
    }

    /// **Type references key on the owner member on the node path**
    /// ([BR-73]): scoped, only the scope's own; unscoped, both, grouped by owner
    /// member in **roster** order (here `svc-b` before `svc-a`, against the
    /// index's name order).
    #[test]
    fn a_type_reference_on_the_node_path_is_scoped_to_its_owner_member() {
        let refs = [
            reference("svc-a", Some(SHARED), "com.acme.a.Dto", "UsesA.java"),
            reference("svc-b", Some(SHARED), "com.acme.b.Dto", "UsesB.java"),
        ];
        let roster = roster(&["svc-b", "svc-a", "app"]);
        for (scope, want) in [(Some("svc-a"), vec!["svc-a:UsesA.java"]), (Some("svc-b"), vec!["svc-b:UsesB.java"])] {
            let reaching = references_reaching(&roster, &refs, SHARED, scope);
            assert_eq!(importers(&reaching.attributed), want, "{scope:?}");
            assert!(reaching.unattributed.is_empty());
        }
        let unscoped = references_reaching(&roster, &refs, SHARED, None);
        assert_eq!(importers(&unscoped.attributed), ["svc-b:UsesB.java", "svc-a:UsesA.java"]);
    }

    /// **A symbol written without a coordinate reaches each owner's node under
    /// that owner's own coordinate** ([FR-EX-36]), as the bridge tier and the
    /// member rows match it: two members holding one shape under two
    /// coordinates are both reached, and the scope still keys on the owner. A
    /// canonical symbol reaches its own owner alone, and a near miss nothing.
    ///
    /// [FR-EX-36]: ../../../docs/specs/requirements/FR-EX-36.md
    #[test]
    fn an_old_form_symbol_reaches_each_owners_node_under_its_own_coordinate() {
        let in_a = "logos . example.com/a . src/main/kotlin/`Dto.kt`/Dto#";
        let in_b = "logos . example.com/b . src/main/kotlin/`Dto.kt`/Dto#";
        let refs = [
            reference("svc-a", Some(in_a), "com.acme.a.Dto", "UsesA.java"),
            reference("svc-b", Some(in_b), "com.acme.b.Dto", "UsesB.java"),
        ];
        let roster = roster(&["svc-a", "svc-b", "app"]);
        let reached =
            |symbol: &str, scope: Option<&str>| importers(&references_reaching(&roster, &refs, symbol, scope).attributed);
        assert_eq!(reached(SHARED, None), ["svc-a:UsesA.java", "svc-b:UsesB.java"]);
        assert_eq!(reached(SHARED, Some("svc-b")), ["svc-b:UsesB.java"]);
        assert_eq!(reached(in_a, None), ["svc-a:UsesA.java"], "a canonical symbol is exact");
        assert!(reached("logos . . . src/main/kotlin/`Dto.kt`/Dtos#", None).is_empty(), "a near miss");
    }

    /// **The dotted-name path takes the scope too** ([BR-73]): an Avro type has
    /// no node, and is reached by its name, filtered on its declaring member.
    #[test]
    fn a_type_reference_on_the_dotted_name_path_is_scoped_to_its_owner_member() {
        let refs = [reference("models", None, "com.acme.events.Evt", "App.java")];
        let roster = roster(&["models", "svc-a", "app"]);
        let evt = "com.acme.events.Evt";
        assert_eq!(importers(&references_reaching(&roster, &refs, evt, Some("models")).attributed), ["models:App.java"]);
        assert!(references_reaching(&roster, &refs, evt, Some("svc-a")).attributed.is_empty());
        assert_eq!(importers(&references_reaching(&roster, &refs, evt, None).attributed), ["models:App.java"]);
    }

    /// **A reference whose owner is no workspace member is reported apart,
    /// never dropped** ([CR-221] CRA-02): under any scope and none, on both
    /// paths. The index builder names a roster member on every owner it makes,
    /// so this is the case of an index and a roster that disagree.
    ///
    /// [CR-221]: ../../../docs/requests/CR-221-cross-service-answers-match-member-and-symbol.md
    #[test]
    fn a_type_reference_with_no_attributable_owner_lands_in_the_unattributed_group() {
        let refs = [
            reference("svc-a", Some(SHARED), "com.acme.a.Dto", "UsesA.java"),
            reference("gone", Some(SHARED), "com.acme.g.Dto", "UsesG.java"),
            reference("gone", None, "com.acme.events.Gone", "Gone.java"),
        ];
        let roster = roster(&["svc-a", "app"]);
        for scope in [Some("svc-a"), Some("svc-b"), None] {
            let node = references_reaching(&roster, &refs, SHARED, scope);
            assert_eq!(importers(&node.unattributed), ["gone:UsesG.java"], "node path, {scope:?}");
            let dotted = references_reaching(&roster, &refs, "com.acme.events.Gone", scope);
            assert_eq!(importers(&dotted.unattributed), ["gone:Gone.java"], "dotted path, {scope:?}");
            assert!(dotted.attributed.is_empty());
        }
        assert_eq!(importers(&references_reaching(&roster, &refs, SHARED, Some("svc-a")).attributed), ["svc-a:UsesA.java"]);
    }

    // ── the workspace security roll-up (S-674, FR-SC-16) ────────────────────

    /// A member row with `scan`, the severity counts, the exclusions, and one
    /// 2025 edition holding `categories` (id, findings); its low findings are
    /// the ones under no category.
    fn security_row(
        member: &str,
        scan: MemberScan,
        findings: Option<SeverityCounts>,
        excluded: Excluded,
        categories: &[(&'static str, Option<usize>)],
    ) -> MemberResult<MemberSecurity> {
        MemberResult {
            member: member.to_string(),
            result: Some(MemberSecurity {
                scan,
                scanners_fresh: usize::from(scan == MemberScan::Fresh),
                scanners_enabled: 1,
                scanners: Vec::new(),
                reports_stored: usize::from(scan != MemberScan::NeverRun),
                reports_fresh: usize::from(scan == MemberScan::Fresh),
                findings,
                excluded,
                owasp: vec![crate::security::MemberEdition {
                    edition: OwaspEdition::V2025,
                    categories: categories
                        .iter()
                        .map(|&(id, findings)| crate::security::MemberCategory { id, name: id, findings })
                        .collect(),
                    unclassified: findings.map_or(0, |f| f.low),
                }],
            }),
            error: None,
        }
    }

    fn counts(high: usize, low: usize) -> SeverityCounts {
        SeverityCounts { high, low, total: high + low, ..SeverityCounts::default() }
    }

    /// The fold sums only the members read: a fresh member's counts, a stale
    /// member's counts too — findings are fresh per finding, not per report
    /// ([S-760]) — and its exclusions, a never-scanned member as such, and an
    /// unread member in the total alone. Reports are summed apart, as "N of M
    /// fresh". A category covered nowhere stays `None`; one covered with no
    /// finding is `Some(0)`.
    ///
    /// [S-760]: ../../../docs/planning/journal.md#s-760-one-freshness-definition-across-the-security-surfaces
    #[test]
    fn the_security_roll_up_sums_fresh_counts_over_the_members_read() {
        let rows = vec![
            security_row(
                "a",
                MemberScan::Fresh,
                Some(counts(2, 1)),
                Excluded { not_fresh: 0, suppressed: 1, test: 2 },
                &[("A01", Some(1)), ("A02", Some(0)), ("A03", None)],
            ),
            security_row(
                "b",
                MemberScan::Fresh,
                Some(counts(1, 0)),
                Excluded::default(),
                &[("A01", Some(1)), ("A02", None), ("A03", None)],
            ),
            security_row(
                "c",
                MemberScan::Stale,
                Some(counts(0, 2)),
                Excluded { not_fresh: 4, suppressed: 0, test: 3 },
                &[("A01", None), ("A02", None), ("A03", None)],
            ),
            security_row("d", MemberScan::NeverRun, None, Excluded::default(), &[("A01", None), ("A02", None), ("A03", None)]),
            MemberResult { member: "e".to_string(), result: None, error: Some("unable to open database file".into()) },
        ];
        let rolled = roll_up_security(rows);
        assert_eq!(
            (rolled.members_total, rolled.members_read, rolled.members_never_scanned, rolled.members_counted),
            (5, 4, 1, 3)
        );
        assert_eq!((rolled.reports_fresh, rolled.reports_stored), (2, 3), "two of the three stored reports are fresh");
        assert_eq!(rolled.totals, counts(3, 3));
        assert_eq!(rolled.excluded, Excluded { not_fresh: 4, suppressed: 1, test: 5 }, "test code is summed apart");
        let names: Vec<&str> = rolled.members.iter().map(|m| m.member.as_str()).collect();
        assert_eq!(names, ["a", "b", "c", "d", "e"], "every row kept, in order");

        assert_eq!(rolled.owasp.len(), 1);
        let summed: Vec<(&str, usize, Option<usize>)> =
            rolled.owasp[0].categories.iter().map(|c| (c.id, c.covered_in, c.findings)).collect();
        assert_eq!(summed, [("A01", 2, Some(2)), ("A02", 1, Some(0)), ("A03", 0, None)]);
        assert_eq!(rolled.owasp[0].unclassified, 3, "the findings under no category are summed apart");
    }

    /// The Dashboard's state read keeps each member's own state and counts —
    /// the figures the full roll-up's row holds — states how many members have
    /// a fresh scan out of all, and lists an unread member as a row with its
    /// error. A stale member is not fresh, and a never-scanned one has no count
    /// (not a zero).
    #[test]
    fn the_security_state_keeps_each_members_row_and_counts_the_fresh_ones() {
        let rows = vec![
            security_row("a", MemberScan::Fresh, Some(counts(2, 1)), Excluded::default(), &[]),
            security_row("b", MemberScan::Stale, Some(counts(0, 2)), Excluded { not_fresh: 4, suppressed: 0, test: 3 }, &[]),
            security_row("c", MemberScan::NeverRun, None, Excluded::default(), &[]),
            MemberResult { member: "d".to_string(), result: None, error: Some("unable to open database file".into()) },
        ];
        let state = roll_up_security_state(rows);
        assert_eq!((state.members_total, state.members_read, state.members_fresh), (4, 3, 1));
        let row = |i: usize| {
            let m = &state.members[i];
            (m.member.as_str(), m.result.clone(), m.error.as_deref())
        };
        let state_of = |scan, findings| Some(MemberSecurityState { scan, findings });
        assert_eq!(row(0), ("a", state_of(MemberScan::Fresh, Some(counts(2, 1))), None));
        assert_eq!(row(1), ("b", state_of(MemberScan::Stale, Some(counts(0, 2))), None));
        assert_eq!(row(2), ("c", state_of(MemberScan::NeverRun, None), None));
        assert_eq!(row(3), ("d", None, Some("unable to open database file")));
    }

    /// BR-56: the state read-model carries no composite. Its serialised keys are
    /// pinned — a total, a mean, a score or any figure summed over members would
    /// be a new key here, and this test names it.
    #[test]
    fn the_security_state_carries_no_figure_summed_across_members() {
        let state = roll_up_security_state(vec![
            security_row("a", MemberScan::Fresh, Some(counts(2, 1)), Excluded::default(), &[]),
            security_row("b", MemberScan::Fresh, Some(counts(1, 0)), Excluded::default(), &[]),
        ]);
        let json = serde_json::to_value(&state).unwrap();
        let mut top: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
        top.sort_unstable();
        assert_eq!(top, ["members", "members_fresh", "members_read", "members_total"]);
        let mut row: Vec<&str> = json["members"][0]["result"].as_object().unwrap().keys().map(String::as_str).collect();
        row.sort_unstable();
        assert_eq!(row, ["findings", "scan"]);
    }

    /// No member read: nothing summed, and no edition is invented.
    #[test]
    fn a_security_roll_up_with_no_member_read_sums_nothing() {
        let rolled = roll_up_security(vec![MemberResult {
            member: "only".to_string(),
            result: None,
            error: Some("store obstructed".into()),
        }]);
        assert_eq!((rolled.members_total, rolled.members_read, rolled.members_counted), (1, 0, 0));
        assert_eq!((rolled.reports_fresh, rolled.reports_stored), (0, 0));
        assert_eq!(rolled.totals, SeverityCounts::default());
        assert!(rolled.owasp.is_empty());
    }

    // ── the workspace coverage roll-up (S-740, FR-CV-16) ────────────────────

    /// A coverage status with `reports` merged reports over `fresh` of `total`
    /// files at `bp`, and one runner row per `(tool, state, last run end)`.
    fn coverage_status(
        reports: usize,
        fresh: usize,
        total: usize,
        bp: Option<i64>,
        tools: &[(&str, &'static str, Option<u64>)],
    ) -> crate::history::CoverageStatus {
        use crate::history::coverage::{CoverageConsentStatus, CoverageLastRun, CoverageRunnerStatus, CoverageToolStatus};
        crate::history::CoverageStatus {
            head_sha: (reports > 0).then(|| "abc".to_string()),
            config_hash: None,
            formats: Vec::new(),
            report_count: reports,
            total_files: total,
            fresh_files: fresh,
            stale_files: total - fresh,
            freshness_bp: None,
            overall_coverage_bp: bp,
            files: Vec::new(),
            notice: None,
            current_head: None,
            head_stale: false,
            staleness_prompt: None,
            runner: Some(CoverageRunnerStatus {
                consent: CoverageConsentStatus { consented: Vec::new(), pending: Vec::new(), command_changed: false },
                tools: tools
                    .iter()
                    .map(|&(tool, state, finished)| CoverageToolStatus {
                        tool: tool.to_string(),
                        state,
                        version: None,
                        origin: None,
                        hint: None,
                        offer: None,
                        last_run: finished.map(|finished_at| CoverageLastRun {
                            outcome: "ran".to_string(),
                            exit_code: Some(0),
                            head_sha: None,
                            finished_at,
                            log: None,
                        }),
                    })
                    .collect(),
            }),
            job: None,
        }
    }

    fn coverage_read(member: &str, status: crate::history::CoverageStatus) -> MemberResult<crate::history::CoverageStatus> {
        MemberResult { member: member.to_string(), result: Some(status), error: None }
    }

    /// A row narrows the member's own status: its figures verbatim, tools that
    /// produced a report (a failing suite's included) apart from tools that
    /// failed, over the tools that apply and are enabled, and the latest end of
    /// any tool's last run — an uninstalled tool's included, since its run
    /// still happened.
    #[test]
    fn a_member_coverage_row_narrows_its_own_status() {
        let status = coverage_status(
            2,
            3,
            5,
            Some(8125),
            &[
                ("cargo-llvm-cov", "ran", Some(100)),
                ("vitest", "tests_failed", Some(300)),
                ("pytest", "failed", Some(200)),
                ("go", "timed_out", None),
                ("maven", "not_installed", Some(400)),
                ("jest", "cancelled", None),
                ("gradle", "never_run", None),
                ("bun", "not_applicable", None),
                ("custom", "disabled", None),
            ],
        );
        assert_eq!(
            MemberCoverage::from(&status),
            MemberCoverage {
                coverage: MemberCoverageState::Fresh,
                overall_coverage_bp: Some(8125),
                fresh_files: 3,
                total_files: 5,
                tools_ran: 2,
                tools_failed: 2,
                tools_applicable: 7,
                last_run: Some(400),
            }
        );
    }

    /// Never run is the state of a member with nothing ingested, whatever its
    /// tools did — a failed run ingests nothing — and it carries no percentage;
    /// a snapshot whose every file moved is stale, not never run; and one whose
    /// reports matched no indexed file is unmatched — nothing in it changed.
    #[test]
    fn nothing_ingested_is_never_run_and_every_file_moved_is_stale() {
        let never = MemberCoverage::from(&coverage_status(0, 0, 0, None, &[("vitest", "failed", Some(7))]));
        assert_eq!(never.coverage, MemberCoverageState::NeverRun);
        assert_eq!(never.overall_coverage_bp, None, "never run is not 0 %");
        assert_eq!((never.tools_failed, never.last_run), (1, Some(7)), "the failed run is still stated");

        let stale = MemberCoverage::from(&coverage_status(1, 0, 4, None, &[]));
        assert_eq!(stale.coverage, MemberCoverageState::Stale);
        assert_eq!((stale.fresh_files, stale.total_files, stale.tools_applicable, stale.last_run), (0, 4, 0, None));

        let unmatched = MemberCoverage::from(&coverage_status(1, 0, 0, None, &[]));
        assert_eq!(unmatched.coverage, MemberCoverageState::Unmatched, "a report matching no file is not stale");
    }

    /// The fold counts members, never sums a figure: the fresh ones of all,
    /// the never-run ones apart, a stale one in neither, and an unread member
    /// in the total alone, as a row with its error.
    #[test]
    fn the_coverage_roll_up_counts_fresh_and_never_run_members_of_all() {
        let rolled = roll_up_coverage(vec![
            coverage_read("fresh", coverage_status(1, 2, 2, Some(5000), &[])),
            coverage_read("idle", coverage_status(0, 0, 0, None, &[])),
            coverage_read("moved", coverage_status(1, 0, 3, None, &[])),
            MemberResult { member: "broken".to_string(), result: None, error: Some("would not open".to_string()) },
        ]);
        let order: Vec<&str> = rolled.members.iter().map(|m| m.member.as_str()).collect();
        assert_eq!(order, ["fresh", "idle", "moved", "broken"], "roster order, the unread member kept");
        assert_eq!(
            (rolled.members_total, rolled.members_read, rolled.members_fresh, rolled.members_never_run),
            (4, 3, 1, 1)
        );
        assert_eq!(rolled.members[3].error.as_deref(), Some("would not open"));
        assert!(rolled.members[3].result.is_none());
        let state = |i: usize| rolled.members[i].result.as_ref().map(|c| c.coverage);
        assert_eq!(state(2), Some(MemberCoverageState::Stale));
        let mut keys: Vec<String> = serde_json::to_value(&rolled).unwrap().as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["members", "members_fresh", "members_never_run", "members_read", "members_total"],
            "no composite across members (BR-56)"
        );
    }

    /// An empty roster rolls up to zeroes, with nothing read.
    #[test]
    fn an_empty_workspace_rolls_coverage_up_to_zeroes() {
        let rolled = roll_up_coverage(Vec::new());
        assert!(rolled.members.is_empty());
        assert_eq!((rolled.members_total, rolled.members_read, rolled.members_fresh, rolled.members_never_run), (0, 0, 0, 0));
    }
}
