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
//! - **to member** — exactly one other member produces it: an edge.
//!
//! # Platform members ([ADR-69] point 3)
//! A member declared `kind = "platform"` has its **inbound** edges counted
//! apart ([`MemberKind::sets_inbound_build_edges_apart`]): they leave the
//! headline for [`PlatformApart`], stated over the same denominator. Logos never
//! declares one: the status lists [`PlatformCandidate`]s by in-degree share and
//! classifies nothing.
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

use crate::graph_store::BuildManifestRow;

use super::bridge::{current_stamps, read_members, MemberContracts, StampCache};
use super::manifest::MemberKind;
use super::registry::{EngineRegistry, MemberEngine};
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
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Parent => "parent",
            Self::Dependency => "dependency",
            Self::Managed => "managed",
            Self::BomImport => "bom-import",
        }
    }

    /// The kind a fact vocabulary token (`build_artifacts.kind`) names, or
    /// `None` for any other text. Migration 22's CHECK admits only these four.
    #[must_use]
    pub fn from_fact(token: &str) -> Option<Self> {
        match token {
            "parent" => Some(Self::Parent),
            "dependency" => Some(Self::Dependency),
            "managed" => Some(Self::Managed),
            "bom-import" => Some(Self::BomImport),
            _ => None,
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
    /// Resolved to exactly one other member — each is part of an edge.
    pub to_member: u64,
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
    /// or the read failed), by name, in roster order. Absent when none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unread: Vec<String>,
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
}

/// One member's build-manifest facts as read: its name and every manifest row
/// its store holds — what [`join`] takes, one entry per member read.
pub type MemberBuildFacts = (String, Vec<BuildManifestRow>);

/// Join member-local build facts into the relation ([FR-WS-33]).
///
/// `roster` is the workspace's member list (manifest order); `facts` holds one
/// entry per member whose facts were read, and a roster member absent from it
/// is reported [`unread`](MembersRead::unread). `kinds` is the resolved
/// `[workspace.member.<name>] kind` map. Pure: no I/O, deterministic output.
///
/// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
pub fn join(
    roster: &[Member],
    kinds: &BTreeMap<String, MemberKind>,
    facts: &[MemberBuildFacts],
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
            None => members.unread.push(member.name.clone()),
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

    let mut references = ReferenceAccounting::default();
    let mut collided: BTreeMap<&str, u64> = BTreeMap::new();
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
                    }
                    Some((_, set)) if set.contains(member) => references.in_member += 1,
                    Some((_, set)) => {
                        references.to_member += 1;
                        let to = set.iter().next().expect("a non-empty producer set");
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

    let is_platform = |member: &str| {
        kinds
            .get(member)
            .is_some_and(|kind| kind.sets_inbound_build_edges_apart())
    };
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
                "{} into {} declared platform member(s), counted apart from the headline \
                 (the same {} references are the denominator)",
                pairs.render(),
                platforms.len(),
                references.references
            ),
            members: platforms,
            build_dependency_pairs: pairs,
        }
    });

    let platform_candidates = platform_candidates(&edges, kinds, members.read);
    let summary = format!(
        "{} built against another member, from {} of {} referenced artifacts \
         ({} external, {} in-member, {} to a colliding artifact, {} refused), over {} of {} \
         members read; a build dependency, never a runtime coupling",
        headline_pairs.render(),
        references.to_member,
        references.references,
        references.external,
        references.in_member,
        references.to_collision,
        references.refused,
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
    /// a warning and named in [`MembersRead::unread`] ([ADR-53]).
    ///
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    pub fn relation<E>(&self, registry: &EngineRegistry<E>) -> Arc<BuildDependencyRelation>
    where
        E: MemberEngine + MemberContracts,
    {
        let answer = registry.answer();
        let stamps = current_stamps(&answer);
        self.cache.get_or_compute(stamps, || {
            let facts = read_members(&answer, "build-manifest facts", |engine| {
                engine.build_manifests()
            });
            let federation = registry.federation();
            join(&federation.members, &federation.member_kinds, &facts)
        })
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
        fn build_manifests(&self) -> Result<Vec<BuildManifestRow>> {
            READS.with(|r| *r.borrow_mut().entry(self.member.clone()).or_default() += 1);
            if self.member == "unreadable" {
                anyhow::bail!("store read failed");
            }
            Ok(FACTS.with(|f| f.borrow().get(&self.member).cloned().unwrap_or_default()))
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
        let relation = join(&roster, &BTreeMap::new(), &facts);

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
                in_member: 2,
                to_collision: 0,
                external: 1,
                refused: 1,
                project_reference: 0,
                build_plugin: 0,
            }
        );
        assert_eq!(
            r.to_member + r.in_member + r.to_collision + r.external + r.refused
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
                with_manifests: 4,
                manifests: 5,
                manifests_read: 5
            }
        );
        assert!(headline.collisions.is_empty());
        assert!(headline.platform_apart.is_none(), "no platform declared, nothing apart");
        assert!(
            headline.summary.starts_with("5 pairs (parent 3 · dependency 2 · managed 1 · bom-import 1)")
                && headline.summary.contains("from 7 of 11 referenced artifacts")
                && headline.summary.contains("over 4 of 4 members read")
                && headline.summary.contains("never a runtime coupling"),
            "{}",
            headline.summary
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
        let relation = join(&roster, &BTreeMap::new(), &facts);
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
            ("mailbox-api".to_string(), vec![pom(vec![produced(core, "api")])]),
            (
                "mailbox-manager".to_string(),
                vec![pom(vec![produced(core, "manager"), dependency(core, "api")])],
            ),
        ];
        let relation = join(&roster, &BTreeMap::new(), &facts);

        assert!(relation.edges.is_empty(), "resolves to neither producer: {:?}", relation.edges);
        let headline = &relation.headline;
        assert_eq!(headline.build_dependency_pairs.pairs, 0);
        assert_eq!(headline.references.to_collision, 1);
        assert_eq!(headline.references.to_member, 0);
        assert_eq!(
            headline.collisions,
            [ArtifactCollision {
                artifact: format!("{core}:api"),
                producers: vec!["deprecated-mailbox-core".into(), "mailbox-api".into()],
                references: 1,
            }]
        );
        assert!(headline.summary.contains("1 to a colliding artifact"), "{}", headline.summary);
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
        let relation = join(&roster, &BTreeMap::new(), &facts);
        assert!(relation.headline.collisions.is_empty());
        assert_eq!(edge_keys(&relation), [("app", "lib", BuildEdgeKind::Dependency, "com.sourcesense.poste.pec:lib")]);
    }

    /// A refused **produced** fact (every Gradle one, and a Maven one whose key
    /// did not resolve) is no producer: a reference to that coordinate is
    /// external, never an edge.
    #[test]
    fn a_refused_produced_fact_produces_nothing() {
        let roster = fed(&["lib", "app"], &[]).members;
        let mut gradle_produced = produced(G, "lib");
        gradle_produced.resolution = "refused".to_string();
        gradle_produced.artifact_id = None;
        gradle_produced.reason = Some("the artifact name comes from settings.gradle".into());
        let facts = vec![
            ("lib".to_string(), vec![manifest("gradle", "build.gradle", vec![gradle_produced])]),
            ("app".to_string(), vec![pom(vec![produced(G, "app"), dependency(G, "lib")])]),
        ];
        let relation = join(&roster, &BTreeMap::new(), &facts);
        assert!(relation.edges.is_empty());
        assert_eq!(relation.headline.references.external, 1);
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
        let relation = join(&roster, &BTreeMap::new(), &facts);
        let r = relation.headline.references;
        assert_eq!((r.project_reference, r.build_plugin, r.to_member), (1, 1, 1));
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
        let relation = join(&roster, &kinds, &facts);
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
        assert!(apart.summary.contains("the same 11 references"), "{}", apart.summary);
        assert_eq!(
            headline.references,
            join(&roster, &BTreeMap::new(), &facts).headline.references,
            "the denominator does not move by a declaration"
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
        let apart = join(&roster, &kinds, &facts).headline.platform_apart.unwrap();
        assert_eq!(apart.members, ["batch"]);
        assert_eq!(apart.build_dependency_pairs, PairCount::default());

        let kinds = BTreeMap::from([
            ("starter".to_string(), MemberKind::Documentation),
            ("common".to_string(), MemberKind::Mock),
        ]);
        let relation = join(&roster, &kinds, &facts);
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
        let relation = join(&roster, &BTreeMap::new(), &facts);
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
        let relation = join(&fed(&names_10, &[]).members, &BTreeMap::new(), &facts);
        assert_eq!(
            relation.headline.platform_candidates,
            [PlatformCandidate { member: "hub".into(), in_degree: 7, of: 9 }]
        );

        // Declared as anything, it is no longer a candidate.
        let kinds = BTreeMap::from([("hub".to_string(), MemberKind::Platform)]);
        assert!(join(&fed(&names_10, &[]).members, &kinds, &facts)
            .headline
            .platform_candidates
            .is_empty());
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
        let relation = join(&roster, &BTreeMap::new(), &facts);
        assert_eq!(relation.headline.build_dependency_pairs.pairs, 1);
        assert!(relation.headline.platform_candidates.is_empty());
    }

    /// A roster member whose facts were not read is named unread and counted
    /// out of the denominator; the per-member view answers only for members read.
    #[test]
    fn an_unread_member_is_named_and_the_per_member_view_answers_for_members_read() {
        let (mut roster, facts) = estate_shape();
        roster.push(Member { name: "broken".into(), root: PathBuf::from("/ws/broken") });
        let relation = join(&roster, &BTreeMap::new(), &facts);
        assert_eq!(relation.headline.members.members, 5);
        assert_eq!(relation.headline.members.read, 4);
        assert_eq!(relation.headline.members.unread, ["broken"]);
        assert!(relation.headline.summary.contains("over 4 of 5 members read"));
        assert!(relation.member("broken").is_none());

        let starter = relation.member("starter").expect("read");
        assert!(starter.builds_against.is_empty());
        assert_eq!(starter.built_against_by.len(), 4);
        let views = relation.per_member();
        assert_eq!(
            views.iter().map(|v| v.member.as_str()).collect::<Vec<_>>(),
            ["starter", "common", "api", "batch"],
            "roster order, members with no edge included"
        );
    }

    /// A workspace with no manifest joins to an empty relation with its
    /// denominators stated, and serializes with no platform key.
    #[test]
    fn a_workspace_without_manifests_joins_to_nothing_and_says_so() {
        let roster = fed(&["a", "b"], &[]).members;
        let facts = vec![("a".to_string(), Vec::new()), ("b".to_string(), Vec::new())];
        let relation = join(&roster, &BTreeMap::new(), &facts);
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
        let (edges, residue) = ContractBridge::new().reachability_inputs(&registry);
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
}
