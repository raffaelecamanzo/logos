//! The **app-wide cross-service reachability union view** ([FR-WS-12], [ADR-56]).
//!
//! A separate, explicitly-labeled union over every member's `Calls`/`RoutesTo`
//! adjacency **plus** the [bridge](super::bridge)'s cross-service edges, whose
//! provider endpoints are folded in as **additional live roots**. It answers the
//! one question the per-repo dead-code verdict structurally cannot: *is this
//! callable dead only because the thing that calls it lives in another repo?*
//!
//! # Additive and monotone toward live ([ADR-56])
//!
//! The composition can only ever *add* reachability. That is not a discipline
//! this module remembers to apply — it is the shape of the computation:
//!
//! - The per-repo verdict already **is** reachability from the per-repo roots
//!   over `Calls`/`RoutesTo` ([`crate::annotate`], [FR-AN-01]).
//! - A [`BridgeEdge`](super::BridgeEdge) contributes **roots**, never adjacency.
//! - Reachability from `per-repo roots ∪ bridge roots` therefore equals
//!   `per-repo live set ∪ closure(bridge roots)` over the *same* adjacency.
//!
//! So the union view starts from the persisted per-repo verdict and only ever
//! flips `is_dead = Some(true)` → live. A **demotion is unrepresentable**: a node
//! that is live per-repo is never revisited, and a node whose verdict is `NULL`
//! ("not computed" — its language does not declare the reachability capability,
//! [CR-043]) yields **no claim at all**, in either direction ([NFR-CC-04]). The
//! app-wide dead set is a strict subset of the per-repo dead set, always.
//!
//! The corollary is the honest one: a *missing* invocation edge (an arm that has
//! not yet extracted the caller) can only fail to promote — it can never demote a
//! live handler to dead ([FR-WS-12], [NFR-RA-05]).
//!
//! # Every provider endpoint is a root
//!
//! The view roots the provider (`to`) endpoint of **every** bridge edge, not just
//! the invocation-arm ones ([FR-WS-08]–[FR-WS-10]). A superset of roots is a
//! superset of *live*, so this is the monotone-toward-live choice; and it needs no
//! per-arm knowledge, so a newly-registered arm reaches the union view with no
//! edit here.
//!
//! # Why the promotion set is empty on every real workspace today
//!
//! The composition is correct and its roots **do** resolve — since S-256 the broker
//! arm emits bridge edges whose provider endpoint is the real subscribing method,
//! an ordinary callable (not a promoted marker node). What still cannot happen is
//! the *promotion itself*, and the reason is a *language-capability* gap, not a
//! missing arm:
//!
//! - A node is a promotion candidate only if its per-repo verdict is
//!   `is_dead = Some(true)` (see [`member_view`]). Everything else is skipped.
//! - `is_dead` is only computed for a file whose language declares the
//!   `reachability` capability ([CR-043], [`crate::annotate`]); otherwise it is
//!   `NULL`. Today **`rust` is the only such language**.
//! - Capturing a broker *subscribe* requires the language to ship a `brokers.scm`
//!   query. Today **`java` is the only such language**.
//!
//! The two sets are **disjoint**, so no node can be both broker-rooted and
//! dead-verdicted, and the promotion bucket is provably empty on a real index.
//! (Behind that sits a second gate: the canonical Spring listener is `public`, and
//! java's `public-modifier` export convention makes an exported node a per-repo
//! live root — `is_dead = Some(false)` — so even a reachability-capable java would
//! only promote a *package-private* listener.)
//!
//! This is the view being correct and inert, not the view being wrong — but the
//! inertness is a property of the **plugin capability matrix**, so it is pinned
//! there by `federation::reach::tests::the_broker_promotion_path_is_still_blocked_
//! by_the_capability_matrix`, which fails the day a language becomes both
//! broker-capable and reachability-capable. That is the moment to write the
//! real-path promotion E2E [FR-WS-12] AC1 ultimately wants.
//!
//! # Advisory, never a gate input ([ADR-56])
//!
//! Structurally, not by convention — exactly as the [coverage](super::coverage)
//! tier: this module is reachable only through an [`EngineRegistry`], which exists
//! only when a workspace manifest is present. `scan` / `gate` / `check_rules`
//! ([`crate::governance`]) operate on a single [`Engine`](crate::Engine) and have
//! no dependency on `federation` at all, so the union view is *incapable* of
//! moving a member's gated signal. Nothing here writes: the per-repo `is_dead`
//! column is read, never set.
//!
//! # The coverage rider ([FR-WS-05], [ADR-53])
//!
//! App-wide reachability is only as complete as the *materialized* invocation
//! graph, so every claim rides with a [`CoverageRider`] stating how much of that
//! graph bound. The rider is carried on each individual [`ReachabilityClaim`] —
//! deliberately redundant with [`AppWideReachability::coverage`] — so a claim
//! cannot be filtered, quoted, or re-serialised apart from the coverage it rests
//! on ([NFR-CC-04]).
//!
//! [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
//! [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
//! [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
//! [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
//! [FR-WS-12]: ../../../docs/specs/requirements/FR-WS-12.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
//! [ADR-56]: ../../../docs/specs/architecture/decisions/ADR-56.md
//! [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::graph_store::{AnnotationNodeRow, EdgeRow, NodeRow};
use crate::model::{EdgeKind, LogosSymbol, NodeId, NodeKind};

use super::bridge::{read_members, BridgeEdge, MemberContracts};
use super::coverage::{cross_service_coverage, CrossServiceCoverage};
use super::registry::{EngineRegistry, MemberEngine};

/// The view's label ([ADR-56]) — the union view is *explicitly labeled* so no
/// consumer can mistake it for a member's gated per-repo dead-code signal.
pub const UNION_VIEW: &str = "cross-service-union";

/// One node of a member's reachability surface: its identity, and the per-repo
/// dead-code verdict the union view starts from.
///
/// Carries the **tri-state** verdict verbatim ([FR-AN-01]): `Some(true)` dead,
/// `Some(false)` live, `None` not computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachNode {
    /// The node ontology kind.
    pub kind: NodeKind,
    /// The human-facing name.
    pub name: String,
    /// The node's canonical, database-portable symbol identity — the identity a
    /// [`BridgeEndpoint`](super::BridgeEndpoint) roots on.
    pub symbol: LogosSymbol,
    /// The member's own per-repo dead-code verdict, read (never written).
    pub is_dead: Option<bool>,
}

/// One member's **reachability surface**: the nodes the union view claims over,
/// and the `Calls`/`RoutesTo` adjacency it walks ([FR-WS-12]).
///
/// The adjacency is expressed in **surface-local** indices into
/// [`nodes`](Self::nodes) — never a [`NodeId`] — so no per-database rowid can
/// leak into an overlay endpoint ([ADR-52]), the same discipline
/// [`ContractNode`](super::ContractNode) keeps.
///
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReachabilitySurface {
    /// The member's nodes; a node's index in this vector is its surface-local id.
    pub nodes: Vec<ReachNode>,
    /// `(source, target)` surface-local index pairs for every `Calls`/`RoutesTo`
    /// edge — the only adjacency the live-set walk traverses ([FR-AN-01]).
    pub edges: Vec<(u32, u32)>,
}

/// How much of the cross-service invocation graph was actually materialized —
/// the rider **every** reachability claim carries ([FR-WS-05], [ADR-53]).
///
/// An app-wide claim is only as complete as the edges that bound. A caller that
/// reads `dead` without reading this is over-reading the evidence, which is why
/// the rider rides on each claim rather than only on the envelope.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CoverageRider {
    /// Cross-boundary references bound to exactly one cross-member provider.
    pub bound: u64,
    /// References with 2+ providers across the workspace (no edge, never
    /// fabricated).
    pub ambiguous: u64,
    /// References unbound for a reason other than ambiguity or
    /// no-provider-in-workspace.
    pub unbound: u64,
    /// References with no provider anywhere in the workspace — outside this
    /// workspace's boundary, not a defect ([ADR-53]).
    pub no_provider_in_workspace: u64,
    /// `bound / (bound + ambiguous + unbound)` ([ADR-53]).
    pub bound_ratio: f64,
    /// Members whose reachability surface was read successfully.
    pub members_read: u64,
    /// Members declared in the workspace. `members_read < members_total` means a
    /// member degraded (engine-start or read failure) and contributed **no**
    /// nodes and **no** roots — so it can only have suppressed promotions, never
    /// caused a demotion ([ADR-53]).
    pub members_total: u64,
}

impl CoverageRider {
    /// Fold the 3-state coverage read-model plus the member-read tally into the
    /// rider ([FR-WS-05]).
    fn new(coverage: &CrossServiceCoverage, members_read: usize, members_total: usize) -> Self {
        Self {
            bound: coverage.bound,
            ambiguous: coverage.ambiguous,
            unbound: coverage.unbound,
            no_provider_in_workspace: coverage.no_provider_in_workspace,
            bound_ratio: coverage.bound_ratio,
            members_read: members_read as u64,
            members_total: members_total as u64,
        }
    }
}

/// A node's verdict **in the union view** ([FR-WS-12]).
///
/// Only ever attached to a node the member's own graph already called dead — the
/// union view has no vocabulary for demoting anything, so there is deliberately
/// no "newly dead" variant to construct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppWideVerdict {
    /// Dead in its own repo, **live** app-wide: reached from a cross-service
    /// provider endpoint the bridge matched. The promotion this view exists for.
    LiveViaCrossService,
    /// Dead in its own repo and still unreached across the union — dead app-wide,
    /// *up to the [`CoverageRider`]* riding on this claim.
    Dead,
}

/// One node's app-wide reachability claim, with the coverage it rests on
/// ([FR-WS-12], [ADR-56]).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReachabilityClaim {
    /// The owning member's name (its workspace-relative path) — every claim is
    /// repo-qualified ([FR-WS-03]).
    ///
    /// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
    pub member: String,
    /// The claimed node's portable symbol identity.
    pub symbol: LogosSymbol,
    /// The claimed node's human-facing name.
    pub name: String,
    /// The claimed node's ontology kind — every other dead-code surface in the
    /// product is kind-qualified, so a consumer reading `dead` can tell what it
    /// is looking at without a second lookup.
    pub kind: NodeKind,
    /// The union-view verdict.
    pub verdict: AppWideVerdict,
    /// The coverage rider this claim is only as good as — carried per claim by
    /// design (see the module docs), so it cannot be separated from the claim.
    pub coverage: CoverageRider,
}

/// One member's union-view tally ([FR-WS-12]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberReachability {
    /// The member's name.
    pub member: String,
    /// Bridge provider endpoints that resolved to a node here and seeded the walk.
    pub extra_roots: u64,
    /// Bridge provider endpoints naming a symbol this member's surface does not
    /// carry (a stale or unindexed endpoint). Reported rather than silently
    /// dropped — an unresolved root simply promotes nothing ([NFR-RA-05]).
    pub unresolved_roots: u64,
    /// Callables this member's own graph verdicts as dead — the promotion base.
    pub dead_per_repo: u64,
    /// Of those, the ones the union view promotes to live.
    pub live_via_cross_service: u64,
    /// Of those, the ones still dead app-wide. Always
    /// `dead_per_repo - live_via_cross_service` — the monotonicity invariant, in
    /// arithmetic.
    pub dead_app_wide: u64,
}

/// The app-wide cross-service reachability union view ([FR-WS-12], [ADR-56]).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppWideReachability {
    /// The view label ([`UNION_VIEW`]) — this is the union, not a member's gated
    /// signal.
    pub view: &'static str,
    /// Always `true`: this view is advisory and is never a gate input ([ADR-56]).
    /// Serialised as a field rather than left implicit so the label travels with
    /// the payload to every surface.
    pub advisory: bool,
    /// The coverage rider for the whole view (identical to the one on each claim).
    pub coverage: CoverageRider,
    /// Per-member tallies, sorted by member ([NFR-RA-06]).
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub members: Vec<MemberReachability>,
    /// Members the view could **not** read — engine-start or surface-read failure
    /// ([ADR-53]) — by name, sorted. These carry no tally and no claims, so
    /// without naming them the only trace of the shortfall is
    /// `coverage.members_read < coverage.members_total`, which tells a reader
    /// *that* the workspace degraded but not *where*. A skipped member contributes
    /// no nodes and no roots, so it can only suppress a promotion, never demote.
    pub skipped_members: Vec<String>,
    /// The promotions: dead per-repo, live across the union. Sorted.
    pub live_via_cross_service: Vec<ReachabilityClaim>,
    /// The app-wide dead set — a strict subset of the union of the per-repo dead
    /// sets. Sorted.
    pub dead: Vec<ReachabilityClaim>,
}

/// Compute the app-wide cross-service reachability union view over `registry`'s
/// members and the bridge `edges` ([FR-WS-12], [ADR-56]).
///
/// `edges` is the bridge's cross-service edge set (the caller passes the cached
/// set, exactly as the [`query`](super::query) read-models do). A member whose
/// engine fails to start, or whose surface read fails, is **skipped** (degraded,
/// not fatal, [ADR-53]) — and because a skipped member contributes neither nodes
/// nor roots, degradation can only ever suppress a promotion, never manufacture a
/// dead verdict.
pub fn app_wide_reachability<E>(
    registry: &EngineRegistry<E>,
    edges: &[BridgeEdge],
) -> AppWideReachability
where
    E: MemberEngine + MemberContracts,
{
    let coverage = cross_service_coverage(registry);
    let surfaces = read_members(registry, "reachability surface", |e| e.reachability_surface());
    let rider = CoverageRider::new(&coverage, surfaces.len(), registry.members().len());
    let roots = union_roots(edges);

    let mut members = Vec::with_capacity(surfaces.len());
    let mut live_via_cross_service = Vec::new();
    let mut dead = Vec::new();

    for (member, surface) in &surfaces {
        let seeds: &[&LogosSymbol] = roots.get(member.as_str()).map_or(&[], Vec::as_slice);
        let (tally, claims) = member_view(member, surface, seeds, rider);
        for claim in claims {
            match claim.verdict {
                AppWideVerdict::LiveViaCrossService => live_via_cross_service.push(claim),
                AppWideVerdict::Dead => dead.push(claim),
            }
        }
        members.push(tally);
    }

    // The members `read_members` dropped as degraded — named, not just counted.
    let mut skipped_members: Vec<String> = registry
        .members()
        .iter()
        .map(|m| m.name.clone())
        .filter(|name| !surfaces.iter().any(|(read, _)| read == name))
        .collect();

    // Deterministic output regardless of member fan-out order ([NFR-RA-06]).
    members.sort_by(|a, b| a.member.cmp(&b.member));
    skipped_members.sort();
    live_via_cross_service.sort_by(claim_order);
    dead.sort_by(claim_order);

    AppWideReachability {
        view: UNION_VIEW,
        advisory: true,
        coverage: rider,
        members,
        skipped_members,
        live_via_cross_service,
        dead,
    }
}

/// The extra live roots the bridge contributes, grouped by the member that owns
/// them: the **provider** (`to`) endpoint of every cross-service edge ([ADR-56]).
///
/// The consumer (`from`) side is deliberately *not* a root — a call site is not an
/// entry point, and rooting it would promote the caller's own dead neighbourhood
/// on no evidence.
fn union_roots(edges: &[BridgeEdge]) -> HashMap<&str, Vec<&LogosSymbol>> {
    let mut roots: HashMap<&str, Vec<&LogosSymbol>> = HashMap::new();
    for edge in edges {
        roots
            .entry(edge.to.member.as_str())
            .or_default()
            .push(&edge.to.symbol);
    }
    // One seed per **distinct** provider endpoint. Several consumers calling the
    // same route (or publishing to the same subscribed topic) is the common shape,
    // and it is *one* extra live root, not one per inbound edge. The walk itself
    // does not care — re-seeding is idempotent against a `HashSet` — but the
    // tallies count seeds, so without this `extra_roots` would silently report
    // "inbound cross-service edges" while its name and doc promise "provider
    // endpoints" ([NFR-CC-04]: a reported number must mean what it says).
    for seeds in roots.values_mut() {
        seeds.sort_unstable_by_key(|symbol| symbol.as_str());
        seeds.dedup();
    }
    roots
}

/// One member's tally + claims: walk its adjacency from the bridge `seeds`, then
/// verdict **only** the nodes its own graph already called dead.
///
/// The `is_dead != Some(true)` skip is the whole tri-state guarantee: a `None`
/// ("not computed") node and a live node are both passed over silently, so
/// neither can acquire an app-wide verdict it has no basis for ([NFR-CC-04]).
fn member_view(
    member: &str,
    surface: &ReachabilitySurface,
    seeds: &[&LogosSymbol],
    coverage: CoverageRider,
) -> (MemberReachability, Vec<ReachabilityClaim>) {
    let walk = walk_union(surface, seeds);
    let mut tally = MemberReachability {
        member: member.to_string(),
        extra_roots: walk.roots_resolved,
        unresolved_roots: walk.roots_unresolved,
        dead_per_repo: 0,
        live_via_cross_service: 0,
        dead_app_wide: 0,
    };
    let mut claims = Vec::new();

    for (index, node) in surface.nodes.iter().enumerate() {
        if node.is_dead != Some(true) {
            continue; // live, or NULL — the union view has nothing to say.
        }
        tally.dead_per_repo += 1;
        let verdict = if walk.reached.contains(&(index as u32)) {
            tally.live_via_cross_service += 1;
            AppWideVerdict::LiveViaCrossService
        } else {
            tally.dead_app_wide += 1;
            AppWideVerdict::Dead
        };
        claims.push(ReachabilityClaim {
            member: member.to_string(),
            symbol: node.symbol.clone(),
            name: node.name.clone(),
            kind: node.kind,
            verdict,
            coverage,
        });
    }

    (tally, claims)
}

/// What one member's union walk reached, and how its roots resolved.
///
/// [`default`](Default::default) is the no-roots outcome: nothing reached,
/// nothing to resolve — which is precisely "this member gains no reachability
/// from the bridge", the identity element of a monotone-toward-live composition.
#[derive(Default)]
struct Walk {
    /// Surface-local indices reachable from the bridge roots.
    reached: HashSet<u32>,
    /// Roots that named a node in this surface.
    roots_resolved: u64,
    /// Roots that named a symbol this surface does not carry.
    roots_unresolved: u64,
}

/// BFS a member's `Calls`/`RoutesTo` adjacency from the bridge-contributed roots
/// ([FR-WS-12]).
///
/// This walks **only** the extra roots' closure — the per-repo roots' closure is
/// already baked into the `is_dead` column [`member_view`] reads, so re-walking it
/// would be redundant work for a byte-identical answer.
///
/// A symbol backing more than one node roots **all** of them: the false-live bias
/// ([AR-05]) says an ambiguity in the root's identity must resolve toward live, not
/// toward a fabricated dead verdict.
///
/// [AR-05]: ../../../docs/specs/architecture.md#13-risk-register
fn walk_union(surface: &ReachabilitySurface, seeds: &[&LogosSymbol]) -> Walk {
    // A member no bridge edge points into reaches nothing extra, so skip building
    // its symbol index and adjacency entirely — that is the common case (most
    // members are not the provider side of a cross-service call), and the surface
    // it would index is the member's whole graph.
    if seeds.is_empty() {
        return Walk::default();
    }

    let mut by_symbol: HashMap<&LogosSymbol, Vec<u32>> = HashMap::new();
    for (index, node) in surface.nodes.iter().enumerate() {
        by_symbol.entry(&node.symbol).or_default().push(index as u32);
    }
    let mut adjacency: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(source, target) in &surface.edges {
        adjacency.entry(source).or_default().push(target);
    }

    let mut reached: HashSet<u32> = HashSet::new();
    let mut queue: VecDeque<u32> = VecDeque::new();
    let mut roots_resolved = 0u64;
    let mut roots_unresolved = 0u64;
    for seed in seeds {
        let Some(ids) = by_symbol.get(*seed) else {
            roots_unresolved += 1;
            continue;
        };
        roots_resolved += 1;
        for &id in ids {
            if reached.insert(id) {
                queue.push_back(id);
            }
        }
    }
    while let Some(current) = queue.pop_front() {
        let Some(next) = adjacency.get(&current) else {
            continue;
        };
        for &callee in next {
            if reached.insert(callee) {
                queue.push_back(callee);
            }
        }
    }

    Walk {
        reached,
        roots_resolved,
        roots_unresolved,
    }
}

/// Total order on claims: `(member, symbol)` — deterministic output ([NFR-RA-06]).
fn claim_order(a: &ReachabilityClaim, b: &ReachabilityClaim) -> std::cmp::Ordering {
    (&a.member, a.symbol.as_str()).cmp(&(&b.member, b.symbol.as_str()))
}

/// Project a member's `(nodes, annotations, edges)` read onto its
/// [`ReachabilitySurface`] ([FR-WS-12]).
///
/// The per-repo dead-code verdict comes from the `annotations` read (the
/// `is_dead` column [`crate::annotate`] last wrote) and is joined onto the
/// symbol-carrying `nodes` read by node id; a node the annotation snapshot does
/// not carry renders `None` ("not computed"), never a fabricated `false`
/// ([NFR-CC-04]). The node ids are then discarded — only surface-local indices
/// survive into the overlay type ([ADR-52]).
///
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
pub(super) fn surface_from(
    nodes: &[NodeRow],
    annotations: &[AnnotationNodeRow],
    edges: &[EdgeRow],
) -> ReachabilitySurface {
    let verdict: HashMap<NodeId, Option<bool>> =
        annotations.iter().map(|a| (a.id, a.is_dead)).collect();

    let mut index: HashMap<NodeId, u32> = HashMap::with_capacity(nodes.len());
    let mut surface_nodes = Vec::with_capacity(nodes.len());
    for node in nodes {
        // A surface indexes into a `u32`; a graph beyond that is not a graph this
        // advisory view can address, so stop rather than wrap (unreachable in
        // practice — no member repo has 4 billion nodes).
        let Ok(local) = u32::try_from(surface_nodes.len()) else {
            break;
        };
        index.insert(node.id, local);
        surface_nodes.push(ReachNode {
            kind: node.kind,
            name: node.name.clone(),
            symbol: node.symbol.clone(),
            is_dead: verdict.get(&node.id).copied().flatten(),
        });
    }

    // The same adjacency the per-repo live-set walk uses ([FR-AN-01]): `Calls`
    // plus `RoutesTo`, and nothing else. The per-repo walk *additionally* drops
    // derived nodes and their edges; this projection does not, and does not need
    // to — a derived artifact is a policy node whose only edge kind is
    // `ForbiddenDependency`, which this filter already excludes. The two
    // adjacencies are therefore equal, which is what lets the union view reuse the
    // per-repo verdict instead of re-walking (see the module docs). An edge whose
    // endpoint fell outside the indexed nodes is dropped rather than fabricated.
    let adjacency = edges
        .iter()
        .filter(|edge| matches!(edge.kind, EdgeKind::Calls | EdgeKind::RoutesTo))
        .filter_map(|edge| Some((*index.get(&edge.source)?, *index.get(&edge.target)?)))
        .collect();

    ReachabilitySurface {
        nodes: surface_nodes,
        edges: adjacency,
    }
}

#[cfg(test)]
mod tests;
