//! The workspace **cross-member type-reference overlay** — an unresolved import
//! of a type exactly one other member declares binds, in memory and advisory,
//! to that member's type ([FR-WS-35], [ADR-70], [BR-60]).
//!
//! Each member's indexer records the fully-qualified types it declares, from
//! Java/Kotlin source and from Avro schemas (S-472, [ADR-70] point 1). This
//! module builds, **in memory** ([ADR-52]), an index from fully-qualified name
//! to the `(member, LogosSymbol)` declaring it, and matches every member's
//! still-unresolved `Imports`/`TypeUses` rows of a `.java`/`.kt` file against
//! it. Nothing is persisted and no member database is `ATTACH`-ed; the facts
//! are read through each member's own engine.
//!
//! # Never a coupling ([BR-60], [ADR-53])
//! A [`TypeReference`] is a read-model of its own, with its own headline
//! (`type_reference_pairs`) and its own cache. It is never a
//! [`BridgeEdge`](super::BridgeEdge), and nothing in the [`bridge`](super::bridge),
//! [`coverage`](super::coverage) or [`build_deps`](super::build_deps) reads
//! anything defined here, so `resolved_cross_service_edges`,
//! `egress_resolution`, the bridge edge set and the build headline cannot move
//! with it. It is never a gate input: `scan`/`gate`/`check_rules` run on one
//! engine and never construct a registry.
//!
//! # The match ([NFR-RA-05])
//! A row's target is read with `::` as `.` (the ledger spells an import
//! `com::x::Svc`, a fact `com.x.Svc`). It names a type **exactly**, or else by
//! its **enclosing** name one level up — a static-member import or a nested
//! type — the rule the [S-471] gate measured. Every row considered is filed in
//! exactly one bucket of [`TypeRowAccounting`]:
//!
//! - **unqualified** — the target names no package (a bare type use such as
//!   `Dto`): never looked up, since no owned name is dotless, and never read
//!   as "no member declares it" — the type may well be another member's, named
//!   by the file's own import row;
//! - **no owner** — no member declares the name (a JDK, Spring or third-party
//!   type);
//! - **self-owned** — the importing member is among the owners: intra-member,
//!   out of this tier (the member's own resolution is its business);
//! - **ambiguous owner** — two or more other members declare it: unbound, the
//!   owners named;
//! - exactly one other owner, then **by the pair restriction** ([ADR-70]
//!   point 3):
//!   - **bound** — the build relation ([FR-WS-33]) relates the importer to the
//!     owner (any kind, into a declared platform included), or the importer
//!     references a colliding artifact the owner is one of the producers of
//!     (the artifact named);
//!   - **type-only** — it does neither: counted and listed, never bound, never
//!     dropped;
//!   - **pair unread** — the build relation could not judge the pair, because
//!     one of the two members' build facts are unread. Never read as
//!     "unrelated".
//!
//! # Owners
//! A type is owned by the member declaring it in a **main** source tree or in
//! an Avro schema. A test-tree declaration is never an owner: a test class is
//! not on another member's classpath, and a type declared in both trees of one
//! member is that member's main type. Nor is a default-package declaration (a
//! dotless name): no named package can import it. A refused fact names no type.
//!
//! # Unread is never "declares nothing" ([NFR-CC-04])
//! A member is **read** only when its store marks its declared types extracted
//! ([`crate::graph_store::DECLARED_TYPES_EXTRACTED_KEY`]). Every other roster
//! member is named in [`TypeMembersRead::unread`] with its reason, and
//! contributes neither types nor rows: every exactly-one claim is stated over
//! the members read.
//!
//! # Built on first query, never at startup ([ADR-52], [NFR-PE-10])
//! [`TypeReferences`] holds the index behind a cache keyed on the members'
//! sync-stamps, empty until the first [`TypeReferences::index`] call.
//! `workspace status` builds its section from the facts its freshness walk
//! already read, adding no walk.
//!
//! [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
//! [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
//! [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
//! [ADR-70]: ../../../docs/specs/architecture/decisions/ADR-70.md
//! [BR-60]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [S-471]: ../../../docs/planning/journal.md#s-471-measure-cross-member-type-references-over-the-reference-estate

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::Serialize;

use crate::graph_store::{AvroSchemaRow, DeclaredTypeRow, TypeRefRow};
use crate::model::EdgeKind;

use super::bridge::{current_stamps, read_members, MemberContracts, StampCache};
use super::build_deps::{BuildDependencies, BuildDependencyRelation};
use super::registry::{EngineRegistry, MemberEngine};
use super::Member;

/// One member's declared-type facts and its unresolved type references, as
/// read through its engine — what [`build_index`] takes, one per member read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemberTypeFacts {
    /// Every declared-type fact, source and Avro, refused ones included.
    pub declared: Vec<DeclaredTypeRow>,
    /// Every Avro schema found, read or not — the schema denominator.
    pub schemas: Vec<AvroSchemaRow>,
    /// The still-unresolved `Imports`/`TypeUses` rows of its `.java`/`.kt`
    /// files.
    pub rows: Vec<TypeRefRow>,
}

impl MemberTypeFacts {
    /// Whether this member holds any Java/Kotlin/Avro evidence at all: a
    /// declared fact (of any tree or resolution), a schema, or a row.
    fn is_java_kotlin_avro(&self) -> bool {
        !(self.declared.is_empty() && self.schemas.is_empty() && self.rows.is_empty())
    }
}

/// One member's facts as read: its name and its [`MemberTypeFacts`].
pub type MemberTypeFactsRead = (String, MemberTypeFacts);

/// The [`unread`](TypeMembersRead::unread) reason of a member whose store does
/// not mark its declared types extracted: upgraded across migration 24 and not
/// yet fully re-read, or never indexed. Its empty tables say nothing about the
/// types it declares.
pub const UNREAD_NOT_EXTRACTED: &str = "declared types not yet extracted";

/// The [`unread`](TypeMembersRead::unread) reason of a member whose engine did
/// not open or whose read failed ([ADR-53]); the cause is in the log line.
///
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
pub const UNREAD_FAILED: &str = "declared types could not be read";

/// The reason an [`AmbiguousReference`] stays unbound.
pub const AMBIGUOUS_OWNER: &str = "ambiguous-owner";

/// Sort one member's read into [`build_index`]'s two inputs: its facts onto
/// `facts` when its store marks them extracted, its name onto `not_extracted`
/// when it does not. Shared by both read paths — the lazy index and `workspace
/// status`'s freshness walk — so they cannot disagree about which member is
/// read.
pub(super) fn sort_read(
    member: String,
    read: Option<MemberTypeFacts>,
    facts: &mut Vec<MemberTypeFactsRead>,
    not_extracted: &mut Vec<String>,
) {
    match read {
        Some(read) => facts.push((member, read)),
        None => not_extracted.push(member),
    }
}

/// Where an owning type is declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TypeOrigin {
    /// A main-tree Java/Kotlin source file.
    Source,
    /// An `.avsc` record or enum.
    Avro,
}

impl TypeOrigin {
    /// The fact vocabulary's `origin` token this origin reads, or `None`.
    fn from_fact(origin: &str) -> Option<Self> {
        match origin {
            "source" => Some(Self::Source),
            "avro" => Some(Self::Avro),
            _ => None,
        }
    }
}

/// The member declaring a type, with the declaration's provenance.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct TypeOwner {
    /// The declaring member.
    pub member: String,
    /// Source or Avro.
    pub origin: TypeOrigin,
    /// The declaring file or schema, member-relative.
    pub declared_in: String,
    /// The declaring node's `LogosSymbol`; absent for an Avro type, which has
    /// no node in any member store.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// `class`, `interface`, `enum` or `record`.
    pub kind: String,
}

/// The importing end of a reference: the ledger row's declaration and site.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct TypeImporter {
    /// The importing member.
    pub member: String,
    /// The importing file, member-relative.
    pub file: String,
    /// 1-based line of the import, when the ledger knows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    /// The referencing declaration's `LogosSymbol`.
    pub symbol: String,
}

/// Which ledger form the row was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TypeRefForm {
    /// An `Imports` row.
    Import,
    /// A `TypeUses` row.
    TypeUse,
}

/// How a row's target reached the type it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TypeNaming {
    /// The target is the type's fully-qualified name.
    Exact,
    /// The target names a member of the type one level down — a static member
    /// or a nested type.
    Enclosing,
}

/// What admits (or does not admit) an exactly-one match's member pair
/// ([ADR-70] point 3).
///
/// [ADR-70]: ../../../docs/specs/architecture/decisions/ADR-70.md
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "via", rename_all = "kebab-case")]
pub enum PairEvidence {
    /// A `builds-against(importer → owner)` edge of any kind.
    Build {
        /// The owner is a declared `platform` member: its inbound build edges
        /// are counted apart from the build headline. Absent when `false`.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        platform: bool,
    },
    /// The importer references a colliding artifact one of whose producers is
    /// the owner — the build relation resolves the coordinate to neither, the
    /// type names the one.
    Collision {
        /// The colliding coordinates, `groupId:artifactId`, sorted.
        artifacts: Vec<String>,
    },
    /// No build edge and no collision relates the two: never bound.
    TypeOnly,
    /// One of the two members' build facts are unread, so the pair is not
    /// judged: never bound, never read as unrelated.
    PairUnread,
}

impl PairEvidence {
    /// Whether this evidence admits the match into the bound tier.
    #[must_use]
    pub fn admits(&self) -> bool {
        matches!(self, Self::Build { .. } | Self::Collision { .. })
    }
}

/// One exactly-one match ([FR-WS-35]): an importer bound — or, with
/// [`PairEvidence::TypeOnly`], merely matched — to the one other member
/// declaring the type. **Never a bridge edge** ([BR-60]).
///
/// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
/// [BR-60]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeReference {
    /// The type named, dotted.
    pub fqn: String,
    /// Exactly, or through its enclosing name.
    pub naming: TypeNaming,
    /// The row's form.
    pub form: TypeRefForm,
    /// Who imports it, where.
    pub importer: TypeImporter,
    /// Who declares it, where.
    pub owner: TypeOwner,
    /// What admits the pair.
    pub evidence: PairEvidence,
}

/// One row whose type two or more other members declare: unbound, reason
/// [`AMBIGUOUS_OWNER`], the owners named ([NFR-RA-05]).
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AmbiguousReference {
    /// The type named, dotted.
    pub fqn: String,
    /// Exactly, or through its enclosing name.
    pub naming: TypeNaming,
    /// The row's form.
    pub form: TypeRefForm,
    /// Who imports it, where.
    pub importer: TypeImporter,
    /// The declaring members, sorted.
    pub owners: Vec<String>,
    /// Always [`AMBIGUOUS_OWNER`].
    pub reason: &'static str,
}

/// Every row considered, filed into exactly one bucket — the denominator
/// `type_reference_pairs` is stated over ([BR-51], [NFR-CC-04]). The buckets
/// sum to [`considered`](Self::considered); the module docs define each one.
///
/// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TypeRowAccounting {
    /// Every still-unresolved `Imports`/`TypeUses` row of a `.java`/`.kt` file
    /// in the members read.
    pub considered: u64,
    /// Of those, `Imports` rows.
    pub imports: u64,
    /// Of those, `TypeUses` rows.
    pub type_uses: u64,
    /// Exactly one other owner, the pair admitted: a [`TypeReference`].
    pub bound: u64,
    /// Exactly one other owner, the pair related by nothing the build relation
    /// holds.
    pub type_only: u64,
    /// Exactly one other owner, the pair not judged: a member's build facts are
    /// unread.
    pub pair_unread: u64,
    /// Two or more other owners.
    pub ambiguous_owner: u64,
    /// The importing member is an owner: out of this tier.
    pub self_owned: u64,
    /// The target names no package (a bare type use, `Dto`): never looked up,
    /// so never claimed to have no owner.
    pub unqualified: u64,
    /// No member declares the name.
    pub no_owner: u64,
}

/// How much of the workspace the index was built over ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TypeMembersRead {
    /// The roster size.
    pub members: u64,
    /// Members whose declared types were read.
    pub read: u64,
    /// Roster members whose facts were not read, by name, in roster order.
    /// Absent when none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unread: Vec<String>,
    /// Why each [`unread`](Self::unread) member was not read:
    /// [`UNREAD_NOT_EXTRACTED`] or [`UNREAD_FAILED`]. Absent when none.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub unread_reasons: BTreeMap<String, &'static str>,
    /// Members read holding any Java/Kotlin/Avro evidence: a declared type, a
    /// schema or a row considered.
    pub java_kotlin_avro: u64,
    /// Declarations admitted as owners: main-tree source and Avro.
    pub owned_declarations: u64,
    /// Test-tree source declarations, never owners.
    pub test_tree_declarations: u64,
    /// Declarations in the default package (a source file with no `package`,
    /// an Avro type with no namespace): a dotless name no named package can
    /// import, so never an owner.
    pub default_package_declarations: u64,
    /// Refused facts (a `package` disagreeing with its directory, a malformed
    /// schema): they name no type.
    pub refused_declarations: u64,
    /// Avro schemas found across the members read.
    pub schemas: u64,
    /// Of those, the schemas read (`status = read`).
    pub schemas_read: u64,
}

/// One pair bound only through a build collision, with the artifacts named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CollisionBackedPair {
    /// The importing member.
    pub from: String,
    /// The owning member.
    pub to: String,
    /// The colliding coordinates admitting it, sorted.
    pub artifacts: Vec<String>,
    /// Rows bound across the pair.
    pub references: u64,
}

/// One pair of type-only matches, listed — never bound, never dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeOnlyPair {
    /// The importing member.
    pub from: String,
    /// The owning member.
    pub to: String,
    /// The types matched, sorted.
    pub types: Vec<String>,
    /// Rows matched across the pair.
    pub references: u64,
}

/// One type two or more members declare, with the rows naming it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AmbiguousType {
    /// The type, dotted.
    pub fqn: String,
    /// The declaring members, sorted.
    pub owners: Vec<String>,
    /// Rows naming it, from members that are not among its owners.
    pub references: u64,
}

/// The `type_reference` headline ([FR-WS-35], [BR-51], [BR-60]): the bound
/// member pairs beside every row considered, and the members read.
///
/// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
/// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [BR-60]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeReferenceHeadline {
    /// Distinct directed `(importer, owner)` member pairs with a bound
    /// reference.
    pub type_reference_pairs: u64,
    /// Of those, pairs the build relation relates.
    pub build_pairs: u64,
    /// Of those, pairs admitted only through a build collision.
    pub collision_backed_pairs: u64,
    /// Distinct `(importer, owner, type)` triples bound.
    pub triples: u64,
    /// The denominator: every row considered, by bucket.
    pub rows: TypeRowAccounting,
    /// The members the index was built over.
    pub members: TypeMembersRead,
    /// Collision-backed pairs, the artifacts named, sorted by pair.
    pub collision_backed: Vec<CollisionBackedPair>,
    /// Type-only pairs, listed, sorted by pair.
    pub type_only: Vec<TypeOnlyPair>,
    /// Types with several owners, sorted by name.
    pub ambiguous_owner: Vec<AmbiguousType>,
    /// The headline, its denominator and what is apart, as one line.
    pub summary: String,
}

/// The whole overlay: every match beside its headline, and the FQN index it
/// was matched against — the API [S-474]'s `xservice type-refs` and the
/// `callers`/`impact` stitching read.
///
/// [S-474]: ../../../docs/planning/journal.md#s-474-cross-member-type-references-on-the-cli-mcp-and-the-xservice-queries
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeReferenceIndex {
    /// Bound references, sorted by `(owner member, type, importer)`.
    pub references: Vec<TypeReference>,
    /// Type-only matches, in the same order.
    pub type_only: Vec<TypeReference>,
    /// Pair-unread matches, in the same order.
    pub pair_unread: Vec<TypeReference>,
    /// Ambiguous rows, sorted by `(type, importer)`.
    pub ambiguous: Vec<AmbiguousReference>,
    /// The headline.
    pub headline: TypeReferenceHeadline,
    /// Fully-qualified name → its owners, one per member — the index itself.
    #[serde(skip)]
    owners: BTreeMap<String, Vec<TypeOwner>>,
    /// The members read, in roster order.
    #[serde(skip)]
    roster_read: Vec<String>,
}

/// One member's side of the overlay: the types it imports from others, and its
/// types others import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberTypeReferences {
    /// The member.
    pub member: String,
    /// Bound references whose importer is this member.
    pub imports: Vec<TypeReference>,
    /// Bound references whose owner is this member.
    pub imported_by: Vec<TypeReference>,
    /// Type-only matches with this member at either end.
    pub type_only: Vec<TypeReference>,
}

impl TypeReferenceIndex {
    /// The members declaring `fqn` (dotted), one entry each, sorted — empty
    /// when none does.
    #[must_use]
    pub fn owners(&self, fqn: &str) -> &[TypeOwner] {
        self.owners.get(fqn).map_or(&[], Vec::as_slice)
    }

    /// Every bound reference naming `fqn`: its importers across members.
    pub fn importers<'a>(&'a self, fqn: &'a str) -> impl Iterator<Item = &'a TypeReference> + 'a {
        self.references.iter().filter(move |r| r.fqn == fqn)
    }

    /// Every bound reference whose owner is `symbol` in `member` — the
    /// stitching entry for a cross-member `callers`/`impact` on a type.
    pub fn references_to<'a>(
        &'a self,
        member: &'a str,
        symbol: &'a str,
    ) -> impl Iterator<Item = &'a TypeReference> + 'a {
        self.references
            .iter()
            .filter(move |r| r.owner.member == member && r.owner.symbol.as_deref() == Some(symbol))
    }

    /// `member`'s references in both directions, or `None` when `member` is
    /// not a member the index was built over.
    #[must_use]
    pub fn member(&self, member: &str) -> Option<MemberTypeReferences> {
        self.roster_read.iter().any(|m| m == member).then(|| MemberTypeReferences {
            member: member.to_string(),
            imports: self.references.iter().filter(|r| r.importer.member == member).cloned().collect(),
            imported_by: self.references.iter().filter(|r| r.owner.member == member).cloned().collect(),
            type_only: self
                .type_only
                .iter()
                .filter(|r| r.importer.member == member || r.owner.member == member)
                .cloned()
                .collect(),
        })
    }

    /// Every member read, in roster order, with its references — members with
    /// none included.
    #[must_use]
    pub fn per_member(&self) -> Vec<MemberTypeReferences> {
        self.roster_read.iter().filter_map(|m| self.member(m)).collect()
    }

    /// The `workspace status` section: the headline, or `None` when every
    /// member was read and none is Java/Kotlin/Avro — so such a workspace
    /// serializes exactly as before the overlay existed. A member not read
    /// keeps it present: unread is not "no types" ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[must_use]
    pub fn section(&self) -> Option<TypeReferenceHeadline> {
        let members = &self.headline.members;
        (members.java_kotlin_avro > 0 || !members.unread.is_empty()).then(|| self.headline.clone())
    }
}

/// A row's class before the pair restriction, over the non-empty owner list
/// [`lookup`] found (a name with no owner never reaches it).
enum Match<'a> {
    SelfOwned,
    One(&'a TypeOwner),
    Many(&'a [TypeOwner]),
}

/// The type a target names — exactly, else through its enclosing name one level
/// up — with its owners, or `None` when neither has one.
fn lookup<'a>(
    target: &str,
    owners: &'a BTreeMap<String, Vec<TypeOwner>>,
) -> Option<(String, TypeNaming, &'a [TypeOwner])> {
    if let Some(found) = owners.get(target) {
        return Some((target.to_string(), TypeNaming::Exact, found));
    }
    let (enclosing, _) = target.rsplit_once('.')?;
    owners
        .get(enclosing)
        .map(|found| (enclosing.to_string(), TypeNaming::Enclosing, found.as_slice()))
}

fn classify<'a>(importer: &str, owners: &'a [TypeOwner]) -> Match<'a> {
    match owners {
        _ if owners.iter().any(|o| o.member == importer) => Match::SelfOwned,
        [one] => Match::One(one),
        many => Match::Many(many),
    }
}

/// The pair restriction, read off the build relation ([ADR-70] point 3).
///
/// [ADR-70]: ../../../docs/specs/architecture/decisions/ADR-70.md
struct PairJudge<'a> {
    build: &'a BuildDependencyRelation,
    /// Directed build pairs → whether any of their edges is into a platform.
    pairs: BTreeMap<(&'a str, &'a str), bool>,
}

impl<'a> PairJudge<'a> {
    fn new(build: &'a BuildDependencyRelation) -> Self {
        let mut pairs: BTreeMap<(&str, &str), bool> = BTreeMap::new();
        for edge in &build.edges {
            *pairs.entry((edge.from.as_str(), edge.to.as_str())).or_default() |= edge.platform;
        }
        Self { build, pairs }
    }

    fn judge(&self, from: &str, to: &str) -> PairEvidence {
        if let Some(&platform) = self.pairs.get(&(from, to)) {
            return PairEvidence::Build { platform };
        }
        let artifacts: Vec<String> = self
            .build
            .collisions_referenced_by(from)
            .filter(|c| c.producers.iter().any(|p| p == to))
            .map(|c| c.artifact.clone())
            .collect();
        if !artifacts.is_empty() {
            return PairEvidence::Collision { artifacts };
        }
        let unread = &self.build.headline.members.unread;
        if unread.iter().any(|m| m == from || m == to) {
            PairEvidence::PairUnread
        } else {
            PairEvidence::TypeOnly
        }
    }
}

/// The owners a member's declared facts contribute, one per fully-qualified
/// name — the first declaration in `(origin, path, symbol)` order when the
/// member declares a name more than once — counting every fact into `read`.
fn member_owners(member: &str, declared: &[DeclaredTypeRow], read: &mut TypeMembersRead) -> Vec<(String, TypeOwner)> {
    let mut mine: BTreeMap<String, TypeOwner> = BTreeMap::new();
    for fact in declared {
        let (Some(fqn), Some(origin)) = (fact.fqn.as_deref(), TypeOrigin::from_fact(&fact.origin)) else {
            read.refused_declarations += 1;
            continue;
        };
        if fact.resolution != "resolved" {
            read.refused_declarations += 1;
            continue;
        }
        if origin == TypeOrigin::Source && fact.tree.as_deref() != Some("main") {
            read.test_tree_declarations += 1;
            continue;
        }
        // A dotless name is never a cross-member key: a type in the default
        // package cannot be imported from a named one, and a bare type-use
        // target of the same spelling would otherwise bind to it.
        if !fqn.contains('.') {
            read.default_package_declarations += 1;
            continue;
        }
        read.owned_declarations += 1;
        let owner = TypeOwner {
            member: member.to_string(),
            origin,
            declared_in: fact.path.clone(),
            symbol: fact.symbol.clone(),
            kind: fact.kind.clone(),
        };
        mine.entry(fqn.to_string())
            .and_modify(|kept| {
                if owner < *kept {
                    *kept = owner.clone();
                }
            })
            .or_insert(owner);
    }
    mine.into_iter().collect()
}

/// Build the overlay from member-local facts ([FR-WS-35]).
///
/// `roster` is the workspace's member list (manifest order); `facts` holds one
/// entry per member whose declared types were read, and a roster member absent
/// from it is reported [`unread`](TypeMembersRead::unread) — with
/// [`UNREAD_NOT_EXTRACTED`] when it is named in `not_extracted`, else
/// [`UNREAD_FAILED`]. `build` is the build relation the pair restriction reads.
/// Pure: no I/O, deterministic output.
///
/// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
#[must_use]
pub fn build_index(
    roster: &[Member],
    facts: &[MemberTypeFactsRead],
    not_extracted: &[String],
    build: &BuildDependencyRelation,
) -> TypeReferenceIndex {
    let read: BTreeMap<&str, &MemberTypeFacts> = facts.iter().map(|(m, f)| (m.as_str(), f)).collect();

    let mut members = TypeMembersRead {
        members: roster.len() as u64,
        ..TypeMembersRead::default()
    };
    let mut owners: BTreeMap<String, Vec<TypeOwner>> = BTreeMap::new();
    let mut roster_read = Vec::new();
    for member in roster {
        let Some(member_facts) = read.get(member.name.as_str()) else {
            let reason = if not_extracted.contains(&member.name) {
                UNREAD_NOT_EXTRACTED
            } else {
                UNREAD_FAILED
            };
            members.unread.push(member.name.clone());
            members.unread_reasons.insert(member.name.clone(), reason);
            continue;
        };
        members.read += 1;
        members.java_kotlin_avro += u64::from(member_facts.is_java_kotlin_avro());
        members.schemas += member_facts.schemas.len() as u64;
        members.schemas_read += member_facts.schemas.iter().filter(|s| s.status == "read").count() as u64;
        for (fqn, owner) in member_owners(&member.name, &member_facts.declared, &mut members) {
            owners.entry(fqn).or_default().push(owner);
        }
        roster_read.push(member.name.clone());
    }
    for list in owners.values_mut() {
        list.sort();
    }

    let judge = PairJudge::new(build);
    let mut rows = TypeRowAccounting::default();
    let (mut references, mut type_only, mut pair_unread, mut ambiguous) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for name in &roster_read {
        for row in &read[name.as_str()].rows {
            let form = match row.kind {
                EdgeKind::Imports => TypeRefForm::Import,
                EdgeKind::TypeUses => TypeRefForm::TypeUse,
                // The store read returns only these two kinds; anything else
                // is not a row of this tier, and is not counted.
                _ => continue,
            };
            rows.considered += 1;
            match form {
                TypeRefForm::Import => rows.imports += 1,
                TypeRefForm::TypeUse => rows.type_uses += 1,
            }
            let target = row.target.replace("::", ".");
            if !target.contains('.') {
                rows.unqualified += 1;
                continue;
            }
            let Some((fqn, naming, found)) = lookup(&target, &owners) else {
                rows.no_owner += 1;
                continue;
            };
            let importer = TypeImporter {
                member: name.clone(),
                file: row.path.clone(),
                line: row.line,
                symbol: row.source_symbol.clone(),
            };
            match classify(name, found) {
                Match::SelfOwned => rows.self_owned += 1,
                Match::Many(many) => {
                    rows.ambiguous_owner += 1;
                    ambiguous.push(AmbiguousReference {
                        fqn,
                        naming,
                        form,
                        importer,
                        owners: many.iter().map(|o| o.member.clone()).collect(),
                        reason: AMBIGUOUS_OWNER,
                    });
                }
                Match::One(owner) => {
                    let evidence = judge.judge(name, &owner.member);
                    let reference = TypeReference {
                        fqn,
                        naming,
                        form,
                        importer,
                        owner: owner.clone(),
                        evidence,
                    };
                    match reference.evidence {
                        PairEvidence::Build { .. } | PairEvidence::Collision { .. } => {
                            rows.bound += 1;
                            references.push(reference);
                        }
                        PairEvidence::TypeOnly => {
                            rows.type_only += 1;
                            type_only.push(reference);
                        }
                        PairEvidence::PairUnread => {
                            rows.pair_unread += 1;
                            pair_unread.push(reference);
                        }
                    }
                }
            }
        }
    }
    let order = |r: &TypeReference| {
        (r.owner.member.clone(), r.fqn.clone(), r.importer.clone(), r.form, r.naming)
    };
    for list in [&mut references, &mut type_only, &mut pair_unread] {
        list.sort_by_key(order);
    }
    ambiguous.sort_by(|a, b| (&a.fqn, &a.importer).cmp(&(&b.fqn, &b.importer)));

    let headline = headline(&references, &type_only, &ambiguous, rows, members);
    TypeReferenceIndex {
        references,
        type_only,
        pair_unread,
        ambiguous,
        headline,
        owners,
        roster_read,
    }
}

/// The headline over the sorted matches.
fn headline(
    references: &[TypeReference],
    type_only: &[TypeReference],
    ambiguous: &[AmbiguousReference],
    rows: TypeRowAccounting,
    members: TypeMembersRead,
) -> TypeReferenceHeadline {
    let mut pairs: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut collision: BTreeMap<(&str, &str), (BTreeSet<&str>, u64)> = BTreeMap::new();
    let mut triples: BTreeSet<(&str, &str, &str)> = BTreeSet::new();
    for r in references {
        let pair = (r.importer.member.as_str(), r.owner.member.as_str());
        pairs.insert(pair);
        triples.insert((pair.0, pair.1, r.fqn.as_str()));
        if let PairEvidence::Collision { artifacts } = &r.evidence {
            let entry = collision.entry(pair).or_default();
            entry.0.extend(artifacts.iter().map(String::as_str));
            entry.1 += 1;
        }
    }
    let collision_backed_pairs = collision.len() as u64;
    let type_reference_pairs = pairs.len() as u64;

    let mut only: BTreeMap<(&str, &str), (BTreeSet<&str>, u64)> = BTreeMap::new();
    for r in type_only {
        let entry = only.entry((r.importer.member.as_str(), r.owner.member.as_str())).or_default();
        entry.0.insert(r.fqn.as_str());
        entry.1 += 1;
    }
    let mut by_type: BTreeMap<&str, (&[String], u64)> = BTreeMap::new();
    for a in ambiguous {
        by_type.entry(a.fqn.as_str()).or_insert((a.owners.as_slice(), 0)).1 += 1;
    }

    let build_pairs = type_reference_pairs - collision_backed_pairs;
    // Every bucket is named, so the figures in the parentheses and the bound
    // count sum to the "of N" they are stated over.
    let summary = format!(
        "{type_reference_pairs} member pairs ({build_pairs} build · {collision_backed_pairs} \
         collision-backed) bind {} of {} unresolved Java/Kotlin import and type-use rows to a type \
         another member declares ({} type-only, {} pair unread, {} ambiguous-owner, {} self-owned, \
         {} unqualified, {} no owner in the workspace), over {} of {} members read; an advisory \
         type reference, never a coupling",
        rows.bound,
        rows.considered,
        rows.type_only,
        rows.pair_unread,
        rows.ambiguous_owner,
        rows.self_owned,
        rows.unqualified,
        rows.no_owner,
        members.read,
        members.members,
    );
    let pair_list = |(from, to): (&str, &str)| (from.to_string(), to.to_string());
    TypeReferenceHeadline {
        type_reference_pairs,
        build_pairs,
        collision_backed_pairs,
        triples: triples.len() as u64,
        rows,
        members,
        collision_backed: collision
            .into_iter()
            .map(|(pair, (artifacts, references))| {
                let (from, to) = pair_list(pair);
                CollisionBackedPair {
                    from,
                    to,
                    artifacts: artifacts.into_iter().map(str::to_string).collect(),
                    references,
                }
            })
            .collect(),
        type_only: only
            .into_iter()
            .map(|(pair, (types, references))| {
                let (from, to) = pair_list(pair);
                TypeOnlyPair {
                    from,
                    to,
                    types: types.into_iter().map(str::to_string).collect(),
                    references,
                }
            })
            .collect(),
        ambiguous_owner: by_type
            .into_iter()
            .map(|(fqn, (owners, references))| AmbiguousType {
                fqn: fqn.to_string(),
                owners: owners.to_vec(),
                references,
            })
            .collect(),
        summary,
    }
}

/// The overlay for one workspace, built on the **first** query and cached on
/// the members' sync-stamps ([FR-WS-35], [ADR-52]).
///
/// Empty at construction — building one reads nothing, so holding it on a
/// serve surface costs nothing at startup. One holder per registry.
///
/// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
#[derive(Debug, Default)]
pub struct TypeReferences {
    cache: StampCache<TypeReferenceIndex>,
}

impl TypeReferences {
    /// A holder with nothing built yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The overlay over `registry`'s members: the stamps are read first, and
    /// the facts are read and matched only on a miss — the first call, or any
    /// call after a member re-synced. The pair restriction reads `build`'s
    /// relation, itself cached on the same stamps.
    ///
    /// A member whose engine will not open or whose read fails is skipped with
    /// a warning and named unread, reason [`UNREAD_FAILED`] ([ADR-53]); a
    /// member whose store holds no extracted facts yet is named unread with
    /// [`UNREAD_NOT_EXTRACTED`].
    ///
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    pub fn index<E>(&self, registry: &EngineRegistry<E>, build: &BuildDependencies) -> Arc<TypeReferenceIndex>
    where
        E: MemberEngine + MemberContracts,
    {
        let answer = registry.answer();
        let stamps = current_stamps(&answer);
        self.cache.get_or_compute(stamps, || {
            let (mut facts, mut not_extracted) = (Vec::new(), Vec::new());
            for (member, read) in read_members(&answer, "declared-type facts", |engine| engine.type_facts()) {
                sort_read(member, read, &mut facts, &mut not_extracted);
            }
            let relation = build.relation(registry);
            build_index(&registry.federation().members, &facts, &not_extracted, &relation)
        })
    }
}

#[cfg(test)]
#[path = "type_refs_tests.rs"]
mod tests;
