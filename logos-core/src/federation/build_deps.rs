//! The workspace **build-dependency relation** — `builds-against(A → B, kind,
//! scope, artifact)` joined in memory from member-local build-manifest facts
//! ([FR-WS-33], [ADR-69] points 2–3).
//!
//! Each member's indexer records the Maven/Gradle artifacts it **produces** and
//! **references** ([ADR-69] point 1, `extract::build_manifest`). This module
//! joins them across members: a reference in member A to an artifact that
//! exactly one *other* member B produces is one `builds-against` edge. Nothing
//! is persisted and no member database is `ATTACH`-ed; the facts are read
//! through each member's own engine ([ADR-52]).
//!
//! # Never a runtime coupling ([BR-58], [ADR-26])
//! The relation has its own types, its own headline
//! (`build_dependency_pairs`) and its own cache. It never enters the
//! [`bridge`](super::bridge) matcher, a [`BridgeEdge`](super::BridgeEdge), a
//! provider bucket, `resolved_cross_service_edges` or `egress_resolution`: no
//! function in those modules reads anything defined here. The byte-identity of
//! every runtime figure with and without build facts is pinned by this
//! module's tests.
//!
//! # The join
//! The key is `groupId:artifactId` — the version is not part of it, so a
//! `version-refused` fact (the key resolved, the version did not) joins like a
//! resolved one. A referenced fact is filed into exactly one bucket of
//! [`ReferenceAccounting`], so the edges are always stated over every reference
//! read ([BR-51], [NFR-CC-04]):
//!
//! - **refused** — its key did not resolve (`${…}` left verbatim); never joined;
//! - **project reference** — Gradle `project(':x')`, an in-member project, not
//!   a coordinate;
//! - **build plugin** — Gradle `buildscript` `classpath`, a build-plugin
//!   coordinate rather than a project dependency;
//! - **collision** — two or more members produce the key, so it resolves to
//!   **neither** ([NFR-RA-05]'s exactly-one rule) and the collision is reported;
//! - **in member** — the referencing member produces it itself (a multi-module
//!   build's sibling module);
//! - **external** — no member produces it (a third-party dependency);
//! - **to member** — exactly one other member produces it: an edge;
//! - **to platform** — the same, into a declared `platform` member: an edge
//!   counted apart.
//!
//! # Platform members ([ADR-69] point 3)
//! A member declared `kind = "platform"` has its **inbound** edges counted
//! apart ([`MemberKind::sets_inbound_build_edges_apart`]): they leave the
//! headline for [`PlatformApart`], stated over the same denominator. Logos never
//! declares one: the status lists [`PlatformCandidate`]s by in-degree share and
//! classifies nothing.
//!
//! # Unread is never "no manifests" ([NFR-CC-04])
//! A member is **read** only when its store marks its build facts extracted —
//! a full walk ran over it ([`crate::graph_store::BUILD_FACTS_EXTRACTED_KEY`]).
//! Every other roster member is named in [`MembersRead::unread`] with its
//! reason in [`MembersRead::unread_reasons`]: [`UNREAD_NOT_EXTRACTED`] for a
//! store upgraded across migration 22 (or never indexed), whose empty tables
//! say nothing about its manifests; [`UNREAD_FAILED`] for an engine that did
//! not open or a read that failed.
//!
//! # Built on first query, never at startup ([ADR-52], [NFR-PE-10])
//! [`BuildDependencies`] holds the relation behind a cache keyed on the
//! members' sync-stamps, empty until the first [`BuildDependencies::relation`]
//! call; every sync advances a member's stamp, a manifest change included, so
//! the next call re-joins. `workspace status` joins the facts its freshness walk
//! already read, adding no walk.
//!
//! [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
//! [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
//! [ADR-69]: ../../../docs/specs/architecture/decisions/ADR-69.md
//! [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [BR-58]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::Serialize;

use crate::extract::build_manifest::ReferenceKind;
use crate::graph_store::BuildManifestRow;

use super::bridge::{current_stamps, read_members, MemberContracts, StampCache, Stamps};
use super::manifest::MemberKind;
use super::registry::{AnswerScope, EngineRegistry, MemberEngine};
use super::Member;

/// The kind of one `builds-against` edge — the kind of the reference it was
/// joined from ([ADR-69] point 1). Serialized as the fact's own token.
///
/// [ADR-69]: ../../../docs/specs/architecture/decisions/ADR-69.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildEdgeKind {
    /// Maven `<parent>`.
    Parent,
    /// A declared dependency (Maven `<dependencies>`, Gradle configuration).
    Dependency,
    /// Maven `<dependencyManagement>` — a version pin, not a dependency.
    Managed,
    /// A BOM import (Maven `scope=import`, Gradle `platform(…)`).
    BomImport,
}

impl BuildEdgeKind {
    /// Every kind, in declaration order.
    pub const ALL: [Self; 4] = [Self::Parent, Self::Dependency, Self::Managed, Self::BomImport];

    /// The fact vocabulary token — the same one the payloads serialize.
    ///
    /// Taken from the writer's own [`ReferenceKind::as_str`], the vocabulary
    /// migration 22's CHECK is pinned against, so the join can never drift
    /// from what the indexer stores (a drifted token would file every
    /// reference as refused).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.fact_kind().as_str()
    }

    /// The kind a fact vocabulary token (`build_artifacts.kind`) names, or
    /// `None` for any other text — the inverse of [`as_str`](Self::as_str),
    /// derived from it rather than written out a second time.
    #[must_use]
    pub fn from_fact(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == token)
    }

    /// The reader's kind this edge kind is joined from.
    fn fact_kind(self) -> ReferenceKind {
        match self {
            Self::Parent => ReferenceKind::Parent,
            Self::Dependency => ReferenceKind::Dependency,
            Self::Managed => ReferenceKind::Managed,
            Self::BomImport => ReferenceKind::BomImport,
        }
    }
}

/// One `builds-against(from → to, kind, scope, artifact)` edge ([FR-WS-33]).
///
/// One row per distinct `(from, to, kind, scope, artifact)`; a member whose
/// several manifests reference the same artifact the same way is one edge with
/// [`references`](Self::references) counting them.
///
/// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct BuildsAgainst {
    /// The referencing member — the one that builds against [`to`](Self::to).
    pub from: String,
    /// The member producing the artifact.
    pub to: String,
    /// The reference kind.
    pub kind: BuildEdgeKind,
    /// The scope as declared; `None` when the manifest declared none (never
    /// defaulted to `compile`, [NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub scope: Option<String>,
    /// The joined coordinate, `groupId:artifactId`.
    pub artifact: String,
    /// How many referenced facts in `from`'s manifests this edge stands for.
    pub references: u64,
    /// `to` is a declared `platform` member, so this edge is counted in
    /// [`PlatformApart`] and not in the headline. Absent when `false`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub platform: bool,
}

/// Member pairs, and how many of them carry each kind ([FR-WS-33]).
///
/// A pair is a directed `(from, to)` with at least one edge. A pair can carry
/// several kinds (a module whose parent is also a managed import), so the
/// per-kind counts can sum to more than [`pairs`](Self::pairs).
///
/// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PairCount {
    /// Distinct directed member pairs.
    pub pairs: u64,
    /// Pairs with at least one `parent` edge.
    pub parent: u64,
    /// Pairs with at least one `dependency` edge.
    pub dependency: u64,
    /// Pairs with at least one `managed` edge.
    pub managed: u64,
    /// Pairs with at least one `bom-import` edge.
    #[serde(rename = "bom-import")]
    pub bom_import: u64,
}

impl PairCount {
    fn over<'a>(edges: impl Iterator<Item = &'a BuildsAgainst>) -> Self {
        let mut kinds: BTreeMap<(&str, &str), BTreeSet<BuildEdgeKind>> = BTreeMap::new();
        for edge in edges {
            kinds
                .entry((edge.from.as_str(), edge.to.as_str()))
                .or_default()
                .insert(edge.kind);
        }
        let with = |kind| kinds.values().filter(|set| set.contains(&kind)).count() as u64;
        Self {
            pairs: kinds.len() as u64,
            parent: with(BuildEdgeKind::Parent),
            dependency: with(BuildEdgeKind::Dependency),
            managed: with(BuildEdgeKind::Managed),
            bom_import: with(BuildEdgeKind::BomImport),
        }
    }

    fn render(&self) -> String {
        format!(
            "{} pairs (parent {} · dependency {} · managed {} · bom-import {})",
            self.pairs, self.parent, self.dependency, self.managed, self.bom_import
        )
    }
}

/// Every referenced fact read, filed into exactly one bucket — the denominator
/// `build_dependency_pairs` is stated over ([BR-51], [NFR-CC-04]). The buckets
/// sum to [`references`](Self::references); the module docs define each one.
///
/// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ReferenceAccounting {
    /// Every referenced fact in every manifest read.
    pub references: u64,
    /// Resolved to exactly one other member that is not a declared platform —
    /// each is part of an edge counted in the headline.
    pub to_member: u64,
    /// Resolved to exactly one other member that **is** a declared platform —
    /// each is part of an edge counted in [`PlatformApart`], never in the
    /// headline. Zero when no platform is declared.
    pub to_platform: u64,
    /// Produced by the referencing member itself.
    pub in_member: u64,
    /// Produced by two or more members: resolves to neither.
    pub to_collision: u64,
    /// Produced by no member.
    pub external: u64,
    /// The key did not resolve; never joined.
    pub refused: u64,
    /// Gradle `project(':x')` — an in-member project, not a coordinate.
    pub project_reference: u64,
    /// Gradle `classpath` — a build-plugin coordinate.
    pub build_plugin: u64,
}

/// How much of the workspace the relation was joined over ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MembersRead {
    /// The roster size.
    pub members: u64,
    /// Members whose build facts were read.
    pub read: u64,
    /// Roster members whose facts could not be read (the engine did not open,
    /// the read failed, or the store holds no extracted facts yet), by name, in
    /// roster order. Absent when none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unread: Vec<String>,
    /// Why each [`unread`](Self::unread) member was not read, keyed by name:
    /// [`UNREAD_NOT_EXTRACTED`] or [`UNREAD_FAILED`]. Exactly the unread
    /// members; absent when none.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub unread_reasons: BTreeMap<String, &'static str>,
    /// Members read that hold at least one build manifest.
    pub with_manifests: u64,
    /// Manifests recorded across the members read.
    pub manifests: u64,
    /// Of those, the manifests actually parsed (`status = read`); the rest were
    /// malformed or unreadable and contribute no fact.
    pub manifests_read: u64,
}

/// One artifact two or more members produce — it resolves to **neither**, and
/// every reference to it is counted in
/// [`to_collision`](ReferenceAccounting::to_collision) ([FR-WS-33]).
///
/// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactCollision {
    /// The coordinate, `groupId:artifactId`.
    pub artifact: String,
    /// The members producing it, sorted.
    pub producers: Vec<String>,
    /// Referenced facts naming it, across every member.
    pub references: u64,
}

/// The inbound edges of declared `platform` members, counted apart from the
/// headline ([ADR-69] point 3). Present whenever a platform is declared, even
/// at zero, so a declaration is always visible.
///
/// [ADR-69]: ../../../docs/specs/architecture/decisions/ADR-69.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlatformApart {
    /// The declared platform members, sorted.
    pub members: Vec<String>,
    /// The pairs into them.
    pub build_dependency_pairs: PairCount,
    /// The count and the denominator it shares with the headline, as one line.
    pub summary: String,
}

/// A member **most** of the workspace builds against, not declared as any kind
/// — a hint for a human to declare `platform`, never a classification
/// ([ADR-69] point 3).
///
/// The rule: at least [`PLATFORM_CANDIDATE_MIN_IN_DEGREE`] members, and at
/// least one in [`PLATFORM_CANDIDATE_SHARE_DIVISOR`] of the other members read,
/// build against it (any kind).
///
/// [ADR-69]: ../../../docs/specs/architecture/decisions/ADR-69.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlatformCandidate {
    /// The candidate member.
    pub member: String,
    /// Distinct members with an edge into it.
    pub in_degree: u64,
    /// The members that could have one: every other member read.
    pub of: u64,
}

/// The fewest members that must build against a member for it to be a
/// platform candidate — one inbound edge is never a hub.
pub const PLATFORM_CANDIDATE_MIN_IN_DEGREE: u64 = 2;

/// A candidate's in-degree is at least `1 / PLATFORM_CANDIDATE_SHARE_DIVISOR`
/// of the other members read (a quarter).
pub const PLATFORM_CANDIDATE_SHARE_DIVISOR: u64 = 4;

/// The `build_dependency` headline: the pair count by kind beside its
/// denominator, the platform pairs apart, the collisions and the platform
/// candidates ([FR-WS-33], [BR-51], [BR-58]).
///
/// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
/// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [BR-58]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildDependencyHeadline {
    /// Member pairs with a build edge, by kind — the declared platforms'
    /// inbound pairs excluded.
    pub build_dependency_pairs: PairCount,
    /// The denominator: every referenced fact read, by bucket.
    pub references: ReferenceAccounting,
    /// The members the relation was joined over.
    pub members: MembersRead,
    /// Declared platforms' inbound pairs; absent when none is declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_apart: Option<PlatformApart>,
    /// Artifacts produced by two or more members, sorted by coordinate.
    pub collisions: Vec<ArtifactCollision>,
    /// Platform candidates, highest in-degree first (ties by name).
    pub platform_candidates: Vec<PlatformCandidate>,
    /// The headline, its denominator and what is apart, as one line.
    pub summary: String,
}

/// The whole relation: every edge beside its headline ([FR-WS-33]).
///
/// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildDependencyRelation {
    /// Every edge, the platforms' inbound ones included (flagged), sorted by
    /// `(from, to, kind, scope, artifact)`.
    pub edges: Vec<BuildsAgainst>,
    /// The headline.
    pub headline: BuildDependencyHeadline,
    /// The members whose facts were read, in roster order — what
    /// [`member`](Self::member) and [`per_member`](Self::per_member) answer
    /// over. Not serialized: the headline's [`MembersRead`] states it.
    #[serde(skip)]
    roster_read: Vec<String>,
    /// Colliding coordinate → the members whose references reached it — what
    /// [`collisions_referenced_by`](Self::collisions_referenced_by) answers.
    /// Not serialized, so the build headline's bytes do not move with it.
    #[serde(skip)]
    collision_referencers: BTreeMap<String, BTreeSet<String>>,
}

/// One member's side of the relation: what it builds against and what builds
/// against it — the per-member view `xservice build-deps` renders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberBuildDependencies {
    /// The member.
    pub member: String,
    /// Edges out of it.
    pub builds_against: Vec<BuildsAgainst>,
    /// Edges into it.
    pub built_against_by: Vec<BuildsAgainst>,
}

impl BuildDependencyRelation {
    /// `member`'s edges in both directions, or `None` when `member` is not a
    /// member the relation was joined over.
    pub fn member(&self, member: &str) -> Option<MemberBuildDependencies> {
        self.roster_read.iter().any(|m| m == member).then(|| MemberBuildDependencies {
            member: member.to_string(),
            builds_against: self.edges.iter().filter(|e| e.from == member).cloned().collect(),
            built_against_by: self.edges.iter().filter(|e| e.to == member).cloned().collect(),
        })
    }

    /// Every member read, in roster order, with its edges in both directions —
    /// including the members with none.
    pub fn per_member(&self) -> Vec<MemberBuildDependencies> {
        self.roster_read
            .iter()
            .filter_map(|member| self.member(member))
            .collect()
    }

    /// The collisions `member`'s references reach — each counted in its
    /// [`to_collision`](ReferenceAccounting::to_collision) — sorted by
    /// coordinate. The type-reference overlay reads them as pair evidence
    /// ([`super::type_refs`]): a coordinate resolving to neither producer
    /// still says the member builds against one of them.
    pub fn collisions_referenced_by<'a>(
        &'a self,
        member: &'a str,
    ) -> impl Iterator<Item = &'a ArtifactCollision> + 'a {
        self.headline.collisions.iter().filter(move |c| {
            self.collision_referencers
                .get(&c.artifact)
                .is_some_and(|members| members.contains(member))
        })
    }
}

/// One member's build-manifest facts as read: its name and every manifest row
/// its store holds — what [`join`] takes, one entry per member read.
pub type MemberBuildFacts = (String, Vec<BuildManifestRow>);

/// The [`unread`](MembersRead::unread) reason of a member whose store records
/// no full-walk build-manifest pass: upgraded across migration 22 and not yet
/// fully re-read, or never indexed ([FR-WS-33]). Its empty tables say nothing
/// about its manifests.
///
/// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
pub const UNREAD_NOT_EXTRACTED: &str = "build facts not yet extracted";

/// The [`unread`](MembersRead::unread) reason of a member whose engine did not
/// open or whose facts read failed ([ADR-53]); the cause is in the log line and,
/// for an unopenable store, in the degraded roll-up.
///
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
pub const UNREAD_FAILED: &str = "build facts could not be read";

/// Sort one member's facts read into a join's two inputs: its facts onto
/// `facts` when its store marks them extracted, its name onto `not_extracted`
/// when it does not (the `None` of [`MemberContracts::build_manifests`] and
/// of `MemberContracts::type_facts`). Shared by every read path — the lazy
/// build relation and type-reference index, and `workspace status`'s
/// freshness walk — so they cannot disagree about which member is read.
pub(super) fn sort_read<T>(
    member: String,
    read: Option<T>,
    facts: &mut Vec<(String, T)>,
    not_extracted: &mut Vec<String>,
) {
    match read {
        Some(rows) => facts.push((member, rows)),
        None => not_extracted.push(member),
    }
}

/// Join member-local build facts into the relation ([FR-WS-33]).
///
/// `roster` is the workspace's member list (manifest order); `facts` holds one
/// entry per member whose facts were read, and a roster member absent from it
/// is reported [`unread`](MembersRead::unread) — with
/// [`UNREAD_NOT_EXTRACTED`] when it is named in `not_extracted`, else
/// [`UNREAD_FAILED`]. `kinds` is the resolved `[workspace.member.<name>] kind`
/// map. Pure: no I/O, deterministic output.
///
/// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
pub fn join(
    roster: &[Member],
    kinds: &BTreeMap<String, MemberKind>,
    facts: &[MemberBuildFacts],
    not_extracted: &[String],
) -> BuildDependencyRelation {
    let read: BTreeMap<&str, &[BuildManifestRow]> = facts
        .iter()
        .map(|(member, rows)| (member.as_str(), rows.as_slice()))
        .collect();

    let mut members = MembersRead {
        members: roster.len() as u64,
        ..MembersRead::default()
    };
    for member in roster {
        match read.get(member.name.as_str()) {
            None => {
                let reason = if not_extracted.contains(&member.name) {
                    UNREAD_NOT_EXTRACTED
                } else {
                    UNREAD_FAILED
                };
                members.unread.push(member.name.clone());
                members.unread_reasons.insert(member.name.clone(), reason);
            }
            Some(rows) => {
                members.read += 1;
                members.with_manifests += u64::from(!rows.is_empty());
                members.manifests += rows.len() as u64;
                members.manifests_read += rows.iter().filter(|r| r.status == "read").count() as u64;
            }
        }
    }

    // Every joinable produced key, with the members producing it.
    let mut producers: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for (member, rows) in &read {
        for artifact in rows.iter().flat_map(|row| &row.artifacts) {
            if artifact.role != "produced" || artifact.resolution == "refused" {
                continue;
            }
            if let Some(key) = coordinate(artifact.group_id.as_deref(), artifact.artifact_id.as_deref()) {
                producers.entry(key).or_default().insert(member);
            }
        }
    }

    let is_platform = |member: &str| {
        kinds
            .get(member)
            .is_some_and(|kind| kind.sets_inbound_build_edges_apart())
    };
    let mut references = ReferenceAccounting::default();
    let mut collided: BTreeMap<&str, u64> = BTreeMap::new();
    let mut collision_referencers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut edges: BTreeMap<(String, String, BuildEdgeKind, Option<String>, String), u64> =
        BTreeMap::new();
    for (member, rows) in &read {
        for row in rows.iter() {
            for artifact in row.artifacts.iter().filter(|a| a.role == "referenced") {
                references.references += 1;
                let kind = artifact.kind.as_deref().and_then(BuildEdgeKind::from_fact);
                let key = coordinate(artifact.group_id.as_deref(), artifact.artifact_id.as_deref());
                // Refusal first: a refused fact's fields are declared text
                // (`${…}`) and must never reach a lookup. An unrecognised kind
                // (which migration 22's CHECK rules out) is refused with it —
                // an edge is never fabricated from a fact this join cannot read.
                if artifact.resolution == "refused" || kind.is_none() {
                    references.refused += 1;
                    continue;
                }
                if artifact.project_path.is_some() {
                    references.project_reference += 1;
                    continue;
                }
                if row.format == "gradle" && artifact.scope.as_deref() == Some("classpath") {
                    references.build_plugin += 1;
                    continue;
                }
                let (Some(kind), Some(key)) = (kind, key) else {
                    references.refused += 1;
                    continue;
                };
                match producers.get_key_value(&key) {
                    None => references.external += 1,
                    Some((key, set)) if set.len() > 1 => {
                        references.to_collision += 1;
                        *collided.entry(key.as_str()).or_default() += 1;
                        collision_referencers
                            .entry(key.clone())
                            .or_default()
                            .insert((*member).to_string());
                    }
                    Some((_, set)) if set.contains(member) => references.in_member += 1,
                    Some((_, set)) => {
                        let to = set.iter().next().expect("a non-empty producer set");
                        if is_platform(to) {
                            references.to_platform += 1;
                        } else {
                            references.to_member += 1;
                        }
                        *edges
                            .entry((
                                (*member).to_string(),
                                (*to).to_string(),
                                kind,
                                artifact.scope.clone(),
                                key,
                            ))
                            .or_default() += 1;
                    }
                }
            }
        }
    }

    let edges: Vec<BuildsAgainst> = edges
        .into_iter()
        .map(|((from, to, kind, scope, artifact), references)| BuildsAgainst {
            platform: is_platform(&to),
            from,
            to,
            kind,
            scope,
            artifact,
            references,
        })
        .collect();

    let collisions: Vec<ArtifactCollision> = producers
        .iter()
        .filter(|(_, set)| set.len() > 1)
        .map(|(artifact, set)| ArtifactCollision {
            artifact: artifact.clone(),
            producers: set.iter().map(|m| (*m).to_string()).collect(),
            references: collided.get(artifact.as_str()).copied().unwrap_or(0),
        })
        .collect();

    let headline_pairs = PairCount::over(edges.iter().filter(|e| !e.platform));
    let platforms: Vec<String> = kinds
        .iter()
        .filter(|(_, kind)| kind.sets_inbound_build_edges_apart())
        .map(|(member, _)| member.clone())
        .collect();
    let platform_apart = (!platforms.is_empty()).then(|| {
        let pairs = PairCount::over(edges.iter().filter(|e| e.platform));
        PlatformApart {
            summary: format!(
                "{} into {} declared platform member(s), from {} of {} referenced artifacts, \
                 counted apart from the headline over the same denominator",
                pairs.render(),
                platforms.len(),
                references.to_platform,
                references.references
            ),
            members: platforms,
            build_dependency_pairs: pairs,
        }
    });

    let platform_candidates = platform_candidates(&edges, kinds, members.read);
    // Every bucket is named, so the figures in the parentheses sum to the
    // "of N" they are stated over — the denominator is never partial.
    let summary = format!(
        "{} built against another member, from {} of {} referenced artifacts \
         ({} to a declared platform, {} external, {} in-member, {} to a colliding artifact, \
         {} refused, {} project reference(s), {} build plugin(s)), over {} of {} members read; \
         a build dependency, never a runtime coupling",
        headline_pairs.render(),
        references.to_member,
        references.references,
        references.to_platform,
        references.external,
        references.in_member,
        references.to_collision,
        references.refused,
        references.project_reference,
        references.build_plugin,
        members.read,
        members.members,
    );

    let roster_read: Vec<String> = roster
        .iter()
        .filter(|m| read.contains_key(m.name.as_str()))
        .map(|m| m.name.clone())
        .collect();
    BuildDependencyRelation {
        roster_read,
        collision_referencers,
        edges,
        headline: BuildDependencyHeadline {
            build_dependency_pairs: headline_pairs,
            references,
            members,
            platform_apart,
            collisions,
            platform_candidates,
            summary,
        },
    }
}

/// `groupId:artifactId`, or `None` when either half is missing or empty — an
/// empty key is refused upstream, and re-checked here so it can never join.
fn coordinate(group: Option<&str>, artifact: Option<&str>) -> Option<String> {
    match (group, artifact) {
        (Some(g), Some(a)) if !g.is_empty() && !a.is_empty() => Some(format!("{g}:{a}")),
        _ => None,
    }
}

/// The undeclared members at or above the candidate rule, highest in-degree
/// first. Every candidate shares one denominator (`read - 1`), so in-degree
/// order is share order.
fn platform_candidates(
    edges: &[BuildsAgainst],
    kinds: &BTreeMap<String, MemberKind>,
    read: u64,
) -> Vec<PlatformCandidate> {
    let of = read.saturating_sub(1);
    let mut inbound: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for edge in edges {
        inbound.entry(edge.to.as_str()).or_default().insert(edge.from.as_str());
    }
    let mut candidates: Vec<PlatformCandidate> = inbound
        .into_iter()
        .filter(|(member, _)| !kinds.contains_key(*member))
        .map(|(member, from)| PlatformCandidate {
            member: member.to_string(),
            in_degree: from.len() as u64,
            of,
        })
        .filter(|c| {
            c.in_degree >= PLATFORM_CANDIDATE_MIN_IN_DEGREE
                && c.in_degree * PLATFORM_CANDIDATE_SHARE_DIVISOR >= of
        })
        .collect();
    candidates.sort_by(|a, b| b.in_degree.cmp(&a.in_degree).then_with(|| a.member.cmp(&b.member)));
    candidates
}

/// The relation for one workspace, joined on the **first** query and cached on
/// the members' sync-stamps ([FR-WS-33], [ADR-52]).
///
/// Empty at construction — building one reads nothing, so holding it on a
/// serve surface costs nothing at startup. One holder per registry: the join
/// also reads the registry's declared kinds, which are fixed for its lifetime.
///
/// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
#[derive(Debug, Default)]
pub struct BuildDependencies {
    cache: StampCache<BuildDependencyRelation>,
}

impl BuildDependencies {
    /// A holder with nothing joined yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The relation over `registry`'s members: the stamps are read first, and
    /// the facts are read and joined only on a miss — the first call, or any
    /// call after a member re-synced.
    ///
    /// A member whose engine will not open or whose read fails is skipped with
    /// a warning and named in [`MembersRead::unread`] ([ADR-53]), reason
    /// [`UNREAD_FAILED`]; a member whose store holds no extracted facts yet is
    /// named there too, reason [`UNREAD_NOT_EXTRACTED`].
    ///
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    pub fn relation<E>(&self, registry: &EngineRegistry<E>) -> Arc<BuildDependencyRelation>
    where
        E: MemberEngine + MemberContracts,
    {
        let answer = registry.answer();
        let stamps = current_stamps(&answer);
        self.relation_in(registry, &answer, stamps)
    }

    /// [`relation`](Self::relation) inside a caller's [`AnswerScope`], keyed on
    /// the stamps it already read — so a read-model built beside the relation
    /// (the type-reference overlay) shares one open attempt per member and one
    /// stamp snapshot with it, as `ContractBridge::reachability_read` does
    /// for its two caches ([FR-WS-16], [CR-125]).
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [CR-125]: ../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
    pub(super) fn relation_in<E>(
        &self,
        registry: &EngineRegistry<E>,
        answer: &AnswerScope<'_, E>,
        stamps: Stamps,
    ) -> Arc<BuildDependencyRelation>
    where
        E: MemberEngine + MemberContracts,
    {
        self.cache.get_or_compute(stamps, || {
            let (mut facts, mut not_extracted) = (Vec::new(), Vec::new());
            for (member, read) in read_members(answer, "build-manifest facts", |engine| {
                engine.build_manifests()
            }) {
                sort_read(member, read, &mut facts, &mut not_extracted);
            }
            let federation = registry.federation();
            join(&federation.members, &federation.member_kinds, &facts, &not_extracted)
        })
    }
}

/// The `artifactId` of a bounded context's **model library** — the one naming
/// convention the [cross-context hint](CrossContextHint) recognises
/// ([CR-148] §2.1).
///
/// On the reference estate the `*-kafka-models` jars partition the members by
/// bounded context exactly (`archive-kafka-models` ← 7, `mailbox-kafka-models`
/// ← 8, …). Those are the **repositories'** names; the coordinate each one
/// produces is `<group>.<context>:kafka-models` — every one of the six — so the
/// context is read off the coordinate, in either of two spellings:
///
/// - `artifactId` exactly `kafka-models`: the context is the `groupId`'s last
///   dot-separated segment (`com.acme.archive:kafka-models` → `archive`) — the
///   estate's only shape;
/// - `artifactId` `<context>-kafka-models`: the context is the prefix
///   (`com.acme:archive-kafka-models` → `archive`).
///
/// Anything else names no context, and nothing is inferred from a member's name.
///
/// [CR-148]: ../../../docs/requests/CR-148-build-manifests-yield-a-build-dependency-relation.md
pub const MODEL_LIBRARY_ARTIFACT: &str = "kafka-models";

/// The bounded context a joined `groupId:artifactId` coordinate's model library
/// names ([`MODEL_LIBRARY_ARTIFACT`] states the two spellings), or `None` when
/// it names none — including an empty context either way.
#[must_use]
pub fn model_library_context(artifact: &str) -> Option<&str> {
    let (group, artifact_id) = artifact.rsplit_once(':')?;
    let context = if artifact_id == MODEL_LIBRARY_ARTIFACT {
        group.rsplit('.').next()?
    } else {
        artifact_id.strip_suffix(MODEL_LIBRARY_ARTIFACT)?.strip_suffix('-')?
    };
    (!context.is_empty()).then_some(context)
}

/// One model library a member builds against, named ([CR-148] §3.2 D).
///
/// [CR-148]: ../../../docs/requests/CR-148-build-manifests-yield-a-build-dependency-relation.md
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ModelLibrary {
    /// The bounded context its `artifactId` names.
    pub context: String,
    /// The joined coordinate, `groupId:artifactId`.
    pub artifact: String,
    /// The member producing it.
    pub member: String,
}

/// A member that depends on the model libraries of **two or more** bounded
/// contexts — a cross-context stream hint ([CR-148] §3.2 D, [CR-131] §3.3).
///
/// A report, never an edge: it is derived from `dependency` edges already in the
/// relation and adds none, and no surface draws it ([BR-58]). A `managed` edge
/// is a version pin, not a dependency, so a parent POM pinning every context's
/// models is not a hint.
///
/// [CR-148]: ../../../docs/requests/CR-148-build-manifests-yield-a-build-dependency-relation.md
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
/// [BR-58]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CrossContextHint {
    /// The depending member.
    pub member: String,
    /// The distinct contexts it depends on, sorted.
    pub contexts: Vec<String>,
    /// Every model library behind them, sorted by `(context, artifact)`.
    pub libraries: Vec<ModelLibrary>,
}

impl BuildDependencyRelation {
    /// The members depending on two or more contexts' model libraries, sorted by
    /// member.
    pub fn cross_context_hints(&self) -> Vec<CrossContextHint> {
        let mut by_member: BTreeMap<&str, BTreeSet<ModelLibrary>> = BTreeMap::new();
        for edge in self.edges.iter().filter(|e| e.kind == BuildEdgeKind::Dependency) {
            if let Some(context) = model_library_context(&edge.artifact) {
                by_member.entry(edge.from.as_str()).or_default().insert(ModelLibrary {
                    context: context.to_string(),
                    artifact: edge.artifact.clone(),
                    member: edge.to.clone(),
                });
            }
        }
        by_member
            .into_iter()
            .filter_map(|(member, libraries)| {
                let contexts: BTreeSet<String> =
                    libraries.iter().map(|l| l.context.clone()).collect();
                (contexts.len() >= 2).then(|| CrossContextHint {
                    member: member.to_string(),
                    contexts: contexts.into_iter().collect(),
                    libraries: libraries.into_iter().collect(),
                })
            })
            .collect()
    }
}

/// The `xservice build-deps` read-model — one per surface, the CLI, MCP and web
/// twins serialize it alike ([FR-WS-05], [FR-WS-33]).
///
/// Per member, what it **builds against** and what is **built against it**,
/// each row naming kind, scope and artifact; the headline with its denominator
/// beside them ([BR-51]); and the cross-context hint. A build dependency, never
/// a runtime coupling ([BR-58]).
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
/// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [BR-58]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct XserviceBuildDeps {
    /// The `--repo` scope, when one was applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Set when the scope names no member the relation was read over — so the
    /// empty [`members`](Self::members) is never read as "no edges"
    /// ([NFR-CC-04]). It states why: the member's
    /// [unread reason](MembersRead::unread_reasons), or "not in the workspace".
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_note: Option<String>,
    /// The workspace-wide headline, whatever the scope: the denominator the rows
    /// are read against.
    pub headline: BuildDependencyHeadline,
    /// One entry per member read (roster order), or the scoped member alone; a
    /// member with no edge is listed with both lists empty.
    pub members: Vec<MemberBuildDependencies>,
    /// Members depending on two or more contexts' model libraries — within the
    /// scope, when one was applied. Never an edge.
    pub cross_context: Vec<CrossContextHint>,
}

/// The `scope_note` of an `xservice` listing scoped to `repo` that lists
/// nothing (`listed` false): the member is not one the read-model was built
/// `over`, for its `unread` reason, or because it is not in the workspace — so
/// an empty listing is never read as "nothing here" ([NFR-CC-04]). `None`
/// unscoped, or when something is listed. Shared by `build-deps` and
/// `type-refs`, so the two can never state the rule differently.
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub(super) fn scope_note(
    repo: Option<&str>,
    listed: bool,
    unread: &BTreeMap<String, &'static str>,
    over: &str,
) -> Option<String> {
    repo.filter(|_| !listed).map(|member| {
        let why = unread.get(member).copied().unwrap_or("not in the workspace");
        format!("`{member}` is not a member the {over} ({why})")
    })
}

/// The `xservice build-deps` answer over `relation`, scoped to one member when
/// `repo` is given.
pub fn xservice_build_deps(relation: &BuildDependencyRelation, repo: Option<&str>) -> XserviceBuildDeps {
    let members = match repo {
        Some(member) => relation.member(member).into_iter().collect(),
        None => relation.per_member(),
    };
    let scope_note = scope_note(
        repo,
        !members.is_empty(),
        &relation.headline.members.unread_reasons,
        "build relation was read over",
    );
    XserviceBuildDeps {
        scope: repo.map(str::to_string),
        scope_note,
        headline: relation.headline.clone(),
        members,
        cross_context: relation
            .cross_context_hints()
            .into_iter()
            .filter(|hint| repo.is_none_or(|member| hint.member == member))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use anyhow::Result;

    use super::*;
    use crate::federation::bridge::{ContractBridge, ContractNode, InvocationRef};
    use crate::federation::coverage::cross_service_coverage;
    use crate::federation::registry::RegistryMode;
    use crate::federation::Federation;
    use crate::graph_store::BuildArtifactRow;
    use crate::model::{ArtifactRelation, LogosSymbol, NodeKind};

    // Per-test-thread fixtures, the `coverage::tests` idiom: each `#[test]` runs
    // on its own thread and the registry fans out sequentially on it.
    thread_local! {
        static FACTS: RefCell<HashMap<String, Vec<BuildManifestRow>>> = RefCell::new(HashMap::new());
        static SURFACES: RefCell<HashMap<String, Vec<ContractNode>>> = RefCell::new(HashMap::new());
        static CALLS: RefCell<HashMap<String, Vec<InvocationRef>>> = RefCell::new(HashMap::new());
        static STAMPS: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
        /// Every `build_manifests` read served, by member.
        static READS: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    }

    #[derive(Debug)]
    struct FakeEngine {
        member: String,
    }

    impl MemberEngine for FakeEngine {
        type Watcher = ();
        fn start(root: &Path, _: usize, _: crate::SharedWorkerPool) -> Result<Arc<Self>> {
            let member = root.file_name().unwrap().to_string_lossy().into_owned();
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
            Ok(SURFACES.with(|s| s.borrow().get(&self.member).cloned().unwrap_or_default()))
        }
        fn contract_stamp(&self) -> u64 {
            STAMPS.with(|s| s.borrow().get(&self.member).copied().unwrap_or(0))
        }
        fn invocation_refs(&self) -> Result<Vec<InvocationRef>> {
            Ok(CALLS.with(|c| c.borrow().get(&self.member).cloned().unwrap_or_default()))
        }
        fn build_manifests(&self) -> Result<Option<Vec<BuildManifestRow>>> {
            READS.with(|r| *r.borrow_mut().entry(self.member.clone()).or_default() += 1);
            if self.member == "unreadable" {
                anyhow::bail!("store read failed");
            }
            if self.member == "upgraded" {
                return Ok(None);
            }
            Ok(Some(FACTS.with(|f| f.borrow().get(&self.member).cloned().unwrap_or_default())))
        }
    }

    fn reads() -> u64 {
        READS.with(|r| r.borrow().values().sum())
    }

    fn set_facts(member: &str, rows: Vec<BuildManifestRow>) {
        FACTS.with(|f| f.borrow_mut().insert(member.to_string(), rows));
    }

    fn fed(names: &[&str], kinds: &[(&str, MemberKind)]) -> Federation {
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
            member_kinds: kinds.iter().map(|(m, k)| ((*m).to_string(), *k)).collect(),
        }
    }

    // ── fact builders: the persisted row shapes S-462 writes ──────────────

    fn fact(role: &str, kind: Option<&str>, g: &str, a: &str) -> BuildArtifactRow {
        BuildArtifactRow {
            role: role.to_string(),
            kind: kind.map(str::to_string),
            group_id: Some(g.to_string()),
            artifact_id: Some(a.to_string()),
            version: Some("1.0".to_string()),
            scope: None,
            project_path: None,
            resolution: "resolved".to_string(),
            reason: None,
        }
    }
    fn produced(g: &str, a: &str) -> BuildArtifactRow {
        fact("produced", None, g, a)
    }
    fn parent(g: &str, a: &str) -> BuildArtifactRow {
        fact("referenced", Some("parent"), g, a)
    }
    fn dependency(g: &str, a: &str) -> BuildArtifactRow {
        fact("referenced", Some("dependency"), g, a)
    }
    fn managed(g: &str, a: &str) -> BuildArtifactRow {
        fact("referenced", Some("managed"), g, a)
    }
    fn bom(g: &str, a: &str) -> BuildArtifactRow {
        BuildArtifactRow {
            scope: Some("import".to_string()),
            ..fact("referenced", Some("bom-import"), g, a)
        }
    }
    fn scoped(mut row: BuildArtifactRow, scope: &str) -> BuildArtifactRow {
        row.scope = Some(scope.to_string());
        row
    }
    fn resolution(mut row: BuildArtifactRow, resolution: &str) -> BuildArtifactRow {
        row.resolution = resolution.to_string();
        row.reason = Some("`${x}` is not defined in the member".to_string());
        if resolution == "refused" {
            row.group_id = Some("${x}".to_string());
        } else {
            row.version = Some("${x}".to_string());
        }
        row
    }
    fn manifest(format: &str, path: &str, artifacts: Vec<BuildArtifactRow>) -> BuildManifestRow {
        BuildManifestRow {
            path: path.to_string(),
            format: format.to_string(),
            content_hash: Some("h".to_string()),
            status: "read".to_string(),
            detail: None,
            artifacts,
        }
    }
    fn pom(artifacts: Vec<BuildArtifactRow>) -> BuildManifestRow {
        manifest("maven", "pom.xml", artifacts)
    }

    const G: &str = "com.sourcesense.poste.pec";

    fn edge_keys(relation: &BuildDependencyRelation) -> Vec<(&str, &str, BuildEdgeKind, &str)> {
        relation
            .edges
            .iter()
            .map(|e| (e.from.as_str(), e.to.as_str(), e.kind, e.artifact.as_str()))
            .collect()
    }

    /// The estate's dominant shape (a parent POM most members inherit, a shared
    /// library most depend on) plus one member of every non-edge bucket, joined
    /// over one roster.
    fn estate_shape() -> (Vec<Member>, Vec<(String, Vec<BuildManifestRow>)>) {
        let names = ["starter", "common", "api", "batch"];
        let roster = fed(&names, &[]).members;
        let facts = vec![
            ("starter".to_string(), vec![pom(vec![produced(G, "poste-pec-starter")])]),
            (
                "common".to_string(),
                vec![pom(vec![
                    produced(G, "poste-pec-common"),
                    parent(G, "poste-pec-starter"),
                ])],
            ),
            (
                "api".to_string(),
                vec![
                    pom(vec![
                        produced(G, "api-parent"),
                        parent(G, "poste-pec-starter"),
                        dependency(G, "poste-pec-common"),
                        // A second module of the same member: in-member, never an edge.
                        dependency(G, "api-domain"),
                        // Third-party: external.
                        dependency("org.springframework", "spring-web"),
                        managed(G, "poste-pec-common"),
                        bom(G, "poste-pec-starter"),
                    ]),
                    manifest(
                        "maven",
                        "domain/pom.xml",
                        vec![produced(G, "api-domain"), parent(G, "api-parent")],
                    ),
                ],
            ),
            (
                "batch".to_string(),
                vec![pom(vec![
                    produced(G, "batch"),
                    parent(G, "poste-pec-starter"),
                    // Version unresolved but key resolved: joinable.
                    resolution(scoped(dependency(G, "poste-pec-common"), "test"), "version-refused"),
                    // Key unresolved: never joined.
                    resolution(dependency(G, "poste-pec-common"), "refused"),
                ])],
            ),
        ];
        (roster, facts)
    }

    /// **The join.** A reference to an artifact exactly one other member
    /// produces is an edge carrying its kind, scope and artifact; every other
    /// reference lands in exactly one named bucket, and the buckets sum to the
    /// denominator. A `version-refused` fact joins; a `refused` one never does.
    #[test]
    fn a_reference_to_another_members_artifact_is_an_edge_and_every_other_reference_is_accounted() {
        let (roster, facts) = estate_shape();
        let relation = join(&roster, &BTreeMap::new(), &facts, &[]);

        let starter = "com.sourcesense.poste.pec:poste-pec-starter";
        let common = "com.sourcesense.poste.pec:poste-pec-common";
        assert_eq!(
            edge_keys(&relation),
            [
                ("api", "common", BuildEdgeKind::Dependency, common),
                ("api", "common", BuildEdgeKind::Managed, common),
                ("api", "starter", BuildEdgeKind::Parent, starter),
                ("api", "starter", BuildEdgeKind::BomImport, starter),
                ("batch", "common", BuildEdgeKind::Dependency, common),
                ("batch", "starter", BuildEdgeKind::Parent, starter),
                ("common", "starter", BuildEdgeKind::Parent, starter),
            ]
        );
        let batch_dep = relation
            .edges
            .iter()
            .find(|e| e.from == "batch" && e.kind == BuildEdgeKind::Dependency)
            .unwrap();
        assert_eq!(batch_dep.scope.as_deref(), Some("test"), "scope as declared");
        let api_parent = relation.edges.iter().find(|e| e.from == "api").unwrap();
        assert_eq!(api_parent.scope, None, "an undeclared scope is never defaulted");

        let headline = &relation.headline;
        assert_eq!(
            headline.build_dependency_pairs,
            PairCount { pairs: 5, parent: 3, dependency: 2, managed: 1, bom_import: 1 }
        );
        let r = headline.references;
        assert_eq!(
            r,
            ReferenceAccounting {
                references: 11,
                to_member: 7,
                to_platform: 0,
                in_member: 2,
                to_collision: 0,
                external: 1,
                refused: 1,
                project_reference: 0,
                build_plugin: 0,
            }
        );
        assert_eq!(
            r.to_member + r.to_platform + r.in_member + r.to_collision + r.external + r.refused
                + r.project_reference + r.build_plugin,
            r.references,
            "every reference is filed exactly once"
        );
        assert_eq!(
            headline.members,
            MembersRead {
                members: 4,
                read: 4,
                unread: Vec::new(),
                unread_reasons: BTreeMap::new(),
                with_manifests: 4,
                manifests: 5,
                manifests_read: 5
            }
        );
        assert!(headline.collisions.is_empty());
        assert!(headline.platform_apart.is_none(), "no platform declared, nothing apart");
        assert_eq!(
            headline.summary,
            "5 pairs (parent 3 · dependency 2 · managed 1 · bom-import 1) built against another \
             member, from 7 of 11 referenced artifacts (0 to a declared platform, 1 external, \
             2 in-member, 0 to a colliding artifact, 1 refused, 0 project reference(s), \
             0 build plugin(s)), over 4 of 4 members read; a build dependency, never a runtime \
             coupling"
        );
    }

    /// The kind token round-trips through the fact vocabulary and serde, so the
    /// stored token, the payload token and `as_str` cannot drift apart.
    #[test]
    fn each_edge_kind_round_trips_its_fact_token() {
        for kind in BuildEdgeKind::ALL {
            assert_eq!(BuildEdgeKind::from_fact(kind.as_str()), Some(kind));
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        }
        assert_eq!(BuildEdgeKind::from_fact("bom_import"), None, "the near miss");
        // Every kind the reader writes has an edge kind: none is silently refused.
        for written in [
            ReferenceKind::Parent,
            ReferenceKind::Dependency,
            ReferenceKind::Managed,
            ReferenceKind::BomImport,
        ] {
            let kind = BuildEdgeKind::from_fact(written.as_str()).expect("joinable");
            assert_eq!(kind.as_str(), written.as_str());
        }
    }

    /// A manifest the reader could not parse is recorded but read: it counts in
    /// `manifests`, not in `manifests_read`, and contributes no fact.
    #[test]
    fn a_malformed_manifest_is_counted_but_not_read() {
        let roster = fed(&["lib", "app"], &[]).members;
        let mut malformed = manifest("maven", "broken/pom.xml", Vec::new());
        malformed.status = "malformed".to_string();
        malformed.detail = Some("a DTD entity is used".to_string());
        let facts = vec![
            ("lib".to_string(), vec![pom(vec![produced(G, "lib")]), malformed]),
            ("app".to_string(), vec![pom(vec![produced(G, "app"), dependency(G, "lib")])]),
        ];
        let headline = join(&roster, &BTreeMap::new(), &facts, &[]).headline;
        assert_eq!((headline.members.manifests, headline.members.manifests_read), (3, 2));
        assert_eq!(headline.members.with_manifests, 2);
        assert_eq!(headline.references.references, 1, "the malformed manifest adds no reference");
    }

    /// Two references from two manifests of one member the same way are one
    /// edge counting both — pairs and edges are never inflated by module count.
    #[test]
    fn two_manifests_referencing_one_artifact_the_same_way_are_one_edge() {
        let roster = fed(&["lib", "app"], &[]).members;
        let facts = vec![
            ("lib".to_string(), vec![pom(vec![produced(G, "lib")])]),
            (
                "app".to_string(),
                vec![
                    manifest("maven", "a/pom.xml", vec![produced(G, "a"), dependency(G, "lib")]),
                    manifest("maven", "b/pom.xml", vec![produced(G, "b"), dependency(G, "lib")]),
                ],
            ),
        ];
        let relation = join(&roster, &BTreeMap::new(), &facts, &[]);
        assert_eq!(relation.edges.len(), 1);
        assert_eq!(relation.edges[0].references, 2);
        assert_eq!(relation.headline.build_dependency_pairs.pairs, 1);
        assert_eq!(relation.headline.references.to_member, 2);
    }

    /// **The collision, pinned on the estate's own shape.** Two members producing
    /// one coordinate resolve to **neither**: no edge into either, every
    /// reference counted as a collision, and the collision reported with both
    /// producers and its reference count.
    #[test]
    fn an_artifact_two_members_produce_resolves_to_neither_and_is_reported() {
        let core = "com.sourcesense.poste.pec.mailbox.core";
        let roster = fed(&["deprecated-mailbox-core", "mailbox-api", "mailbox-manager"], &[]).members;
        let facts = vec![
            (
                "deprecated-mailbox-core".to_string(),
                vec![manifest("maven", "api/pom.xml", vec![produced(core, "api")])],
            ),
            // A co-producer referencing the colliding key: still a collision,
            // never `in_member` — its own module is one of two candidates.
            (
                "mailbox-api".to_string(),
                vec![
                    pom(vec![produced(core, "api")]),
                    manifest("maven", "client/pom.xml", vec![produced(core, "client"), dependency(core, "api")]),
                ],
            ),
            (
                "mailbox-manager".to_string(),
                vec![pom(vec![produced(core, "manager"), dependency(core, "api")])],
            ),
        ];
        let relation = join(&roster, &BTreeMap::new(), &facts, &[]);

        assert!(relation.edges.is_empty(), "resolves to neither producer: {:?}", relation.edges);
        let headline = &relation.headline;
        assert_eq!(headline.build_dependency_pairs.pairs, 0);
        assert_eq!(headline.references.to_collision, 2);
        assert_eq!(headline.references.in_member, 0, "the co-producer's reference is a collision");
        assert_eq!(headline.references.to_member, 0);
        assert_eq!(
            headline.collisions,
            [ArtifactCollision {
                artifact: format!("{core}:api"),
                producers: vec!["deprecated-mailbox-core".into(), "mailbox-api".into()],
                references: 2,
            }]
        );
        assert!(headline.summary.contains("2 to a colliding artifact"), "{}", headline.summary);
    }

    /// One member producing a coordinate twice (two poms, one coordinate) is
    /// still one producer — a collision is between members, never within one.
    #[test]
    fn one_member_producing_a_coordinate_twice_is_not_a_collision() {
        let roster = fed(&["lib", "app"], &[]).members;
        let facts = vec![
            (
                "lib".to_string(),
                vec![
                    manifest("maven", "pom.xml", vec![produced(G, "lib")]),
                    manifest("maven", "copy/pom.xml", vec![produced(G, "lib")]),
                ],
            ),
            ("app".to_string(), vec![pom(vec![produced(G, "app"), dependency(G, "lib")])]),
        ];
        let relation = join(&roster, &BTreeMap::new(), &facts, &[]);
        assert!(relation.headline.collisions.is_empty());
        assert_eq!(edge_keys(&relation), [("app", "lib", BuildEdgeKind::Dependency, "com.sourcesense.poste.pec:lib")]);
    }

    /// A refused **produced** fact (every Gradle one, and a Maven one whose key
    /// did not resolve) is no producer: a reference to that coordinate is
    /// external, never an edge. The refusal alone decides it — the Maven fixture
    /// keeps both key fields filled, so no missing field can stand in for it.
    #[test]
    fn a_refused_produced_fact_produces_nothing() {
        let roster = fed(&["lib", "tool", "app"], &[]).members;
        let mut gradle_produced = produced(G, "lib");
        gradle_produced.resolution = "refused".to_string();
        gradle_produced.artifact_id = None;
        gradle_produced.reason = Some("the artifact name comes from settings.gradle".into());
        let mut maven_refused = produced(G, "tool");
        maven_refused.resolution = "refused".to_string();
        maven_refused.reason = Some("`${revision}` is not defined in the member".into());
        let facts = vec![
            ("lib".to_string(), vec![manifest("gradle", "build.gradle", vec![gradle_produced])]),
            ("tool".to_string(), vec![pom(vec![maven_refused])]),
            (
                "app".to_string(),
                vec![pom(vec![produced(G, "app"), dependency(G, "lib"), dependency(G, "tool")])],
            ),
        ];
        let relation = join(&roster, &BTreeMap::new(), &facts, &[]);
        assert!(relation.edges.is_empty(), "{:?}", relation.edges);
        assert_eq!(relation.headline.references.external, 2);
    }

    /// Gradle's two non-coordinate shapes are filed apart and never joined: a
    /// `project(':x')` reference names an in-member project, and a `classpath`
    /// entry is a build plugin. The near miss — a **Maven** fact whose scope
    /// happens to read `classpath` — is an ordinary coordinate and joins.
    #[test]
    fn gradle_project_references_and_classpath_entries_are_never_joined() {
        let roster = fed(&["lib", "app", "mvn"], &[]).members;
        let project = BuildArtifactRow {
            group_id: None,
            artifact_id: None,
            version: None,
            scope: Some("implementation".into()),
            project_path: Some(":core".into()),
            ..dependency(G, "unused")
        };
        let facts = vec![
            ("lib".to_string(), vec![pom(vec![produced(G, "lib")])]),
            (
                "app".to_string(),
                vec![manifest(
                    "gradle",
                    "build.gradle.kts",
                    vec![project, scoped(dependency(G, "lib"), "classpath")],
                )],
            ),
            (
                "mvn".to_string(),
                vec![pom(vec![produced(G, "mvn"), scoped(dependency(G, "lib"), "classpath")])],
            ),
        ];
        let relation = join(&roster, &BTreeMap::new(), &facts, &[]);
        let r = relation.headline.references;
        assert_eq!((r.project_reference, r.build_plugin, r.to_member), (1, 1, 1));
        assert!(
            relation.headline.summary.contains(
                "from 1 of 3 referenced artifacts (0 to a declared platform, 0 external, \
                 0 in-member, 0 to a colliding artifact, 0 refused, 1 project reference(s), \
                 1 build plugin(s))"
            ),
            "the summary names every bucket, so they sum to its denominator: {}",
            relation.headline.summary
        );
        assert_eq!(
            edge_keys(&relation),
            [("mvn", "lib", BuildEdgeKind::Dependency, "com.sourcesense.poste.pec:lib")]
        );
    }

    /// **A declared platform's inbound edges are counted apart.** They leave the
    /// headline for `platform_apart`, over the same denominator, and each edge
    /// into it is flagged; its *outbound* edges stay in the headline.
    #[test]
    fn a_declared_platforms_inbound_edges_are_counted_apart_from_the_headline() {
        let (roster, facts) = estate_shape();
        let kinds = BTreeMap::from([("starter".to_string(), MemberKind::Platform)]);
        let relation = join(&roster, &kinds, &facts, &[]);
        let headline = &relation.headline;

        assert_eq!(
            headline.build_dependency_pairs,
            PairCount { pairs: 2, parent: 0, dependency: 2, managed: 1, bom_import: 0 },
            "only the pairs into `common` remain"
        );
        let apart = headline.platform_apart.as_ref().expect("a platform is declared");
        assert_eq!(apart.members, ["starter"]);
        assert_eq!(
            apart.build_dependency_pairs,
            PairCount { pairs: 3, parent: 3, dependency: 0, managed: 0, bom_import: 1 }
        );
        assert_eq!(
            apart.summary,
            "3 pairs (parent 3 · dependency 0 · managed 0 · bom-import 1) into 1 declared platform \
             member(s), from 4 of 11 referenced artifacts, counted apart from the headline over \
             the same denominator"
        );
        // The headline states only its own share: the 2 pairs into `common`
        // stand for 3 references, the 4 into the platform are apart.
        let undeclared = join(&roster, &BTreeMap::new(), &facts, &[]).headline.references;
        assert_eq!((headline.references.to_member, headline.references.to_platform), (3, 4));
        assert_eq!(
            headline.references.to_member + headline.references.to_platform,
            undeclared.to_member,
            "a declaration only moves references between the two edge buckets"
        );
        assert_eq!(headline.references.references, undeclared.references, "the denominator holds");
        assert!(
            headline.summary.contains("from 3 of 11 referenced artifacts (4 to a declared platform,"),
            "{}",
            headline.summary
        );
        assert!(relation.edges.iter().all(|e| e.platform == (e.to == "starter")));
        assert_eq!(relation.edges.len(), 7, "the relation keeps every edge");
    }

    /// A declared platform nobody builds against is still reported, at zero —
    /// a declaration is always visible. `documentation` and `mock` set nothing
    /// apart here: only `platform` answers that question with yes.
    #[test]
    fn a_platform_with_no_inbound_edge_is_reported_at_zero_and_other_kinds_set_nothing_apart() {
        let (roster, facts) = estate_shape();
        let kinds = BTreeMap::from([("batch".to_string(), MemberKind::Platform)]);
        let apart = join(&roster, &kinds, &facts, &[]).headline.platform_apart.unwrap();
        assert_eq!(apart.members, ["batch"]);
        assert_eq!(apart.build_dependency_pairs, PairCount::default());

        let kinds = BTreeMap::from([
            ("starter".to_string(), MemberKind::Documentation),
            ("common".to_string(), MemberKind::Mock),
        ]);
        let relation = join(&roster, &kinds, &facts, &[]);
        assert!(relation.headline.platform_apart.is_none());
        assert_eq!(relation.headline.build_dependency_pairs.pairs, 5);
    }

    /// **Candidates by in-degree share, classifying nothing.** The hub most
    /// members build against is listed with its in-degree over the other
    /// members read, highest first; listing moves nothing out of the headline.
    /// A member built against by one member is never one, a share just under a
    /// quarter is not one (the near miss), and a declared member is not listed.
    #[test]
    fn platform_candidates_are_listed_by_in_degree_share_and_classify_nothing() {
        // 9 members: `hub` ← 7, `lib` ← 2 (2 of 8 = exactly a quarter),
        // `solo` ← 1. `near` ← 2 of 9 others once a tenth member is read.
        let mut names = vec!["hub", "lib", "solo"];
        let clients = ["c1", "c2", "c3", "c4", "c5", "c6"];
        names.extend(clients);
        let mut facts: Vec<(String, Vec<BuildManifestRow>)> = vec![
            ("hub".into(), vec![pom(vec![produced(G, "hub")])]),
            ("lib".into(), vec![pom(vec![produced(G, "lib"), parent(G, "hub")])]),
            ("solo".into(), vec![pom(vec![produced(G, "solo")])]),
        ];
        for (i, c) in clients.iter().enumerate() {
            let mut refs = vec![produced(G, c), parent(G, "hub")];
            if i < 2 {
                refs.push(dependency(G, "lib"));
            }
            if i == 0 {
                refs.push(dependency(G, "solo"));
            }
            facts.push(((*c).to_string(), vec![pom(refs)]));
        }
        let roster = fed(&names, &[]).members;
        let relation = join(&roster, &BTreeMap::new(), &facts, &[]);
        assert_eq!(
            relation.headline.platform_candidates,
            [
                PlatformCandidate { member: "hub".into(), in_degree: 7, of: 8 },
                PlatformCandidate { member: "lib".into(), in_degree: 2, of: 8 },
            ]
        );
        assert_eq!(relation.headline.build_dependency_pairs.pairs, 10, "nothing moved");
        assert!(relation.headline.platform_apart.is_none());

        // One more member read: `lib`'s 2 of 9 is just under a quarter.
        let mut names_10 = names.clone();
        names_10.push("extra");
        facts.push(("extra".into(), vec![pom(vec![produced(G, "extra")])]));
        let relation = join(&fed(&names_10, &[]).members, &BTreeMap::new(), &facts, &[]);
        assert_eq!(
            relation.headline.platform_candidates,
            [PlatformCandidate { member: "hub".into(), in_degree: 7, of: 9 }]
        );

        // An unread roster member is no member "that could have one": `of`
        // stays 8, and `lib`'s 2 of 8 still clears the quarter.
        let mut names_unread = names.clone();
        names_unread.push("broken");
        let relation = join(&fed(&names_unread, &[]).members, &BTreeMap::new(), &facts[..9], &[]);
        assert_eq!(relation.headline.members.unread, ["broken"]);
        assert_eq!(
            relation.headline.platform_candidates,
            [
                PlatformCandidate { member: "hub".into(), in_degree: 7, of: 8 },
                PlatformCandidate { member: "lib".into(), in_degree: 2, of: 8 },
            ]
        );

        // Declared as anything, it is no longer a candidate — every kind, not
        // only `platform`: a human who declared the member already decided.
        for kind in [MemberKind::Platform, MemberKind::Documentation, MemberKind::Mock] {
            let kinds = BTreeMap::from([("hub".to_string(), kind)]);
            let candidates = join(&fed(&names_10, &[]).members, &kinds, &facts, &[])
                .headline
                .platform_candidates;
            assert!(
                candidates.iter().all(|c| c.member != "hub"),
                "a hub declared {} is still listed: {candidates:?}",
                kind.as_str()
            );
        }
    }

    /// One inbound edge is never a hub, whatever share it is: in a three-member
    /// workspace 1 of 2 others clears the quarter, and is still no candidate.
    #[test]
    fn one_inbound_edge_is_never_a_platform_candidate() {
        let roster = fed(&["a", "b", "c"], &[]).members;
        let facts = vec![
            ("a".to_string(), vec![pom(vec![produced(G, "a")])]),
            ("b".to_string(), vec![pom(vec![produced(G, "b"), dependency(G, "a")])]),
            ("c".to_string(), vec![pom(vec![produced(G, "c")])]),
        ];
        let relation = join(&roster, &BTreeMap::new(), &facts, &[]);
        assert_eq!(relation.headline.build_dependency_pairs.pairs, 1);
        assert!(relation.headline.platform_candidates.is_empty());
    }

    /// A roster member whose facts were not read is named unread and counted
    /// out of the denominator; the per-member view answers only for members read.
    #[test]
    fn an_unread_member_is_named_and_the_per_member_view_answers_for_members_read() {
        let (mut roster, facts) = estate_shape();
        roster.push(Member { name: "broken".into(), root: PathBuf::from("/ws/broken") });
        let relation = join(&roster, &BTreeMap::new(), &facts, &[]);
        assert_eq!(relation.headline.members.members, 5);
        assert_eq!(relation.headline.members.read, 4);
        assert_eq!(relation.headline.members.unread, ["broken"]);
        assert!(relation.headline.summary.contains("over 4 of 5 members read"));
        assert!(relation.member("broken").is_none());

        let starter = relation.member("starter").expect("read");
        assert!(starter.builds_against.is_empty());
        assert_eq!(starter.built_against_by.len(), 4);
        let api = relation.member("api").expect("read");
        assert_eq!(
            api.builds_against
                .iter()
                .map(|e| (e.to.as_str(), e.kind))
                .collect::<Vec<_>>(),
            [
                ("common", BuildEdgeKind::Dependency),
                ("common", BuildEdgeKind::Managed),
                ("starter", BuildEdgeKind::Parent),
                ("starter", BuildEdgeKind::BomImport),
            ],
            "the outbound half of the per-member view"
        );
        assert!(api.built_against_by.is_empty());
        let views = relation.per_member();
        assert_eq!(
            views.iter().map(|v| v.member.as_str()).collect::<Vec<_>>(),
            ["starter", "common", "api", "batch"],
            "roster order, members with no edge included"
        );
    }

    /// Every member degraded — nothing read — joins without underflowing the
    /// candidate denominator, names every member unread and lists no candidate.
    #[test]
    fn a_workspace_with_no_member_read_joins_to_nothing_and_names_them_all() {
        let roster = fed(&["a", "b"], &[]).members;
        let relation = join(&roster, &BTreeMap::new(), &[], &[]);
        assert_eq!(relation.headline.members.unread, ["a", "b"]);
        assert_eq!(relation.headline.members.read, 0);
        assert!(relation.headline.platform_candidates.is_empty());
        assert!(relation.headline.summary.contains("over 0 of 2 members read"));
        assert!(relation.per_member().is_empty());
    }

    /// A workspace with no manifest joins to an empty relation with its
    /// denominators stated, and serializes with no platform key.
    #[test]
    fn a_workspace_without_manifests_joins_to_nothing_and_says_so() {
        let roster = fed(&["a", "b"], &[]).members;
        let facts = vec![("a".to_string(), Vec::new()), ("b".to_string(), Vec::new())];
        let relation = join(&roster, &BTreeMap::new(), &facts, &[]);
        assert!(relation.edges.is_empty());
        assert_eq!(relation.headline.members.with_manifests, 0);
        let json = serde_json::to_value(&relation).unwrap();
        assert!(json["headline"].get("platform_apart").is_none());
        assert!(json.get("roster_read").is_none(), "a private helper, never on the wire");
        assert_eq!(json["headline"]["build_dependency_pairs"]["bom-import"], 0);
    }

    // ── laziness and the cache (ADR-52, NFR-PE-10) ───────────────────────

    /// **Never at startup.** An eagerly-warming serve registry starts every
    /// member engine, and a fresh holder beside it — yet no build fact is read
    /// until the first query. That query reads each member once; a second with
    /// no re-sync reads nothing; a member's re-sync makes the next one re-join.
    #[test]
    fn the_relation_is_built_on_first_query_never_at_startup_and_cached_on_stamps() {
        let (_, facts) = estate_shape();
        for (member, rows) in facts {
            set_facts(&member, rows);
        }
        let registry = EngineRegistry::<FakeEngine>::new(
            fed(&["starter", "common", "api", "batch"], &[]),
            RegistryMode::Serve,
        );
        let holder = BuildDependencies::new();
        assert!(registry.engine_starts() > 0, "the serve registry warmed at startup");
        assert_eq!(reads(), 0, "startup read no build fact");

        let first = holder.relation(&registry);
        assert_eq!(reads(), 4, "the first query reads every member once");
        assert_eq!(first.headline.build_dependency_pairs.pairs, 5);

        let second = holder.relation(&registry);
        assert_eq!(reads(), 4, "no re-sync, no re-read");
        assert!(Arc::ptr_eq(&first, &second));

        STAMPS.with(|s| s.borrow_mut().insert("api".into(), 1));
        set_facts("api", vec![pom(vec![produced(G, "api-parent")])]);
        let third = holder.relation(&registry);
        assert_eq!(reads(), 8, "a stamp advance re-joins");
        assert_eq!(third.headline.build_dependency_pairs.pairs, 3);
    }

    /// A member that will not open, and one whose read fails, are skipped and
    /// named unread; the rest still join.
    #[test]
    fn a_degraded_member_is_named_unread_and_the_rest_still_join() {
        let (_, facts) = estate_shape();
        for (member, rows) in facts {
            set_facts(&member, rows);
        }
        let registry = EngineRegistry::<FakeEngine>::new(
            fed(&["starter", "common", "api", "batch", "broken", "unreadable"], &[]),
            RegistryMode::Lazy,
        );
        let relation = BuildDependencies::new().relation(&registry);
        assert_eq!(relation.headline.members.unread, ["broken", "unreadable"]);
        assert_eq!(relation.headline.members.read, 4);
        assert_eq!(relation.headline.build_dependency_pairs.pairs, 5);
        assert_eq!(
            relation.headline.members.unread_reasons,
            BTreeMap::from([
                ("broken".to_string(), UNREAD_FAILED),
                ("unreadable".to_string(), UNREAD_FAILED),
            ])
        );
    }

    /// **An upgraded store is unread, never "read, 0 manifests"** (S-462 task
    /// 2, [FR-WS-33]). A member whose store marks no extraction (`upgraded`
    /// answers `None`) is named unread with [`UNREAD_NOT_EXTRACTED`] through
    /// the lazy relation, beside a failed read's [`UNREAD_FAILED`]; it counts
    /// in neither `read` nor `with_manifests`.
    ///
    /// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
    #[test]
    fn a_member_whose_facts_are_not_yet_extracted_is_unread_with_that_reason() {
        let (_, facts) = estate_shape();
        for (member, rows) in facts {
            set_facts(&member, rows);
        }
        let registry = EngineRegistry::<FakeEngine>::new(
            fed(&["starter", "common", "api", "batch", "upgraded", "unreadable"], &[]),
            RegistryMode::Lazy,
        );
        let members = BuildDependencies::new().relation(&registry).headline.members.clone();
        assert_eq!(members.unread, ["upgraded", "unreadable"]);
        assert_eq!(
            members.unread_reasons,
            BTreeMap::from([
                ("unreadable".to_string(), UNREAD_FAILED),
                ("upgraded".to_string(), UNREAD_NOT_EXTRACTED),
            ])
        );
        assert_eq!((members.read, members.with_manifests), (4, 4));
    }

    /// The reasons serialize under `members`, one per unread member, and are
    /// absent when every member was read — so a fully-read payload does not
    /// move a byte.
    #[test]
    fn unread_reasons_ride_beside_unread_and_vanish_when_every_member_is_read() {
        let roster = fed(&["a", "b", "c"], &[]).members;
        let facts = vec![("a".to_string(), Vec::new())];
        let json = serde_json::to_value(join(&roster, &BTreeMap::new(), &facts, &["b".to_string()]))
            .unwrap();
        assert_eq!(json["headline"]["members"]["unread"], serde_json::json!(["b", "c"]));
        assert_eq!(
            json["headline"]["members"]["unread_reasons"],
            serde_json::json!({"b": UNREAD_NOT_EXTRACTED, "c": UNREAD_FAILED})
        );

        let all_read: Vec<MemberBuildFacts> =
            ["a", "b", "c"].iter().map(|m| ((*m).to_string(), Vec::new())).collect();
        let json = serde_json::to_value(join(&roster, &BTreeMap::new(), &all_read, &[])).unwrap();
        assert!(json["headline"]["members"].get("unread").is_none());
        assert!(json["headline"]["members"].get("unread_reasons").is_none());
    }

    // ── BR-58 / ADR-26: every runtime figure is byte-identical ────────────

    fn symbol(name: &str) -> LogosSymbol {
        LogosSymbol::parse(&format!("local {name}")).unwrap()
    }

    /// Runtime coupling between the same members the build facts join: `api`
    /// serves two routes, `web` calls one and misses one, `batch` publishes to
    /// a topic nobody consumes.
    fn runtime_fixture() {
        SURFACES.with(|s| {
            s.borrow_mut().insert(
                "api".into(),
                vec![
                    ContractNode { kind: NodeKind::Route, name: "GET /users/{id}".into(), symbol: symbol("get_user") },
                    ContractNode { kind: NodeKind::Route, name: "DELETE /users/{id}".into(), symbol: symbol("delete_user") },
                ],
            )
        });
        CALLS.with(|c| {
            let mut c = c.borrow_mut();
            c.insert(
                "common".into(),
                vec![
                    InvocationRef { relation: ArtifactRelation::HttpClientCall, target: "GET /users/{id}".into(), symbol: symbol("fetch") },
                    InvocationRef { relation: ArtifactRelation::HttpClientCall, target: "POST /orders".into(), symbol: symbol("order") },
                ],
            );
            c.insert(
                "batch".into(),
                vec![InvocationRef { relation: ArtifactRelation::BrokerPublish, target: "events".into(), symbol: symbol("emit") }],
            );
        });
    }

    /// The runtime payloads, serialized: the coverage summary (it carries
    /// `resolved_cross_service_edges` and `egress_resolution`), the bridge edge
    /// set and the egress residue (by its full `Debug` rendering).
    fn runtime_bytes(kinds: &[(&str, MemberKind)]) -> [String; 3] {
        let registry = EngineRegistry::<FakeEngine>::new(
            fed(&["starter", "common", "api", "batch"], kinds),
            RegistryMode::Lazy,
        );
        let coverage = cross_service_coverage(&registry.answer());
        let (edges, residue, _) = ContractBridge::new().reachability_read(&registry);
        [
            serde_json::to_string(&coverage).unwrap(),
            serde_json::to_string(&*edges).unwrap(),
            // Not `Serialize` itself (a surface serializes its per-answer
            // projection); its `Debug` rendering is every field, in order.
            format!("{:?}", *residue),
        ]
    }

    /// **Byte-identical before and after** ([BR-58], [ADR-26]). The same
    /// runtime fixture with no build fact, then with the estate's build facts
    /// joined — and a platform declared — serializes every runtime payload to
    /// the same bytes. The fixture is not vacuous: it binds one route edge and
    /// leaves egress unresolved, so each figure is non-trivial.
    ///
    /// [BR-58]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
    #[test]
    fn runtime_figures_and_the_bridge_edge_set_are_byte_identical_with_build_facts() {
        runtime_fixture();
        let before = runtime_bytes(&[]);

        let (_, facts) = estate_shape();
        for (member, rows) in facts {
            set_facts(&member, rows);
        }
        let registry = EngineRegistry::<FakeEngine>::new(
            fed(&["starter", "common", "api", "batch"], &[]),
            RegistryMode::Lazy,
        );
        assert_eq!(BuildDependencies::new().relation(&registry).headline.build_dependency_pairs.pairs, 5);
        let after = runtime_bytes(&[]);
        let after_platform = runtime_bytes(&[("starter", MemberKind::Platform)]);

        let coverage: serde_json::Value = serde_json::from_str(&before[0]).unwrap();
        assert_eq!(coverage["resolved_cross_service_edges"], 1, "the fixture binds an edge");
        assert!(before[1].contains("get_user"), "the edge set is non-empty: {}", before[1]);
        for (i, name) in ["coverage", "bridge edges", "egress residue"].iter().enumerate() {
            assert_eq!(before[i], after[i], "{name} moved when build facts were present");
            assert_eq!(before[i], after_platform[i], "{name} moved when a platform was declared");
        }
    }

    // ── the cross-context model hint and the `xservice build-deps` read-model ──

    /// The estate's shape: one model library per context, adapters depending on
    /// their own context's models and a second one's, a parent POM pinning every
    /// context's models (managed), and near-miss artifact names.
    fn model_library_shape() -> BuildDependencyRelation {
        let names = [
            "starter",
            "archive-kafka-models",
            "mailbox-kafka-models",
            "reporting-kafka-models",
            "archive-reporting-adapter",
            "archive-manager",
            "near-miss",
        ];
        let roster = fed(&names, &[]).members;
        // The estate's shape: each context's library is `<group>.<context>:kafka-models`,
        // produced by a repository named `<context>-kafka-models`; one library
        // uses the prefixed spelling so both are joined in one relation.
        let archive = format!("{G}.archive");
        let mailbox = format!("{G}.mailbox");
        let facts = vec![
            (
                "starter".to_string(),
                vec![pom(vec![
                    produced(G, "poste-pec-starter"),
                    managed(&archive, "kafka-models"),
                    managed(&mailbox, "kafka-models"),
                    managed(G, "reporting-kafka-models"),
                ])],
            ),
            ("archive-kafka-models".to_string(), vec![pom(vec![produced(&archive, "kafka-models")])]),
            ("mailbox-kafka-models".to_string(), vec![pom(vec![produced(&mailbox, "kafka-models")])]),
            (
                "reporting-kafka-models".to_string(),
                vec![pom(vec![produced(G, "reporting-kafka-models")])],
            ),
            (
                "archive-reporting-adapter".to_string(),
                vec![pom(vec![
                    produced(G, "archive-reporting-adapter"),
                    parent(G, "poste-pec-starter"),
                    dependency(&archive, "kafka-models"),
                    scoped(dependency(G, "reporting-kafka-models"), "test"),
                    dependency(&mailbox, "kafka-models"),
                ])],
            ),
            (
                "archive-manager".to_string(),
                vec![pom(vec![produced(G, "archive-manager"), dependency(&archive, "kafka-models")])],
            ),
            (
                "near-miss".to_string(),
                vec![pom(vec![
                    produced(G, "near-miss"),
                    // Each one step from a model library: none names a context.
                    produced(G, "xkafka-models"),
                    produced(&archive, "kafka-model"),
                    dependency(&archive, "kafka-models"),
                ])],
            ),
        ];
        join(&roster, &BTreeMap::new(), &facts, &[])
    }

    #[test]
    fn a_model_librarys_context_is_read_off_its_coordinate_in_either_spelling_and_nothing_else() {
        // The estate's only shape: `<group>.<context>:kafka-models`.
        assert_eq!(model_library_context("com.sourcesense.poste.pec.archive:kafka-models"), Some("archive"));
        assert_eq!(model_library_context("com.sourcesense.poste.pec.officiallog:kafka-models"), Some("officiallog"));
        assert_eq!(model_library_context("archive:kafka-models"), Some("archive"));
        // The prefixed spelling.
        assert_eq!(model_library_context("g:archive-kafka-models"), Some("archive"));
        assert_eq!(model_library_context("g.h:official-log-kafka-models"), Some("official-log"));
        // Near misses, each one step from a match: an empty group segment, an
        // empty prefix, a missing hyphen, a singular artifactId, a suffix
        // mid-name, the name in the groupId only, and no groupId at all.
        assert_eq!(model_library_context("com.acme.:kafka-models"), None);
        assert_eq!(model_library_context(":kafka-models"), None);
        assert_eq!(model_library_context("g:-kafka-models"), None);
        assert_eq!(model_library_context("g:xkafka-models"), None);
        assert_eq!(model_library_context("com.acme.archive:kafka-model"), None);
        assert_eq!(model_library_context("g:archive-kafka-models-api"), None);
        assert_eq!(model_library_context("com.acme.kafka-models:core"), None);
        assert_eq!(model_library_context("kafka-models"), None);
    }

    /// A member depending on two contexts' model libraries is a hint naming
    /// every library and its producer; one context is not; a `managed` pin of
    /// every context is not; and the hint adds no edge ([BR-58]).
    ///
    /// [BR-58]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[test]
    fn a_member_depending_on_two_contexts_model_libraries_is_a_hint_naming_each_library() {
        let relation = model_library_shape();
        let edges_before = relation.edges.clone();
        let hints = relation.cross_context_hints();
        assert_eq!(
            hints,
            vec![CrossContextHint {
                member: "archive-reporting-adapter".to_string(),
                contexts: vec!["archive".to_string(), "mailbox".to_string(), "reporting".to_string()],
                libraries: vec![
                    ModelLibrary {
                        context: "archive".to_string(),
                        artifact: format!("{G}.archive:kafka-models"),
                        member: "archive-kafka-models".to_string(),
                    },
                    ModelLibrary {
                        context: "mailbox".to_string(),
                        artifact: format!("{G}.mailbox:kafka-models"),
                        member: "mailbox-kafka-models".to_string(),
                    },
                    ModelLibrary {
                        context: "reporting".to_string(),
                        artifact: format!("{G}:reporting-kafka-models"),
                        member: "reporting-kafka-models".to_string(),
                    },
                ],
            }],
            "starter only pins (managed), archive-manager and near-miss depend on one context",
        );
        assert_eq!(relation.edges, edges_before, "the hint is a report, never an edge");
    }

    #[test]
    fn two_libraries_of_one_context_are_not_a_cross_context_hint() {
        let roster = fed(&["a-models", "b-models", "user"], &[]).members;
        let facts = vec![
            ("a-models".to_string(), vec![pom(vec![produced("com.acme.archive", "kafka-models")])]),
            ("b-models".to_string(), vec![pom(vec![produced("org.other", "archive-kafka-models")])]),
            (
                "user".to_string(),
                vec![pom(vec![
                    dependency("com.acme.archive", "kafka-models"),
                    dependency("org.other", "archive-kafka-models"),
                ])],
            ),
        ];
        let relation = join(&roster, &BTreeMap::new(), &facts, &[]);
        assert_eq!(relation.edges.len(), 2, "both libraries joined");
        assert!(relation.cross_context_hints().is_empty(), "one context, two producers");
    }

    /// Unscoped: every member read, each edge in its `from`'s `builds_against`
    /// and its `to`'s `built_against_by`. Scoped: that member alone, its hint
    /// alone, and the same workspace headline. An unknown or unread scope is an
    /// empty list **with a note**, never a silent "no edges" ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn xservice_build_deps_lists_every_member_scopes_to_one_and_names_an_unknown_scope() {
        let relation = model_library_shape();
        let all = xservice_build_deps(&relation, None);
        assert_eq!(all.members.len(), 7, "every member read, those with no edge included");
        assert_eq!(all.scope, None);
        assert_eq!(all.scope_note, None);
        assert_eq!(all.headline, relation.headline);
        assert_eq!(all.cross_context.len(), 1);
        let out: usize = all.members.iter().map(|m| m.builds_against.len()).sum();
        let into: usize = all.members.iter().map(|m| m.built_against_by.len()).sum();
        assert_eq!((out, into), (relation.edges.len(), relation.edges.len()));

        let one = xservice_build_deps(&relation, Some("archive-kafka-models"));
        assert_eq!(one.scope.as_deref(), Some("archive-kafka-models"));
        assert_eq!(one.scope_note, None);
        assert_eq!(one.headline, relation.headline, "the denominator is workspace-wide");
        assert_eq!(one.members.len(), 1);
        let by: Vec<&str> = one.members[0].built_against_by.iter().map(|e| e.from.as_str()).collect();
        assert_eq!(by, ["archive-manager", "archive-reporting-adapter", "near-miss", "starter"]);
        assert!(one.cross_context.is_empty(), "the hint is scoped with the rows");

        let adapter = xservice_build_deps(&relation, Some("archive-reporting-adapter"));
        assert_eq!(adapter.cross_context.len(), 1);

        let unknown = xservice_build_deps(&relation, Some("nope"));
        assert!(unknown.members.is_empty());
        // The substance, not just the name: the note exists so an empty list is
        // never read as "no build dependencies".
        assert_eq!(
            unknown.scope_note.as_deref(),
            Some("`nope` is not a member the build relation was read over (not in the workspace)"),
        );
    }

    /// A scope naming an unread roster member states that member's reason —
    /// "not yet extracted" is never folded into "not in the workspace".
    #[test]
    fn a_scope_naming_an_unread_member_states_its_reason() {
        let roster = fed(&["a", "b", "c"], &[]).members;
        let facts = vec![("a".to_string(), Vec::new())];
        let relation = join(&roster, &BTreeMap::new(), &facts, &["b".to_string()]);
        let note = |m| xservice_build_deps(&relation, Some(m)).scope_note;
        assert_eq!(
            note("b").as_deref(),
            Some("`b` is not a member the build relation was read over (build facts not yet extracted)")
        );
        assert_eq!(
            note("c").as_deref(),
            Some("`c` is not a member the build relation was read over (build facts could not be read)")
        );
        assert_eq!(note("a"), None, "a member read answers, even with no edge");
    }

    /// The wire shape the three surfaces print: each row names `kind`, `scope`
    /// and `artifact`; an undeclared scope is `null`, never defaulted; the scope
    /// keys are absent unscoped.
    #[test]
    fn the_build_deps_payload_names_kind_scope_and_artifact_on_every_row() {
        let json = serde_json::to_value(xservice_build_deps(&model_library_shape(), None)).unwrap();
        assert!(json.get("scope").is_none() && json.get("scope_note").is_none(), "{json}");
        assert!(json["headline"]["summary"].as_str().unwrap().contains("never a runtime coupling"));
        let adapter = json["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["member"] == "archive-reporting-adapter")
            .unwrap();
        let rows = adapter["builds_against"].as_array().unwrap();
        let test_row = rows.iter().find(|r| r["scope"] == "test").expect("a declared scope");
        assert_eq!(test_row["kind"], "dependency");
        assert_eq!(test_row["artifact"], format!("{G}:reporting-kafka-models"));
        assert_eq!(json["cross_context"][0]["libraries"][0]["artifact"], format!("{G}.archive:kafka-models"));
        assert_eq!(test_row["to"], "reporting-kafka-models");
        let parent_row = rows.iter().find(|r| r["kind"] == "parent").unwrap();
        assert!(parent_row["scope"].is_null(), "undeclared is null, never `compile`");
        assert_eq!(json["cross_context"][0]["libraries"][1]["context"], "mailbox");
    }
}
