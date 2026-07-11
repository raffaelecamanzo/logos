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

/// One arm-tagged **cross-service invocation consumer** read from a member's
/// `unresolved_refs` ledger ([FR-WS-07], [FR-WS-08], [ADR-54]).
///
/// Where a [`ContractNode`] carries a member's *declared contract surface*
/// (routes, operations, proto/graphql types the bridge already indexes), an
/// invocation consumer is a *captured call site* — an HTTP client call, a gRPC
/// stub call, a broker publish — that S-251's generic interpreter emitted into
/// the ledger under an invocation-arm [`ArtifactRelation`]. It is the consumer
/// side the bridge feeds into its candidate stream via the arm's
/// [`bridge_namespace`](ArtifactRelation::bridge_namespace) /
/// [`bridge_role`](ArtifactRelation::bridge_role) descriptors, so a new arm
/// reaches the bridge with no edit to the namespace-generic match loop.
///
/// [FR-WS-07]: ../../../docs/specs/requirements/FR-WS-07.md
/// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvocationConsumer {
    /// The invocation arm this consumer reference belongs to — its
    /// [`bridge_namespace`](ArtifactRelation::bridge_namespace) decides the
    /// portable-key form and the match discipline.
    pub relation: ArtifactRelation,
    /// The arm-normalized reference target the interpreter emitted (for HTTP, the
    /// raw `"METHOD /template"` string a `route_key` reduces to the portable key).
    pub target: String,
    /// The canonical, database-portable symbol of the *call site* (the enclosing
    /// declaration the interpreter attributed the reference to) — the consumer
    /// endpoint a bridge edge starts at.
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

    /// Read this member's arm-tagged cross-service **invocation consumers** — the
    /// captured call sites (HTTP client calls, gRPC stub calls, broker publishes)
    /// in its `unresolved_refs` ledger whose relation declares
    /// [`bridge_role`](ArtifactRelation::bridge_role) `Consumer` ([FR-WS-07],
    /// [ADR-54]).
    ///
    /// These feed the bridge's candidate stream alongside the contract-surface
    /// consumers ([`contract_surface`](Self::contract_surface)). The default is
    /// **empty** — a member/engine with no invocation-arm capture contributes no
    /// consumers, so pre-arm members and lightweight test doubles need not
    /// implement it; the real [`Engine`](crate::Engine) overrides it to read the
    /// ledger.
    ///
    /// # Errors
    /// Propagates a read failure so the bridge can skip the member as degraded
    /// rather than aborting the whole workspace ([ADR-53]).
    ///
    /// [FR-WS-07]: ../../../docs/specs/requirements/FR-WS-07.md
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    /// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
    fn invocation_consumers(&self) -> Result<Vec<InvocationConsumer>> {
        Ok(Vec::new())
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

    fn invocation_consumers(&self) -> Result<Vec<InvocationConsumer>> {
        let runtime = self.runtime().context(
            "reading a member's invocation consumers requires a long-lived engine \
             (Engine::start) with a read-only pool",
        )?;
        let rows = runtime.submit_read(|store| store.unresolved_refs())?;
        Ok(invocation_consumers_from(rows))
    }
}

/// Project a member's `unresolved_refs` ledger onto its arm-tagged invocation
/// **consumers** ([FR-WS-07], [ADR-54]).
///
/// A row is a consumer iff its `payload` names an [`ArtifactRelation`] whose
/// [`bridge_role`](ArtifactRelation::bridge_role) is
/// [`Consumer`](BridgeRole::Consumer) — the same generic test for every arm, so
/// this projection never names a concrete arm. A row whose payload is absent, is
/// not a known relation, or is a non-invocation / provider relation is skipped;
/// a row whose `source_symbol` does not parse is skipped rather than fabricating
/// a malformed endpoint ([NFR-RA-05]). Both `resolved` and unresolved rows are
/// included: the cross-service bind is an overlay fact independent of whether the
/// call also bound a route intra-repo.
///
/// [FR-WS-07]: ../../../docs/specs/requirements/FR-WS-07.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn invocation_consumers_from(
    rows: Vec<crate::graph_store::UnresolvedRefRow>,
) -> Vec<InvocationConsumer> {
    rows.into_iter()
        .filter_map(|row| {
            let relation = row.payload.as_deref().and_then(ArtifactRelation::from_wire)?;
            if relation.bridge_role() != Some(BridgeRole::Consumer) {
                return None;
            }
            let symbol = LogosSymbol::parse(&row.source_symbol).ok()?;
            Some(InvocationConsumer {
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

/// Reduce an arm-tagged invocation **consumer** ([`InvocationConsumer`]) to the
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

    // Arm-tagged invocation consumers (HTTP client calls, S-252, and the gRPC/
    // broker arms as they land) join the same candidate stream via the arm's
    // portable key — the consumer-side feed S-251's contract deferred to the
    // arms ([FR-WS-07], [ADR-54]).
    for (member, refs) in read_members(registry, "invocation consumers", |e| {
        e.invocation_consumers()
    }) {
        for consumer in refs {
            let Some(key) = consumer_portable_key(consumer.relation, &consumer.target) else {
                continue; // an unkeyable / not-yet-registered arm contributes nothing
            };
            consumers.push((
                key,
                BridgeEndpoint {
                    member: member.clone(),
                    symbol: consumer.symbol,
                },
            ));
        }
    }

    match_indexed(providers, consumers)
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
        consumers: Vec<InvocationConsumer>,
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
    /// Attach arm-tagged invocation consumers (client-call sites, gRPC stub calls,
    /// broker publishes) to a member — the ledger-sourced consumer stream, distinct
    /// from the node-surface providers `set_member` supplies.
    fn set_consumers(name: &str, consumers: Vec<InvocationConsumer>) {
        FIXTURES.with(|f| {
            f.borrow_mut().entry(name.to_string()).or_default().consumers = consumers;
        });
    }
    /// A gRPC stub-call consumer at `symbol` invoking `key` (`package.Service/Method`).
    fn grpc_consumer(key: &str, symbol: &str) -> InvocationConsumer {
        InvocationConsumer {
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
    fn http_call(target: &str, symbol: &str) -> InvocationConsumer {
        InvocationConsumer {
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
        fn invocation_consumers(&self) -> Result<Vec<InvocationConsumer>> {
            // "unreadable" fails its surface read; keep the same degrade behaviour
            // here so a degraded member is skipped for consumers too.
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
    // `Consumer` candidates (via `invocation_consumers`), keyed through the shared
    // `route_key`, and relies on the unchanged namespace-generic match loop.

    /// The ledger projection keeps only invocation-arm **consumer** rows: a
    /// non-invocation relation (`route`, `proto-import`), a non-consumer arm role,
    /// an absent/unknown payload, and an unparseable source symbol are all
    /// dropped — never a fabricated endpoint ([NFR-RA-05]).
    #[test]
    fn invocation_consumers_from_keeps_only_consumer_arm_rows() {
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
            // A `route` binding is a contract relation, not an invocation arm — dropped.
            row("local op", "GET /users/{id}", Some("route")),
            // A proto import (not an invocation arm) — dropped.
            row("local file", "common.proto", Some("proto-import")),
            // No payload / unknown payload — dropped.
            row("local x", "GET /a", None),
            row("local y", "GET /b", Some("not-a-relation")),
            // A consumer arm row whose source symbol does not parse — dropped.
            row("", "GET /c", Some("http-client-call")),
        ];

        let consumers = invocation_consumers_from(rows);
        assert_eq!(consumers.len(), 1, "only the one parseable consumer row survives");
        assert_eq!(consumers[0].relation, ArtifactRelation::HttpClientCall);
        assert_eq!(consumers[0].target, "GET /users/{id}");
        assert_eq!(consumers[0].symbol.as_str(), "local handler");
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

    /// The generic ledger classifier ([`invocation_consumers_from`]) keeps only
    /// rows whose relation declares a Consumer bridge role, recovering the arm
    /// relation and portable target; a no-payload, non-arm, or provider-side row
    /// contributes nothing. Exercised here on a gRPC-call row.
    #[test]
    fn invocation_consumers_from_recovers_only_arm_tagged_grpc_consumer_refs() {
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
        let consumers = invocation_consumers_from(vec![row(Some("grpc-call"))]);
        assert_eq!(consumers.len(), 1, "grpc-call is a consumer arm");
        assert_eq!(consumers[0].relation, ArtifactRelation::GrpcCall);
        assert_eq!(consumers[0].target, "example.v1.UserService/GetUser");
        assert_eq!(consumers[0].symbol.as_str(), "local stub");
        // A code/doc ref (no payload), a non-arm artifact relation, and a contract
        // relation that is not an invocation arm are all ignored.
        assert!(invocation_consumers_from(vec![row(None)]).is_empty());
        assert!(invocation_consumers_from(vec![row(Some("proto-import"))]).is_empty());
        assert!(invocation_consumers_from(vec![row(Some("route"))]).is_empty());
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
}
