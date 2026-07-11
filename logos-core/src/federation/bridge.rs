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
use crate::model::{EdgeKind, LogosSymbol, NodeId, NodeKind};
use crate::resolve::route_template::route_key;

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
}

impl MemberContracts for crate::Engine {
    fn contract_surface(&self) -> Result<Vec<ContractNode>> {
        let runtime = self.runtime().context(
            "reading a member's contract surface requires a long-lived engine \
             (Engine::start) with a read-only pool",
        )?;
        // Nodes AND the `Contains` tree in one read: an `ApiOperation`'s route
        // reference is reconstructed from its parent `ApiPath` (see `surface_from`).
        let (nodes, edges) =
            runtime.submit_read(|store| Ok((store.all_nodes()?, store.all_edges()?)))?;
        Ok(surface_from(&nodes, &edges))
    }

    fn contract_stamp(&self) -> u64 {
        // The inherent `Engine::sync_stamp` returns a `SyncStamp(u64)`; the
        // bridge caches on the bare monotonic value.
        self.sync_stamp().0
    }
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
/// [route_key]: crate::resolve::route_template::route_key
fn surface_from(nodes: &[NodeRow], edges: &[EdgeRow]) -> Vec<ContractNode> {
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
        .map(|n| {
            let name = if n.kind == NodeKind::ApiOperation {
                match parent_of.get(&n.id).and_then(|p| path_template.get(p)) {
                    Some(template) => format!("{} {}", n.name.to_ascii_uppercase(), template),
                    None => n.name.clone(),
                }
            } else {
                n.name.clone()
            };
            ContractNode {
                kind: n.kind,
                name,
                symbol: n.symbol.clone(),
            }
        })
        .collect()
}

/// The portable, database-independent identity a contract-surface node is
/// matched on across members.
///
/// Only the HTTP key is populated in this story; the gRPC `package.Service/Method`
/// and broker-topic keys arrive with the invocation arms ([FR-WS-07]+). Proto and
/// GraphQL nodes are still *read* into the surface (so the reader is complete and
/// future-ready) but yield no key yet, so they contribute no edges — honestly
/// unbound rather than approximately matched ([NFR-RA-05]).
///
/// [FR-WS-07]: ../../../docs/specs/requirements/FR-WS-07.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// Visible within `federation` (not just this file) so the coverage read-model
/// ([`super::coverage`], [FR-WS-05], [ADR-53]) classifies references with the
/// exact same key vocabulary the bridge matches edges on — one classifier, no
/// drift between "why did this bind" and "why didn't this bind".
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum PortableKey {
    /// An HTTP endpoint keyed by the shared positional `route_key`:
    /// `(upper-cased METHOD, positionally-normalized template)`.
    Http(String, String),
}

impl PortableKey {
    /// The relation class a binding on this key is filed under.
    pub(super) fn relation(&self) -> &'static str {
        match self {
            PortableKey::Http(..) => "route",
        }
    }
}

/// Which side of a portable-key match a node sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    /// Exposes the endpoint (a framework `Route` handler).
    Provider,
    /// Refers to the endpoint (an OpenAPI `ApiOperation`).
    Consumer,
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
            Some((PortableKey::Http(method, template), Role::Provider))
        }
        NodeKind::ApiOperation => {
            let (method, template) = route_key(name)?;
            Some((PortableKey::Http(method, template), Role::Consumer))
        }
        // Proto/GraphQL contract nodes are read into the surface but have no
        // portable HTTP key in this story — deferred to the invocation arms.
        _ => None,
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

/// Read every member's contract surface through its read pool, index providers
/// on portable keys, and match consumers across members with the exactly-one
/// rule. Deterministic: provider lists and the emitted edges are sorted.
fn compute_edges<E>(registry: &EngineRegistry<E>) -> Vec<BridgeEdge>
where
    E: MemberEngine + MemberContracts,
{
    let mut providers: HashMap<PortableKey, Vec<BridgeEndpoint>> = HashMap::new();
    let mut consumers: Vec<(PortableKey, BridgeEndpoint)> = Vec::new();

    for scoped in registry.fan_out(|_, engine| engine.contract_surface()) {
        let member = scoped.member;
        let surface = match scoped.value {
            Ok(Ok(nodes)) => nodes,
            Ok(Err(err)) => {
                tracing::warn!(
                    member = %member,
                    "reading a workspace member's contract surface failed; \
                     bridging degraded without it: {err:#}"
                );
                continue;
            }
            Err(err) => {
                tracing::warn!(
                    member = %member,
                    "a workspace member engine failed to start; \
                     bridging degraded without it: {err:#}"
                );
                continue;
            }
        };

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

    // Deterministic candidate order regardless of member fan-out order.
    for endpoints in providers.values_mut() {
        endpoints.sort();
    }

    let mut edges = Vec::new();
    for (key, consumer) in consumers {
        let Some(candidates) = providers.get(&key) else {
            continue; // no provider anywhere in the workspace — no edge
        };
        // Exactly-one across members ([NFR-RA-05]): two or more providers of the
        // same key are ambiguous and never fabricate an edge.
        let [only] = candidates.as_slice() else {
            continue;
        };
        // A sole provider in the consumer's *own* member is an intra-repo fact
        // the per-repo graph owns; the bridge only emits cross-member links.
        if only.member == consumer.member {
            continue;
        }
        edges.push(BridgeEdge {
            relation: key.relation().to_string(),
            from: consumer,
            to: only.clone(),
        });
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
    }

    fn reset() {
        FIXTURES.with(|f| f.borrow_mut().clear());
        SURFACE_READS.with(|c| c.set(0));
    }
    fn set_member(name: &str, stamp: u64, nodes: Vec<ContractNode>) {
        FIXTURES.with(|f| {
            f.borrow_mut()
                .insert(name.to_string(), MemberFixture { stamp, nodes });
        });
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

        let surface = surface_from(&nodes, &edges);
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
}
