//! `[[links]]` resolution — a declared cross-service coupling validated into an
//! **asserted** bridge edge, or a stated reason it is not one ([FR-WS-41],
//! [CR-224]).
//!
//! The manifest ([`super::manifest`]) parses each link and holds its `relation`
//! to the closed vocabulary. This module does the rest, one link at a time:
//!
//! 1. **Endpoint grammar.** `from` and `to` are each `<member>::<target>`. The
//!    member is the longest workspace member name the endpoint starts with
//!    followed by `::`, so a member path holding a `/`, a member name holding
//!    `::`, and a target holding `::` all parse. An endpoint naming no member is `unknown-member` — and a
//!    member `[workspace] exclude` removed is not a member ([FR-WS-40]).
//! 2. **Cross-service only.** Both endpoints in one member is `same-member`: an
//!    intra-repo edge belongs to that member's own graph.
//! 3. **Exactly one, in the endpoint's own member.** The target is resolved by
//!    that member's own lookup ([`MemberContracts::resolve_target`]): a canonical
//!    symbol, else every node bearing that name — counting only the nodes a
//!    coupling can start or end at ([`is_link_endpoint`]), so a documentation
//!    heading or a file never answers for a handler. None is `no-match`; two or more
//!    is `ambiguous`, listing at most [`CANDIDATE_LIMIT`] candidates and counting
//!    the rest. A member that will not open or read is `member-degraded` —
//!    never `no-match`, which would claim a target is absent from a member
//!    nobody read ([NFR-CC-04]).
//! 4. **Never a second edge.** A valid link duplicating an edge already held —
//!    an evidence edge, or an earlier link's — is `redundant`, naming that edge.
//!    "Duplicating" is endpoint for endpoint, where **a route and the handler it
//!    serves are one endpoint** ([`Endpoint::alike`]): a measured HTTP edge ends
//!    at the route node, so a link naming the handler behind an already-bound
//!    route repeats it, and is never listed beside it. Otherwise it is `valid`
//!    and carries its one [`BridgeEdge`], intake
//!    [`Asserted`](BridgeIntake::Asserted), relation class from
//!    [`LinkRelation::namespace`].
//!
//! # A claim, never evidence ([BR-75])
//! The evidence edge set is an **input** here and never an output: nothing in
//! this module feeds the bridge's exactly-one matcher, so a link can neither bind
//! a reference nor make an evidence binding ambiguous ([NFR-RA-05]). Every
//! consumer that switches on intake asks [`BridgeIntake::is_evidence`] before
//! counting an edge.
//!
//! # Where a valid link is used ([S-709])
//! The asserted edges join `xservice callers` and `impact` in a section of their
//! own, each row marked [`ASSERTED_MARKER`], and seed app-wide reachability
//! under the distinct `live-via-asserted-link` verdict when the relation is a
//! call ([`seeds_asserted_root`]). They are counted apart — [`AssertedEdges`]
//! beside the coverage headline — and a residue row a valid link covers keeps
//! its reason and gains a pointer to the link ([`links_covering`]). A renamed target resolves to nothing
//! on the next answer, so its link turns `no-match` and stops seeding.
//!
//! [FR-WS-40]: ../../../docs/specs/requirements/FR-WS-40.md
//! [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
//! [CR-224]: ../../../docs/requests/CR-224-asserted-cross-service-links.md
//! [BR-75]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [S-709]: ../../../docs/planning/journal.md#s-709-asserted-links-join-callers-impact-and-reachability-labelled

use std::collections::BTreeMap;

use serde::Serialize;

use crate::model::{LogosSymbol, NodeKind};
use crate::resolve::binding::Provenance;

use super::bridge::{compute_edges, BridgeEdge, BridgeEndpoint, BridgeIntake, MemberContracts};
use super::coverage::CANDIDATE_LIMIT;
use super::manifest::{Link, LinkRelation};
use super::registry::{AnswerScope, MemberEngine};
use super::Federation;

/// One declared `[[links]]` entry and what became of it ([FR-WS-41]) — a row of
/// `workspace status`'s `links` list.
///
/// The declaration is echoed as written, so a reader can find the entry in the
/// manifest; the verdict is flattened beside it under `status`.
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkStatus {
    /// The declared relation.
    pub relation: LinkRelation,
    /// The declared `from` endpoint, as written.
    pub from: String,
    /// The declared `to` endpoint, as written.
    pub to: String,
    /// What the link resolved to.
    #[serde(flatten)]
    pub verdict: LinkVerdict,
}

impl LinkStatus {
    /// The asserted edge this link is, when it is [`Valid`](LinkVerdict::Valid)
    /// — and nothing for an invalid link or a redundant one, whose
    /// [`duplicates`](LinkVerdict::Redundant) edge is someone else's. The one
    /// place a consumer reads a link's edge from.
    #[must_use]
    pub fn asserted_edge(&self) -> Option<&BridgeEdge> {
        match &self.verdict {
            LinkVerdict::Valid { edge } => Some(edge),
            LinkVerdict::Invalid(_) | LinkVerdict::Redundant { .. } => None,
        }
    }
}

/// What one link resolved to ([FR-WS-41]). Internally tagged: the row carries
/// `"status": "valid" | "invalid" | "redundant"` and the fields of its arm.
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum LinkVerdict {
    /// Both endpoints resolved to exactly one node, in two different members:
    /// the link is one [`Asserted`](BridgeIntake::Asserted) edge.
    Valid {
        /// The asserted edge.
        edge: BridgeEdge,
    },
    /// The link names no edge; [`InvalidLink::reason`] says why. It yields
    /// nothing.
    Invalid(InvalidLink),
    /// The link is valid but an edge already joins the same two endpoints, so it
    /// yields no second one.
    Redundant {
        /// The edge it duplicates — an evidence edge (its intake says which), or
        /// an earlier link's asserted one.
        duplicates: BridgeEdge,
    },
}

/// Why a link names no edge ([FR-WS-41]).
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InvalidLink {
    /// The reason, from the closed vocabulary.
    pub reason: InvalidReason,
    /// The endpoint the reason is about. Absent for `same-member`, which is
    /// about the pair.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<LinkEnd>,
    /// The nodes an `ambiguous` target matched. Absent for every other reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidates: Option<LinkCandidates>,
    /// One line naming the endpoint, the member and what to do about it.
    pub detail: String,
}

/// The reasons a link is invalid ([FR-WS-41]).
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InvalidReason {
    /// The endpoint names no workspace member — including one `[workspace]
    /// exclude` removed, and an endpoint with no `<member>::` at all.
    UnknownMember,
    /// The member exists but could not be opened or read, so whether it holds
    /// the target is unknown.
    MemberDegraded,
    /// The member was read and nothing in it is the target.
    NoMatch,
    /// The target matched two or more nodes in the member.
    Ambiguous,
    /// Both endpoints are in one member — not a cross-service edge.
    SameMember,
}

impl InvalidReason {
    /// The wire token, as `serde` writes it — for a composed line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            InvalidReason::UnknownMember => "unknown-member",
            InvalidReason::MemberDegraded => "member-degraded",
            InvalidReason::NoMatch => "no-match",
            InvalidReason::Ambiguous => "ambiguous",
            InvalidReason::SameMember => "same-member",
        }
    }
}

/// Which end of a link an [`InvalidLink`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkEnd {
    /// The `from` endpoint.
    From,
    /// The `to` endpoint.
    To,
}

impl LinkEnd {
    fn key(self) -> &'static str {
        match self {
            LinkEnd::From => "from",
            LinkEnd::To => "to",
        }
    }
}

/// The nodes an ambiguous target matched — bounded, and never silently trimmed
/// ([NFR-CC-04]): the same bound and the same `total`/`omitted` arithmetic as a
/// coverage row's [`ProviderCandidates`](super::coverage::ProviderCandidates).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkCandidates {
    /// At most [`CANDIDATE_LIMIT`] of the matches, in symbol order — each one a
    /// spelling the endpoint can name to be exactly one.
    pub symbols: Vec<LogosSymbol>,
    /// How many nodes matched before truncation.
    pub total: u64,
    /// How many of [`total`](Self::total) are not listed — `0` when all are,
    /// and stated even then.
    pub omitted: u64,
}

/// Whether a node of `kind` can be a link endpoint — a declaration a coupling can
/// start or end at ([FR-WS-41], [NFR-RA-05]).
///
/// Not a documentation node (a heading, an ADR, a story) and not a whole file
/// (a source file's `Module`, a `ConfigFile`): neither calls, serves, publishes
/// or subscribes, and counting them would let a README heading named like a
/// handler make the handler `ambiguous` — or, worse, answer for it as a valid
/// endpoint and keep a renamed handler's link alive. Everything else stays: the
/// code declarations, and the contract-surface nodes (a route, an operation, a
/// proto service, a topic) a coupling is drawn to.
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub(super) const fn is_link_endpoint(kind: NodeKind) -> bool {
    !(kind.is_documentation() || matches!(kind, NodeKind::Module | NodeKind::ConfigFile))
}

/// Every edge the valid links assert, in manifest order ([FR-WS-41]) — the set
/// a consumer of asserted edges reads, derived from the rows so the edges and
/// the report cannot disagree.
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
pub fn asserted_edges(links: &[LinkStatus]) -> Vec<BridgeEdge> {
    links.iter().filter_map(LinkStatus::asserted_edge).cloned().collect()
}

/// The marker every asserted row of an `xservice callers`/`impact` answer
/// carries in its `reached` field — what a human reading the answer sees beside
/// the row's `intake: "asserted"` ([FR-WS-41], [BR-75]).
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
/// [BR-75]: ../../../docs/specs/software-spec.md#327-workspace-federation
pub const ASSERTED_MARKER: &str = "(asserted)";

/// Whether `edge` seeds a `live-via-asserted-link` root in app-wide
/// reachability ([FR-WS-12], [FR-WS-41]): an [`Asserted`](BridgeIntake::Asserted)
/// edge declared as a **call** ([`LinkRelation::is_call`]) — never a
/// `publishes` link, which reaches a topic and no callable, and never an
/// evidence edge, whose roots are [`BridgeIntake::seeds_reachability_root`]'s.
///
/// The relation is read back from the edge's class through
/// [`LinkRelation::namespace`], the one mapping that filed it there.
///
/// [FR-WS-12]: ../../../docs/specs/requirements/FR-WS-12.md
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
#[must_use]
pub fn seeds_asserted_root(edge: &BridgeEdge) -> bool {
    match edge.intake {
        BridgeIntake::Asserted => LinkRelation::ALL
            .into_iter()
            .any(|relation| relation.is_call() && relation.namespace().relation() == edge.relation),
        BridgeIntake::Invocation | BridgeIntake::ContractSurface => false,
    }
}

/// The declared links counted **beside** the coverage headline, never inside
/// it ([FR-WS-41], [BR-75]): how many became an asserted edge, how many are
/// invalid and why, and how many repeat an edge already held.
///
/// Carried as `coverage.asserted_edges` on `workspace status`. Absent when the
/// manifest declares no link, so such a payload serializes as before.
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
/// [BR-75]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssertedEdges {
    /// Links that are one asserted edge each.
    pub valid: u64,
    /// Links that name no edge, of every reason.
    pub invalid: u64,
    /// [`invalid`](Self::invalid) split by reason, in reason order; the entries
    /// sum to it. Empty when nothing is invalid.
    pub invalid_by_reason: BTreeMap<InvalidReason, u64>,
    /// Links repeating an edge already held, which yield no second one.
    pub redundant: u64,
    /// The three counts as one line, naming them as declared — e.g. `"1 asserted
    /// edge from 3 declared links (1 invalid: no-match 1; 1 redundant); declared,
    /// never counted as evidence"`.
    pub summary: String,
}

impl AssertedEdges {
    /// Tally `links`, or `None` when there is none.
    #[must_use]
    pub fn tally(links: &[LinkStatus]) -> Option<Self> {
        if links.is_empty() {
            return None;
        }
        let (mut valid, mut redundant) = (0u64, 0u64);
        let mut invalid_by_reason: BTreeMap<InvalidReason, u64> = BTreeMap::new();
        for link in links {
            match &link.verdict {
                LinkVerdict::Valid { .. } => valid += 1,
                LinkVerdict::Redundant { .. } => redundant += 1,
                LinkVerdict::Invalid(invalid) => *invalid_by_reason.entry(invalid.reason).or_default() += 1,
            }
        }
        let invalid: u64 = invalid_by_reason.values().sum();
        let edges = if valid == 1 { "edge" } else { "edges" };
        let declared = if links.len() == 1 { "link" } else { "links" };
        let reasons: Vec<String> =
            invalid_by_reason.iter().map(|(reason, n)| format!("{} {n}", reason.as_str())).collect();
        let invalid_part = if invalid == 0 {
            "0 invalid".to_string()
        } else {
            format!("{invalid} invalid: {}", reasons.join(", "))
        };
        Some(Self {
            valid,
            invalid,
            invalid_by_reason,
            redundant,
            summary: format!(
                "{valid} asserted {edges} from {} declared {declared} ({invalid_part}; {redundant} redundant); \
                 declared, never counted as evidence",
                links.len()
            ),
        })
    }
}

/// Every **valid** link whose asserted edge starts at `from` in relation class
/// `relation` — the links covering a residue row with that consumer endpoint —
/// each as the manifest declared it, so a reader can find it ([FR-WS-41],
/// [BR-75]). Empty when none does. The row keeps its reason: an assertion
/// explains a gap, it does not close it.
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
/// [BR-75]: ../../../docs/specs/software-spec.md#327-workspace-federation
pub fn links_covering(links: &[LinkStatus], relation: &str, from: &BridgeEndpoint) -> Vec<Link> {
    links
        .iter()
        .filter(|link| link.asserted_edge().is_some_and(|edge| edge.relation == relation && edge.from == *from))
        .map(|link| Link {
            relation: link.relation,
            from: link.from.clone(),
            to: link.to.clone(),
        })
        .collect()
}

/// Every declared link judged against `edges`, or nothing — without reading
/// anything — when the manifest declares no link ([FR-WS-41]). The entry the
/// bridge's answer cache fills its link slot through, over the evidence edges
/// it already holds; [`workspace_links`] is the same short-circuit over a fresh
/// edge computation.
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
pub(super) fn links_over<E>(answer: &AnswerScope<'_, E>, edges: &[BridgeEdge]) -> Vec<LinkStatus>
where
    E: MemberEngine + MemberContracts,
{
    if answer.registry().federation().links.is_empty() {
        return Vec::new();
    }
    resolve_links(answer, edges)
}

/// `workspace status`'s `links` rows over `answer` ([FR-WS-41]).
///
/// Empty **without reading anything** when the manifest declares no link, so a
/// workspace without `[[links]]` pays no walk and serializes as before. With
/// links, the evidence edge set redundancy is judged against is computed over
/// the same answer — the walks share its open attempts — and each link is
/// resolved by [`resolve_links`].
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
pub(super) fn workspace_links<E>(answer: &AnswerScope<'_, E>) -> Vec<LinkStatus>
where
    E: MemberEngine + MemberContracts,
{
    if answer.registry().federation().links.is_empty() {
        return Vec::new();
    }
    resolve_links(answer, &compute_edges(answer))
}

/// Resolve and validate every declared link against `edges`, in manifest order
/// ([FR-WS-41]).
///
/// `edges` is the set a link must not duplicate. Only its evidence edges
/// ([`BridgeIntake::is_evidence`]) are read, so a caller may pass a set that
/// already holds asserted edges. Opens only the members the links name.
///
/// A link duplicates a held edge when each of its endpoints is **alike** the
/// edge's — the same node, or a route and the handler it serves
/// ([`Endpoint::alike`]) — so a link naming the handler of a route a measured
/// edge binds is `redundant`, naming that edge.
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
pub fn resolve_links<E>(answer: &AnswerScope<'_, E>, edges: &[BridgeEdge]) -> Vec<LinkStatus>
where
    E: MemberEngine + MemberContracts,
{
    let federation = answer.registry().federation();
    let mut held: Vec<BridgeEdge> = edges.iter().filter(|e| e.intake.is_evidence()).cloned().collect();
    federation
        .links
        .iter()
        .map(|link| {
            let resolved = resolve_link(answer, federation, link).and_then(|edge| {
                let from = Endpoint::alike(answer, &edge.from, LinkEnd::From)?;
                let to = Endpoint::alike(answer, &edge.to, LinkEnd::To)?;
                Ok((edge, from, to))
            });
            let verdict = match resolved {
                Err(invalid) => LinkVerdict::Invalid(invalid),
                Ok((edge, from, to)) => match held.iter().find(|e| from.holds(&e.from) && to.holds(&e.to)) {
                    Some(duplicate) => LinkVerdict::Redundant {
                        duplicates: duplicate.clone(),
                    },
                    None => {
                        held.push(edge.clone());
                        LinkVerdict::Valid { edge }
                    }
                },
            };
            LinkStatus {
                relation: link.relation,
                from: link.from.clone(),
                to: link.to.clone(),
                verdict,
            }
        })
        .collect()
}

/// One resolved link endpoint **as an endpoint of a coupling**: its node and
/// every node that is one endpoint with it because a route serves through it —
/// the handler a route routes to, or the route(s) routing to a handler
/// ([FR-WS-41]).
///
/// The one route↔handler lookup redundancy uses; the store read behind it is
/// [`MemberContracts::route_counterparts`].
///
/// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
struct Endpoint {
    member: String,
    symbols: Vec<LogosSymbol>,
}

impl Endpoint {
    /// `end` and its route/handler counterparts, read in `end`'s own member. A
    /// member that will not read them now is `member-degraded`: whether the link
    /// repeats a measured edge is then unknown, and a guess either way would
    /// list a claim beside the evidence or hide one ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    fn alike<E>(answer: &AnswerScope<'_, E>, end: &BridgeEndpoint, which: LinkEnd) -> Result<Self, InvalidLink>
    where
        E: MemberEngine + MemberContracts,
    {
        let member = end.member.as_str();
        let counterparts = answer
            .open_for_walk(member)
            .and_then(|engine| engine.route_counterparts(&end.symbol))
            .map_err(|err| InvalidLink {
                reason: InvalidReason::MemberDegraded,
                endpoint: Some(which),
                candidates: None,
                detail: format!(
                    "`{}` endpoint's member `{member}` could not be read for the routes serving \
                     `{}`, so whether the link repeats a measured edge is unknown: {err:#}",
                    which.key(),
                    end.symbol.as_str()
                ),
            })?;
        let mut symbols = vec![end.symbol.clone()];
        symbols.extend(counterparts);
        Ok(Self {
            member: member.to_string(),
            symbols,
        })
    }

    /// Whether `other` is this endpoint: the same member, and its node or one of
    /// the counterparts.
    fn holds(&self, other: &BridgeEndpoint) -> bool {
        other.member == self.member && self.symbols.contains(&other.symbol)
    }
}

/// One link's asserted edge, or the first reason it has none.
///
/// The checks run in the order the reasons are cheapest to know: the member
/// names (no read), then whether they differ (no read), then each target in its
/// own member. `same-member` is decided before any lookup on purpose — an
/// intra-repo pair is not a cross-service claim however its targets resolve.
fn resolve_link<E>(
    answer: &AnswerScope<'_, E>,
    federation: &Federation,
    link: &Link,
) -> Result<BridgeEdge, InvalidLink>
where
    E: MemberEngine + MemberContracts,
{
    let (from_member, from_target) = split_endpoint(federation, &link.from, LinkEnd::From)?;
    let (to_member, to_target) = split_endpoint(federation, &link.to, LinkEnd::To)?;
    if from_member == to_member {
        return Err(InvalidLink {
            reason: InvalidReason::SameMember,
            endpoint: None,
            candidates: None,
            detail: format!(
                "both endpoints are in member `{from_member}`: a link asserts a \
                 cross-service edge, and an edge inside one member is its own graph's"
            ),
        });
    }
    let from = resolve_endpoint(answer, from_member, from_target, LinkEnd::From)?;
    let to = resolve_endpoint(answer, to_member, to_target, LinkEnd::To)?;
    Ok(BridgeEdge {
        relation: link.relation.namespace().relation().to_string(),
        from,
        to,
        intake: BridgeIntake::Asserted,
        // Both endpoints are read verbatim from the manifest — no configuration
        // was consulted to compose either.
        from_value: Provenance::Literal,
        to_value: Provenance::Literal,
    })
}

/// Split `endpoint` into `(member, target)` on the **longest** workspace member
/// name it starts with followed by `::` — so `services/web::a::b` is member
/// `services/web`, target `a::b`.
fn split_endpoint<'a>(
    federation: &'a Federation,
    endpoint: &'a str,
    end: LinkEnd,
) -> Result<(&'a str, &'a str), InvalidLink> {
    let split = federation
        .members
        .iter()
        .filter_map(|member| {
            let rest = endpoint.strip_prefix(member.name.as_str())?.strip_prefix("::")?;
            Some((member.name.as_str(), rest))
        })
        .max_by_key(|(member, _)| member.len());
    split.ok_or_else(|| InvalidLink {
        reason: InvalidReason::UnknownMember,
        endpoint: Some(end),
        candidates: None,
        detail: unknown_member_detail(federation, endpoint, end),
    })
}

/// The `unknown-member` line: no `::` at all, a member `[workspace] exclude`
/// removed, or a name the workspace does not have.
fn unknown_member_detail(federation: &Federation, endpoint: &str, end: LinkEnd) -> String {
    let key = end.key();
    let Some((named, _)) = endpoint.split_once("::") else {
        return format!(
            "`{key}` endpoint {endpoint:?} is not `<member>::<target>` — it names no member"
        );
    };
    if federation.exclusion.excluded.iter().any(|excluded| excluded == named) {
        return format!(
            "`{key}` endpoint names member `{named}`, which `[workspace] exclude` removed \
             from workspace `{}`",
            federation.name
        );
    }
    format!(
        "`{key}` endpoint names member `{named}`, which is not a member of workspace `{}`",
        federation.name
    )
}

/// Resolve one endpoint's `target` to exactly one node of `member`, through that
/// member's own lookup.
fn resolve_endpoint<E>(
    answer: &AnswerScope<'_, E>,
    member: &str,
    target: &str,
    end: LinkEnd,
) -> Result<BridgeEndpoint, InvalidLink>
where
    E: MemberEngine + MemberContracts,
{
    let key = end.key();
    let invalid = |reason, candidates, detail| InvalidLink {
        reason,
        endpoint: Some(end),
        candidates,
        detail,
    };
    if target.is_empty() {
        return Err(invalid(
            InvalidReason::NoMatch,
            None,
            format!("`{key}` endpoint names member `{member}` and no target after `::`"),
        ));
    }
    let degraded = |err: anyhow::Error| {
        invalid(
            InvalidReason::MemberDegraded,
            None,
            format!(
                "`{key}` endpoint's member `{member}` could not be read, so whether it \
                 holds `{target}` is unknown: {err:#}"
            ),
        )
    };
    let engine = answer.open_for_walk(member).map_err(degraded)?;
    let mut matches = engine.resolve_target(target).map_err(degraded)?;
    matches.sort();
    matches.dedup();
    let total = matches.len();
    match <[LogosSymbol; 1]>::try_from(matches) {
        Ok([symbol]) => Ok(BridgeEndpoint {
            member: member.to_string(),
            symbol,
        }),
        Err(matches) if matches.is_empty() => Err(invalid(
            InvalidReason::NoMatch,
            None,
            format!(
                "`{key}` endpoint's target `{target}` matches no declaration in member \
                 `{member}` (documentation and whole-file nodes never count)"
            ),
        )),
        Err(mut matches) => {
            matches.truncate(CANDIDATE_LIMIT);
            let listed = matches.len();
            Err(invalid(
                InvalidReason::Ambiguous,
                Some(LinkCandidates {
                    symbols: matches,
                    total: total as u64,
                    omitted: (total - listed) as u64,
                }),
                format!(
                    "`{key}` endpoint's target `{target}` matches {total} nodes in member \
                     `{member}` ({listed} listed); name one by its canonical symbol"
                ),
            ))
        }
    }
}

#[cfg(test)]
mod tests;
