//! The **declared-contract relation** — `declares-contract(A → C)`, provenance
//! `vendored-spec` — and the **named-external registry** it resolves into
//! ([FR-WS-31], [ADR-68] points 1–2).
//!
//! A member that holds an API spec document it does not implement is stating
//! which API it talks to. This module reads that statement: every spec document
//! a member holds is judged against the coverage tier's own verdict on each of
//! its operations, and a document its holder provides **none** of is *vendored*.
//! A vendored document declares a contract
//!
//! - to the **member** whose own spec it is, by **document identity** — at least
//!   90 % of the vendored document's operations (verb and positional template)
//!   appear in that member's own spec; the score and the matched document are
//!   named. Two members at the same best score resolve to **neither**, and the
//!   collision is reported;
//! - else to a **named external**: the vendored copies are grouped by the same
//!   containment rule into one external per group, named by the spec's
//!   `info.title` unless it is springdoc's default [`SPRINGDOC_DEFAULT_TITLE`],
//!   else by the document's file stem. A display name is not unique — two groups
//!   can carry one title — so an external is identified by [`ExternalId`].
//!
//! # Declared, never observed ([BR-57], [ADR-26])
//!
//! The relation has its own types and its own headline,
//! [`DeclaredContractHeadline::declared_contract_pairs`], stated over every spec
//! document read ([`DocumentAccounting`], [BR-51]). It never enters a provider
//! bucket, a [`BridgeEdge`](super::BridgeEdge), `resolved_cross_service_edges`
//! or `egress_resolution`: nothing in [`bridge`](super::bridge) reads this
//! module, and the coverage tier only **adds** the relation beside its figures.
//! The byte-identity of those figures and of the bridge edge set is pinned by
//! the coverage tier's tests.
//!
//! Where document identity names a member, the holder's document's per-operation
//! contract-surface **ties** that include that member are resolved to it
//! ([`ResolvedTie`]) — for **that holder's document only**. The coverage row
//! itself stays ambiguous under the exactly-one rule ([FR-WS-04]): a declared
//! contract is evidence about the holder's intent, never a binding candidate.
//!
//! # Member kinds ([ADR-68] point 5)
//!
//! What a kind means here is asked of the kind through one exhaustive `match`
//! (`Standing::of`): a `mock` member is a **stand-in provider** — its copies
//! join the external they stand in for and it declares nothing; a
//! `documentation` member's copies stay out altogether; a `platform` member is
//! an ordinary one. A mock or documentation member is never the member a
//! document identifies.
//!
//! # Built from member-local facts, in memory ([ADR-52])
//!
//! [`derive()`] is pure. Its inputs are each member's contract-surface operations
//! with the coverage tier's provider verdict ([`SpecOperation`]) and the
//! `info.title` of each document the grouping names ([`read_title`] reads it
//! from the file in the member's working tree — a display label only, so a
//! title edited since the last index can label operations indexed before it).
//! The coverage tier builds it on the
//! cross-service query, from the walk it already makes; nothing is persisted and
//! no store is migrated.
//!
//! # The reference derivation
//!
//! The identity score, the implementation bands, the copy graph and the naming
//! rule are those of S-456's gate harness
//! (`logos-core/tests/operand_resolvability/vendored_spec_contracts.rs`), which
//! measured this relation before it was built. Three differences are
//! deliberate: a provider verdict here is read from the untruncated candidate
//! list (the harness read a row's list, truncated at eight, and called a
//! truncated one undecidable); a `documentation` holder's copies do not join
//! the copy graph (the harness pooled them); and [`spec_title`] reads the
//! `title` key under `info` through a parser, where the harness took the first
//! `title` substring after `info` — which a Swagger 2 `description` placed
//! first defeats.
//!
//! [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
//! [FR-WS-31]: ../../../docs/specs/requirements/FR-WS-31.md
//! [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
//! [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
//! [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [BR-57]: ../../../docs/specs/software-spec.md#327-workspace-federation

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde::Serialize;

use crate::model::LogosSymbol;

use super::bridge::BridgeEndpoint;
use super::manifest::MemberKind;

/// The provenance token every [`DeclaredContract`] carries ([ADR-68] point 1).
///
/// [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
pub const VENDORED_SPEC: &str = "vendored-spec";

/// springdoc's default `info.title`, which names no external: every internal
/// springdoc spec on the reference estate carries it ([CR-147] §2.1).
///
/// [CR-147]: ../../../docs/requests/CR-147-vendored-specs-declare-contracts-and-name-externals.md
pub const SPRINGDOC_DEFAULT_TITLE: &str = "OpenAPI definition";

/// The document-identity threshold, in percent of the held document's
/// operations ([ADR-68] point 1). Also the "implements" threshold: a holder
/// providing at least this share of its document's operations holds its own
/// spec.
///
/// [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
pub const IDENTITY_THRESHOLD_PERCENT: usize = 90;

/// An operation's key: `(upper-cased METHOD, positional template)` — the shared
/// `route_key` the bridge binds on, so identity compares what binding compares.
pub type OperationKey = (String, String);

/// What the coverage tier decided about the providers of one operation's key
/// — the verdict a document's implementation is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationProviders {
    /// The holder is the sole provider: the tier's intra-repo answer (no row).
    Holder,
    /// Two or more providers tie, listed in full (never truncated).
    Tied(Vec<BridgeEndpoint>),
    /// Anything else — bound to another member, or no provider at all.
    Elsewhere,
}

impl OperationProviders {
    /// Whether `holder` provides the operation, by this verdict.
    fn provided_by(&self, holder: &str) -> bool {
        match self {
            Self::Holder => true,
            Self::Tied(candidates) => candidates.iter().any(|c| c.member == holder),
            Self::Elsewhere => false,
        }
    }
}

/// One `ApiOperation` a member holds, as the coverage tier read it.
#[derive(Debug, Clone)]
pub struct SpecOperation {
    /// The holding member.
    pub member: String,
    /// The operation node's symbol — its descriptors carry the document path.
    pub symbol: LogosSymbol,
    /// Its key, or `None` when its template does not compose.
    pub key: Option<OperationKey>,
    /// The tier's provider verdict for that key (`Elsewhere` when unkeyed).
    pub providers: OperationProviders,
}

/// A member-relative spec document: `(member, document path)`.
pub type DocumentRef = (String, String);

/// Identifies one named external: the `member:path` of the first copy in its
/// group, in `(member, path)` order. Stable while the copies are, and unique
/// where the display name is not.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ExternalId(pub String);

impl ExternalId {
    fn of(member: &str, path: &str) -> Self {
        Self(format!("{member}:{path}"))
    }
}

/// What a vendored document declares a contract to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ContractTarget {
    /// A workspace member, by document identity.
    Member {
        /// The member whose own spec the document is.
        member: String,
        /// That member's matched own-spec document.
        document: String,
        /// Operations of the held document found in the matched one.
        shared: u64,
        /// Operations of the held document — the score's denominator.
        total: u64,
    },
    /// A named external.
    External {
        /// The external's identity.
        external: ExternalId,
        /// Its display name (not unique).
        name: String,
    },
}

impl ContractTarget {
    /// Who the contract is with — the member or the external, without the
    /// per-document score — which is what a pair is counted on.
    pub fn counterparty(&self) -> Counterparty<'_> {
        match self {
            Self::Member { member, .. } => Counterparty::Member(member),
            Self::External { external, .. } => Counterparty::External(external),
        }
    }
}

/// The other end of a declared contract: a member or a named external.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Counterparty<'a> {
    /// A workspace member, by name.
    Member(&'a str),
    /// A named external, by identity.
    External(&'a ExternalId),
}

/// One `declares-contract(holder → target)` fact: a vendored document and what
/// it declares ([ADR-68] point 1).
///
/// [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeclaredContract {
    /// The member holding the document — the declaring consumer.
    pub holder: String,
    /// The held document, member-relative.
    pub document: String,
    /// Always [`VENDORED_SPEC`].
    pub provenance: &'static str,
    /// What it declares a contract to.
    pub target: ContractTarget,
    /// The held document's operation keys — what S-459's external join
    /// compares a composed call against. Not serialized: the document names it.
    #[serde(skip)]
    pub operations: BTreeSet<OperationKey>,
}

/// One copy in a [`NamedExternal`]'s group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalCopy {
    /// The member holding the copy.
    pub member: String,
    /// The copy, member-relative.
    pub document: String,
    /// Its `info.title`, when one was read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// One named external — an API no single member's own spec is identified as,
/// grouped across the copies members hold ([ADR-68] point 1). Usually no member
/// implements it; a document whose identity collided between members falls
/// through to one too ([`IdentityCollision`]). An external is not a member: it has
/// no engine and no [`BridgeEdge`](super::BridgeEdge).
///
/// [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NamedExternal {
    /// Its identity.
    pub id: ExternalId,
    /// Its display name: the most common non-default title across its copies,
    /// else the first copy's file stem.
    pub name: String,
    /// Every copy in the group, in `(member, document)` order.
    pub copies: Vec<ExternalCopy>,
    /// The members declaring a contract to it, sorted.
    pub declared_by: Vec<String>,
    /// The `mock` members standing in for it, sorted — providers, never
    /// consumers ([ADR-68] point 5).
    ///
    /// [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
    pub stand_ins: Vec<String>,
}

/// A vendored document whose best identity score two or more members share:
/// it resolves to **neither** ([ADR-68] point 1, [NFR-RA-05]) and falls through
/// to its named external.
///
/// [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IdentityCollision {
    /// The holder.
    pub holder: String,
    /// The held document.
    pub document: String,
    /// The members at the shared best score, sorted.
    pub members: Vec<String>,
    /// The shared best score's numerator.
    pub shared: u64,
    /// Its denominator — the held document's operations.
    pub total: u64,
}

/// One contract-surface tie resolved by document identity — for the holder's
/// document only. The coverage row stays ambiguous; this is the declared
/// contract's reading of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedTie {
    /// The holder whose document the operation is in.
    pub holder: String,
    /// That document.
    pub document: String,
    /// The operation — the tied row's consumer end.
    pub operation: LogosSymbol,
    /// The tied candidate in the identified member.
    pub provider: BridgeEndpoint,
}

/// Every spec document read, filed into exactly one bucket — the denominator
/// [`DeclaredContractHeadline::declared_contract_pairs`] is stated over
/// ([BR-51], [NFR-CC-04]).
///
/// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DocumentAccounting {
    /// Every spec document read — the sum of the buckets below.
    pub documents: u64,
    /// Held by a member providing ≥ 90 % of its operations: its own spec.
    pub own: u64,
    /// Held by a member providing none of them: each declares one contract.
    pub vendored: u64,
    /// Neither: counts toward nothing.
    pub partial: u64,
    /// No operation with a key: counts toward nothing.
    pub unjudged: u64,
    /// Held by a declared `mock` member: a stand-in copy, declaring nothing.
    pub mock: u64,
    /// Held by a declared `documentation` member: out of the relation.
    pub documentation: u64,
}

/// The relation's own headline, beside its denominator ([BR-51], [BR-57]).
///
/// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [BR-57]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeclaredContractHeadline {
    /// Distinct ordered `(holder, target)` pairs.
    pub declared_contract_pairs: u64,
    /// Of those, pairs to a member by document identity.
    pub to_member: u64,
    /// Of those, pairs to a named external.
    pub to_external: u64,
    /// The denominator: every spec document read, by bucket.
    pub documents: DocumentAccounting,
    /// Named externals in the registry.
    pub named_externals: u64,
    /// Vendored documents whose identity tied between members.
    pub identity_collisions: u64,
    /// Contract-surface ties resolved by document identity.
    pub resolved_ties: u64,
    /// The headline, its denominator and what it is not, as one line.
    pub summary: String,
}

/// The whole relation: the headline, every declared contract, the named-external
/// registry, the identity collisions and the ties document identity resolves
/// ([FR-WS-31]).
///
/// The callable API S-459 joins against and S-461 renders:
/// [`contracts_of`](Self::contracts_of),
/// [`externals_declared_by`](Self::externals_declared_by),
/// [`external`](Self::external), [`resolved_tie`](Self::resolved_tie) and
/// [`pairs`](Self::pairs).
///
/// [FR-WS-31]: ../../../docs/specs/requirements/FR-WS-31.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeclaredContractRelation {
    /// The headline.
    pub headline: DeclaredContractHeadline,
    /// One per vendored document, in `(holder, document)` order.
    pub contracts: Vec<DeclaredContract>,
    /// Every external declared by or stood in for by a member, in id order.
    pub externals: Vec<NamedExternal>,
    /// Every identity collision, in `(holder, document)` order.
    pub collisions: Vec<IdentityCollision>,
    /// Every resolved tie, in `(holder, document, operation)` order.
    pub resolved_ties: Vec<ResolvedTie>,
}

impl DeclaredContractRelation {
    /// The distinct ordered `(holder, target)` pairs the headline counts — the
    /// same set [`derive()`] counts, so the two cannot disagree.
    pub fn pairs(&self) -> BTreeSet<(&str, Counterparty<'_>)> {
        pairs_of(&self.contracts)
    }

    /// Every contract `holder` declares, in document order.
    pub fn contracts_of<'a>(&'a self, holder: &'a str) -> impl Iterator<Item = &'a DeclaredContract> {
        self.contracts.iter().filter(move |c| c.holder == holder)
    }

    /// The external `id` identifies, if it is in the registry.
    pub fn external(&self, id: &ExternalId) -> Option<&NamedExternal> {
        self.externals.iter().find(|e| &e.id == id)
    }

    /// Every external `member` itself declares, each with the declaring
    /// contract (whose [`operations`](DeclaredContract::operations) are the
    /// member's own copy's) — the join S-459 binds a no-provider call against.
    pub fn externals_declared_by<'a>(
        &'a self,
        member: &'a str,
    ) -> impl Iterator<Item = (&'a DeclaredContract, &'a NamedExternal)> {
        self.contracts_of(member).filter_map(|c| match &c.target {
            ContractTarget::External { external, .. } => self.external(external).map(|e| (c, e)),
            ContractTarget::Member { .. } => None,
        })
    }

    /// The tie resolved for `holder`'s operation `operation`, if one was.
    pub fn resolved_tie(&self, holder: &str, operation: &LogosSymbol) -> Option<&ResolvedTie> {
        self.resolved_ties.iter().find(|t| t.holder == holder && &t.operation == operation)
    }

    /// Whether the relation is empty of every fact a surface would render: no
    /// contract, no external, no collision.
    pub fn is_empty(&self) -> bool {
        self.contracts.is_empty() && self.externals.is_empty() && self.collisions.is_empty()
    }
}

/// How a holder's kind places its documents in the relation ([ADR-68] point 5).
///
/// [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// An ordinary member: it may declare a contract and be identified.
    Consumer,
    /// A `mock`: its copies join the external it stands in for; it declares
    /// nothing and is never identified.
    StandIn,
    /// A `documentation` member: its copies stay out of the relation.
    Out,
}

impl Standing {
    /// Asked of the kind, exhaustively, so a new kind fails to compile here
    /// until it has an answer.
    fn of(kind: Option<MemberKind>) -> Self {
        match kind {
            None | Some(MemberKind::Platform) => Self::Consumer,
            Some(MemberKind::Mock) => Self::StandIn,
            Some(MemberKind::Documentation) => Self::Out,
        }
    }
}

/// How far a holder implements a document it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Implementation {
    Own,
    Vendored,
    Partial,
    Unjudged,
}

fn implementation(provided: usize, keyed: usize) -> Implementation {
    if keyed == 0 {
        Implementation::Unjudged
    } else if provided * 100 >= keyed * IDENTITY_THRESHOLD_PERCENT {
        Implementation::Own
    } else if provided == 0 {
        Implementation::Vendored
    } else {
        Implementation::Partial
    }
}

/// One document a member holds, assembled from its operations.
#[derive(Debug, Default)]
struct Document {
    member: String,
    path: String,
    keys: BTreeSet<OperationKey>,
    keyed_ops: usize,
    provided: usize,
    /// `(operation, tied candidates)` for each tied keyed operation.
    ties: Vec<(LogosSymbol, Vec<BridgeEndpoint>)>,
}

impl Document {
    fn implementation(&self) -> Implementation {
        implementation(self.provided, self.keyed_ops)
    }
}

/// The member-relative path of the file a SCIP symbol is declared in — its
/// descriptors up to the last `/` outside a backtick-quoted name, unquoted.
///
/// `logos . . . api/openapi/`v1.yaml`/v1-users#get#` → `api/openapi/v1.yaml`.
/// `None` for a symbol with no descriptor path (a `local` symbol).
pub fn document_path(symbol: &LogosSymbol) -> Option<String> {
    let descriptors = symbol.as_str().splitn(5, ' ').nth(4)?;
    let mut quoted = false;
    let mut last_slash = None;
    for (i, c) in descriptors.char_indices() {
        match c {
            '`' => quoted = !quoted,
            '/' if !quoted => last_slash = Some(i),
            _ => {}
        }
    }
    let path: String = descriptors[..last_slash?].chars().filter(|c| *c != '`').collect();
    (!path.is_empty()).then_some(path)
}

/// `(shared, |held|)` — how many of `held`'s keys `other` holds.
fn score(held: &BTreeSet<OperationKey>, other: &BTreeSet<OperationKey>) -> (usize, usize) {
    (held.intersection(other).count(), held.len())
}

/// The identity threshold, in integers.
fn meets_identity((shared, total): (usize, usize)) -> bool {
    total > 0 && shared * 100 >= total * IDENTITY_THRESHOLD_PERCENT
}

/// The outcome of matching a vendored document against the own specs.
#[derive(Debug, PartialEq, Eq)]
enum Identity {
    Member { member: String, path: String, shared: usize, total: usize },
    Collision { members: Vec<String>, shared: usize, total: usize },
    NoMember,
}

/// Match `held` against every other identifiable member's own specs: the best
/// spec per member at ≥ the threshold, then the best member — one, or a
/// collision.
fn resolve_identity(held: &Document, own_specs: &[&Document]) -> Identity {
    let mut best: BTreeMap<&str, (usize, &str)> = BTreeMap::new();
    for spec in own_specs.iter().filter(|s| s.member != held.member) {
        let s = score(&held.keys, &spec.keys);
        if !meets_identity(s) {
            continue;
        }
        let entry = best.entry(spec.member.as_str()).or_insert((s.0, spec.path.as_str()));
        if s.0 > entry.0 {
            *entry = (s.0, spec.path.as_str());
        }
    }
    let Some(top) = best.values().map(|(shared, _)| *shared).max() else {
        return Identity::NoMember;
    };
    let winners: Vec<(&str, &str)> = best
        .iter()
        .filter(|(_, (shared, _))| *shared == top)
        .map(|(m, (_, p))| (*m, *p))
        .collect();
    let total = held.keys.len();
    match winners.as_slice() {
        [(member, path)] => Identity::Member {
            member: (*member).to_string(),
            path: (*path).to_string(),
            shared: top,
            total,
        },
        _ => Identity::Collision {
            members: winners.iter().map(|(m, _)| (*m).to_string()).collect(),
            shared: top,
            total,
        },
    }
}

/// Connected components of the copy graph: two documents are joined when
/// either holds ≥ the threshold of the other's keys. Each document's component
/// is the smallest index in it.
fn copy_components(keysets: &[&BTreeSet<OperationKey>]) -> Vec<usize> {
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let contained = |a: &BTreeSet<OperationKey>, b: &BTreeSet<OperationKey>| meets_identity(score(a, b));
    let mut parent: Vec<usize> = (0..keysets.len()).collect();
    for i in 0..keysets.len() {
        for j in (i + 1)..keysets.len() {
            if contained(keysets[i], keysets[j]) || contained(keysets[j], keysets[i]) {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    (0..keysets.len()).map(|i| find(&mut parent, i)).collect()
}

/// An external's display name: the most common title across `copies` that is
/// neither empty nor [`SPRINGDOC_DEFAULT_TITLE`] (ties to the smallest), else
/// the file stem of the first copy in path order.
pub fn external_name(copies: &[(&str, Option<&str>)]) -> String {
    let mut titles: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, title) in copies {
        if let Some(t) = title.filter(|t| !t.is_empty() && *t != SPRINGDOC_DEFAULT_TITLE) {
            *titles.entry(t).or_default() += 1;
        }
    }
    if let Some(top) = titles.values().max().copied() {
        if let Some((t, _)) = titles.iter().find(|(_, n)| **n == top) {
            return (*t).to_string();
        }
    }
    let mut paths: Vec<&str> = copies.iter().map(|(p, _)| *p).collect();
    paths.sort_unstable();
    let first = paths.first().copied().unwrap_or("");
    let file = first.rsplit('/').next().unwrap_or(first);
    file.rsplit_once('.').map_or(file, |(stem, _)| stem).to_string()
}

/// The `info.title` of a JSON or YAML spec, or `None` when there is none. A
/// label, never an identity.
///
/// Read through parsers this crate already ships, never a substring scan: a
/// JSON document through `serde_json`, anything else through the configuration
/// corpus's YAML scalar subset ([`parse_yaml`]), so `title` is only ever the
/// `title` key directly under `info` — never a word inside a description that
/// happens to precede it (Swagger 2's key order puts `description` first).
/// Forms that subset deliberately does not bind — a flow mapping
/// (`info: {title: …}`) or a block scalar (`title: >-`) — yield `None`, and the
/// external falls back to its file stem rather than to a partial value.
///
/// [`parse_yaml`]: crate::extract::config::corpus::parse_yaml
pub fn spec_title(text: &str) -> Option<String> {
    let title = if text.trim_start().starts_with('{') {
        let json: serde_json::Value = serde_json::from_str(text).ok()?;
        json.get("info")?.get("title")?.as_str()?.to_string()
    } else {
        crate::extract::config::corpus::parse_yaml(text).remove("info.title")?.pop_first()?
    };
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_string())
}

/// The `info.title` of `document` under `member_root`, or `None` when the path
/// is not a plain relative one, the file cannot be read, or it has no title.
///
/// The path comes from the member's own graph, but it is joined onto a
/// filesystem root, so anything but `Normal` components (`..`, a root, a
/// prefix) is refused rather than followed.
pub fn read_title(member_root: &Path, document: &str) -> Option<String> {
    let relative = Path::new(document);
    if !relative.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    std::fs::read_to_string(member_root.join(relative)).ok().as_deref().and_then(spec_title)
}

/// Assemble the members' operations into documents, in `(member, path)` order.
/// An operation whose symbol names no file is not in any document.
fn documents(operations: &[SpecOperation]) -> Vec<Document> {
    let mut by_doc: BTreeMap<DocumentRef, Document> = BTreeMap::new();
    for op in operations {
        let Some(path) = document_path(&op.symbol) else { continue };
        let doc = by_doc.entry((op.member.clone(), path.clone())).or_insert_with(|| Document {
            member: op.member.clone(),
            path,
            ..Document::default()
        });
        let Some(key) = &op.key else { continue };
        doc.keyed_ops += 1;
        doc.keys.insert(key.clone());
        if op.providers.provided_by(&op.member) {
            doc.provided += 1;
        }
        if let OperationProviders::Tied(candidates) = &op.providers {
            doc.ties.push((op.symbol.clone(), candidates.clone()));
        }
    }
    by_doc.into_values().collect()
}

/// Derive the relation from member-local facts ([FR-WS-31]). Pure: no I/O,
/// deterministic output.
///
/// `operations` is every `ApiOperation` of every member read, with the coverage
/// tier's provider verdict; `kinds` the resolved member kinds; `title` answers
/// a copy's `info.title` — it is asked only of the documents the external
/// grouping names, never of an own spec.
///
/// [FR-WS-31]: ../../../docs/specs/requirements/FR-WS-31.md
pub fn derive(
    operations: &[SpecOperation],
    kinds: &BTreeMap<String, MemberKind>,
    mut title: impl FnMut(&str, &str) -> Option<String>,
) -> DeclaredContractRelation {
    let docs = documents(operations);
    let standing = |member: &str| Standing::of(kinds.get(member).copied());
    let accounting = account(&docs, standing);

    // The members a document can identify: an ordinary member's own specs.
    let own: Vec<&Document> = docs
        .iter()
        .filter(|d| standing(&d.member) == Standing::Consumer && d.implementation() == Implementation::Own)
        .collect();
    // The copy graph: every ordinary member's document that is not its own
    // spec, plus EVERY copy a mock holds — a mock whose routes serve its copy
    // still stands in for the external it mocks ([ADR-68] point 5).
    let pool: Vec<&Document> = docs
        .iter()
        .filter(|d| match standing(&d.member) {
            Standing::Consumer => d.implementation() != Implementation::Own,
            Standing::StandIn => true,
            Standing::Out => false,
        })
        .collect();
    let keysets: Vec<&BTreeSet<OperationKey>> = pool.iter().map(|d| &d.keys).collect();
    let component = copy_components(&keysets);
    let groups = name_groups(&pool, &component, &mut title);

    let mut declared = Declarations::default();
    for (d, group) in pool.iter().zip(&component) {
        match standing(&d.member) {
            Standing::StandIn => declared.stand_in(*group, d),
            Standing::Out => {}
            Standing::Consumer if d.implementation() == Implementation::Vendored => {
                declared.declare(d, *group, &own, &groups);
            }
            Standing::Consumer => {}
        }
    }
    declared.finish(accounting, groups)
}

/// File every document into exactly one [`DocumentAccounting`] bucket.
fn account(docs: &[Document], standing: impl Fn(&str) -> Standing) -> DocumentAccounting {
    let mut accounting = DocumentAccounting { documents: docs.len() as u64, ..Default::default() };
    for d in docs {
        let bucket = match (standing(&d.member), d.implementation()) {
            (Standing::Out, _) => &mut accounting.documentation,
            (Standing::StandIn, _) => &mut accounting.mock,
            (Standing::Consumer, Implementation::Own) => &mut accounting.own,
            (Standing::Consumer, Implementation::Vendored) => &mut accounting.vendored,
            (Standing::Consumer, Implementation::Partial) => &mut accounting.partial,
            (Standing::Consumer, Implementation::Unjudged) => &mut accounting.unjudged,
        };
        *bucket += 1;
    }
    accounting
}

/// One copy group's identity, display name and copies.
type NamedGroup = (ExternalId, String, Vec<ExternalCopy>);

/// Name every copy group, whether or not anyone declares it yet, keyed by its
/// component. `pool` is in `(member, path)` order, so a group's first copy is
/// its smallest and names its [`ExternalId`].
fn name_groups(
    pool: &[&Document],
    component: &[usize],
    title: &mut impl FnMut(&str, &str) -> Option<String>,
) -> BTreeMap<usize, NamedGroup> {
    let mut members: BTreeMap<usize, Vec<&Document>> = BTreeMap::new();
    for (d, group) in pool.iter().zip(component) {
        members.entry(*group).or_default().push(d);
    }
    members
        .into_iter()
        .map(|(group, docs)| {
            let copies: Vec<ExternalCopy> = docs
                .iter()
                .map(|d| ExternalCopy {
                    member: d.member.clone(),
                    document: d.path.clone(),
                    title: title(&d.member, &d.path),
                })
                .collect();
            let labels: Vec<(&str, Option<&str>)> =
                copies.iter().map(|c| (c.document.as_str(), c.title.as_deref())).collect();
            let name = external_name(&labels);
            let id = ExternalId::of(&copies[0].member, &copies[0].document);
            (group, (id, name, copies))
        })
        .collect()
}

/// The ties of `d` that document identity resolves to `member`: those in which
/// `member` is **exactly one** of the tied candidates — this document only.
fn resolve_ties<'a>(d: &'a Document, member: &'a str) -> impl Iterator<Item = ResolvedTie> + 'a {
    d.ties.iter().filter_map(move |(operation, candidates)| {
        let mut hits = candidates.iter().filter(|c| c.member == member);
        match (hits.next(), hits.next()) {
            (Some(provider), None) => Some(ResolvedTie {
                holder: d.member.clone(),
                document: d.path.clone(),
                operation: operation.clone(),
                provider: provider.clone(),
            }),
            _ => None,
        }
    })
}

/// The relation as it is accumulated over the copy pool.
#[derive(Default)]
struct Declarations {
    contracts: Vec<DeclaredContract>,
    collisions: Vec<IdentityCollision>,
    resolved_ties: Vec<ResolvedTie>,
    declared_by: BTreeMap<usize, BTreeSet<String>>,
    stand_ins: BTreeMap<usize, BTreeSet<String>>,
}

impl Declarations {
    /// A mock's copy: it stands in for its group and declares nothing.
    fn stand_in(&mut self, group: usize, d: &Document) {
        self.stand_ins.entry(group).or_default().insert(d.member.clone());
    }

    /// A vendored document: a contract to the member document identity names,
    /// else to its group's external (a collision reported on the way).
    fn declare(&mut self, d: &Document, group: usize, own: &[&Document], groups: &BTreeMap<usize, NamedGroup>) {
        let target = match resolve_identity(d, own) {
            Identity::Member { member, path, shared, total } => {
                self.resolved_ties.extend(resolve_ties(d, &member));
                ContractTarget::Member { member, document: path, shared: shared as u64, total: total as u64 }
            }
            identity => {
                if let Identity::Collision { members, shared, total } = identity {
                    self.collisions.push(IdentityCollision {
                        holder: d.member.clone(),
                        document: d.path.clone(),
                        members,
                        shared: shared as u64,
                        total: total as u64,
                    });
                }
                self.declared_by.entry(group).or_default().insert(d.member.clone());
                let (id, name, _) = &groups[&group];
                ContractTarget::External { external: id.clone(), name: name.clone() }
            }
        };
        self.contracts.push(DeclaredContract {
            holder: d.member.clone(),
            document: d.path.clone(),
            provenance: VENDORED_SPEC,
            target,
            operations: d.keys.clone(),
        });
    }

    /// Seal the relation: the registry holds every group someone declares or
    /// stands in for, and the headline is stated over `accounting`.
    fn finish(mut self, accounting: DocumentAccounting, groups: BTreeMap<usize, NamedGroup>) -> DeclaredContractRelation {
        let mut externals: Vec<NamedExternal> = groups
            .into_iter()
            .filter_map(|(group, (id, name, copies))| {
                let declared_by = self.declared_by.remove(&group).unwrap_or_default();
                let stand_ins = self.stand_ins.remove(&group).unwrap_or_default();
                (!declared_by.is_empty() || !stand_ins.is_empty()).then(|| NamedExternal {
                    id,
                    name,
                    copies,
                    declared_by: declared_by.into_iter().collect(),
                    stand_ins: stand_ins.into_iter().collect(),
                })
            })
            .collect();
        externals.sort_by(|a, b| a.id.cmp(&b.id));
        self.resolved_ties.sort_by(|a, b| {
            (&a.holder, &a.document, &a.operation).cmp(&(&b.holder, &b.document, &b.operation))
        });
        let headline = headline(accounting, &self.contracts, externals.len(), self.collisions.len(), self.resolved_ties.len());
        DeclaredContractRelation {
            headline,
            contracts: self.contracts,
            externals,
            collisions: self.collisions,
            resolved_ties: self.resolved_ties,
        }
    }
}

/// The headline over the sealed relation's counts.
fn headline(
    documents: DocumentAccounting,
    contracts: &[DeclaredContract],
    named_externals: usize,
    identity_collisions: usize,
    resolved_ties: usize,
) -> DeclaredContractHeadline {
    let pairs = pairs_of(contracts);
    let declared_contract_pairs = pairs.len() as u64;
    let to_member = pairs.iter().filter(|(_, c)| matches!(c, Counterparty::Member(_))).count() as u64;
    let mut headline = DeclaredContractHeadline {
        declared_contract_pairs,
        to_member,
        to_external: declared_contract_pairs - to_member,
        documents,
        named_externals: named_externals as u64,
        identity_collisions: identity_collisions as u64,
        resolved_ties: resolved_ties as u64,
        summary: String::new(),
    };
    headline.summary = summarize(&headline);
    headline
}

/// Distinct ordered `(holder, target)` pairs: a holder vendoring two copies of
/// one external declares one pair.
fn pairs_of(contracts: &[DeclaredContract]) -> BTreeSet<(&str, Counterparty<'_>)> {
    contracts.iter().map(|c| (c.holder.as_str(), c.target.counterparty())).collect()
}

/// The headline as one line, never bare: the pairs, their split, the documents
/// they were derived from over every document read, and what they are not
/// ([BR-51], [BR-57]).
///
/// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [BR-57]: ../../../docs/specs/software-spec.md#327-workspace-federation
fn summarize(h: &DeclaredContractHeadline) -> String {
    format!(
        "{} declared contract pairs ({} by document identity, {} to named externals) from {} \
         vendored of {} spec documents; {} named externals; {} contract-surface ties resolved by \
         document identity; declared by vendored specs, never observed calls",
        h.declared_contract_pairs,
        h.to_member,
        h.to_external,
        h.documents.vendored,
        h.documents.documents,
        h.named_externals,
        h.resolved_ties,
    )
}

#[cfg(test)]
mod tests;
