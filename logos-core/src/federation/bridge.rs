//! The in-memory **cross-service contract bridge** ([FR-WS-04], [ADR-52]).
//!
//! The bridge is the overlay's matcher: it reads each workspace member's
//! **contract surface** — its [`Route`](NodeKind::Route),
//! [`ApiOperation`](NodeKind::ApiOperation),
//! [`ProtoService`](NodeKind::ProtoService),
//! [`ProtoMessage`](NodeKind::ProtoMessage), and [`GqlType`](NodeKind::GqlType)
//! nodes — through that member's **read pool** (via the [`EngineRegistry`]
//! fan-out, [FR-WS-03]), indexes them on **portable keys**, and matches those
//! keys **across members** with the exactly-one rule. The result is a set of
//! [`BridgeEdge`] values held **in memory only**.
//!
//! # Portable keys, never `NodeId`s ([ADR-52])
//! A [`NodeId`](crate::model::NodeId) is a SQLite rowid — per-database and
//! meaningless across members — so it can never be an overlay endpoint. The
//! bridge matches on a **portable** identity instead: the shared
//! [`route_key`](crate::resolve::route_template::route_key) for HTTP endpoints
//! ([FR-CG-09]) — a `(METHOD, positionally-normalized template)` both an
//! `ApiOperation` and a framework `Route` reduce to. Every [`BridgeEdge`]
//! endpoint is a [`BridgeEndpoint`] carrying `(member, LogosSymbol)`; the type
//! has **no** `NodeId` field, so the "no id crosses a database boundary"
//! invariant is structurally impossible to violate.
//!
//! # Exactly-one across members ([NFR-RA-05])
//! Providers of a key are collected across the **whole** workspace. A consumer
//! key that resolves to **exactly one** provider *in another member* binds; a
//! key with **two or more** providers is **ambiguous** and produces **no edge**
//! (never fabricated), exactly as the intra-repo binder's
//! [`exactly_one`](crate::resolve) gate. A sole provider in the consumer's *own*
//! member is an intra-repo fact the per-repo graph already owns, so the bridge
//! emits no cross-service edge for it.
//!
//! # Ephemeral, cached on sync-stamps ([ADR-13], [ADR-52])
//! The edge set is **never** persisted, **never** written as `edges` rows, and
//! members are **never** `ATTACH`-ed — the bridge only ever issues read-pool
//! reads. The computed set is cached against the members' [`sync_stamp`]s
//! ([`SyncStamp`](crate::hydrate::SyncStamp)); when any member re-syncs and its
//! stamp advances, the next [`ContractBridge::edges`] recomputes.
//!
//! [`sync_stamp`]: crate::Engine::sync_stamp
//! [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
//! [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
//! [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-13]: ../../../docs/specs/architecture/decisions/ADR-13.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::graph_store::{EdgeRow, NodeRow};
use crate::model::{
    ArtifactRelation, BridgeNamespace, BridgeRole, EdgeKind, LogosSymbol, MatchDiscipline, NodeId,
    NodeKind,
};
use crate::resolve::route_template::route_key;

/// Which side of a portable-key match a node sits on — the bridge's local alias
/// for the model's [`BridgeRole`](crate::model::BridgeRole), the same
/// `Consumer`/`Provider` split an invocation arm declares via
/// [`ArtifactRelation::bridge_role`](crate::model::ArtifactRelation::bridge_role).
/// Re-exported so [`super::coverage`] classifies with one role vocabulary.
pub(super) use crate::model::BridgeRole as Role;

use super::registry::{EngineRegistry, MemberEngine};

/// One cross-service endpoint: the portable `(member, symbol)` identity of a
/// contract-surface node ([FR-WS-04], [ADR-52]).
///
/// The `symbol` is the node's canonical [`LogosSymbol`] — the *string* identity
/// that is meaningful across member databases. There is deliberately **no**
/// `NodeId` here: a rowid is per-database and must never cross a member
/// boundary ([ADR-52]).
///
/// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct BridgeEndpoint {
    /// The owning member's name (its workspace-relative path), as the registry
    /// tags it ([`Member::name`](super::Member::name)).
    pub member: String,
    /// The node's canonical, database-portable symbol identity.
    pub symbol: LogosSymbol,
}

/// A cross-service link computed by the bridge — an in-memory overlay edge whose
/// endpoints are `(member, symbol)` pairs ([FR-WS-04], [ADR-52]).
///
/// Emitted only when a consumer's portable key binds to **exactly one** provider
/// in **another** member; never persisted, never an `edges` row.
///
/// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct BridgeEdge {
    /// The relation class of the binding. For an HTTP contract match this is
    /// `"route"` — the same relation class the intra-repo artifact binder files
    /// an `ApiOperation`→`Route` [`ArtifactBinding`](crate::model::EdgeKind::ArtifactBinding)
    /// under, so cross-service answers speak the intra-repo vocabulary.
    pub relation: String,
    /// The consumer endpoint the link starts at (e.g. the `ApiOperation`).
    pub from: BridgeEndpoint,
    /// The provider endpoint the link points to (e.g. the framework `Route`).
    pub to: BridgeEndpoint,
}

/// A contract-surface node read from one member — the minimal view the bridge
/// matches on.
///
/// Purpose-built to carry **no** [`NodeId`](crate::model::NodeId): the bridge's
/// own input type has no rowid field, so a per-database id cannot leak into an
/// overlay endpoint ([ADR-52]).
///
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractNode {
    /// The node's ontology kind (one of the contract-surface kinds).
    pub kind: NodeKind,
    /// The human-facing name (e.g. a route node's `"METHOD /path"`).
    pub name: String,
    /// The node's canonical, portable symbol identity.
    pub symbol: LogosSymbol,
}

/// One arm-tagged **cross-service invocation reference** read from a member's
/// `unresolved_refs` ledger ([FR-WS-07], [FR-WS-08], [ADR-54]).
///
/// Where a [`ContractNode`] carries a member's *declared contract surface*
/// (routes, operations, proto/graphql types the bridge already indexes), an
/// invocation reference is a *captured call site* — an HTTP client call, a gRPC
/// stub call, a broker publish or subscribe — that S-251's generic interpreter
/// emitted into the ledger under an invocation-arm [`ArtifactRelation`]. It feeds
/// the bridge's candidate stream via the arm's
/// [`bridge_namespace`](ArtifactRelation::bridge_namespace) /
/// [`bridge_role`](ArtifactRelation::bridge_role) descriptors, so a new arm
/// reaches the bridge with no edit to the namespace-generic match loop.
///
/// Carries **either role**: most arms capture only their consumer side (the
/// provider is a contract-surface node the bridge already indexes), but the broker
/// arm captures *both* — a subscribe is a `Provider` with no contract node behind
/// it ([FR-WS-10]). The role is not a field: it is read from `relation`, so the
/// two can never disagree.
///
/// [FR-WS-07]: ../../../docs/specs/requirements/FR-WS-07.md
/// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvocationRef {
    /// The invocation arm this reference belongs to — its
    /// [`bridge_namespace`](ArtifactRelation::bridge_namespace) decides the
    /// portable-key form and the match discipline, its
    /// [`bridge_role`](ArtifactRelation::bridge_role) which side it is.
    pub relation: ArtifactRelation,
    /// The arm-normalized reference target the interpreter emitted (for HTTP, the
    /// raw `"METHOD /template"` string a `route_key` reduces to the portable key;
    /// for the broker arm, the normalized topic key).
    pub target: String,
    /// The canonical, database-portable symbol of the *call site* (the enclosing
    /// declaration the interpreter attributed the reference to) — the endpoint a
    /// bridge edge starts or ends at.
    pub symbol: LogosSymbol,
}

/// The per-member read the bridge needs: a member's contract surface plus the
/// sync-stamp the bridge caches against.
///
/// Abstracted (rather than calling [`Engine`](crate::Engine) directly) so the
/// bridge's matching and cache invalidation are exercisable without standing up
/// real on-disk engines — the same testability seam the registry's
/// [`MemberEngine`] provides. Implemented by [`Engine`](crate::Engine) for
/// production.
pub trait MemberContracts {
    /// Read this member's contract-surface nodes through its **read pool**.
    ///
    /// # Errors
    /// Propagates a read failure (e.g. a transient engine with no read pool, or
    /// a store read error) so the bridge can skip the member as degraded rather
    /// than aborting the whole workspace ([ADR-53]).
    ///
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    fn contract_surface(&self) -> Result<Vec<ContractNode>>;

    /// The member's current sync-stamp as a monotonic `u64` — the value the
    /// bridge caches against; it advances when the member re-syncs.
    fn contract_stamp(&self) -> u64;

    /// Read this member's arm-tagged cross-service **invocation references** — the
    /// captured call sites (HTTP client calls, gRPC stub calls, broker publishes
    /// *and subscribes*) in its `unresolved_refs` ledger, on **either** side of
    /// their arm ([FR-WS-07], [FR-WS-10], [ADR-54]).
    ///
    /// These feed the bridge's candidate stream alongside the contract-surface
    /// nodes ([`contract_surface`](Self::contract_surface)). The default is
    /// **empty** — a member/engine with no invocation-arm capture contributes
    /// nothing, so pre-arm members and lightweight test doubles need not implement
    /// it; the real [`Engine`](crate::Engine) overrides it to read the ledger.
    ///
    /// This is the **one and only** ledger seam. Both consumers of it — the bridge's
    /// [`compute_edges`] and the coverage tier ([`super::coverage`]) — read this method
    /// and apply the arm's own [`bridge_role`](ArtifactRelation::bridge_role)
    /// themselves. There is deliberately no second, role-filtered trait method: a
    /// defaulted one would be *overridable*, so an implementor could make the two views
    /// of one ledger disagree — the very drift the single seam exists to prevent.
    ///
    /// # Errors
    /// Propagates a read failure so the bridge can skip the member as degraded
    /// rather than aborting the whole workspace ([ADR-53]).
    ///
    /// [FR-WS-07]: ../../../docs/specs/requirements/FR-WS-07.md
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    /// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
    fn invocation_refs(&self) -> Result<Vec<InvocationRef>> {
        Ok(Vec::new())
    }

    /// Read this member's **topic surface** — its promoted per-repo topic graph,
    /// summarised as one entry per [`Topic`](NodeKind::Topic) with the number of
    /// declarations publishing to and subscribing from it (S-256, [FR-WS-11]).
    ///
    /// Read-only, and read from the **graph** rather than from the bind: a topic
    /// this member publishes and nobody consumes has no bridge edge, yet must still
    /// surface ([FR-WS-11]). See [`super::topics`].
    ///
    /// The default is **empty** — a member/engine with no promoted topic (every repo
    /// that indexes no broker coupling) contributes nothing, so lightweight test
    /// doubles need not implement it.
    ///
    /// # Errors
    /// Propagates a read failure so the caller can skip the member as degraded
    /// rather than aborting the whole workspace ([ADR-53]).
    ///
    /// [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    fn topic_surface(&self) -> Result<Vec<super::topics::TopicSummary>> {
        Ok(Vec::new())
    }

    /// Read this member's **reachability surface** — every node with the per-repo
    /// tri-state dead-code verdict its own graph last computed, plus the
    /// `Calls`/`RoutesTo` adjacency the app-wide union view walks ([FR-WS-12],
    /// [ADR-56]).
    ///
    /// Read-only: the `is_dead` column is *read*, never set. The union view is a
    /// separate advisory overlay and cannot move a member's gated signal.
    ///
    /// The default is **empty** — a member/engine that cannot serve a surface
    /// contributes no nodes and no roots, so it can only ever fail to *promote*,
    /// never demote ([ADR-56]); lightweight test doubles need not implement it.
    ///
    /// # Errors
    /// Propagates a read failure so the union view can skip the member as degraded
    /// rather than aborting the whole workspace ([ADR-53]).
    ///
    /// [FR-WS-12]: ../../../docs/specs/requirements/FR-WS-12.md
    /// [ADR-56]: ../../../docs/specs/architecture/decisions/ADR-56.md
    fn reachability_surface(&self) -> Result<super::reach::ReachabilitySurface> {
        Ok(super::reach::ReachabilitySurface::default())
    }
}

impl MemberContracts for crate::Engine {
    fn contract_surface(&self) -> Result<Vec<ContractNode>> {
        let runtime = self.runtime().context(
            "reading a member's contract surface requires a long-lived engine \
             (Engine::start) with a read-only pool",
        )?;
        // Nodes, the `Contains` tree, AND the ProtoService rpc-method bodies in one
        // read: an `ApiOperation`'s route reference is reconstructed from its parent
        // `ApiPath`, and a `ProtoService` is expanded into one gRPC provider per
        // rpc method from its body (see `surface_from`).
        let (nodes, edges, bodies) = runtime.submit_read(|store| {
            Ok((
                store.all_nodes()?,
                store.all_edges()?,
                store.proto_service_bodies()?,
            ))
        })?;
        let bodies: HashMap<LogosSymbol, String> = bodies.into_iter().collect();
        Ok(surface_from(&nodes, &edges, &bodies))
    }

    fn contract_stamp(&self) -> u64 {
        // The inherent `Engine::sync_stamp` returns a `SyncStamp(u64)`; the
        // bridge caches on the bare monotonic value.
        self.sync_stamp().0
    }

    fn invocation_refs(&self) -> Result<Vec<InvocationRef>> {
        let runtime = self.runtime().context(
            "reading a member's invocation references requires a long-lived engine \
             (Engine::start) with a read-only pool",
        )?;
        let rows = runtime.submit_read(|store| store.unresolved_refs())?;
        Ok(invocation_refs_from(rows))
    }

    fn topic_surface(&self) -> Result<Vec<super::topics::TopicSummary>> {
        let runtime = self.runtime().context(
            "reading a member's topic surface requires a long-lived engine \
             (Engine::start) with a read-only pool",
        )?;
        // A **targeted** read of just the promoted broker subgraph — the nodes
        // `crate::resolve::topics` reconciled on this member's last index. This
        // read-model is served on every `workspace status` request, per member, and its
        // answer is O(topics) — usually zero — so materialising each member's whole
        // node+edge set for it (which is what `all_nodes` + `all_edges` would do, and on
        // top of the full read `contract_surface` already performs on the same request)
        // would be a whole-graph cost for an empty answer ([NFR-PE-10]).
        let (nodes, edges) = runtime.submit_read(|store| store.broker_subgraph())?;
        Ok(super::topics::topic_summaries_from(&nodes, &edges))
    }

    fn reachability_surface(&self) -> Result<super::reach::ReachabilitySurface> {
        let runtime = self.runtime().context(
            "reading a member's reachability surface requires a long-lived engine \
             (Engine::start) with a read-only pool",
        )?;
        // Three reads in one snapshot: `all_nodes` carries the portable symbol a
        // bridge endpoint roots on, `annotation_nodes` the per-repo `is_dead`
        // verdict, and `all_edges` the adjacency — joined by node id in
        // `reach::surface_from`, which then discards the ids ([ADR-52]).
        let (nodes, annotations, edges) = runtime.submit_read(|store| {
            Ok((
                store.all_nodes()?,
                store.annotation_nodes()?,
                store.all_edges()?,
            ))
        })?;
        Ok(super::reach::surface_from(&nodes, &annotations, &edges))
    }
}

/// Project a member's `unresolved_refs` ledger onto its arm-tagged invocation
/// **references**, on either side of their arm ([FR-WS-07], [FR-WS-10], [ADR-54]).
///
/// A row qualifies iff its `payload` names an [`ArtifactRelation`] that declares an
/// invocation arm — i.e. it has a
/// [`bridge_namespace`](ArtifactRelation::bridge_namespace). That is the same
/// generic test for every arm, so this projection never names a concrete one; the
/// arm's own [`bridge_role`](ArtifactRelation::bridge_role) then decides which
/// index a candidate lands in. A row whose payload is absent, is not a known
/// relation, or names a non-invocation relation is skipped; a row whose
/// `source_symbol` does not parse is skipped rather than fabricating a malformed
/// endpoint ([NFR-RA-05]). Both `resolved` and unresolved rows are included: the
/// cross-service bind is an overlay fact independent of whether the call also bound
/// a route intra-repo.
///
/// Keeping **both** roles is what lets the broker arm bind at all: a subscribe is a
/// `Provider` with no contract-surface node behind it, so a consumer-only intake
/// would index no broker provider anywhere and every publish would be honestly —
/// but wrongly — reported as having no provider in the workspace ([FR-WS-10],
/// [FR-WS-11]).
///
/// [FR-WS-07]: ../../../docs/specs/requirements/FR-WS-07.md
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
/// [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn invocation_refs_from(rows: Vec<crate::graph_store::UnresolvedRefRow>) -> Vec<InvocationRef> {
    rows.into_iter()
        .filter_map(|row| {
            let relation = row.payload.as_deref().and_then(ArtifactRelation::from_wire)?;
            relation.bridge_namespace()?; // not an invocation arm — not a candidate
            let symbol = LogosSymbol::parse(&row.source_symbol).ok()?;
            Some(InvocationRef {
                relation,
                target: row.target,
                symbol,
            })
        })
        .collect()
}

/// `true` for the contract-surface node kinds the bridge reads ([FR-WS-04]).
///
/// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
fn is_contract_surface(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Route
            | NodeKind::ApiOperation
            | NodeKind::ProtoService
            | NodeKind::ProtoMessage
            | NodeKind::GqlType
    )
}

/// Project a member's `(nodes, edges)` read onto its contract surface, rendering
/// each [`ApiOperation`](NodeKind::ApiOperation) as the `"METHOD /template"`
/// route reference the portable [`route_key`] matches on.
///
/// The OpenAPI promotion shapes a spec into an [`ApiPath`](NodeKind::ApiPath)
/// per template (its `name` is the template, e.g. `/users/{id}`) with one
/// `ApiOperation` child per HTTP method (its `name` is the lower-cased method,
/// e.g. `get`) hung off it by [`EdgeKind::Contains`]. So an operation's route
/// reference is recovered exactly as the intra-repo capture does
/// (`extract::config::refs::capture_openapi_routes`): its own name is the
/// method, its parent `ApiPath`'s name is the template. An operation with no
/// `Contains` parent keeps its bare method name (which `route_key` then rejects
/// as non-normalizing — never fabricated). [`Route`](NodeKind::Route) and the
/// proto/graphql kinds pass through with their own names.
///
/// A [`ProtoService`](NodeKind::ProtoService) whose S-253 enrichment recorded its
/// rpc method names in `bodies` (keyed by symbol) is **expanded** into one
/// contract node per method, named `package.Service/Method` — the fully-qualified
/// gRPC provider key [`classify`] keys on ([FR-WS-09]). A service with no method
/// body passes through unexpanded (its bare package-qualified name carries no
/// portable key, so [`classify`] drops it).
///
/// [route_key]: crate::resolve::route_template::route_key
/// [FR-WS-09]: ../../../docs/specs/requirements/FR-WS-09.md
fn surface_from(
    nodes: &[NodeRow],
    edges: &[EdgeRow],
    bodies: &HashMap<LogosSymbol, String>,
) -> Vec<ContractNode> {
    use std::collections::HashSet;

    // Each `ApiPath` id → its path-template name (`/users/{id}`).
    let path_template: HashMap<NodeId, &str> = nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ApiPath)
        .map(|n| (n.id, n.name.as_str()))
        .collect();
    let operations: HashSet<NodeId> = nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ApiOperation)
        .map(|n| n.id)
        .collect();
    // Each `ApiOperation` id → its containing `ApiPath` id
    // (ApiPath --Contains--> ApiOperation).
    let parent_of: HashMap<NodeId, NodeId> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Contains && operations.contains(&e.target))
        .map(|e| (e.target, e.source))
        .collect();

    nodes
        .iter()
        .filter(|n| is_contract_surface(n.kind))
        .flat_map(|n| {
            // A ProtoService fans out into one node per rpc method (S-253): the
            // provider key is the package-qualified service joined to each method.
            if n.kind == NodeKind::ProtoService {
                if let Some(body) = bodies.get(&n.symbol) {
                    return rpc_methods(body)
                        .map(|method| ContractNode {
                            kind: n.kind,
                            name: format!("{}/{}", n.name, method),
                            symbol: n.symbol.clone(),
                        })
                        .collect::<Vec<_>>();
                }
            }
            let name = if n.kind == NodeKind::ApiOperation {
                match parent_of.get(&n.id).and_then(|p| path_template.get(p)) {
                    Some(template) => format!("{} {}", n.name.to_ascii_uppercase(), template),
                    None => n.name.clone(),
                }
            } else {
                n.name.clone()
            };
            vec![ContractNode {
                kind: n.kind,
                name,
                symbol: n.symbol.clone(),
            }]
        })
        .collect()
}

/// The non-empty rpc method names encoded in a `ProtoService` node `body` (S-253
/// provider enrichment): the newline-joined list the proto extractor wrote,
/// split back and trimmed of any blank entry.
fn rpc_methods(body: &str) -> impl Iterator<Item = &str> {
    body.lines().map(str::trim).filter(|m| !m.is_empty())
}

/// The portable identity a candidate is matched on across members: a
/// [`BridgeNamespace`] plus the arm's normalized **key string** ([FR-WS-07],
/// [ADR-54]).
///
/// Namespace-generic by construction — the match loop keys on this struct and
/// applies the namespace's [`match_discipline`](BridgeNamespace::match_discipline),
/// never any per-arm code. The HTTP key folds the shared positional
/// [`route_key`] `(METHOD, template)` into `"METHOD /template"`; the gRPC and
/// broker arms ([FR-WS-09], [FR-WS-10]) build their own key strings
/// (`package.Service/Method`, a topic name) under their own namespace. Any two
/// candidates with the same `(namespace, key)` meet — regardless of which arm
/// produced them.
///
/// [route_key]: crate::resolve::route_template::route_key
/// [FR-WS-07]: ../../../docs/specs/requirements/FR-WS-07.md
/// [FR-WS-09]: ../../../docs/specs/requirements/FR-WS-09.md
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
/// Visible within `federation` (not just this file) so the coverage read-model
/// ([`super::coverage`], [FR-WS-05], [ADR-53]) classifies references with the
/// exact same key vocabulary the bridge matches edges on — one classifier, no
/// drift between "why did this bind" and "why didn't this bind".
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct PortableKey {
    /// The invocation namespace this key lives in — decides the match discipline.
    pub(super) namespace: BridgeNamespace,
    /// The arm-normalized, database-portable key string two candidates meet on.
    pub(super) key: String,
}

impl PortableKey {
    /// An HTTP key from the shared positional [`route_key`] parts: the
    /// upper-cased method and positionally-normalized template folded into one
    /// `"METHOD /template"` string under the [`Http`](BridgeNamespace::Http)
    /// namespace.
    ///
    /// [route_key]: crate::resolve::route_template::route_key
    pub(super) fn http(method: String, template: String) -> PortableKey {
        PortableKey {
            namespace: BridgeNamespace::Http,
            key: format!("{method} {template}"),
        }
    }

    /// A broker-topic key from an arm-normalized topic string (a topic name,
    /// optionally guarded by a `#`-appended message-schema FQN) under the fan-out
    /// [`BrokerTopic`](BridgeNamespace::BrokerTopic) namespace (S-254,
    /// [FR-WS-10]). The [`super::broker`] classifier builds these; two sides meet
    /// iff their whole topic key (topic + optional guard) is byte-equal.
    ///
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    pub(super) fn broker(key: String) -> PortableKey {
        PortableKey {
            namespace: BridgeNamespace::BrokerTopic,
            key,
        }
    }

    /// The relation class a binding on this key is filed under — the namespace's
    /// stable relation label ([`BridgeNamespace::relation`]).
    pub(super) fn relation(&self) -> &'static str {
        self.namespace.relation()
    }
}

/// Reduce a contract-surface node to its portable key and role, or `None` when
/// the node carries no portable key yet (proto/graphql) or its template does not
/// normalize cleanly (a catch-all/regex route is never a candidate, [NFR-RA-05]).
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub(super) fn classify(kind: NodeKind, name: &str) -> Option<(PortableKey, Role)> {
    match kind {
        NodeKind::Route => {
            let (method, template) = route_key(name)?;
            Some((PortableKey::http(method, template), Role::Provider))
        }
        NodeKind::ApiOperation => {
            let (method, template) = route_key(name)?;
            Some((PortableKey::http(method, template), Role::Consumer))
        }
        NodeKind::ProtoService => {
            // A ProtoService reaches `classify` already expanded by `surface_from`
            // into its `package.Service/Method` form (S-253, [FR-WS-09]); that
            // fully-qualified string is the gRPC provider key directly. A bare
            // (method-less) service has no `/` and carries no portable key.
            if name.contains('/') {
                Some((
                    PortableKey {
                        namespace: BridgeNamespace::Grpc,
                        key: name.to_string(),
                    },
                    Role::Provider,
                ))
            } else {
                None
            }
        }
        // The remaining contract nodes (`ProtoMessage`, `GqlType`) are read into
        // the surface but carry no portable invocation key yet.
        _ => None,
    }
}

/// Reduce an arm-tagged invocation **consumer** ([`InvocationRef`]) to the
/// portable key it meets a provider on, or `None` when it does not compose
/// ([FR-WS-07], [ADR-54]).
///
/// The consumer-side twin of [`classify`]: where `classify` keys a
/// contract-surface *provider node*, this keys a captured *call site*. It routes
/// on the arm's [`bridge_namespace`](ArtifactRelation::bridge_namespace) — the
/// per-arm registration point ([ADR-54]) — and produces the **same**
/// [`PortableKey`] the provider side does, so the two meet through the
/// namespace-generic [`match_indexed`] loop unchanged:
///
/// - [`Http`](BridgeNamespace::Http): the target is the arm's raw
///   `"METHOD /template"`; the shared [`route_key`] reduces it to the identical
///   `(METHOD, normalized-template)` a framework `Route` provider reduces to. A
///   target that does not normalize yields `None` (never approximately matched,
///   [NFR-RA-05]) — though the arm's normalizer has already refused those before
///   the ledger, so a stored HTTP target normalizes by construction.
///
/// The gRPC and broker arms ([FR-WS-09], [FR-WS-10]) register their own namespace
/// key here; until then their consumers contribute nothing (an arm lacking its
/// key builder is inert, exactly like a language lacking its capture).
///
/// [FR-WS-07]: ../../../docs/specs/requirements/FR-WS-07.md
/// [FR-WS-09]: ../../../docs/specs/requirements/FR-WS-09.md
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
pub(super) fn consumer_portable_key(relation: ArtifactRelation, target: &str) -> Option<PortableKey> {
    match relation.bridge_namespace()? {
        BridgeNamespace::Http => {
            let (method, template) = route_key(target)?;
            Some(PortableKey::http(method, template))
        }
        // A gRPC stub call's target is already the normalized, fully-qualified
        // `package.Service/Method` key the `grpc_key` normalizer wrote (S-253,
        // [FR-WS-09]); it is the exact string the expanded `ProtoService` provider
        // classifies to, so the two meet on an identical `Grpc` key.
        BridgeNamespace::Grpc => Some(PortableKey {
            namespace: BridgeNamespace::Grpc,
            key: target.to_string(),
        }),
        // A broker publish's target is the arm-normalized topic key (a topic name,
        // optionally `#`-guarded by a message-schema FQN) the broker normalizer
        // wrote (S-254, [FR-WS-10]) — the same string [`super::broker::classify`]
        // builds its [`PortableKey::broker`] from, so a publish keys identically
        // whichever intake it arrives through.
        //
        // The broker arm's *provider* side (a subscribe) is a `Provider`-role
        // ledger relation, which the consumer-only ledger intake here cannot index;
        // the fan-out that binds one publish to every subscribe is therefore
        // computed by [`super::broker::broker_edges`], which builds both indexes and
        // runs the *same* namespace-generic [`match_indexed`] loop. Keying the
        // publish here still matters: it keeps the coverage tier from reporting a
        // perfectly-composed topic as `path-not-composed`.
        BridgeNamespace::BrokerTopic => Some(PortableKey::broker(target.to_string())),
    }
}

/// The in-memory cross-service contract bridge over a workspace's members
/// ([FR-WS-04], [ADR-52]).
///
/// Holds only the cached edge set (keyed on member sync-stamps); the member set
/// it reads is supplied per call as an [`EngineRegistry`], so one bridge tracks
/// one workspace's registry. Shareable behind an [`Arc`]: the interior cache is
/// a [`Mutex`], so the serve surface and concurrent callers see one bridge.
///
/// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
#[derive(Debug, Default)]
pub struct ContractBridge {
    cache: Mutex<Option<CacheEntry>>,
}

/// The cached bridge result and the member sync-stamps it was computed against.
#[derive(Debug)]
struct CacheEntry {
    /// `(member, sync-stamp)` at compute time, sorted by member — the cache key.
    /// Any change (a stamp advance, or a member appearing/disappearing) is a
    /// miss.
    stamps: Vec<(String, u64)>,
    /// The computed edge set, shared so repeated reads clone an `Arc`, not a
    /// `Vec`.
    edges: Arc<Vec<BridgeEdge>>,
}

impl ContractBridge {
    /// A bridge with an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// The cross-service edge set over `registry`'s members, recomputing only
    /// when a member's sync-stamp has advanced since the last call ([FR-WS-04]).
    ///
    /// The member sync-stamps are read **first** (cheap); on a cache hit the
    /// per-member `all_nodes` reads are skipped entirely. On a miss the full
    /// contract surface is read through each member's read pool via the
    /// registry fan-out and the edge set is recomputed and re-cached.
    ///
    /// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
    pub fn edges<E>(&self, registry: &EngineRegistry<E>) -> Arc<Vec<BridgeEdge>>
    where
        E: MemberEngine + MemberContracts,
    {
        let stamps = current_stamps(registry);

        {
            let cache = self.lock_cache();
            if let Some(entry) = cache.as_ref() {
                if entry.stamps == stamps {
                    return Arc::clone(&entry.edges);
                }
            }
        }

        let edges = Arc::new(compute_edges(registry));
        let mut cache = self.lock_cache();
        *cache = Some(CacheEntry {
            stamps,
            edges: Arc::clone(&edges),
        });
        edges
    }

    /// Lock the cache, recovering a poisoned lock rather than propagating the
    /// poison — the cache is a derived read-model, so a poisoned view is still
    /// usable and one caller's panic must not brick the bridge for the rest.
    fn lock_cache(&self) -> std::sync::MutexGuard<'_, Option<CacheEntry>> {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Snapshot each member's current sync-stamp, sorted by member — the bridge
/// cache key. A member whose engine fails to start contributes no stamp (it is
/// skipped), so if it later starts the stamp vector changes and the cache
/// invalidates.
fn current_stamps<E>(registry: &EngineRegistry<E>) -> Vec<(String, u64)>
where
    E: MemberEngine + MemberContracts,
{
    let mut stamps: Vec<(String, u64)> = registry
        .fan_out(|_, engine| engine.contract_stamp())
        .into_iter()
        .filter_map(|scoped| scoped.value.ok().map(|stamp| (scoped.member, stamp)))
        .collect();
    stamps.sort();
    stamps
}

/// Read a per-member value through the registry fan-out, **degrading** (a warn +
/// skip) on either an engine-start failure or a read failure rather than aborting
/// the whole workspace ([ADR-53]). Returns `(member, value)` for every member
/// whose read succeeded.
///
/// The one place the bridge and the coverage read-model express "read each
/// member's `X` through its read pool, skip the degraded ones" — `subject` names
/// `X` in the warning so both the contract-surface and invocation-consumer reads
/// (and both tiers) share this handling verbatim.
///
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
pub(super) fn read_members<E, T>(
    registry: &EngineRegistry<E>,
    subject: &str,
    read: impl Fn(&Arc<E>) -> Result<T>,
) -> Vec<(String, T)>
where
    E: MemberEngine + MemberContracts,
{
    let mut out = Vec::new();
    for scoped in registry.fan_out(|_, engine| read(engine)) {
        let member = scoped.member;
        match scoped.value {
            Ok(Ok(value)) => out.push((member, value)),
            Ok(Err(err)) => tracing::warn!(
                member = %member,
                "reading a workspace member's {subject} failed; degraded without it: {err:#}"
            ),
            Err(err) => tracing::warn!(
                member = %member,
                "a workspace member engine failed to start; degraded without it: {err:#}"
            ),
        }
    }
    out
}

/// Read every member's contract surface through its read pool, index providers
/// on portable keys, split providers from consumers by role, and hand the two
/// indexes to the namespace-generic [`match_indexed`] matcher.
fn compute_edges<E>(registry: &EngineRegistry<E>) -> Vec<BridgeEdge>
where
    E: MemberEngine + MemberContracts,
{
    let mut providers: HashMap<PortableKey, Vec<BridgeEndpoint>> = HashMap::new();
    let mut consumers: Vec<(PortableKey, BridgeEndpoint)> = Vec::new();

    for (member, surface) in read_members(registry, "contract surface", |e| e.contract_surface()) {
        for node in surface {
            let Some((key, role)) = classify(node.kind, &node.name) else {
                continue;
            };
            let endpoint = BridgeEndpoint {
                member: member.clone(),
                symbol: node.symbol,
            };
            match role {
                Role::Provider => providers.entry(key).or_default().push(endpoint),
                Role::Consumer => consumers.push((key, endpoint)),
            }
        }
    }

    // Arm-tagged invocation references (HTTP client calls, S-252; gRPC stub calls,
    // S-253; broker publishes and subscribes, S-254) join the candidate stream via
    // the arm's portable key — the ledger-side feed S-251's contract deferred to
    // the arms ([FR-WS-07], [ADR-54]).
    //
    // The broker arm is routed to its **own** classifier ([`super::broker`]) rather
    // than into the loop's indexes, and that split is load-bearing, not stylistic:
    //
    //   - its *provider* side (a subscribe) has no contract-surface node behind it,
    //     so it can only be indexed from the ledger; and
    //   - it must de-duplicate endpoints before the fan-out (the loop emits one edge
    //     per (consumer, provider) pair and would otherwise multiply a twice-captured
    //     site into duplicate edges, [NFR-RA-05]).
    //
    // Both indexes are therefore built inside `broker_edges`, which runs the very
    // same namespace-generic [`match_indexed`] loop over them. Routing the arm here
    // — instead of *also* pushing its publishes into `consumers` below — is what
    // keeps a publish from being counted twice, once through each intake
    // ([FR-WS-10], [FR-WS-11]).
    let mut broker_candidates: Vec<super::broker::BrokerCandidate> = Vec::new();
    for (member, refs) in read_members(registry, "invocation references", |e| e.invocation_refs()) {
        for reference in refs {
            let endpoint = BridgeEndpoint {
                member: member.clone(),
                symbol: reference.symbol,
            };
            match reference.relation.bridge_namespace() {
                Some(BridgeNamespace::BrokerTopic) => {
                    broker_candidates.push(super::broker::BrokerCandidate {
                        relation: reference.relation,
                        key: reference.target,
                        endpoint,
                    });
                }
                // Every other arm feeds the loop's consumer index directly; its
                // providers are contract-surface nodes, already indexed above.
                _ => {
                    if reference.relation.bridge_role() != Some(BridgeRole::Consumer) {
                        continue;
                    }
                    let Some(key) = consumer_portable_key(reference.relation, &reference.target)
                    else {
                        continue; // an unkeyable / not-yet-registered arm contributes nothing
                    };
                    consumers.push((key, endpoint));
                }
            }
        }
    }

    let mut edges = match_indexed(providers, consumers);
    // The broker arm's cross-member fan-out: one publish binds every subscribe on
    // the same topic identity, across members ([FR-WS-10], [FR-WS-11]).
    edges.extend(super::broker::broker_edges(broker_candidates));
    // Re-sort the union: each half is sorted, their concatenation is not
    // ([NFR-RA-06]).
    edges.sort();
    edges
}

/// The **namespace-generic** cross-service match core ([FR-WS-04], [ADR-54]).
///
/// Given providers indexed on their [`PortableKey`] and the consumer keys, emit
/// one [`BridgeEdge`] per cross-member binding, applying the *key's namespace*
/// [`match_discipline`](BridgeNamespace::match_discipline) — the **only**
/// namespace-specific decision made here, which is what keeps the loop genuinely
/// arm-agnostic:
///
/// - [`MatchDiscipline::ExactlyOne`] (HTTP, gRPC): a consumer binds the **sole**
///   provider of its key across the whole workspace; two or more providers are
///   ambiguous and produce no edge (never fabricated, [NFR-RA-05]). A sole
///   provider in the consumer's *own* member is an intra-repo fact the per-repo
///   graph already owns, so no cross-service edge is emitted for it — the same
///   relation binds it locally.
/// - [`MatchDiscipline::FanOut`] (broker topic): a consumer binds **every**
///   cross-member provider of its key — one publish reaches all subscribers.
///   Same-member providers are the intra-repo fan-out, excluded here.
///
/// The matcher never names a concrete namespace, so a freshly-registered
/// namespace matches through the exact same code. Deterministic: provider lists
/// and the emitted edges are sorted.
///
/// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
pub(super) fn match_indexed(
    mut providers: HashMap<PortableKey, Vec<BridgeEndpoint>>,
    consumers: Vec<(PortableKey, BridgeEndpoint)>,
) -> Vec<BridgeEdge> {
    // Deterministic candidate order regardless of member fan-out order.
    for endpoints in providers.values_mut() {
        endpoints.sort();
    }

    let mut edges = Vec::new();
    for (key, consumer) in consumers {
        let Some(candidates) = providers.get(&key) else {
            continue; // no provider anywhere in the workspace — no edge
        };
        match key.namespace.match_discipline() {
            MatchDiscipline::ExactlyOne => {
                // Exactly-one across members ([NFR-RA-05]): two or more providers
                // of the same key are ambiguous and never fabricate an edge.
                let [only] = candidates.as_slice() else {
                    continue;
                };
                // A sole provider in the consumer's *own* member is an intra-repo
                // fact the per-repo graph owns; the bridge only emits cross-member
                // links.
                if only.member == consumer.member {
                    continue;
                }
                edges.push(BridgeEdge {
                    relation: key.relation().to_string(),
                    from: consumer,
                    to: only.clone(),
                });
            }
            MatchDiscipline::FanOut => {
                // One publish → every cross-member subscriber. No ambiguity: a
                // topic with many subscribers fans out to all of them. A
                // same-member subscriber is the intra-repo fan-out, owned by the
                // per-repo graph.
                for provider in candidates {
                    if provider.member == consumer.member {
                        continue;
                    }
                    edges.push(BridgeEdge {
                        relation: key.relation().to_string(),
                        from: consumer.clone(),
                        to: provider.clone(),
                    });
                }
            }
        }
    }

    edges.sort();
    edges
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::{Cell, RefCell};
    use std::path::{Path, PathBuf};

    use super::super::{Federation, Member};
    use super::super::registry::RegistryMode;

    // Per-test-thread fixtures: each `#[test]` runs on its own thread, so the
    // thread-local member fixtures and the surface-read counter are isolated.
    thread_local! {
        static FIXTURES: RefCell<HashMap<String, MemberFixture>> = RefCell::new(HashMap::new());
        static SURFACE_READS: Cell<usize> = const { Cell::new(0) };
    }

    #[derive(Clone, Default)]
    struct MemberFixture {
        stamp: u64,
        nodes: Vec<ContractNode>,
        consumers: Vec<InvocationRef>,
    }

    fn reset() {
        FIXTURES.with(|f| f.borrow_mut().clear());
        SURFACE_READS.with(|c| c.set(0));
    }
    fn set_member(name: &str, stamp: u64, nodes: Vec<ContractNode>) {
        FIXTURES.with(|f| {
            f.borrow_mut().entry(name.to_string()).or_default().nodes = nodes;
        });
        FIXTURES.with(|f| {
            f.borrow_mut().get_mut(name).unwrap().stamp = stamp;
        });
    }
    /// Attach arm-tagged invocation references (client-call sites, gRPC stub calls,
    /// broker publishes *and subscribes*) to a member — the ledger-sourced stream,
    /// distinct from the node-surface providers `set_member` supplies.
    fn set_consumers(name: &str, consumers: Vec<InvocationRef>) {
        FIXTURES.with(|f| {
            f.borrow_mut().entry(name.to_string()).or_default().consumers = consumers;
        });
    }
    /// A broker **publish** at `symbol` on `topic` — a `Consumer`-role arm row (the
    /// edge source), keyed on the normalized topic identity.
    fn broker_publish(topic: &str, symbol: &str) -> InvocationRef {
        InvocationRef {
            relation: ArtifactRelation::BrokerPublish,
            target: topic.to_string(),
            symbol: LogosSymbol::parse(symbol).unwrap(),
        }
    }
    /// A broker **subscribe** at `symbol` on `topic` — a `Provider`-role arm row
    /// that exists **only** in the ledger (no contract-surface node stands behind a
    /// subscriber), which is why the bridge must index the ledger's provider side.
    fn broker_subscribe(topic: &str, symbol: &str) -> InvocationRef {
        InvocationRef {
            relation: ArtifactRelation::BrokerSubscribe,
            target: topic.to_string(),
            symbol: LogosSymbol::parse(symbol).unwrap(),
        }
    }
    /// A gRPC stub-call consumer at `symbol` invoking `key` (`package.Service/Method`).
    fn grpc_consumer(key: &str, symbol: &str) -> InvocationRef {
        InvocationRef {
            relation: ArtifactRelation::GrpcCall,
            target: key.to_string(),
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
    /// An HTTP client-call consumer at `symbol` calling `target` (`"METHOD /path"`).
    fn http_call(target: &str, symbol: &str) -> InvocationRef {
        InvocationRef {
            relation: ArtifactRelation::HttpClientCall,
            target: target.to_string(),
            symbol: LogosSymbol::parse(symbol).unwrap(),
        }
    }
    fn bump_stamp(name: &str) {
        FIXTURES.with(|f| {
            f.borrow_mut().entry(name.to_string()).or_default().stamp += 1;
        });
    }
    fn surface_reads() -> usize {
        SURFACE_READS.with(Cell::get)
    }

    /// A fake member engine reading its surface/stamp from the thread-local
    /// fixtures, keyed by the member name derived from its root. Stands in for a
    /// real [`Engine`] so the bridge's matching and cache invalidation are
    /// testable without any on-disk store. A member literally named `"broken"`
    /// fails to start, exercising the degrade-don't-abort path.
    #[derive(Debug)]
    struct FakeEngine {
        member: String,
    }

    fn member_of(root: &Path) -> String {
        root.file_name().unwrap().to_string_lossy().into_owned()
    }

    impl MemberEngine for FakeEngine {
        type Watcher = ();
        fn start(root: &Path) -> Result<Arc<Self>> {
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
            SURFACE_READS.with(|c| c.set(c.get() + 1));
            // A member literally named "unreadable" starts fine but its surface
            // READ fails — the `Ok(Err)` degrade arm, distinct from a start
            // failure ("broken").
            if self.member == "unreadable" {
                anyhow::bail!("store read failed");
            }
            Ok(FIXTURES.with(|f| {
                f.borrow()
                    .get(&self.member)
                    .map(|m| m.nodes.clone())
                    .unwrap_or_default()
            }))
        }
        fn contract_stamp(&self) -> u64 {
            FIXTURES.with(|f| f.borrow().get(&self.member).map(|m| m.stamp).unwrap_or(0))
        }
        // The single ledger seam — the bridge and the coverage tier both read it and
        // apply the role themselves, so a fixture's provider-role rows (a broker
        // subscribe) reach both.
        fn invocation_refs(&self) -> Result<Vec<InvocationRef>> {
            // "unreadable" fails its surface read; keep the same degrade behaviour
            // here so a degraded member is skipped for its ledger too.
            if self.member == "unreadable" {
                anyhow::bail!("store read failed");
            }
            Ok(FIXTURES.with(|f| {
                f.borrow()
                    .get(&self.member)
                    .map(|m| m.consumers.clone())
                    .unwrap_or_default()
            }))
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
        }
    }

    fn registry(names: &[&str]) -> EngineRegistry<FakeEngine> {
        EngineRegistry::new(fed(names), RegistryMode::Lazy)
    }

    fn nrow(id: i64, kind: NodeKind, name: &str, symbol: &str) -> NodeRow {
        NodeRow {
            id: NodeId(id),
            symbol: LogosSymbol::parse(symbol).unwrap(),
            kind,
            name: name.to_string(),
            file_path: None,
            start_line: None,
            end_line: None,
        }
    }
    fn contains(source: i64, target: i64) -> EdgeRow {
        EdgeRow {
            source: NodeId(source),
            target: NodeId(target),
            kind: EdgeKind::Contains,
        }
    }

    /// `surface_from` renders an `ApiOperation` as `"METHOD /template"` by joining
    /// it to its parent `ApiPath` over the `Contains` tree — the exact shape a
    /// `Route` node's name carries, so both sides meet on one `route_key`.
    #[test]
    fn surface_from_reconstructs_operation_route_reference_from_its_apipath() {
        let nodes = vec![
            nrow(1, NodeKind::ApiPath, "/users/{user_id}", "local path"),
            nrow(2, NodeKind::ApiOperation, "get", "local op_get"),
            nrow(3, NodeKind::ApiOperation, "delete", "local op_del"),
            // A route in the same store passes through with its own name.
            nrow(4, NodeKind::Route, "GET /users/{id}", "local route"),
            // An orphan operation (no Contains parent) keeps its bare method name.
            nrow(5, NodeKind::ApiOperation, "post", "local op_orphan"),
        ];
        let edges = vec![contains(1, 2), contains(1, 3)];

        let surface = surface_from(&nodes, &edges, &HashMap::new());
        let named: HashMap<&str, &ContractNode> =
            surface.iter().map(|c| (c.symbol.as_str(), c)).collect();

        assert_eq!(named["local op_get"].name, "GET /users/{user_id}");
        assert_eq!(named["local op_del"].name, "DELETE /users/{user_id}");
        assert_eq!(named["local route"].name, "GET /users/{id}");
        assert_eq!(
            named["local op_orphan"].name, "post",
            "an operation with no ApiPath parent keeps its bare (non-normalizing) name"
        );
        // The ApiPath itself is not part of the contract surface.
        assert!(
            surface.iter().all(|c| c.kind != NodeKind::ApiPath),
            "ApiPath is a structural parent, not a matched contract kind"
        );
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

    /// Acceptance: an OpenAPI operation in one member binds a matching framework
    /// route in another via `route_key`, across the {id}/{user_id} param drift.
    #[test]
    fn an_operation_binds_a_sole_route_in_another_member() {
        reset();
        set_member("api", 0, vec![op("GET /users/{user_id}", "local op_get")]);
        set_member("web", 0, vec![route("GET /users/{id}", "local route_get")]);

        let edges = ContractBridge::new().edges(&registry(&["api", "web"]));

        assert_eq!(edges.len(), 1, "the operation binds its one cross-member route");
        let edge = &edges[0];
        assert_eq!(edge.relation, "route");
        assert_eq!(edge.from.member, "api");
        assert_eq!(edge.from.symbol.as_str(), "local op_get");
        assert_eq!(edge.to.member, "web");
        assert_eq!(edge.to.symbol.as_str(), "local route_get");
    }

    /// Acceptance: two providers of the same key across the workspace yield
    /// ambiguous — no edge (never fabricated, [NFR-RA-05]).
    #[test]
    fn two_providers_of_the_same_key_are_ambiguous_no_edge() {
        reset();
        set_member("api", 0, vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", 0, vec![route("GET /users/{id}", "local route_web")]);
        set_member("admin", 0, vec![route("GET /users/{userId}", "local route_admin")]);

        let edges = ContractBridge::new().edges(&registry(&["api", "web", "admin"]));

        assert!(
            edges.is_empty(),
            "two providers of one key are ambiguous — no edge: {edges:?}"
        );
    }

    /// A sole provider in the consumer's OWN member is an intra-repo fact, not a
    /// cross-service bridge edge.
    #[test]
    fn a_sole_same_member_provider_is_not_a_bridge_edge() {
        reset();
        set_member(
            "api",
            0,
            vec![
                op("GET /users/{id}", "local op_get"),
                route("GET /users/{id}", "local route_local"),
            ],
        );

        let edges = ContractBridge::new().edges(&registry(&["api"]));
        assert!(edges.is_empty(), "no cross-member provider — no edge: {edges:?}");
    }

    /// A consumer whose own member also provides the key AND another member
    /// provides it is still ambiguous (2 providers) — never fabricated.
    #[test]
    fn a_local_plus_remote_provider_pair_is_ambiguous() {
        reset();
        set_member(
            "api",
            0,
            vec![
                op("GET /users/{id}", "local op_get"),
                route("GET /users/{id}", "local route_local"),
            ],
        );
        set_member("web", 0, vec![route("GET /users/{id}", "local route_web")]);

        let edges = ContractBridge::new().edges(&registry(&["api", "web"]));
        assert!(edges.is_empty(), "two providers (local + remote) are ambiguous: {edges:?}");
    }

    /// A route whose template does not normalize (a catch-all) is never indexed,
    /// so it is never an approximate match ([NFR-RA-05]).
    #[test]
    fn a_non_normalizing_route_is_never_a_candidate() {
        reset();
        set_member("api", 0, vec![op("GET /files/{id}", "local op_files")]);
        set_member("web", 0, vec![route("GET /files/{*rest}", "local route_catchall")]);

        let edges = ContractBridge::new().edges(&registry(&["api", "web"]));
        assert!(edges.is_empty(), "a catch-all route is not a candidate: {edges:?}");
    }

    /// A method mismatch never binds — the key carries the HTTP method.
    #[test]
    fn a_method_mismatch_never_binds() {
        reset();
        set_member("api", 0, vec![op("POST /users/{id}", "local op_post")]);
        set_member("web", 0, vec![route("GET /users/{id}", "local route_get")]);

        let edges = ContractBridge::new().edges(&registry(&["api", "web"]));
        assert!(edges.is_empty(), "GET route never binds a POST operation: {edges:?}");
    }

    /// Proto/GraphQL contract nodes are read but carry no portable HTTP key, so
    /// they contribute no edges in this story (honestly unbound, not guessed).
    #[test]
    fn proto_and_graphql_surface_nodes_yield_no_http_edges() {
        reset();
        set_member(
            "api",
            0,
            vec![ContractNode {
                kind: NodeKind::ProtoService,
                name: "user.UserService".to_string(),
                symbol: LogosSymbol::parse("local svc").unwrap(),
            }],
        );
        set_member(
            "web",
            0,
            vec![ContractNode {
                kind: NodeKind::GqlType,
                name: "User".to_string(),
                symbol: LogosSymbol::parse("local gqltype").unwrap(),
            }],
        );

        let edges = ContractBridge::new().edges(&registry(&["api", "web"]));
        assert!(edges.is_empty(), "no HTTP key on proto/graphql nodes: {edges:?}");
    }

    /// The cache hits when no member sync-stamp has changed: the per-member
    /// `all_nodes` read is skipped on the second call.
    #[test]
    fn cache_hits_when_no_member_stamp_changed() {
        reset();
        set_member("api", 3, vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", 7, vec![route("GET /users/{id}", "local route_get")]);
        let reg = registry(&["api", "web"]);
        let bridge = ContractBridge::new();

        let first = bridge.edges(&reg);
        let reads_after_first = surface_reads();
        assert!(reads_after_first >= 2, "the first computation reads each member");

        let second = bridge.edges(&reg);
        assert_eq!(
            surface_reads(),
            reads_after_first,
            "a cache hit must not re-read any member's surface"
        );
        assert_eq!(first, second, "the cached edge set is returned unchanged");
        assert!(Arc::ptr_eq(&first, &second), "the very same cached Arc is returned");
    }

    /// A member sync-stamp advance invalidates the cache: the next call
    /// recomputes (re-reads the surfaces) and reflects the new state.
    #[test]
    fn cache_invalidates_when_a_member_stamp_advances() {
        reset();
        set_member("api", 0, vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", 0, vec![]); // no route yet — no edge
        let reg = registry(&["api", "web"]);
        let bridge = ContractBridge::new();

        let before = bridge.edges(&reg);
        assert!(before.is_empty(), "no route provider yet — no edge");
        let reads_before = surface_reads();

        // The web member re-syncs, adding the matching route, and its stamp bumps.
        set_member("web", 1, vec![route("GET /users/{id}", "local route_get")]);
        // (set_member replaced the fixture with stamp 1; the stamp changed.)
        let after = bridge.edges(&reg);

        assert!(
            surface_reads() > reads_before,
            "a stamp advance must force a recompute (re-read the surfaces)"
        );
        assert_eq!(after.len(), 1, "the newly-synced route now binds");
        assert_eq!(after[0].to.symbol.as_str(), "local route_get");
    }

    /// Bumping only the stamp (same surface) still recomputes — proving the
    /// cache key is the stamp, not the content.
    #[test]
    fn a_bare_stamp_bump_forces_recompute() {
        reset();
        set_member("api", 0, vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", 0, vec![route("GET /users/{id}", "local route_get")]);
        let reg = registry(&["api", "web"]);
        let bridge = ContractBridge::new();

        let first = bridge.edges(&reg);
        let reads_after_first = surface_reads();
        bump_stamp("api");
        let second = bridge.edges(&reg);

        assert!(surface_reads() > reads_after_first, "a stamp bump recomputes");
        assert_eq!(first, second, "the edge set is unchanged, but freshly computed");
    }

    /// The cache key is the member *set*, not just the stamps: a member
    /// appearing (e.g. on re-discovery) is a miss, not a stale hit.
    #[test]
    fn cache_invalidates_when_the_member_set_changes() {
        reset();
        set_member("api", 0, vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", 0, vec![route("GET /users/{id}", "local route_get")]);
        let bridge = ContractBridge::new();

        let before = bridge.edges(&registry(&["api"]));
        assert!(before.is_empty(), "no provider in the one-member workspace");
        let reads_before = surface_reads();

        // A second member appears; the stamps vector grows → cache miss.
        let after = bridge.edges(&registry(&["api", "web"]));
        assert!(
            surface_reads() > reads_before,
            "a member appearing is a cache miss (the key is the member set)"
        );
        assert_eq!(after.len(), 1, "the newly-present member's route now binds");
        assert_eq!(after[0].to.member, "web");
    }

    /// A member whose engine fails to start is skipped, not fatal — the bridge
    /// still answers for the healthy members ([ADR-53]).
    #[test]
    fn a_degraded_member_is_skipped_not_fatal() {
        reset();
        set_member("api", 0, vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", 0, vec![route("GET /users/{id}", "local route_get")]);
        // "broken" fails to start; it must not abort the whole bridge.
        let edges = ContractBridge::new().edges(&registry(&["api", "web", "broken"]));

        assert_eq!(edges.len(), 1, "the healthy members still bind despite a degraded one");
        assert_eq!(edges[0].from.member, "api");
        assert_eq!(edges[0].to.member, "web");
    }

    /// A member whose engine starts but whose surface READ fails is skipped, not
    /// fatal — the `Ok(Err)` degrade arm, distinct from the start-failure `Err`
    /// arm ([ADR-53]).
    #[test]
    fn a_surface_read_failure_is_skipped_not_fatal() {
        reset();
        set_member("api", 0, vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", 0, vec![route("GET /users/{id}", "local route_get")]);
        // "unreadable" starts fine but its `contract_surface()` errors.
        set_member("unreadable", 0, vec![]);

        let edges = ContractBridge::new().edges(&registry(&["api", "web", "unreadable"]));

        assert_eq!(
            edges.len(),
            1,
            "the healthy members still bind despite a read-failed member"
        );
        assert_eq!(edges[0].from.member, "api");
        assert_eq!(edges[0].to.member, "web");
    }

    /// The emitted set is deterministic (sorted) regardless of member order.
    #[test]
    fn edges_are_deterministic_across_member_order() {
        reset();
        set_member("api", 0, vec![op("GET /a/{id}", "local op_a"), op("GET /b/{id}", "local op_b")]);
        set_member("web", 0, vec![route("GET /a/{id}", "local route_a")]);
        set_member("svc", 0, vec![route("GET /b/{id}", "local route_b")]);

        let one = ContractBridge::new().edges(&registry(&["api", "web", "svc"]));
        let two = ContractBridge::new().edges(&registry(&["svc", "web", "api"]));
        assert_eq!(one, two, "the edge set is independent of discovery order");
        assert_eq!(one.len(), 2);
    }

    // ── FR-WS-07 / ADR-54: the namespace-generic match core ──────────────────
    //
    // These drive `match_indexed` directly with **synthetic** namespaces — gRPC
    // and broker-topic — that have NO classifier feeding them yet (no invocation
    // arm exists). They prove the match loop resolves a freshly-registered
    // namespace generically, through the exact same code path HTTP takes, for
    // both disciplines. This is the sprint's "synthetic-namespace test before any
    // real arm exists" gate.

    use crate::model::BridgeNamespace;

    fn ep(member: &str, symbol: &str) -> BridgeEndpoint {
        BridgeEndpoint {
            member: member.to_string(),
            symbol: LogosSymbol::parse(symbol).unwrap(),
        }
    }
    fn pkey(namespace: BridgeNamespace, key: &str) -> PortableKey {
        PortableKey {
            namespace,
            key: key.to_string(),
        }
    }
    fn indexed(
        providers: &[(PortableKey, BridgeEndpoint)],
        consumers: Vec<(PortableKey, BridgeEndpoint)>,
    ) -> Vec<BridgeEdge> {
        let mut index: HashMap<PortableKey, Vec<BridgeEndpoint>> = HashMap::new();
        for (key, endpoint) in providers {
            index.entry(key.clone()).or_default().push(endpoint.clone());
        }
        match_indexed(index, consumers)
    }

    /// A freshly-registered **exactly-one** namespace (gRPC — nothing classifies
    /// into it yet) matches through the same core as HTTP: a sole cross-member
    /// provider binds, two providers are ambiguous (no edge), and a sole
    /// same-member provider is an intra-repo fact (no bridge edge). No
    /// namespace-specific match code exists for gRPC — the loop only consults its
    /// discipline.
    #[test]
    fn a_synthetic_exactly_one_namespace_matches_generically() {
        let key = pkey(BridgeNamespace::Grpc, "pkg.UserService/Get");

        // Sole cross-member provider → binds, under the namespace's relation label.
        let edges = indexed(
            &[(key.clone(), ep("web", "local svc_get"))],
            vec![(key.clone(), ep("api", "local stub_get"))],
        );
        assert_eq!(edges.len(), 1, "a sole cross-member provider binds: {edges:?}");
        assert_eq!(edges[0].relation, "grpc-call");
        assert_eq!(edges[0].from.member, "api");
        assert_eq!(edges[0].to.member, "web");

        // Two providers of the same key → ambiguous, never fabricated.
        let edges = indexed(
            &[
                (key.clone(), ep("web", "local a")),
                (key.clone(), ep("svc", "local b")),
            ],
            vec![(key.clone(), ep("api", "local stub"))],
        );
        assert!(
            edges.is_empty(),
            "two providers are ambiguous under exactly-one: {edges:?}"
        );
    }

    /// A freshly-registered **fan-out** namespace (broker topic) matches through
    /// the same core: one publish binds EVERY cross-member subscriber (fan-out,
    /// not ambiguous), and a same-member subscriber is the intra-repo fan-out,
    /// excluded.
    #[test]
    fn a_synthetic_fan_out_namespace_binds_every_cross_member_provider() {
        let key = pkey(BridgeNamespace::BrokerTopic, "orders");
        let edges = indexed(
            &[
                (key.clone(), ep("billing", "local sub_bill")),
                (key.clone(), ep("ship", "local sub_ship")),
                (key.clone(), ep("api", "local sub_local")),
            ],
            vec![(key.clone(), ep("api", "local pub_orders"))],
        );

        assert_eq!(
            edges.len(),
            2,
            "one publish fans out to both cross-member subscribers: {edges:?}"
        );
        let tos: Vec<&str> = edges.iter().map(|e| e.to.member.as_str()).collect();
        assert!(tos.contains(&"billing") && tos.contains(&"ship"));
        assert!(
            !tos.contains(&"api"),
            "the same-member subscriber is the intra-repo fan-out, excluded"
        );
        for e in &edges {
            assert_eq!(e.relation, "broker-topic");
            assert_eq!(e.from.member, "api", "the publish is the edge source");
        }
    }

    /// A consumer in a namespace with **no** provider anywhere contributes no
    /// edge — the match-grain form of "a language lacking that capture
    /// contributes nothing" (nothing was classified into the namespace, so the
    /// consumer resolves to no provider and no edge is fabricated).
    #[test]
    fn a_namespace_with_no_provider_contributes_no_edge() {
        let key = pkey(BridgeNamespace::Grpc, "pkg.Svc/Method");
        let edges = match_indexed(HashMap::new(), vec![(key, ep("api", "local stub"))]);
        assert!(edges.is_empty(), "no provider anywhere → no edge: {edges:?}");
    }

    /// Acceptance (3): an intra-repo invocation binds **locally through the same
    /// relation**, not as a cross-service bridge edge. A consumer whose sole
    /// provider is in its own member yields no bridge edge under either
    /// discipline — the per-repo graph already owns that binding (the same
    /// relation resolves it through the intra-repo artifact binder, unchanged).
    #[test]
    fn an_intra_repo_invocation_is_owned_by_the_local_graph_not_the_bridge() {
        // Exactly-one: a sole same-member provider is intra-repo.
        let http = pkey(BridgeNamespace::Http, "GET /users/{}");
        let edges = indexed(
            &[(http.clone(), ep("api", "local route_local"))],
            vec![(http, ep("api", "local op_local"))],
        );
        assert!(
            edges.is_empty(),
            "an in-repo consumer→provider pair binds locally, not via the bridge: {edges:?}"
        );

        // Fan-out: a same-member subscriber is likewise intra-repo, no bridge edge.
        let topic = pkey(BridgeNamespace::BrokerTopic, "orders");
        let edges = indexed(
            &[(topic.clone(), ep("api", "local sub_local"))],
            vec![(topic, ep("api", "local pub_local"))],
        );
        assert!(
            edges.is_empty(),
            "an in-repo publish→subscribe pair is the local graph's fan-out: {edges:?}"
        );
    }

    // ── S-252 / FR-WS-08: the HTTP client-call → route arm ────────────────────
    //
    // The arm feeds its captured client-call sites into the bridge as `Http`
    // `Consumer` candidates (via `invocation_refs`), keyed through the shared
    // `route_key`, and relies on the unchanged namespace-generic match loop.

    /// The ledger projection keeps every **invocation-arm** row, on either side of
    /// its arm: a non-invocation relation (`route`, `proto-import`), an
    /// absent/unknown payload, and an unparseable source symbol are all dropped —
    /// never a fabricated endpoint ([NFR-RA-05]).
    ///
    /// The `Provider`-role broker subscribe surviving here is the whole point
    /// (S-256, [FR-WS-11]): it has no contract-surface node behind it, so a
    /// consumer-only intake would index no broker provider anywhere and the arm
    /// could never bind. The role is applied downstream — by [`compute_edges`] and by
    /// the coverage tier, each off the arm's own `bridge_role` — not here.
    ///
    /// [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
    #[test]
    fn invocation_refs_from_keeps_every_arm_row_on_either_side() {
        use crate::graph_store::UnresolvedRefRow;
        use crate::model::RefForm;

        let row = |source_symbol: &str, target: &str, payload: Option<&str>| UnresolvedRefRow {
            id: 0,
            file_id: None,
            source_symbol: source_symbol.to_string(),
            target: target.to_string(),
            alias: None,
            form: RefForm::Path,
            kind: EdgeKind::ArtifactBinding,
            line: None,
            resolved: false,
            payload: payload.map(str::to_string),
        };

        let rows = vec![
            // A genuine HTTP client-call consumer — kept.
            row("local handler", "GET /users/{id}", Some("http-client-call")),
            // A broker PUBLISH (consumer role) and a broker SUBSCRIBE (provider
            // role) — both kept: the arm binds by indexing both sides.
            row("local emit", "orders", Some("broker-publish")),
            row("local listen", "orders", Some("broker-subscribe")),
            // A `route` binding is a contract relation, not an invocation arm — dropped.
            row("local op", "GET /users/{id}", Some("route")),
            // A proto import (not an invocation arm) — dropped.
            row("local file", "common.proto", Some("proto-import")),
            // No payload / unknown payload — dropped.
            row("local x", "GET /a", None),
            row("local y", "GET /b", Some("not-a-relation")),
            // An arm row whose source symbol does not parse — dropped.
            row("", "GET /c", Some("http-client-call")),
        ];

        let refs = invocation_refs_from(rows);
        let kept: Vec<(&str, &str)> = refs
            .iter()
            .map(|r| (r.relation.as_str(), r.symbol.as_str()))
            .collect();
        assert_eq!(
            kept,
            [
                ("http-client-call", "local handler"),
                ("broker-publish", "local emit"),
                ("broker-subscribe", "local listen"),
            ],
            "every parseable arm row survives, both roles; no non-arm row does"
        );
        assert_eq!(refs[0].target, "GET /users/{id}");

        // The consumer projection then applies the role filter — the subscribe is a
        // provider and drops out of *that* view (but not out of the bridge).
        let consumers: Vec<&str> = refs
            .iter()
            .filter(|r| r.relation.bridge_role() == Some(BridgeRole::Consumer))
            .map(|r| r.relation.as_str())
            .collect();
        assert_eq!(consumers, ["http-client-call", "broker-publish"]);
    }

    /// A client-call consumer key equals the provider `Route`'s key across the
    /// `{id}`/`{userId}` parameter-name drift — so they meet on one `PortableKey`.
    #[test]
    fn a_client_call_keys_equal_to_its_matching_route_provider() {
        let consumer = consumer_portable_key(ArtifactRelation::HttpClientCall, "GET /users/{id}")
            .expect("a static client-call target keys");
        let (provider, role) =
            classify(NodeKind::Route, "GET /users/{userId}").expect("a route classifies");
        assert_eq!(role, Role::Provider);
        assert_eq!(
            consumer, provider,
            "the client call and its route meet on one key (param-name drift erased)"
        );
        // A non-normalizing client-call target keys to nothing (never approximated).
        assert!(consumer_portable_key(ArtifactRelation::HttpClientCall, "GET /files/{*rest}").is_none());
    }

    /// Acceptance (1): a static client call in one member binds the sole matching
    /// `Route` in **another** member — the edge starts at the call site and points
    /// at the route, filed under the `route` relation.
    #[test]
    fn a_static_client_call_binds_a_sole_route_in_another_member() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers("web", vec![http_call("GET /users/{id}", "local get_user_call")]);
        set_member("api", 0, vec![route("GET /users/{userId}", "local route_get")]);

        let edges = ContractBridge::new().edges(&registry(&["web", "api"]));

        assert_eq!(edges.len(), 1, "the client call binds its one cross-member route: {edges:?}");
        let edge = &edges[0];
        assert_eq!(edge.relation, "route", "an HTTP arm edge speaks the intra-repo `route` vocabulary");
        assert_eq!(edge.from.member, "web");
        assert_eq!(edge.from.symbol.as_str(), "local get_user_call");
        assert_eq!(edge.to.member, "api");
        assert_eq!(edge.to.symbol.as_str(), "local route_get");
    }

    /// Acceptance (1): two matching routes across the workspace make the same
    /// client call **ambiguous** — no edge (never fabricated, [NFR-RA-05]).
    #[test]
    fn two_matching_routes_make_a_client_call_ambiguous_no_edge() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers("web", vec![http_call("GET /users/{id}", "local get_user_call")]);
        set_member("api", 0, vec![route("GET /users/{id}", "local route_api")]);
        set_member("admin", 0, vec![route("GET /users/{userId}", "local route_admin")]);

        let edges = ContractBridge::new().edges(&registry(&["web", "api", "admin"]));
        assert!(
            edges.is_empty(),
            "two providers of one client-call key are ambiguous — no edge: {edges:?}"
        );
    }

    /// A client call whose only matching route is in its **own** member is an
    /// intra-repo fact the per-repo graph already owns (via the same `route`
    /// relation) — the bridge emits no cross-service edge for it.
    #[test]
    fn a_client_call_to_a_same_member_route_is_not_a_bridge_edge() {
        reset();
        set_member("web", 0, vec![route("GET /users/{id}", "local route_local")]);
        set_consumers("web", vec![http_call("GET /users/{id}", "local get_user_call")]);

        let edges = ContractBridge::new().edges(&registry(&["web"]));
        assert!(
            edges.is_empty(),
            "an in-repo client call→route pair binds locally, not via the bridge: {edges:?}"
        );
    }

    /// Acceptance (3): a client call whose target does not normalize is never a
    /// candidate — it keys to nothing, so no edge is fabricated even when a
    /// same-shaped route exists. (The arm's normalizer refuses such calls before
    /// the ledger; this guards the bridge intake belt-and-suspenders.)
    #[test]
    fn a_non_normalizing_client_call_target_never_binds() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers("web", vec![http_call("GET /files/{*rest}", "local list_files_call")]);
        set_member("api", 0, vec![route("GET /files/{id}", "local route_files")]);

        let edges = ContractBridge::new().edges(&registry(&["web", "api"]));
        assert!(edges.is_empty(), "a catch-all client-call target is not a candidate: {edges:?}");
    }

    /// A client call with no matching route anywhere produces no edge (honestly
    /// unbound, not guessed) — and the method still keys, so a method mismatch
    /// never binds.
    #[test]
    fn a_client_call_method_mismatch_never_binds() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers("web", vec![http_call("POST /users/{id}", "local create_user_call")]);
        set_member("api", 0, vec![route("GET /users/{id}", "local route_get")]);

        let edges = ContractBridge::new().edges(&registry(&["web", "api"]));
        assert!(edges.is_empty(), "a POST call never binds a GET route: {edges:?}");
    }

    // ── S-253 / FR-WS-09: the gRPC stub-call → proto-service arm ──────────────

    /// Provider enrichment at the bridge boundary: a `ProtoService` node whose
    /// body carries its rpc method names fans out into one contract node per
    /// method, named `package.Service/Method` — the fully-qualified provider key.
    /// A method-less service passes through as its bare name (no portable key).
    #[test]
    fn surface_from_expands_a_proto_service_into_per_method_provider_nodes() {
        let nodes = vec![
            nrow(1, NodeKind::ProtoService, "example.v1.UserService", "local svc"),
            // A second service with no captured methods: passes through unexpanded.
            nrow(2, NodeKind::ProtoService, "example.v1.Empty", "local empty"),
        ];
        let mut bodies = HashMap::new();
        bodies.insert(
            LogosSymbol::parse("local svc").unwrap(),
            "GetUser\nListUsers".to_string(),
        );

        let surface = surface_from(&nodes, &[], &bodies);
        let names: Vec<&str> = surface.iter().map(|c| c.name.as_str()).collect();
        assert!(
            names.contains(&"example.v1.UserService/GetUser")
                && names.contains(&"example.v1.UserService/ListUsers"),
            "each rpc method becomes a per-method provider node: {names:?}"
        );
        // Every expansion keeps the service's own symbol as the endpoint identity.
        for c in surface.iter().filter(|c| c.name.contains("UserService")) {
            assert_eq!(c.symbol.as_str(), "local svc");
        }
        // The method-less service is present unexpanded (and carries no key).
        assert!(names.contains(&"example.v1.Empty"));
        assert!(classify(NodeKind::ProtoService, "example.v1.Empty").is_none());
    }

    /// An expanded `ProtoService` (a `/`-bearing FQN) classifies as a gRPC
    /// **provider**; a bare, method-less service carries no portable key.
    #[test]
    fn classify_maps_an_expanded_proto_service_to_a_grpc_provider() {
        let (key, role) =
            classify(NodeKind::ProtoService, "example.v1.UserService/GetUser").unwrap();
        assert_eq!(role, Role::Provider);
        assert_eq!(key.namespace, BridgeNamespace::Grpc);
        assert_eq!(key.key, "example.v1.UserService/GetUser");
        assert_eq!(key.relation(), "grpc-call");
        // A bare service (no rpc method) is not a provider key.
        assert!(classify(NodeKind::ProtoService, "example.v1.UserService").is_none());
    }

    /// The generic ledger classifier ([`invocation_refs_from`]) keeps only
    /// rows whose relation declares a Consumer bridge role, recovering the arm
    /// relation and portable target; a no-payload, non-arm, or provider-side row
    /// contributes nothing. Exercised here on a gRPC-call row.
    #[test]
    fn invocation_refs_from_recovers_only_arm_tagged_grpc_consumer_refs() {
        let row = |payload: Option<&str>| crate::graph_store::UnresolvedRefRow {
            id: 1,
            file_id: None,
            source_symbol: "local stub".to_string(),
            target: "example.v1.UserService/GetUser".to_string(),
            alias: None,
            form: crate::model::RefForm::Method,
            kind: EdgeKind::ArtifactRef,
            line: Some(1),
            resolved: false,
            payload: payload.map(str::to_string),
        };
        // A gRPC-call row → a GrpcCall consumer keyed on its target.
        let consumers = invocation_refs_from(vec![row(Some("grpc-call"))]);
        assert_eq!(consumers.len(), 1, "grpc-call is a consumer arm");
        assert_eq!(consumers[0].relation, ArtifactRelation::GrpcCall);
        assert_eq!(consumers[0].target, "example.v1.UserService/GetUser");
        assert_eq!(consumers[0].symbol.as_str(), "local stub");
        // A code/doc ref (no payload), a non-arm artifact relation, and a contract
        // relation that is not an invocation arm are all ignored.
        assert!(invocation_refs_from(vec![row(None)]).is_empty());
        assert!(invocation_refs_from(vec![row(Some("proto-import"))]).is_empty());
        assert!(invocation_refs_from(vec![row(Some("route"))]).is_empty());
    }

    /// Acceptance (1): a gRPC stub call binds the `package.Service/Method`
    /// provider in another member — the consumer reaches the bridge from the
    /// ledger, the provider from the enriched proto surface, and they meet on the
    /// fully-qualified key under the `grpc-call` relation.
    #[test]
    fn a_grpc_stub_call_binds_the_package_service_method_provider_in_another_member() {
        reset();
        set_member(
            "svc",
            0,
            vec![proto_service("example.v1.UserService/GetUser", "local svc_getuser")],
        );
        set_consumers(
            "api",
            vec![grpc_consumer("example.v1.UserService/GetUser", "local stub_getuser")],
        );

        let edges = ContractBridge::new().edges(&registry(&["api", "svc"]));

        assert_eq!(edges.len(), 1, "the stub call binds its one cross-member provider: {edges:?}");
        assert_eq!(edges[0].relation, "grpc-call");
        assert_eq!(edges[0].from.member, "api");
        assert_eq!(edges[0].from.symbol.as_str(), "local stub_getuser");
        assert_eq!(edges[0].to.member, "svc");
        assert_eq!(edges[0].to.symbol.as_str(), "local svc_getuser");
    }

    /// Acceptance (3a): two members exposing the identical `package.Service/Method`
    /// provider make the call ambiguous — exactly-one is violated, so no edge is
    /// fabricated ([NFR-RA-05]).
    #[test]
    fn two_providers_of_the_same_grpc_key_are_ambiguous_no_edge() {
        reset();
        set_member(
            "svc1",
            0,
            vec![proto_service("example.v1.UserService/GetUser", "local a")],
        );
        set_member(
            "svc2",
            0,
            vec![proto_service("example.v1.UserService/GetUser", "local b")],
        );
        set_consumers(
            "api",
            vec![grpc_consumer("example.v1.UserService/GetUser", "local stub")],
        );

        let edges = ContractBridge::new().edges(&registry(&["api", "svc1", "svc2"]));
        assert!(
            edges.is_empty(),
            "two providers of one gRPC key are ambiguous — no edge: {edges:?}"
        );
    }

    /// Provider enrichment value (the "not just the bare service name" acceptance):
    /// a same-named service in a **different package** is a different key, so it
    /// does NOT collide — the consumer still binds the one same-package provider.
    /// Without package qualification the two would have collided into ambiguity.
    #[test]
    fn a_same_service_in_a_different_package_does_not_collide() {
        reset();
        set_member(
            "svc1",
            0,
            vec![proto_service("example.v1.UserService/GetUser", "local v1")],
        );
        set_member(
            "svc2",
            0,
            vec![proto_service("example.v2.UserService/GetUser", "local v2")],
        );
        set_consumers(
            "api",
            vec![grpc_consumer("example.v1.UserService/GetUser", "local stub")],
        );

        let edges = ContractBridge::new().edges(&registry(&["api", "svc1", "svc2"]));
        assert_eq!(edges.len(), 1, "the package disambiguates the two services: {edges:?}");
        assert_eq!(edges[0].to.member, "svc1");
        assert_eq!(edges[0].to.symbol.as_str(), "local v1");
    }

    /// An intra-repo gRPC call (stub call and provider in the same member) is an
    /// intra-repo fact the per-repo graph owns — the bridge emits no cross-service
    /// edge for it, exactly as the HTTP arm.
    #[test]
    fn an_intra_repo_grpc_call_is_not_a_bridge_edge() {
        reset();
        set_member(
            "svc",
            0,
            vec![proto_service("example.v1.UserService/GetUser", "local svc_getuser")],
        );
        set_consumers(
            "svc",
            vec![grpc_consumer("example.v1.UserService/GetUser", "local stub_getuser")],
        );

        let edges = ContractBridge::new().edges(&registry(&["svc"]));
        assert!(edges.is_empty(), "a same-member stub→service pair is intra-repo: {edges:?}");
    }

    // ── S-256 / FR-WS-11: the broker arm in the LIVE edge stream ──────────────
    //
    // S-254 built the arm's fan-out classifier (`super::broker`) and proved it in
    // isolation, but nothing called it: the bridge's ledger intake was
    // consumer-only, so a subscribe (a `Provider`-role row with no contract node
    // behind it) was indexed nowhere and the arm produced zero live edges. These
    // tests pin the arm *through `ContractBridge::edges`* — the path the query,
    // coverage, and service-map surfaces actually read.

    /// Acceptance: a publish in one member binds the subscribe on the same topic in
    /// **another** member, through the live bridge, via shared topic identity
    /// ([FR-WS-11]). Both endpoints are the real code symbols, so the far side is a
    /// symbol `xservice_impact` can actually walk.
    ///
    /// [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
    #[test]
    fn a_publish_binds_a_cross_member_subscribe_on_the_same_topic() {
        reset();
        set_member("api", 0, vec![]);
        set_consumers("api", vec![broker_publish("orders", "local emit_order")]);
        set_member("billing", 0, vec![]);
        set_consumers("billing", vec![broker_subscribe("orders", "local on_order")]);

        let edges = ContractBridge::new().edges(&registry(&["api", "billing"]));

        assert_eq!(edges.len(), 1, "the publish binds the cross-member subscribe: {edges:?}");
        let edge = &edges[0];
        assert_eq!(edge.relation, "broker-topic");
        assert_eq!(edge.from.member, "api", "the publish is the edge source");
        assert_eq!(edge.from.symbol.as_str(), "local emit_order");
        assert_eq!(edge.to.member, "billing");
        assert_eq!(edge.to.symbol.as_str(), "local on_order");
    }

    /// The fan-out discipline holds end-to-end: one publish reaches **every**
    /// cross-member subscriber of its topic (not the sole one — a topic with two
    /// subscribers is not "ambiguous"), while the publisher's own same-member
    /// subscriber stays intra-repo ([FR-WS-10], [FR-WS-11]).
    ///
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    /// [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
    #[test]
    fn one_publish_fans_out_to_every_cross_member_subscriber_through_the_bridge() {
        reset();
        set_member("api", 0, vec![]);
        set_consumers(
            "api",
            vec![
                broker_publish("orders", "local emit_order"),
                // The publisher also listens to its own topic — intra-repo, not a
                // bridge edge.
                broker_subscribe("orders", "local api_local_listener"),
            ],
        );
        set_member("billing", 0, vec![]);
        set_consumers("billing", vec![broker_subscribe("orders", "local bill_on_order")]);
        set_member("shipping", 0, vec![]);
        set_consumers("shipping", vec![broker_subscribe("orders", "local ship_on_order")]);

        let edges = ContractBridge::new().edges(&registry(&["api", "billing", "shipping"]));

        let tos: Vec<&str> = edges.iter().map(|e| e.to.member.as_str()).collect();
        assert_eq!(
            edges.len(),
            2,
            "one publish fans out to both cross-member subscribers: {edges:?}"
        );
        assert!(tos.contains(&"billing") && tos.contains(&"shipping"));
        assert!(
            !tos.contains(&"api"),
            "the publisher's own subscriber is the intra-repo fan-out, not a bridge edge"
        );
    }

    /// A topic with a publisher but **no subscriber anywhere** produces no edge —
    /// and, critically, no *fabricated* one. The per-repo topic still exists as a
    /// first-class node (the promotion pass's job, [`crate::resolve::topics`]); the
    /// bridge simply has nothing to bind it to ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn a_publish_with_no_subscriber_anywhere_binds_nothing() {
        reset();
        set_member("api", 0, vec![]);
        set_consumers("api", vec![broker_publish("orders", "local emit_order")]);
        set_member("billing", 0, vec![]);

        let edges = ContractBridge::new().edges(&registry(&["api", "billing"]));
        assert!(
            edges.is_empty(),
            "no subscriber exists — no edge is invented: {edges:?}"
        );
    }

    /// A publish reaches the bridge through **one** intake, not two. The broker arm
    /// is routed to its own fan-out classifier *instead of* the loop's consumer
    /// index; were it pushed into both, this single publish/subscribe pair would
    /// emit its edge twice and every service-map link count would be doubled.
    ///
    /// The fixture deliberately puts a **same-string HTTP namesake** in play — a route
    /// literally named `orders` alongside the `orders` topic — so a namespace mix-up
    /// (a broker key meeting an HTTP key, or a publish leaking into the consumer index
    /// the route provider is indexed against) would surface here as an extra edge
    /// rather than passing silently. The two keys share a string and must still never
    /// meet: they live in different [`BridgeNamespace`]s.
    #[test]
    fn a_publish_is_never_counted_through_two_intakes() {
        reset();
        // `api` publishes to the topic `orders` AND calls an unrelated HTTP route.
        set_member("api", 0, vec![]);
        set_consumers(
            "api",
            vec![
                broker_publish("orders", "local emit_order"),
                http_call("GET /orders", "local list_orders_call"),
            ],
        );
        // `billing` subscribes to `orders` and also PROVIDES a route whose name shares
        // the topic's string — the namesake that would catch a namespace collapse.
        set_member("billing", 0, vec![route("GET /orders", "local route_orders")]);
        set_consumers("billing", vec![broker_subscribe("orders", "local on_order")]);

        let edges = ContractBridge::new().edges(&registry(&["api", "billing"]));

        let mut relations: Vec<&str> = edges.iter().map(|e| e.relation.as_str()).collect();
        relations.sort_unstable();
        assert_eq!(
            relations,
            ["broker-topic", "route"],
            "exactly one edge per coupling — one per (publish, subscribe) pair and one \
             per (call, route), never one per intake, and never a cross-namespace \
             match on the shared `orders` string: {edges:?}"
        );

        // The broker edge is the publish→subscribe pair, exactly once.
        let broker: Vec<&BridgeEdge> = edges
            .iter()
            .filter(|e| e.relation == "broker-topic")
            .collect();
        assert_eq!(broker.len(), 1, "the publish is counted once, not once per intake");
        assert_eq!(broker[0].from.symbol.as_str(), "local emit_order");
        assert_eq!(broker[0].to.symbol.as_str(), "local on_order");
    }

    /// A differing message-schema FQN keeps the two sides apart through the live
    /// bridge: the guard rides the topic key, so a publish on
    /// `orders#OrderCreated` never binds a subscribe on `orders#OrderUpdated`
    /// ([FR-WS-10]) — honest at the contract grain, not merely at the topic name.
    ///
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    #[test]
    fn a_differing_message_schema_fqn_prevents_the_bind_through_the_bridge() {
        reset();
        set_member("api", 0, vec![]);
        set_consumers(
            "api",
            vec![broker_publish("orders#com.acme.OrderCreated", "local emit")],
        );
        set_member("billing", 0, vec![]);
        set_consumers(
            "billing",
            vec![broker_subscribe("orders#com.acme.OrderUpdated", "local on_order")],
        );

        let edges = ContractBridge::new().edges(&registry(&["api", "billing"]));
        assert!(
            edges.is_empty(),
            "a differing schema FQN keeps the topics apart — no bind: {edges:?}"
        );
    }

    /// The broker arm coexists with the HTTP arm in one workspace: both bind, each
    /// under its own relation, and neither perturbs the other's edge count. The
    /// union of the two intakes is re-sorted, so the edge set stays deterministic
    /// ([NFR-RA-06]).
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    #[test]
    fn the_broker_and_http_arms_bind_side_by_side_deterministically() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers("web", vec![http_call("GET /users/{id}", "local get_user_call")]);
        set_member("api", 0, vec![route("GET /users/{userId}", "local route_get")]);
        set_consumers("api", vec![broker_publish("orders", "local emit_order")]);
        set_member("billing", 0, vec![]);
        set_consumers("billing", vec![broker_subscribe("orders", "local on_order")]);

        let bridge = ContractBridge::new();
        let edges = bridge.edges(&registry(&["web", "api", "billing"]));

        let mut relations: Vec<&str> = edges.iter().map(|e| e.relation.as_str()).collect();
        relations.sort_unstable();
        assert_eq!(
            relations,
            ["broker-topic", "route"],
            "both arms bind, each under its own relation: {edges:?}"
        );

        // Deterministic: a second bridge over the same fixtures yields the identical
        // (already-sorted) edge sequence.
        let again = ContractBridge::new().edges(&registry(&["web", "api", "billing"]));
        assert_eq!(*edges, *again, "the union of the two intakes is stably sorted");
    }
}
