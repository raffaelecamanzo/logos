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
//! Checking the stamps reads only the members whose stamp is not already known
//! (S-484, [`current_stamps`]): a resident member's live stamp is read; a member
//! opened before and since evicted would restart at a constant, so its stamp is
//! stated rather than started; a member never opened, or whose last open
//! failed, is opened as before. A computation still reads every member, since a
//! sole provider is a fact about all of them. Every read names the members it
//! read ([`MemberReads`]).
//!
//! [`sync_stamp`]: crate::Engine::sync_stamp
//! [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
//! [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
//! [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-13]: ../../../docs/specs/architecture/decisions/ADR-13.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::graph_store::{EdgeRow, NodeRow};
use crate::model::{
    ArtifactRelation, BridgeNamespace, BridgeRole, EdgeKind, LogosSymbol, MatchDiscipline, NodeId,
    NodeKind,
};
use crate::resolve::binding::{
    config_bound_keys_of, ConfigBound, ConfigLookup, ProfiledTemplate, Provenance, Resolver,
};

/// The committed-configuration corpus, re-exported under the name federation has
/// always called it by.
///
/// The type itself lives in [`crate::resolve::binding`] since [S-424], because the
/// intra-repo promotion pass reads one on every single-root index — where this
/// module is absent by design. Re-exported rather than renamed at ~40 use sites,
/// and because "member" *is* the right word on this side of the seam.
///
/// [S-424]: ../../../docs/planning/journal.md#s-424-the-promoted-topic-inventory-keys-on-the-committed-value
pub(super) use crate::resolve::binding::MemberCorpus;
use crate::resolve::route_method::preferred_candidates;
use crate::resolve::route_template::route_key;

/// Which side of a portable-key match a node sits on — the bridge's local alias
/// for the model's [`BridgeRole`](crate::model::BridgeRole), the same
/// `Consumer`/`Provider` split an invocation arm declares via
/// [`ArtifactRelation::bridge_role`](crate::model::ArtifactRelation::bridge_role).
/// Re-exported so [`super::coverage`] classifies with one role vocabulary.
pub(super) use crate::model::BridgeRole as Role;

use super::registry::{AnswerScope, EngineRegistry, MemberEngine, MemberReads};
use super::residue::{egress_residue, WorkspaceEgressResidue};

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

/// How a bridge edge entered the overlay — its **intake discriminator**
/// ([FR-WS-08]–[FR-WS-10], [FR-WS-12], [CR-083]).
///
/// An *invocation* edge models a captured **call site** — an HTTP client call
/// ([FR-WS-08]), a gRPC stub call ([FR-WS-09]), or a broker publish/subscribe
/// ([FR-WS-10]) — read from a member's `unresolved_refs` ledger. A
/// *contract-surface* edge is a declared **contract** match — an OpenAPI
/// [`ApiOperation`](NodeKind::ApiOperation) binding a framework
/// [`Route`](NodeKind::Route) — that *describes* an endpoint without modelling a
/// call to it.
///
/// The distinction is exactly what the app-wide reachability view roots on: only
/// an invocation edge seeds a live root ([FR-WS-12]), because only a call reaches
/// its provider. A contract-surface edge documents, and documentation is not
/// reachability — rooting it would mark a callable live purely because a spec file
/// mentions it, the false-live class [NFR-CC-04] exists to prevent ([CR-083]).
///
/// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
/// [FR-WS-09]: ../../../docs/specs/requirements/FR-WS-09.md
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
/// [FR-WS-12]: ../../../docs/specs/requirements/FR-WS-12.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [CR-083]: ../../../docs/requests/CR-083-reachability-invocation-edge-roots.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BridgeIntake {
    /// A captured invocation ([FR-WS-08]/[FR-WS-09]/[FR-WS-10]) — an actual call,
    /// stub call, publish, or subscribe. The **only** intake that seeds an
    /// app-wide reachability live root ([FR-WS-12]).
    Invocation,
    /// A declared contract-surface match (an OpenAPI operation → framework route).
    /// Drawn on the service map and carried by the contract bridge, but never a
    /// reachability live root ([CR-083]).
    ContractSurface,
}

impl BridgeIntake {
    /// `true` for the invocation arms ([FR-WS-08]–[FR-WS-10]) — the only intake
    /// whose provider endpoint the app-wide reachability view seeds as a live root
    /// ([FR-WS-12], [CR-083]).
    pub fn seeds_reachability_root(self) -> bool {
        matches!(self, BridgeIntake::Invocation)
    }
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
    /// How the edge entered the overlay — the intake discriminator that decides
    /// whether it seeds an app-wide reachability live root ([FR-WS-12], [CR-083]).
    /// An **additive** wire field: the prior fields (`relation`, `from`, `to`) are
    /// serialized unchanged, so `xservice route-providers` stays
    /// backward-compatible.
    pub intake: BridgeIntake,
    /// Whether the **consumer** end's target was observed at the call site or
    /// admitted from that member's committed configuration ([S-410],
    /// [FR-WS-19] AC6, [NFR-CC-04]).
    ///
    /// `Literal` for every arm that reads its target verbatim, which since
    /// S-420 ([CR-133]) is every arm but the broker and HTTP ones. A broker
    /// publish, or an HTTP client call, whose operand resolves to a committed
    /// key carries `ConfigBound`, naming the key, its defining sources and its
    /// profile set; one whose keys the corpus refuses carries
    /// `ConfigUnresolved`, naming the key and the refusal.
    ///
    /// Before S-420 the HTTP arm read its target verbatim and a `${…}` reduced to
    /// no portable key, so it drew no edge at all and this field was never
    /// `ConfigBound` on it. **No estate count is restated here on purpose**: this
    /// file is not on either of the two rosters that enumerate the prose sites a
    /// re-index must sweep (the one in `config_bound_admission.rs`'s `by_bucket`
    /// assertion message and the `refresh_procedure` step in
    /// `coverage_headline_baseline`'s artifact), so a figure recorded here would
    /// be outside the procedure that keeps figures current — which is precisely
    /// how a measurement goes quietly stale. The counts, their denominators and
    /// their dates live in `logos-core/tests/config_bound_admission.rs`, the
    /// single home for them.
    ///
    /// [CR-133]: ../../../docs/requests/CR-133-bridge-keys-http-consumer-on-committed-target.md
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub from_value: Provenance,
    /// The same, for the **provider** end.
    ///
    /// Two fields rather than one merged list, and the split is the honesty
    /// rather than ergonomics: on a broker fan-out edge each end names its
    /// *own* configuration key, and the two members routinely spell one
    /// property differently — which is precisely why they could not meet before
    /// [S-410]. A single merged `Vec<ConfigBound>` would carry both keys with
    /// nothing saying which member proved which, so a reader could not tell the
    /// publisher's evidence from the subscriber's ([NFR-CC-04]).
    ///
    /// [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
    pub to_value: Provenance,
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

    /// The sync-stamp a fresh start of the member engine rooted at `root`
    /// would report, when that is known **without** starting it — `None` when
    /// only a start can tell ([NFR-PE-10]).
    ///
    /// The stamp lives in the engine, not the store, so a member that was
    /// opened and then evicted has no stamp until it is started again — and a
    /// restarted [`Engine`](crate::Engine) begins at
    /// [`SyncStamp::INITIAL`](crate::hydrate::SyncStamp::INITIAL). A stamp
    /// check can therefore state such a member's stamp without starting it
    /// ([`current_stamps`]). The default, `None`, keeps the start.
    ///
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    fn restart_stamp(_root: &Path) -> Option<u64>
    where
        Self: Sized,
    {
        None
    }

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

    /// Read this member's **committed configuration definitions** for the
    /// canonical keys a configuration-bound reference names (S-382, [FR-WS-19],
    /// [ADR-64]).
    ///
    /// Every definition of each key is returned — including two that disagree.
    /// Disagreement is represented for the caller to judge, never averaged and
    /// never refused here ([ADR-64] decision point 3); a key no source defines is
    /// simply absent from the map.
    ///
    /// Batched over the whole key set rather than asked one key at a time,
    /// because the coverage tier classifies every member's references in one
    /// pass and a per-key round trip would be one store read per call site.
    ///
    /// The default is **empty** — a member with no committed configuration, and
    /// every lightweight test double, admits nothing. That is the honest answer
    /// rather than a fabricated one: an empty map resolves as
    /// `config-key-missing`, never as a guess.
    ///
    /// # Errors
    /// Propagates a read failure so the caller can skip the member as degraded
    /// rather than aborting the whole workspace ([ADR-53]).
    ///
    /// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    /// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
    fn config_definitions(
        &self,
        keys: &[String],
    ) -> Result<std::collections::BTreeMap<String, Vec<crate::graph_store::ConfigDefinition>>> {
        let _ = keys;
        Ok(std::collections::BTreeMap::new())
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

    /// Read this member's **build-manifest facts** — every Maven/Gradle
    /// manifest the indexer read, with its produced and referenced artifacts
    /// ([FR-WS-33], [ADR-69] point 1).
    ///
    /// Read-only and member-local: the cross-member `builds-against` join is
    /// [`super::build_deps`]'s, and it never feeds this bridge's matcher, its
    /// edge set or any runtime figure ([BR-58], [ADR-26]).
    ///
    /// `None` when the store records no full-walk build-manifest pass yet
    /// ([`crate::graph_store::BUILD_FACTS_EXTRACTED_KEY`] absent) — a store
    /// upgraded across migration 22, or one never indexed. Its empty tables
    /// then say nothing about the member's manifests, so the join reports it
    /// unread with that reason, never as "read, no manifests" ([FR-WS-33]).
    ///
    /// The default is **extracted and empty** — every lightweight test double
    /// contributes no fact and reads as a member with no build manifest.
    ///
    /// # Errors
    /// Propagates a read failure so the caller can skip the member as degraded
    /// rather than aborting the whole workspace ([ADR-53]).
    ///
    /// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
    /// [ADR-69]: ../../../docs/specs/architecture/decisions/ADR-69.md
    /// [BR-58]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    fn build_manifests(&self) -> Result<Option<Vec<crate::graph_store::BuildManifestRow>>> {
        Ok(Some(Vec::new()))
    }

    /// Read this member's **declared-type facts** and its still-unresolved
    /// Java/Kotlin `Imports`/`TypeUses` rows — the two inputs of the
    /// cross-member type overlay ([FR-WS-35], [ADR-70] point 2).
    ///
    /// Read-only and member-local: the cross-member match is
    /// [`super::type_refs`]'s, and it never feeds this bridge's matcher, its
    /// edge set or any runtime figure ([BR-60]).
    ///
    /// `None` when the store does not mark its declared types extracted
    /// ([`crate::graph_store::DECLARED_TYPES_EXTRACTED_KEY`] absent) — a store
    /// upgraded across migration 24, or one never indexed — so the overlay
    /// reports it unread, never as a member declaring nothing.
    ///
    /// The default is **extracted and empty** — every lightweight test double
    /// contributes no fact and reads as a member with no Java/Kotlin/Avro file.
    ///
    /// # Errors
    /// Propagates a read failure so the caller can skip the member as degraded
    /// rather than aborting the whole workspace ([ADR-53]).
    ///
    /// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
    /// [ADR-70]: ../../../docs/specs/architecture/decisions/ADR-70.md
    /// [BR-60]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    fn type_facts(&self) -> Result<Option<super::type_refs::MemberTypeFacts>> {
        Ok(Some(super::type_refs::MemberTypeFacts::default()))
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

    fn restart_stamp(root: &Path) -> Option<u64> {
        // Every constructor seeds `sync_stamp` at `INITIAL`, and a start over a
        // store that is there advances nothing. Only a store at the store path
        // makes that claim: with none, a start creates one or seeds a worktree
        // (the seed's diff-reconcile advances the stamp), and with something
        // else there it fails — so either way it must really be attempted.
        matches!(super::registry::store_file(root), super::open_state::StoreFile::Present)
            .then_some(crate::hydrate::SyncStamp::INITIAL.0)
    }

    fn invocation_refs(&self) -> Result<Vec<InvocationRef>> {
        let runtime = self.runtime().context(
            "reading a member's invocation references requires a long-lived engine \
             (Engine::start) with a read-only pool",
        )?;
        let rows = runtime.submit_read(|store| store.unresolved_refs())?;
        Ok(invocation_refs_from(rows))
    }

    fn build_manifests(&self) -> Result<Option<Vec<crate::graph_store::BuildManifestRow>>> {
        let runtime = self.runtime().context(
            "reading a member's build-manifest facts requires a long-lived engine \
             (Engine::start) with a read-only pool",
        )?;
        // One pooled read, two statements (no read transaction, so not one
        // snapshot). Marker first: it is never removed, and a full walk commits
        // it in the same batch as its facts, so rows read after it are at least
        // as fresh as the facts it vouches for.
        runtime.submit_read(|store| {
            if !store.build_facts_extracted()? {
                return Ok(None);
            }
            store.build_manifests().map(Some)
        })
    }

    fn type_facts(&self) -> Result<Option<super::type_refs::MemberTypeFacts>> {
        let runtime = self.runtime().context(
            "reading a member's declared-type facts requires a long-lived engine \
             (Engine::start) with a read-only pool",
        )?;
        runtime.submit_read(super::type_refs::read_type_facts)
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

    fn config_definitions(
        &self,
        keys: &[String],
    ) -> Result<std::collections::BTreeMap<String, Vec<crate::graph_store::ConfigDefinition>>> {
        if keys.is_empty() {
            // No configuration-bound reference in this member: no read at all,
            // not an empty one. The overwhelmingly common case, and the reason
            // this surface costs nothing on an estate that configures nothing.
            return Ok(std::collections::BTreeMap::new());
        }
        let runtime = self.runtime().context(
            "reading a member's configuration definitions requires a long-lived engine \
             (Engine::start) with a read-only pool",
        )?;
        let keys = keys.to_vec();
        runtime.submit_read(move |store| {
            let mut out = std::collections::BTreeMap::new();
            for key in keys {
                let defs = store.config_definitions(&key)?;
                // A key no source defines is ABSENT rather than present-and-empty:
                // the resolver reads an absent key as `missing`, and an empty vec
                // would say the same thing twice.
                if !defs.is_empty() {
                    out.insert(key, defs);
                }
            }
            Ok(out)
        })
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
/// [`BucketKey`] (a [`BridgeNamespace`] plus the arm's normalized **bucket
/// string**) and the within-bucket method facet ([FR-WS-07], [ADR-54]).
///
/// Namespace-generic by construction — the match loop indexes on the bucket and
/// applies the namespace's [`match_discipline`](BridgeNamespace::match_discipline),
/// never any per-arm code. The HTTP key splits the shared positional
/// [`route_key`] `(METHOD, template)` across the two: the template is the
/// bucket, the method is the facet ([CR-109]). The gRPC and broker arms
/// ([FR-WS-09], [FR-WS-10]) build their own bucket strings
/// (`package.Service/Method`, a topic name) under their own namespace with no
/// facet at all. Two candidates in one bucket meet iff the shared
/// [`route_method`](crate::resolve::route_method) rule says the provider's facet
/// serves the consumer's — regardless of which arm produced them.
///
/// [CR-109]: ../../../docs/requests/CR-109-wildcard-method-route-matching.md
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
    /// The bucket two candidates must share before they are compared at all.
    pub(super) bucket: BucketKey,
    /// The within-bucket **method facet**: `Some("GET")`, `Some("ANY")`, … for the
    /// HTTP namespace; `None` for the gRPC and broker namespaces, whose keys carry
    /// no method dimension. Compatibility and wildcard precedence over this facet
    /// are the shared [`route_method`](crate::resolve::route_method) rule's job,
    /// never this struct's ([CR-109]).
    ///
    /// [CR-109]: ../../../docs/requests/CR-109-wildcard-method-route-matching.md
    pub(super) method: Option<String>,
}

/// The **bucket identity** a provider index files candidates under: the
/// namespace plus the arm-normalized bucket string ([CR-109], [ADR-52]).
///
/// For the HTTP namespace that string is the positionally-normalized template
/// **alone**, with the method held apart in [`PortableKey::method`]. That split
/// is the whole point: a `HashMap` keyed on the whole `(method, template)` tuple
/// cannot express a wildcard method, so a wildcard and an exact-method provider
/// of one endpoint have to land in the *same* bucket for the precedence rule to
/// see them together. For the gRPC and broker namespaces, whose keys have no
/// method dimension, the bucket string is the whole key and the facet is `None` —
/// so their matching is untouched.
///
/// [CR-109]: ../../../docs/requests/CR-109-wildcard-method-route-matching.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct BucketKey {
    /// The invocation namespace this key lives in — decides the match discipline.
    pub(super) namespace: BridgeNamespace,
    /// The arm-normalized, database-portable bucket string two candidates meet on.
    pub(super) key: String,
}

/// One provider filed in a bucket: its endpoint plus the method facet deciding
/// which consumers of that bucket it serves and how specific it is ([CR-109]).
///
/// `endpoint` is declared first so a bucket sorts by endpoint, exactly as it did
/// when a bucket was a bare `Vec<BridgeEndpoint>` ([NFR-RA-06]).
///
/// [CR-109]: ../../../docs/requests/CR-109-wildcard-method-route-matching.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ProviderCandidate {
    /// The database-portable endpoint this provider binds to. Declared **first**
    /// so the derived [`Ord`] sorts a bucket by endpoint — the determinism
    /// [`sort_buckets`] relies on ([NFR-RA-06]). Reordering these two fields
    /// silently changes that order.
    pub(super) endpoint: BridgeEndpoint,
    /// The provider's method facet, mirroring [`PortableKey::method`].
    pub(super) method: Option<String>,
    /// Whether the key this provider is filed under was read verbatim from its
    /// own declaration or admitted from its member's committed configuration
    /// ([S-410]) — carried here so a bound edge can name the **provider** end's
    /// evidence, which is nameable from no other surface (a provider-role row
    /// emits no coverage reference of its own).
    ///
    /// Declared **after** `method` so the derived [`Ord`] above is unchanged:
    /// two candidates still sort by `(endpoint, method)` and reach this field
    /// only when both are equal, which the per-`(key, role, member, symbol)`
    /// de-duplication already excludes.
    ///
    /// [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
    pub(super) value: Provenance,
}

/// Providers indexed for matching: each [`BucketKey`] to the candidates filed
/// under it. Built only by [`index_provider`], read only by
/// [`bucket_candidates`], and ordered only by [`sort_buckets`] — so the bridge,
/// the broker arm and the coverage read-model cannot build differently-shaped
/// indexes ([ADR-52]).
pub(super) type ProviderIndex = HashMap<BucketKey, Vec<ProviderCandidate>>;

impl PortableKey {
    /// An HTTP key from the shared positional [`route_key`] parts: the
    /// positionally-normalized template becomes the bucket under the
    /// [`Http`](BridgeNamespace::Http) namespace and the upper-cased method
    /// becomes the facet. Derivation is unchanged — an endpoint still reduces to
    /// `(METHOD, normalized template)`; only where the two halves live differs
    /// ([ADR-52] as amended by [CR-109]).
    ///
    /// [route_key]: crate::resolve::route_template::route_key
    /// [CR-109]: ../../../docs/requests/CR-109-wildcard-method-route-matching.md
    /// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
    pub(super) fn http(method: String, template: String) -> PortableKey {
        PortableKey {
            bucket: BucketKey {
                namespace: BridgeNamespace::Http,
                key: template,
            },
            method: Some(method),
        }
    }

    /// A broker-topic key from an arm-normalized topic string (a topic name,
    /// optionally guarded by a `#`-appended message-schema FQN) under the fan-out
    /// [`BrokerTopic`](BridgeNamespace::BrokerTopic) namespace (S-254,
    /// [FR-WS-10]). The [`super::broker`] classifier builds these; two sides meet
    /// iff their whole topic key (topic + optional guard) is byte-equal — the
    /// namespace carries no method facet, so it is `None`.
    ///
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    pub(super) fn broker(key: String) -> PortableKey {
        PortableKey {
            bucket: BucketKey {
                namespace: BridgeNamespace::BrokerTopic,
                key,
            },
            method: None,
        }
    }

    /// A gRPC key from an already fully-qualified `package.Service/Method` string
    /// under the [`Grpc`](BridgeNamespace::Grpc) namespace (S-253, [FR-WS-09]).
    /// The namespace carries no method facet — the verb is part of the key — so it
    /// is `None`.
    ///
    /// Called from the provider side ([`classify`]) on every indexed
    /// `.proto` service — that half is real. No consumer target ever reaches
    /// here in production; see `ArtifactRelation::GrpcCall`'s doc for why the
    /// arm is honestly absent rather than removed ([S-379]).
    ///
    /// [FR-WS-09]: ../../../docs/specs/requirements/FR-WS-09.md
    /// [S-379]: ../../../docs/planning/journal.md#s-379-the-grpc-invocation-arm-is-marked-honestly-absent
    pub(super) fn grpc(key: String) -> PortableKey {
        PortableKey {
            bucket: BucketKey {
                namespace: BridgeNamespace::Grpc,
                key,
            },
            method: None,
        }
    }

    /// The invocation namespace this key lives in.
    pub(super) fn namespace(&self) -> BridgeNamespace {
        self.bucket.namespace
    }

    /// The relation class a binding on this key is filed under — the namespace's
    /// stable relation label ([`BridgeNamespace::relation`]).
    pub(super) fn relation(&self) -> &'static str {
        self.bucket.namespace.relation()
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
                Some((PortableKey::grpc(name.to_string()), Role::Provider))
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
/// The gRPC and broker arms ([FR-WS-09], [FR-WS-10]) register their own
/// namespace key below — an arm lacking its key builder is inert here, exactly
/// like a language lacking its capture. The gRPC branch is reachable code with
/// no reachable *caller*: no `.scm` query anywhere produces a `GrpcCall` target
/// for this function to key, so it runs only from this module's own tests
/// until a language capture lands ([S-379], [CR-120]).
///
/// [FR-WS-07]: ../../../docs/specs/requirements/FR-WS-07.md
/// [FR-WS-09]: ../../../docs/specs/requirements/FR-WS-09.md
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
/// [S-379]: ../../../docs/planning/journal.md#s-379-the-grpc-invocation-arm-is-marked-honestly-absent
/// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
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
        BridgeNamespace::Grpc => Some(PortableKey::grpc(target.to_string())),
        // A broker publish's target is the arm-normalized topic key (a topic name,
        // optionally `#`-guarded by a message-schema FQN) the broker normalizer
        // wrote (S-254, [FR-WS-10]) — the same string [`crate::resolve::broker_identity::admit`]
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
        //
        // A **keyless** broker row — an empty target — is the arm's recorded
        // `topic-not-literal` refusal ([CR-107]), not a topic: it is refused here so
        // it can never index a provider or bind a publish, and the coverage tier
        // reports it under the arm's own reason. Trimmed, because an all-whitespace
        // topic is no more of an identity than an absent one (the same test
        // `broker_topic_key` applies at capture).
        BridgeNamespace::BrokerTopic if target.trim().is_empty() => None,
        BridgeNamespace::BrokerTopic => Some(PortableKey::broker(target.to_string())),
    }
}

/// The configuration keys a reference names, or [`None`] if it names none —
/// **the single predicate for "is this a configuration-bound reference?"**
/// (S-382, [ADR-64], extended to the broker arm by [S-410]).
///
/// One helper, every caller — [`member_corpora`] below, which decides **which
/// members are opened**, [`identify`], which decides **which HTTP targets are
/// resolved**, and since [S-424] the intra-repo promotion pass
/// ([`crate::resolve::topics`]), which decides **whether its own member's store
/// is read for a corpus at all** — because those questions used to be asked
/// separately and had already drifted once: the coverage tier's classification
/// loop tested the arm's namespace and the corpus read did not, so a member
/// whose only placeholders were **broker topics** had its store opened to read a
/// corpus that was then never consulted.
///
/// [S-424]: ../../../docs/planning/journal.md#s-424-the-promoted-topic-inventory-keys-on-the-committed-value
///
/// A gate that opens a store and a gate that resolves an operand **must** be one
/// predicate: were they two, a member could be opened for a target nothing
/// resolves (the wasted read above) or — worse — a target could resolve in a
/// member whose corpus was never read, and resolve against nothing. Hence the
/// `(relation, target)` form below, which the HTTP classifier calls directly. The
/// broker arm reaches the same answer through
/// [`crate::resolve::broker_identity::topic_identity`]'s own `placeholder_keys` call, which is a
/// narrower test — it has already established its own relation — and is left
/// where it is rather than routed through here for a relation it knows.
///
/// The arm test is on the **relation**, not on its namespace. The guard admits a
/// site and each body then classifies it on its own arm, so asking about the
/// namespace and answering about the relation is one drift away from filing a
/// second Http-namespace arm under the HTTP arm's discipline.
///
/// # The broker carve-out this predicate used to hold is gone, deliberately
///
/// Until [S-410] this admitted [`HttpClientCall`](ArtifactRelation::HttpClientCall)
/// **only**, and its own doc named [ADR-64]'s boundary as the reason:
/// *"this decision does not resolve broker topics against configuration, and
/// must not be read as doing so"*. That boundary was narrowed by [ADR-64]'s
/// 2026-09-15 amendment and then lifted for the accessor-operand population by
/// [FR-WS-10]'s re-proposed criterion, which [S-410] delivers. What the broker
/// arm does with the keys is **not** the HTTP arm's rule, though, and the
/// difference is load-bearing: a broker operand the corpus refuses keeps its
/// placeholder-as-written key ([`crate::resolve::broker_identity::TopicIdentity::Unresolved`]),
/// where an HTTP one is reported unbound. This predicate answers *"does it name
/// a key"*; it does not decide what happens next.
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
/// [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
pub(super) fn config_bound_keys(reference: &InvocationRef) -> Option<Vec<String>> {
    config_bound_keys_of(reference.relation, &reference.target)
}

/// What committed configuration proves about one **HTTP invocation site's
/// target**, and every portable key the site therefore meets a provider on —
/// the HTTP twin of [`crate::resolve::broker_identity::identify`] (S-420, [CR-133]).
///
/// Built only by [`identify`], which is the single place the HTTP arm's
/// committed-value rule is applied: the bridge's consumer arm
/// ([`compute_edges`]), the coverage tier's `arm_identity` and its
/// `record_config_bound` all reduce their targets through that one function, so
/// *"why did this bind"* and *"why didn't this bind"* cannot drift ([ADR-52])
/// the way they did while the bridge keyed a consumer on its **raw** ledger
/// target and the coverage tier resolved the same target through committed
/// configuration. Two call sites sharing a predicate is not one classifier; one
/// function is.
///
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
/// [CR-133]: ../../../docs/requests/CR-133-bridge-keys-http-consumer-on-committed-target.md
#[derive(Debug, Clone)]
pub(super) struct HttpIdentity {
    /// The canonical configuration keys the target names, in source order —
    /// **empty** for a target that names none.
    ///
    /// The same question [`config_bound_keys`] answers, answered here as part of
    /// the one resolution rather than asked again afterwards: a caller that
    /// re-asked could get a different answer, which is the drift this type
    /// exists to close.
    pub(super) named_keys: Vec<String>,
    /// One entry per committed composition that reduces to a portable key, with
    /// the evidence proving **that** composition ([ADR-64] decision point 3:
    /// *edges carry the profiles that produce them*).
    ///
    /// Empty means the site keys on nothing: a target no committed source
    /// proves, or one whose every composition failed to normalize. It is **not**
    /// the same as `composed` being empty — see that field.
    ///
    /// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
    pub(super) keyed: Vec<(PortableKey, Provenance)>,
    /// Every committed composition, whether or not it reduced to a key, in the
    /// resolver's own deterministic order ([NFR-RA-06]).
    ///
    /// Empty whenever no configuration was read — a target naming no key, and a
    /// target whose keys the corpus refused. The coverage recorder names its
    /// *nothing-keyed* refusal from the first entry, which is why the
    /// compositions travel even when none of them keyed.
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub(super) composed: Vec<String>,
    /// The provenance of the **site**, as opposed to of one composition:
    /// `Literal` for a target naming no key, `ConfigBound` carrying every key's
    /// whole evidence where the sources prove one, and `ConfigUnresolved`
    /// naming the keys and the refusal where they prove nothing.
    ///
    /// This is what a coverage row carries — one row per reference, however many
    /// overlays — while [`keyed`](Self::keyed) carries what an *edge* carries.
    pub(super) value: Provenance,
}

/// Reduce an HTTP invocation reference's stored target to [`HttpIdentity`], or
/// [`None`] when `relation` is not an [`Http`](BridgeNamespace::Http) arm
/// (S-420, [CR-133]).
///
/// The three outcomes are the whole rule, and the third is where the HTTP arm
/// and the broker arm genuinely differ:
///
/// - **No `${…}` at all** — the target is keyed by its own text through
///   [`consumer_portable_key`], exactly as it was before this story, with
///   `Literal` provenance. A target that does not normalize keys nothing.
/// - **The committed sources prove it** — the target is resolved against the
///   member's own corpus ([FR-WS-19], [ADR-64]'s within-reach rule is the
///   caller's) and **each** composition is keyed independently, so a key two
///   overlays commit differently yields one key per overlay ([FR-WS-19] AC2). A
///   composition that does not normalize — the estate's dominant
///   `base-url: https://orders:8080` idiom composes an absolute URL, and
///   [`route_key`] takes only a rooted path — simply contributes no key; it is
///   the caller's business to name that refusal, and `composed` carries what it
///   needs to.
/// - **The sources prove nothing** — no key is admitted and none is fabricated
///   ([NFR-RA-05]). This is the arm difference: a broker operand the corpus
///   refuses keeps its placeholder-as-written key ([FR-WS-10]'s re-proposed
///   criterion), an HTTP one is reported unbound. The refusal still travels, as
///   [`Provenance::ConfigUnresolved`].
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
/// [CR-133]: ../../../docs/requests/CR-133-bridge-keys-http-consumer-on-committed-target.md
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [route_key]: crate::resolve::route_template::route_key
pub(super) fn identify(
    relation: ArtifactRelation,
    target: &str,
    corpus: &dyn ConfigLookup,
) -> Option<HttpIdentity> {
    if relation.bridge_namespace()? != BridgeNamespace::Http {
        return None;
    }
    // The pre-S-420 rule, kept in one expression because all three branches that
    // read a target verbatim must read it the same way.
    let verbatim = || -> Vec<(PortableKey, Provenance)> {
        consumer_portable_key(relation, target)
            .into_iter()
            .map(|key| (key, Provenance::Literal))
            .collect()
    };
    // The SAME predicate `member_corpora` gates a member's store open on, so a
    // target this function resolves is a target whose member was opened.
    let Some(named_keys) = config_bound_keys_of(relation, target) else {
        return Some(HttpIdentity {
            named_keys: Vec::new(),
            keyed: verbatim(),
            composed: Vec::new(),
            value: Provenance::Literal,
        });
    };
    let resolver = Resolver { corpus, module: "" };
    match resolver.resolve_template(target) {
        // `placeholder_keys` above already said the target carries a placeholder,
        // so `resolve_template` cannot answer `None` here. Mapped to the literal
        // rule rather than unwrapped: a panic in a read-model is never the right
        // answer to a disagreement between two scans — the same choice
        // `crate::resolve::broker_identity::topic_identity` makes for the identical impossibility.
        None => Some(HttpIdentity {
            named_keys,
            keyed: verbatim(),
            composed: Vec::new(),
            value: Provenance::Literal,
        }),
        Some(Err(refusal)) => Some(HttpIdentity {
            keyed: Vec::new(),
            composed: Vec::new(),
            value: Provenance::ConfigUnresolved {
                keys: named_keys.clone(),
                refusal,
            },
            named_keys,
        }),
        Some(Ok(resolved)) => {
            let keyed = resolved
                .candidates
                .iter()
                .filter_map(|candidate| {
                    consumer_portable_key(relation, &candidate.template).map(|key| {
                        (
                            key,
                            Provenance::ConfigBound {
                                bound: composition_evidence(&resolved.bound, candidate),
                            },
                        )
                    })
                })
                .collect();
            Some(HttpIdentity {
                named_keys,
                keyed,
                composed: resolved
                    .candidates
                    .iter()
                    .map(|candidate| candidate.template.clone())
                    .collect(),
                value: Provenance::ConfigBound {
                    bound: resolved.bound,
                },
            })
        }
    }
}

/// The evidence behind **one** committed composition: for each key the target
/// names, the values whose profiles prove this composition ([ADR-64] decision
/// point 3).
///
/// Narrowed rather than copied whole, because an edge carries the profiles that
/// produce **it**: two overlays committing one key to two templates yield two
/// edges, and handing each the site's entire evidence would leave a reader
/// unable to say which overlay drew which edge ([NFR-CC-04]).
///
/// **Exact, because the composer says which value it used.** An earlier version
/// of this function inferred the attribution from the candidate's profile set —
/// keeping every value whose profiles intersected it, plus the unprofiled base
/// unconditionally — and that inference over-claimed in the estate's dominant
/// idiom: a key committed once in `application.yml` and again under a profile
/// composes the profiled value *instead of* the base
/// ([`values_under`](crate::resolve::binding)), so the profiled edge would have
/// named a value that produced the *other* edge. Two values one profile set
/// cannot tell apart (a key two unprofiled sources commit differently) had no
/// inference at all. [`ProfiledTemplate::used`] records the substitution, so
/// there is nothing left to infer.
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
fn composition_evidence(bound: &[ConfigBound], candidate: &ProfiledTemplate) -> Vec<ConfigBound> {
    bound
        .iter()
        .enumerate()
        .map(|(key, entry)| {
            // `used` is one value per key of `bound`, in the same order, so the
            // index is the join. A short list means the composer and this reader
            // disagree about the key count, which cannot happen — both read the
            // same `ResolvedTemplate` — and is answered with the full evidence
            // rather than a panic, because a read-model never answers a
            // disagreement between two scans with a crash.
            let Some(used) = candidate.used.get(key) else {
                debug_assert!(false, "composition {candidate:?} names no value for {entry:?}");
                return entry.clone();
            };
            let values: Vec<_> =
                entry.values.iter().filter(|value| &value.value == used).cloned().collect();
            // Also unreachable: the substituted value came out of this very
            // entry. Same rule as above if it ever is reached — an empty list
            // would read as "nothing proved this", a stronger claim than the
            // narrowing is entitled to make.
            debug_assert!(
                !values.is_empty(),
                "a composition's own key proved nothing: {entry:?} for {candidate:?}"
            );
            if values.is_empty() {
                return entry.clone();
            }
            ConfigBound {
                values,
                ..entry.clone()
            }
        })
        .collect()
}

/// Read, per member, the committed definitions of every configuration key its
/// own invocation references name (S-382, [ADR-64]).
///
/// **Scoped to the member, which is the whole of the committed-evidence line's
/// second part.** [ADR-64] admits a value that is *within reach of the reading
/// module*, so a key is looked up in the member that reads it and nowhere else:
/// a workspace-wide lookup would let one service's `application.yml` supply
/// another's base URL — or another's topic — which is a search of the estate
/// rather than a name lookup.
///
/// One read per member holding at least one such reference, over that member's
/// whole key set — never one read per call site. A member whose read fails is
/// **absent** from the map and its references then resolve against nothing:
/// degrade-don't-abort, exactly as every other per-member read in this module
/// ([ADR-53]).
///
/// `consumers` is whatever slice the caller wants resolved, and callers pass
/// **only the arms they will consult** — which since S-420 ([CR-133]) is every
/// configuration-bound arm on both tiers: the bridge resolves its HTTP consumer
/// targets through [`identify`] exactly as the coverage tier does, so it passes
/// its HTTP references alongside its broker ones. That costs no store open on a
/// workspace whose HTTP targets carry no `${…}`: the gate below is
/// [`config_bound_keys`], a string test over references already in memory, so a
/// member naming no key is never opened ([NFR-PE-10]).
///
/// Until S-420 the bridge passed its broker references alone, and the reason
/// recorded here was that *"reading an HTTP key it does not resolve would open a
/// member store to produce nothing"* — true while the bridge did not resolve
/// HTTP keys, and superseded by its doing so.
///
/// [CR-133]: ../../../docs/requests/CR-133-bridge-keys-http-consumer-on-committed-target.md
///
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
pub(super) fn member_corpora<E>(
    answer: &AnswerScope<'_, E>,
    consumers: &[(String, InvocationRef)],
) -> BTreeMap<String, MemberCorpus>
where
    E: MemberEngine + MemberContracts,
{
    let mut wanted: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (member, consumer) in consumers {
        let Some(keys) = config_bound_keys(consumer) else {
            continue;
        };
        let entry = wanted.entry(member.clone()).or_default();
        for key in keys {
            if !entry.contains(&key) {
                entry.push(key);
            }
        }
    }
    let mut out = BTreeMap::new();
    // Only the members that actually name a key are opened. The scope's
    // `fan_out` is deliberately NOT used: it reaches every member of the
    // workspace, and on an 84-member estate where one service configures its
    // base URL that would open 83 stores to read nothing ([NFR-PE-10]).
    //
    // Every member reached here already opened for this answer's invocation-refs
    // walk — a member that failed to open contributes no consumer — so this can
    // never add an attempt for a *broken* member, which is what the
    // once-per-answer guarantee ([FR-WS-16] AC5) is about.
    //
    // It is NOT, however, always a resident hit, and an earlier wording here
    // said it was. `engine_for` bypasses `AnswerScope::open_for_walk` and goes
    // straight to admission, which evicts to `max_resident_members()` before
    // every build; that budget is derived from the host's descriptor limit and
    // is routinely far below an 84-member roster. A member that named a key
    // early in the roster has therefore usually been evicted by the time this
    // runs, and is rebuilt — a real engine start, counted in `engine_starts()`
    // and `reconstructions()`. The cost is bounded (one rebuild per member that
    // names a key, not per row) and correctly ledgered, but it is a cost, and
    // S-397 T1 is what moved this path from dormant to live: before that hop the
    // estate emitted zero `config-bound` rows, so `wanted` was always empty.
    // `tests/workspace_connection_budget.rs` cannot see it either — its fixture
    // never indexes, so `wanted` is empty there too and the `engine_starts()`
    // equality it pins against `WALKS_PER_STATUS` omits this term. Recorded for
    // the sprint-68 review rather than silently re-justified.
    for (member, keys) in wanted {
        match answer
            .registry()
            .engine_for(&member)
            .and_then(|e| e.config_definitions(&keys))
        {
            Ok(corpus) => {
                out.insert(member, corpus);
            }
            Err(err) => tracing::warn!(
                member = %member,
                "reading a workspace member's configuration definitions failed; \
                 its configuration-bound references refuse rather than guess: {err:#}"
            ),
        }
    }
    out
}

/// The in-memory cross-service contract bridge over a workspace's members
/// ([FR-WS-04], [ADR-52]).
///
/// Holds the derived read-models keyed on member sync-stamps — the edge set
/// ([`edges`](Self::edges)) and the unresolved egress residue
/// ([`residue`](Self::residue), [CR-125]); the member set it reads is supplied
/// per call as an [`EngineRegistry`], so one bridge tracks one workspace's
/// registry. Shareable behind an [`Arc`]: each interior cache is a [`Mutex`], so
/// the serve surface and concurrent callers see one bridge.
///
/// The two caches are **separate slots**, so an `xservice search` or
/// `route-providers` call pays only for the edges and never for a residue it
/// will not render ([NFR-PE-01]). They are nonetheless filled through **one**
/// entry point on the reachability path ([`reachability_read`](Self::reachability_read)),
/// keyed on one stamp snapshot, because two independent snapshots can straddle a
/// member re-sync and make the answer contradict its own residue.
///
/// # A deliberate upward dependency, recorded rather than incurred silently
/// Holding the residue slot means this module names [`WorkspaceEgressResidue`],
/// a read-model defined above it — the first edge from the bridge back up into
/// the derived modules. It is taken knowingly: the alternative is a second cache
/// owner beside the bridge, and then nothing could key the two read-models on
/// one snapshot, which is the guarantee above. The `super::residue` import is
/// the whole of the coupling; no derived logic lives here.
///
/// [CR-125]: ../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
/// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
/// [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
#[derive(Debug, Default)]
pub struct ContractBridge {
    edge_cache: StampCache<EdgeSnapshot>,
    residue_cache: StampCache<WorkspaceEgressResidue>,
}

/// The cached edge set, with the members it was computed **without**.
///
/// A member whose engine would not start or whose surface read failed when the
/// set was computed contributes no edge to it, and an answer served from the
/// cache still lacks those edges — so every answer read off this snapshot names
/// those members unread, with their reasons, whether or not it recomputed
/// ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug)]
struct EdgeSnapshot {
    edges: Arc<Vec<BridgeEdge>>,
    unread: BTreeMap<String, String>,
}

/// The bridge cache key: each member's sync-stamp at compute time, sorted by
/// member. Any change — a stamp advancing, or a member appearing or
/// disappearing — is a miss.
pub(super) type Stamps = Vec<(String, u64)>;

/// A cache slot: empty, or the stamps a value was computed at beside the value,
/// shared so repeated reads clone an [`Arc`] rather than the value.
type Stamped<T> = Option<(Stamps, Arc<T>)>;

/// One derived read-model cached against the member sync-stamps it was computed
/// at ([FR-WS-04]).
///
/// Generic over what it holds, and **shared** by the bridge's two slots rather
/// than written once per slot: a second hand-mirrored copy of this
/// read-check-compute-store dance is precisely the twin-that-diverges failure the
/// federation modules have been bitten by, and the miss/hit accounting is the
/// part that must not differ between them.
#[derive(Debug)]
pub(super) struct StampCache<T> {
    slot: Mutex<Stamped<T>>,
}

// Derived by hand: `#[derive(Default)]` would demand `T: Default`, which the
// cached value never needs to be — an empty slot holds no `T` at all.
impl<T> Default for StampCache<T> {
    fn default() -> Self {
        Self {
            slot: Mutex::new(None),
        }
    }
}

impl<T> StampCache<T> {
    /// The value for `stamps`, computing and storing it on a miss.
    ///
    /// `compute` runs **outside** the lock: it makes all-member reads through the
    /// registry, and holding this mutex across them would serialise concurrent
    /// serve requests behind one another for no gain. Two callers racing a miss
    /// therefore both compute, and the last writer wins — the values are equal by
    /// construction (same stamps, same inputs), so the race costs work, never
    /// correctness.
    pub(super) fn get_or_compute(&self, stamps: Stamps, compute: impl FnOnce() -> T) -> Arc<T> {
        {
            let slot = self.lock();
            if let Some((cached, value)) = slot.as_ref() {
                if *cached == stamps {
                    return Arc::clone(value);
                }
            }
        }

        let value = Arc::new(compute());
        *self.lock() = Some((stamps, Arc::clone(&value)));
        value
    }

    /// The stamps this slot was last filled at, for the coherence guard that
    /// asserts the bridge's two slots are keyed on one snapshot.
    #[cfg(test)]
    fn cached_stamps(&self) -> Option<Stamps> {
        self.lock().as_ref().map(|(stamps, _)| stamps.clone())
    }

    /// Lock the slot, recovering a poisoned lock rather than propagating the
    /// poison — the cache is a derived read-model, so a poisoned view is still
    /// usable and one caller's panic must not brick the bridge for the rest.
    fn lock(&self) -> std::sync::MutexGuard<'_, Stamped<T>> {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ContractBridge {
    /// A bridge with an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// The cross-service edge set over `registry`'s members, recomputing only
    /// when a member's sync-stamp has advanced since the last call ([FR-WS-04]).
    ///
    /// [`edges_read`](Self::edges_read) without the member reads, for a caller
    /// that renders none.
    ///
    /// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
    pub fn edges<E>(&self, registry: &EngineRegistry<E>) -> Arc<Vec<BridgeEdge>>
    where
        E: MemberEngine + MemberContracts,
    {
        self.edges_read(registry).0
    }

    /// The cross-service edge set over `registry`'s members, and the members
    /// read to answer it ([FR-WS-04], [NFR-PE-10], [NFR-CC-04]).
    ///
    /// The member sync-stamps are checked **first** ([`current_stamps`]): on a
    /// cache hit the per-member surface reads are skipped entirely, and the
    /// check itself starts no engine it does not have to. On a miss the full
    /// contract surface is read through each member's read pool via the
    /// registry fan-out and the edge set is recomputed and re-cached — every
    /// member, because an edge binds the **sole** provider of a key, and only
    /// every member's surface can say a provider is the sole one.
    ///
    /// This is a top-level read-model entry point, so it mints the
    /// [`AnswerScope`] its own walks share — the stamp check and, on a miss, the
    /// two surface reads are **one** answer, and a member that will not open is
    /// attempted and announced once across them ([FR-WS-16]). The
    /// [`MemberReads`] are that answer's, plus the members the cached set was
    /// computed without ([`EdgeSnapshot`]).
    ///
    /// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub fn edges_read<E>(&self, registry: &EngineRegistry<E>) -> (Arc<Vec<BridgeEdge>>, MemberReads)
    where
        E: MemberEngine + MemberContracts,
    {
        let answer = registry.answer();
        let stamps = current_stamps(&answer);
        let snapshot = self.edge_snapshot(&answer, stamps);
        (Arc::clone(&snapshot.edges), reads_over(&answer, &snapshot))
    }

    /// **The derived read-models one cross-service reachability answer needs**,
    /// resolved against a **single** stamp snapshot, and the members read to
    /// answer it ([CR-125], [FR-WS-05], [NFR-CC-04]).
    ///
    /// Returns the edge set the answer is built from and the unresolved egress
    /// residue reported beside it. Both are projections of the members this call
    /// walked, so the answer and its residue cannot drift apart.
    ///
    /// # Why one entry point and not two calls ([FR-WS-16] AC5, [NFR-CC-04])
    /// Calling [`edges`](Self::edges) and a separate residue accessor in sequence
    /// mints **two** [`AnswerScope`]s and checks the member sync-stamps
    /// **twice**. Both are defects, and the second is the serious one:
    ///
    /// - a member that will not open is attempted and diagnosed once per scope,
    ///   and [FR-WS-16] AC5 says *once per command*; and
    /// - between the two stamp checks a member can re-sync under `logos serve`'s
    ///   watcher, so the edge set comes from one generation and the residue from
    ///   the next. A call site resolved in the first then reads as *unresolved*
    ///   in the second — the answer contradicting its own residue, which is
    ///   exactly the drift [CR-125] §4.4 requires be impossible. A transient
    ///   open failure between the two reads splits `covers_all_members` from the
    ///   coverage the edges were actually computed over, so a partial answer can
    ///   read as complete ([NFR-CC-04]).
    ///
    /// One `answer()` and one [`current_stamps`] fixes both: the two slots are
    /// keyed on the **same** vector, so they hit together, miss together, and a
    /// miss recomputes both from one walk of one generation.
    ///
    /// [CR-125]: ../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub fn reachability_read<E>(
        &self,
        registry: &EngineRegistry<E>,
    ) -> (Arc<Vec<BridgeEdge>>, Arc<WorkspaceEgressResidue>, MemberReads)
    where
        E: MemberEngine + MemberContracts,
    {
        let answer = registry.answer();
        let stamps = current_stamps(&answer);
        let snapshot = self.edge_snapshot(&answer, stamps.clone());
        let residue = self
            .residue_cache
            .get_or_compute(stamps, || egress_residue(&answer));
        (Arc::clone(&snapshot.edges), residue, reads_over(&answer, &snapshot))
    }

    /// The edge slot at `stamps`, computing it over `answer` on a miss.
    fn edge_snapshot<E>(&self, answer: &AnswerScope<'_, E>, stamps: Stamps) -> Arc<EdgeSnapshot>
    where
        E: MemberEngine + MemberContracts,
    {
        self.edge_cache.get_or_compute(stamps, || {
            let edges = Arc::new(compute_edges(answer));
            EdgeSnapshot {
                edges,
                unread: answer.reads().unread,
            }
        })
    }
}

/// What one bridge answer read: its own walks, and the members the edge set it
/// was served was computed without ([`EdgeSnapshot`]).
fn reads_over<E: MemberEngine>(answer: &AnswerScope<'_, E>, snapshot: &EdgeSnapshot) -> MemberReads {
    let mut reads = answer.reads();
    for (member, reason) in &snapshot.unread {
        reads.note_unread(member, reason.clone());
    }
    reads
}

/// Snapshot each member's current sync-stamp, sorted by member — the bridge
/// cache key — reading **only** the members whose stamp is not already known
/// ([NFR-PE-10]). A member whose engine fails to start contributes no stamp (it
/// is skipped, and named unread in the answer), so if it later starts the stamp
/// vector changes and the cache invalidates.
///
/// # Which members are read, and why that is every member it must read
/// The vector is exactly the one opening every member would read, built
/// without opening the ones whose stamp is already known:
///
/// - a **resident** member's stamp is read off its live engine — a stamp lives
///   in the engine and only a resident engine can advance one, so these are
///   the members whose stamp can have moved. No engine is started, and the
///   eviction queue is not touched ([`EngineRegistry::peek_resident`]);
/// - a member **opened before and since evicted** has no live stamp: starting
///   it again would read [`restart_stamp`](MemberContracts::restart_stamp), so
///   that is its entry, and it is neither started nor counted read. Opening it
///   to read a constant was the whole cost of the old check — every member, on
///   every cross-service query, whatever `repo` said. An `Engine` states it only
///   while the member's store file is there: a store deleted or obstructed since
///   is opened, as below, so a member that will not start any more drops out of
///   the vector and is named unread, as it was;
/// - a member **never attempted, or whose last open failed**, is opened, as
///   before: whether it starts is not yet known, and a member that now starts
///   adds an entry and invalidates the cache. Likewise every member whose
///   restart stamp cannot be stated (`restart_stamp` is `None`).
///
/// # Where it differs from opening every member
/// A store that is **there but no longer opens** — corrupt contents, a schema
/// newer than this binary, an `open(2)` refused — is not detected until a tier
/// of the answer opens the member: the bridge serves the edges it last read
/// from it, and the member is named unread only by a tier that opens it (the
/// per-member fan-out, `impact`'s far side). The old check opened it and would
/// have dropped it. A member's store **changed** by another process while it was
/// not resident is as invisible here as it was to the old check, which reopened
/// it at the same constant.
pub(super) fn current_stamps<E>(answer: &AnswerScope<'_, E>) -> Stamps
where
    E: MemberEngine + MemberContracts,
{
    let registry = answer.registry();
    let opened = registry.last_opened();
    let mut stamps: Stamps = Vec::new();
    for member in registry.members() {
        let name = member.name.as_str();
        let restart = || E::restart_stamp(&member.root);
        let engine = match (registry.peek_resident(name), opened.contains(name).then(restart).flatten()) {
            (Some(engine), _) => {
                answer.note_read(name);
                engine
            }
            (None, Some(stamp)) => {
                stamps.push((name.to_string(), stamp));
                continue;
            }
            (None, _) => match answer.open_for_walk(name) {
                Ok(engine) => engine,
                Err(_) => continue,
            },
        };
        stamps.push((name.to_string(), engine.contract_stamp()));
    }
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
/// # An engine-start failure is announced once per answer
/// The two arms warn on different schedules, because they report different
/// things. A **read** failure is per-`read`: the same member can read its
/// contract surface fine and fail on its invocation refs, so each read is its
/// own news. Note that this is per *read*, not per *subject* — `subject` is not
/// unique to a call site (`"contract surface"` is read both here and by
/// [`coverage`](super::coverage), as is `"invocation references"`), so
/// `workspace reachability`, which runs both paths, still repeats a read
/// failure once per path. That is an accepted duplicate: it needs a store that
/// opens and a query that then fails, a rarer condition than the one this story
/// targets, and de-duplicating it would mean latching on `(member, subject)`.
/// An **engine-start** failure is per-member and subject-independent — a
/// member whose store will not open fails identically for every subject, and
/// `workspace status` reaches **this helper** three times (`coverage` twice,
/// `topics` once — its fourth walk, the freshness read, does not come through
/// here), so the shipped code emitted one broken member's diagnostic three
/// times over. The start arm therefore asks
/// [`AnswerScope::announce_open_failure`] whether the operator has been told
/// yet *in this answer*, and stays quiet when they have ([FR-WS-16],
/// [NFR-CC-04]). Nothing is
/// lost: the member is still skipped, still recorded in the open-state ledger,
/// and still named — with its cause — by the degraded roll-up's own notice.
///
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
/// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub(super) fn read_members<E, T>(
    answer: &AnswerScope<'_, E>,
    subject: &str,
    read: impl Fn(&Arc<E>) -> Result<T>,
) -> Vec<(String, T)>
where
    E: MemberEngine + MemberContracts,
{
    let mut out = Vec::new();
    for scoped in answer.fan_out(|_, engine| read(engine)) {
        let member = scoped.member;
        match scoped.value {
            Ok(Ok(value)) => out.push((member, value)),
            Ok(Err(err)) => {
                tracing::warn!(
                    member = %member,
                    "reading a workspace member's {subject} failed; degraded without it: {err:#}"
                );
                answer.note_unread(&member, format!("reading its {subject} failed: {err:#}"));
            }
            Err(err) => {
                if answer.announce_open_failure(&member) {
                    tracing::warn!(
                        member = %member,
                        "a workspace member engine failed to start; degraded without it: \
                         {err:#}"
                    );
                }
            }
        }
    }
    out
}

/// Read every member's contract surface through its read pool, index providers
/// on portable keys, split providers from consumers by role, and hand the two
/// indexes to the namespace-generic [`match_indexed`] matcher.
fn compute_edges<E>(answer: &AnswerScope<'_, E>) -> Vec<BridgeEdge>
where
    E: MemberEngine + MemberContracts,
{
    let mut providers: ProviderIndex = ProviderIndex::new();
    let mut consumers: Vec<(PortableKey, BridgeEndpoint, BridgeIntake, Provenance)> = Vec::new();

    for (member, surface) in read_members(answer, "contract surface", |e| e.contract_surface()) {
        for node in surface {
            let Some((key, role)) = classify(node.kind, &node.name) else {
                continue;
            };
            let endpoint = BridgeEndpoint {
                member: member.clone(),
                symbol: node.symbol,
            };
            match role {
                // A DECLARED endpoint is written in the member's own source or
                // spec; no configuration is read for it on any path, so its
                // value provenance is `Literal` by construction.
                Role::Provider => {
                    index_provider(&mut providers, key, endpoint, Provenance::Literal)
                }
                // A contract-surface consumer is a *declared* endpoint (an OpenAPI
                // operation) — it describes a contract, it does not call one, so its
                // edge is contract-surface intake and never seeds a reachability
                // root ([CR-083]).
                Role::Consumer => consumers.push((
                    key,
                    endpoint,
                    BridgeIntake::ContractSurface,
                    Provenance::Literal,
                )),
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
    //
    // Since [S-410] the broker arm's operands are also **resolved against each
    // member's committed configuration** before they are keyed, so a topic
    // identity is the committed value wherever one is committed ([FR-WS-10]'s
    // re-proposed criterion). That is why its references are buffered as
    // `InvocationRef`s here rather than reduced to candidates in place: the keys
    // they name have to be known before any member store is opened, so one read
    // per member serves every one of its broker sites.
    //
    // **The HTTP arm is buffered for the same reason since S-420** ([CR-133]):
    // its targets are keyed by their committed value wherever committed
    // configuration proves one, through the shared [`identify`] the coverage
    // tier also calls, so the keys a member's references name have to be known
    // before any member store is opened. Until then the bridge keyed an HTTP
    // consumer on its RAW ledger target, a `${…}` path reduced to no key, and
    // the reference was dropped — while the coverage tier reported the very same
    // reference bound. One fact, two classifications: the drift [ADR-52]'s
    // one-classifier contract forbids.
    let mut ledger_refs: Vec<(String, InvocationRef)> = Vec::new();
    for (member, refs) in read_members(answer, "invocation references", |e| e.invocation_refs()) {
        for reference in refs {
            match reference.relation.bridge_namespace() {
                Some(BridgeNamespace::BrokerTopic) | Some(BridgeNamespace::Http) => {
                    ledger_refs.push((member.clone(), reference));
                }
                // Every remaining arm feeds the loop's consumer index directly;
                // its providers are contract-surface nodes, already indexed
                // above, and it reads its stored target verbatim — no corpus is
                // opened for it and none would tell it anything.
                _ => {
                    if reference.relation.bridge_role() != Some(BridgeRole::Consumer) {
                        continue;
                    }
                    let endpoint = BridgeEndpoint {
                        member: member.clone(),
                        symbol: reference.symbol,
                    };
                    let Some(key) = consumer_portable_key(reference.relation, &reference.target)
                    else {
                        continue; // an unkeyable / not-yet-registered arm contributes nothing
                    };
                    // A ledger reference is a captured call site ([FR-WS-08]/
                    // [FR-WS-09]) — an invocation edge that seeds a reachability
                    // root ([CR-083]).
                    consumers.push((key, endpoint, BridgeIntake::Invocation, Provenance::Literal));
                }
            }
        }
    }

    // ONE read per member that names a configuration key, over every key its own
    // references name on either arm — never one read per arm and never one per
    // row. A workspace whose broker operands and HTTP targets are all literals
    // names no key at all and opens nothing ([NFR-PE-10]).
    let corpora = member_corpora(answer, &ledger_refs);
    let empty = MemberCorpus::new();
    let mut broker_candidates = Vec::new();
    for (member, reference) in ledger_refs {
        // A member that named no configuration key is absent from `corpora` and
        // resolves against nothing — never against another member's
        // configuration, which is [ADR-64]'s within-reach rule.
        let corpus = corpora.get(&member).unwrap_or(&empty);
        let Some(identity) = identify(reference.relation, &reference.target, corpus) else {
            // Not an HTTP arm: the broker arm's own classifier owns it, and
            // builds BOTH indexes itself because its provider side (a subscribe)
            // stands behind no contract-surface node.
            broker_candidates.push(super::broker::BrokerCandidate {
                relation: reference.relation,
                key: reference.target,
                endpoint: BridgeEndpoint {
                    member,
                    symbol: reference.symbol,
                },
            });
            continue;
        };
        if reference.relation.bridge_role() != Some(BridgeRole::Consumer) {
            continue; // the HTTP arm has no ledger-side provider to index
        }
        let endpoint = BridgeEndpoint {
            member,
            symbol: reference.symbol,
        };
        // One consumer entry per committed composition that keys, each carrying
        // the evidence that proves ITS composition ([ADR-64] decision point 3).
        // A literal target yields exactly one, with `Literal` provenance — the
        // pre-S-420 behaviour, byte for byte.
        for (key, value) in identity.keyed {
            consumers.push((key, endpoint.clone(), BridgeIntake::Invocation, value));
        }
    }

    let mut edges = match_indexed(providers, consumers);
    collapse_by_coupling(&mut edges);
    // The broker arm's cross-member fan-out: one publish binds every subscribe on
    // the same topic identity, across members ([FR-WS-10], [FR-WS-11]).
    edges.extend(super::broker::broker_edges(broker_candidates, &corpora));
    // Re-sort the union: each half is sorted, their concatenation is not
    // ([NFR-RA-06]).
    edges.sort();
    edges
}

/// Collapse edges that differ **only** in the consumer end's evidence: one
/// coupling is one edge, however many committed compositions produce it (S-420,
/// [CR-133]; the rule [S-410] gave the broker arm's `edges.dedup()`).
///
/// Two overlays committing one key to two templates normally bind two *different*
/// providers — two couplings, two edges, each carrying its own profile set. They
/// bind the **same** provider when one symbol declares both templates
/// (`@RequestMapping({"/a","/b"})`), and there the two edges are one coupling: a
/// second row would be a fabricated count, because `resolved_cross_service_edges`
/// reconciles the bridge's edges against the coverage tier's bound rows and that
/// tier names one provider for this site ([NFR-RA-05], [CR-118]).
///
/// The surviving edge carries the **union** of the two evidences, which is what
/// its `to` end is actually proved by: both overlays produce it. The union is a
/// re-widening of [`composition_evidence`]'s narrowing, so it can only restore
/// entries the site's own resolution already held.
///
/// Scoped to `ConfigBound` consumer ends on purpose. Two `Literal` edges that are
/// wholly equal are a pre-existing ledger-duplication question this story does
/// not touch, and collapsing them here would change what a workspace with no
/// committed configuration reports.
///
/// **Only within one call site.** `from` is a `(member, symbol)` endpoint, not a
/// reference, so two different targets in one method that bind the same provider
/// reach this function as a collapsible pair — and merging them would hand the
/// surviving edge a `bound` list naming keys **no single target names**, which
/// [`Provenance::ConfigBound`]'s contract ("one entry per configuration key the
/// target names") forbids. The key sequence is what separates the two cases:
/// every composition of one target narrows the *same* `ResolvedTemplate::bound`,
/// so two edges from one site name the same keys in the same order, and the
/// union then only restores values of those keys. Review found this; the guard
/// is the `same_keys` test below.
///
/// `edges` must be sorted, which is [`match_indexed`]'s postcondition: the
/// derived `Ord` compares `(relation, from, to, intake)` before either
/// provenance, so every candidate for a collapse is adjacent — and it stays
/// adjacent only because an HTTP coupling group's `to_value` is uniform
/// (`Literal` for every contract-surface provider), which is what keeps
/// `to_value` sorting *after* `from_value` harmless. A future provider arm
/// carrying admitted provenance would need a group-wise scan instead of this
/// single-step one.
///
/// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
/// [CR-133]: ../../../docs/requests/CR-133-bridge-keys-http-consumer-on-committed-target.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
fn collapse_by_coupling(edges: &mut Vec<BridgeEdge>) {
    let mut out: Vec<BridgeEdge> = Vec::with_capacity(edges.len());
    for edge in edges.drain(..) {
        let merged = out.last_mut().is_some_and(|prev| {
            let same_coupling = prev.relation == edge.relation
                && prev.from == edge.from
                && prev.to == edge.to
                && prev.intake == edge.intake
                && prev.to_value == edge.to_value;
            match (same_coupling, &mut prev.from_value, &edge.from_value) {
                (
                    true,
                    Provenance::ConfigBound { bound: into },
                    Provenance::ConfigBound { bound: from },
                ) => {
                    // Two edges of ONE site name the same keys in the same
                    // order; two sites do not. See the doc above.
                    let same_keys = into.len() == from.len()
                        && into
                            .iter()
                            .zip(from.iter())
                            .all(|(held, incoming)| {
                                held.key == incoming.key && held.source == incoming.source
                            });
                    if same_keys {
                        union_bound(into, from);
                    }
                    same_keys
                }
                _ => false,
            }
        });
        if !merged {
            out.push(edge);
        }
    }
    *edges = out;
}

/// Merge one composition's evidence into another's, key by key — the union
/// [`collapse_by_coupling`] carries onto the edge it keeps.
///
/// Values are re-sorted and de-duplicated, which restores the order
/// [`crate::resolve::binding::Agreement`] produced them in (by value): both
/// operands are subsets of one resolution's evidence, so the union is that
/// resolution's own list filtered to what the merged compositions prove.
fn union_bound(into: &mut Vec<ConfigBound>, from: &[ConfigBound]) {
    for entry in from {
        match into
            .iter_mut()
            .find(|held| held.key == entry.key && held.source == entry.source)
        {
            Some(held) => {
                held.values.extend(entry.values.iter().cloned());
                held.values.sort();
                held.values.dedup();
            }
            None => into.push(entry.clone()),
        }
    }
}

/// File one provider under its bucket, keeping its method facet beside the
/// endpoint ([CR-109]).
///
/// The one place a [`PortableKey`] is split into an index entry. The bridge, the
/// broker arm and the coverage read-model all index through it, so the three can
/// never build differently-shaped provider indexes and then disagree about what
/// a bucket contains ([ADR-52]).
///
/// [CR-109]: ../../../docs/requests/CR-109-wildcard-method-route-matching.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
pub(super) fn index_provider(
    providers: &mut ProviderIndex,
    key: PortableKey,
    endpoint: BridgeEndpoint,
    value: Provenance,
) {
    let PortableKey { bucket, method } = key;
    providers
        .entry(bucket)
        .or_default()
        .push(ProviderCandidate { endpoint, method, value });
}

/// Put every bucket in a deterministic order, regardless of the member fan-out
/// order it was filled in ([NFR-RA-06]).
///
/// [`ProviderCandidate`] declares `endpoint` before `method`, so the derived
/// [`Ord`] sorts by `(member, symbol)` — the same order a bucket had when it held
/// bare endpoints. Shared with the coverage read-model so both tiers reduce
/// identically ordered buckets.
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(super) fn sort_buckets(providers: &mut ProviderIndex) {
    for candidates in providers.values_mut() {
        candidates.sort();
    }
}

/// The providers a consumer holding `key` may bind: its bucket, narrowed by the
/// shared [`route_method`](crate::resolve::route_method) rule — a wildcard
/// provider serves every verb, and an exact-method provider of the same template
/// is the sole candidate beside a wildcard one ([CR-109], [FR-CG-09]).
///
/// Returns the whole [`ProviderCandidate`], not its endpoint alone, so a caller
/// that binds one can also name **how that provider's own key was proved** —
/// the [`value`](ProviderCandidate::value) an edge carries as its
/// [`to_value`](BridgeEdge::to_value) ([S-410]). Callers that want only the
/// endpoint read `.endpoint`.
///
/// [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
///
/// An **empty** result covers both "no such bucket" and "a bucket holding
/// nothing that serves this method": the same answer, deliberately, because that
/// is exactly what a `(method, template)`-keyed index used to report for a method
/// mismatch. Ranking never picks a winner — equally-specific providers all come
/// back, for the caller's own exactly-one gate to refuse ([NFR-RA-05]).
///
/// The intra-repo binder narrows its own bucket through the same rule, which is
/// what keeps "why did this bind" and "why didn't this bind" from drifting
/// ([ADR-52]).
///
/// [CR-109]: ../../../docs/requests/CR-109-wildcard-method-route-matching.md
/// [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
pub(super) fn bucket_candidates<'a>(
    providers: &'a ProviderIndex,
    key: &PortableKey,
) -> Vec<&'a ProviderCandidate> {
    let Some(bucket) = providers.get(&key.bucket) else {
        return Vec::new();
    };
    preferred_candidates(
        bucket.iter().map(|c| (c.method.as_deref(), c)),
        key.method.as_deref(),
    )
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
    mut providers: ProviderIndex,
    consumers: Vec<(PortableKey, BridgeEndpoint, BridgeIntake, Provenance)>,
) -> Vec<BridgeEdge> {
    sort_buckets(&mut providers);

    let mut edges = Vec::new();
    // The intake rides with each consumer so the emitted edge records *how* the
    // binding was captured — an invocation call site vs a declared contract
    // surface ([CR-083]). The value provenance rides beside it, so the edge also
    // records *what proved the key* at each end ([S-410]). The match discipline
    // is unchanged; only the edge's provenance is carried through.
    for (key, consumer, intake, from_value) in consumers {
        // No bucket, or a bucket holding nothing that serves this consumer's
        // method, both mean the same thing: no provider of this endpoint anywhere
        // in the workspace, so no edge.
        let candidates = bucket_candidates(&providers, &key);
        match key.namespace().match_discipline() {
            MatchDiscipline::ExactlyOne => {
                // Exactly-one across members ([NFR-RA-05]): two or more providers
                // of the same key are ambiguous and never fabricate an edge.
                let [only] = candidates.as_slice() else {
                    continue;
                };
                // A sole provider in the consumer's *own* member is an intra-repo
                // fact the per-repo graph owns; the bridge only emits cross-member
                // links.
                if only.endpoint.member == consumer.member {
                    continue;
                }
                edges.push(BridgeEdge {
                    relation: key.relation().to_string(),
                    from: consumer,
                    to: only.endpoint.clone(),
                    intake,
                    from_value,
                    to_value: only.value.clone(),
                });
            }
            MatchDiscipline::FanOut => {
                // One publish → every cross-member subscriber. No ambiguity: a
                // topic with many subscribers fans out to all of them. A
                // same-member subscriber is the intra-repo fan-out, owned by the
                // per-repo graph.
                for provider in candidates {
                    if provider.endpoint.member == consumer.member {
                        continue;
                    }
                    edges.push(BridgeEdge {
                        relation: key.relation().to_string(),
                        from: consumer.clone(),
                        to: provider.endpoint.clone(),
                        intake,
                        from_value: from_value.clone(),
                        to_value: provider.value.clone(),
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
    use crate::graph_store::ConfigDefinition;

    use std::cell::{Cell, RefCell};
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use super::super::{Federation, Member};
    use super::super::registry::RegistryMode;

    // Per-test-thread fixtures: each `#[test]` runs on its own thread, so the
    // thread-local member fixtures and the surface-read counter are isolated.
    thread_local! {
        static FIXTURES: RefCell<HashMap<String, MemberFixture>> = RefCell::new(HashMap::new());
        static SURFACE_READS: Cell<usize> = const { Cell::new(0) };
        /// When set, the FIRST contract-surface read re-syncs `web` — a member
        /// re-indexing mid-answer, which is what `logos serve`'s watcher does.
        static RESYNC_ON_FIRST_READ: Cell<bool> = const { Cell::new(false) };
        /// How many times each member's committed configuration was read — the
        /// corpus-open budget [NFR-PE-10] bounds (S-420).
        static CONFIG_READS: RefCell<HashMap<String, usize>> = RefCell::new(HashMap::new());
        /// How many times each member's sync-stamp was read — the instrument
        /// S-484's "reads only the members it needs" is asserted on.
        static STAMP_READS: RefCell<HashMap<String, usize>> = RefCell::new(HashMap::new());
        /// What `FakeEngine::restart_stamp` reports: `None` (the default, every
        /// pre-S-484 test) keeps the stamp check opening every member; `Some`
        /// makes the fake behave like `Engine`, whose fresh stamp is a constant.
        static RESTART_STAMP: Cell<Option<u64>> = const { Cell::new(None) };
        /// Members whose engine fails to start until removed from the set — a
        /// store that recovers, unlike the always-failing `"broken"`.
        static FAILS_TO_START: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
        /// Members whose store file is gone or obstructed — what `Engine`'s
        /// `restart_stamp` reads off the store path.
        static STORE_GONE: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
    }

    #[derive(Clone, Default)]
    struct MemberFixture {
        stamp: u64,
        nodes: Vec<ContractNode>,
        consumers: Vec<InvocationRef>,
        /// This member's own committed configuration (S-382, [FR-WS-19]).
        config: MemberCorpus,
    }

    fn reset() {
        FIXTURES.with(|f| f.borrow_mut().clear());
        SURFACE_READS.with(|c| c.set(0));
        RESYNC_ON_FIRST_READ.with(|c| c.set(false));
        CONFIG_READS.with(|c| c.borrow_mut().clear());
        STAMP_READS.with(|c| c.borrow_mut().clear());
        RESTART_STAMP.with(|c| c.set(None));
        FAILS_TO_START.with(|c| c.borrow_mut().clear());
        STORE_GONE.with(|c| c.borrow_mut().clear());
    }

    /// Commit `key` in `member`'s own configuration, once per `(file, profile,
    /// value)` triple — the shape `config_definitions` reads off the store.
    ///
    /// The same helper `coverage::tests` carries, and deliberately a second copy
    /// rather than a shared one: the two harnesses are independent fixtures over
    /// two different `FakeEngine`s, and neither module exports its thread-locals.
    fn commit_config(member: &str, key: &str, defs: &[(&str, Option<&str>, &str)]) {
        FIXTURES.with(|f| {
            f.borrow_mut()
                .entry(member.to_string())
                .or_default()
                .config
                .insert(
                    crate::extract::config::corpus::canonical_key(key),
                    defs.iter()
                        .map(|(path, profile, value)| ConfigDefinition {
                            path: (*path).to_string(),
                            profile: profile.map(str::to_string),
                            value: (*value).to_string(),
                        })
                        .collect(),
                );
        });
    }

    /// How many times `member`'s committed configuration was opened.
    fn config_reads(member: &str) -> usize {
        CONFIG_READS.with(|c| c.borrow().get(member).copied().unwrap_or(0))
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
    /// **The coherence guarantee [CR-125] §4.4 rests on: one answer, one
    /// generation.** The edge set and the residue must never describe the
    /// workspace at two different member sync-stamps — if they can, the same
    /// captured call site is reported *resolved* in `cross_service` and
    /// *unresolved* in the residue printed beside it, the answer contradicting
    /// its own residue.
    ///
    /// Proven against a member that **re-syncs mid-answer**, which is exactly
    /// what `logos serve`'s watcher does while a request is in flight: the
    /// fixture bumps `web`'s stamp on the first contract-surface read. Two
    /// independent snapshots would then straddle that bump and key the two slots
    /// differently; [`ContractBridge::reachability_read`] takes the snapshot
    /// **before** either slot is filled, so both are keyed on one vector and the
    /// window does not exist.
    ///
    /// [CR-125]: ../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
    #[test]
    fn one_reachability_answer_reads_both_models_at_one_generation() {
        reset();
        set_member("web", 1, vec![route("GET /users/{id}", "local web_route")]);
        set_member("api", 1, Vec::new());
        set_consumers("api", vec![http_call("GET /users/{id}", "local api_call")]);
        let reg = registry(&["api", "web"]);
        let bridge = ContractBridge::new();

        // `web` re-indexes the instant this answer starts reading surfaces.
        RESYNC_ON_FIRST_READ.with(|c| c.set(true));
        let (edges, residue, _) = bridge.reachability_read(&reg);

        assert_eq!(
            bridge.edge_cache.cached_stamps(),
            bridge.residue_cache.cached_stamps(),
            "both derived models must be keyed on ONE snapshot; a member re-synced \
             mid-answer and they were filled at different generations"
        );

        // And the two agree about the one captured site: it is an edge, and it is
        // therefore not in the residue.
        assert_eq!(edges.len(), 1, "the call site binds web's route");
        assert_eq!(
            residue
                .members
                .iter()
                .map(|m| m.measured_sites - m.unresolved_sites)
                .sum::<u64>(),
            1,
            "the residue counts that same site as resolved: {:?}",
            residue.members
        );
        assert_eq!(
            residue.members.iter().map(|m| m.unresolved_sites).sum::<u64>(),
            0,
            "nothing is both an edge and a residue row"
        );
    }

    /// **The residue slot is a cache, not a recompute-and-store.** The four
    /// pre-existing cache tests all drive [`ContractBridge::edges`] only, so an
    /// always-recompute residue would make every serve request re-walk every
    /// member's contract surface with nothing failing ([NFR-PE-01]).
    ///
    /// Asserted on both halves — the read counter *and* `Arc::ptr_eq` — because
    /// a store that returns a fresh equal value satisfies neither.
    ///
    /// [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
    #[test]
    fn reachability_read_hits_both_caches_without_re_reading_any_member_surface() {
        reset();
        set_member("api", 3, vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", 7, vec![route("GET /users/{id}", "local route_get")]);
        set_consumers("api", vec![http_call("", "local api_call")]);
        let reg = registry(&["api", "web"]);
        let bridge = ContractBridge::new();

        let (edges_a, residue_a, _) = bridge.reachability_read(&reg);
        let reads = surface_reads();
        assert!(reads >= 2, "the first call reads each member");
        assert_eq!(
            residue_a.members.iter().map(|m| m.unresolved_sites).sum::<u64>(),
            1,
            "guard the guard: a residue that is empty proves nothing about caching"
        );

        let (edges_b, residue_b, _) = bridge.reachability_read(&reg);
        assert_eq!(
            surface_reads(),
            reads,
            "an unchanged stamp vector re-reads NO member surface"
        );
        assert!(Arc::ptr_eq(&edges_a, &edges_b), "the edge set is served, not rebuilt");
        assert!(
            Arc::ptr_eq(&residue_a, &residue_b),
            "and so is the residue — a recompute-then-store would pass the read \
             counter on a warm coverage walk but not this"
        );

        // A stamp advance invalidates BOTH, together.
        bump_stamp("web");
        let (edges_c, residue_c, _) = bridge.reachability_read(&reg);
        assert!(surface_reads() > reads, "the advance forces a recompute");
        assert!(!Arc::ptr_eq(&edges_a, &edges_c));
        assert!(!Arc::ptr_eq(&residue_a, &residue_c));
    }

    /// **A residue over a workspace that could not be read in full says so**
    /// ([FR-WS-16], [NFR-CC-04]) — end to end, through `egress_residue`, rather
    /// than by handing `residue_from` the flag directly.
    ///
    /// `covers_all_members` forwarded as a literal `true` would leave a residue
    /// over a half-open workspace rendering as a whole one, and the unit test
    /// that passes the flag in cannot see it.
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn a_residue_over_an_unreadable_member_is_marked_as_covering_fewer_than_all() {
        reset();
        set_member("api", 1, Vec::new());
        set_consumers("api", vec![http_call("", "local api_call")]);
        // `unreadable` starts but its contract-surface read fails — the degrade
        // arm, so the coverage walk contributes fewer members than the roster.
        set_member("unreadable", 1, Vec::new());
        let reg = registry(&["api", "unreadable"]);
        let bridge = ContractBridge::new();

        let (_edges, residue, _) = bridge.reachability_read(&reg);
        assert!(
            !residue.covers_all_members,
            "one member's surface could not be read: {residue:?}"
        );
        let rendered = residue
            .beside(None, crate::federation::residue::AnswerReach { resolved: 0, noun: "cross-service caller" })
            .expect("the api call is unresolved, so there is a residue");
        assert!(
            rendered
                .summary
                .ends_with("; computed over fewer than all workspace members"),
            "and the rendered line says so: {:?}",
            rendered.summary
        );

        // The whole-workspace case is the control: same fixture, readable member.
        reset();
        set_member("api", 1, Vec::new());
        set_consumers("api", vec![http_call("", "local api_call")]);
        set_member("web", 1, Vec::new());
        let reg = registry(&["api", "web"]);
        let (_edges, residue, _) = ContractBridge::new().reachability_read(&reg);
        assert!(residue.covers_all_members, "both members read: {residue:?}");
    }

    // ── the stamp check reads only the members it needs (S-484) ─────────

    /// **Unread wins** ([NFR-CC-04]): once a member is named unread in an
    /// answer, a later read of something else off it does not re-list it as
    /// read — the answer still lacks what the failed read would have
    /// contributed, as the per-member fan-out reading a member whose contract
    /// surface failed would otherwise claim. The first reason stands.
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn a_member_named_unread_stays_unread_whatever_is_read_off_it_later() {
        let mut reads = MemberReads::default();
        reads.note_read("api");
        reads.note_unread("api", "reading its contract surface failed: store read failed");
        reads.note_unread("api", "a later reason");
        reads.note_read("api");
        assert!(reads.read.is_empty(), "a read after the failure does not re-list it: {reads:?}");
        assert_eq!(
            reads.unread.get("api").map(String::as_str),
            Some("reading its contract surface failed: store read failed"),
            "the first reason stands"
        );
    }

    /// How many times each member's sync-stamp was read since the last reset.
    fn stamp_reads() -> BTreeMap<String, usize> {
        STAMP_READS.with(|c| c.borrow().iter().map(|(m, n)| (m.clone(), *n)).collect())
    }

    fn clear_stamp_reads() {
        STAMP_READS.with(|c| c.borrow_mut().clear());
    }

    /// The five-member workspace S-484's criterion is stated over: `web`
    /// calls a route `api` provides — the one edge, so the answer involves two
    /// members — and three members the answer has nothing to do with. Every
    /// fake starts at the restart stamp, as a real `Engine` does. Two members
    /// may be resident at once.
    fn five_member_workspace() -> EngineRegistry<FakeEngine> {
        RESTART_STAMP.with(|c| c.set(Some(0)));
        set_member("api", 0, vec![route("GET /users/{id}", "local api_route")]);
        set_member("web", 0, Vec::new());
        set_consumers("web", vec![http_call("GET /users/{id}", "local web_call")]);
        for unrelated in ["billing", "search", "audit"] {
            set_member(unrelated, 0, Vec::new());
        }
        let federation = fed(&["api", "web", "billing", "search", "audit"]);
        let tight = super::super::budget::WorkspaceBudget::from_limits(64, 1);
        assert_eq!(tight.max_resident_members(), 2, "the fixture's residency ceiling");
        EngineRegistry::with_budget(federation, RegistryMode::Lazy, tight)
    }

    fn names(members: &[&str]) -> BTreeSet<String> {
        members.iter().map(|m| (*m).to_string()).collect()
    }

    /// **S-484: a warm bridge reads the stamps of only the members it needs**
    /// ([NFR-PE-10]). On five members where the answer involves two, the stamp
    /// check reads exactly those two — counted at the engine, not inferred —
    /// and the answer names them. The old check read all five, opening the
    /// three evicted ones to read a stamp starting them would have reset.
    ///
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    #[test]
    fn a_warm_bridge_reads_the_stamps_of_only_the_members_it_needs() {
        reset();
        let reg = five_member_workspace();
        let bridge = ContractBridge::new();
        let (cold, cold_reads) = bridge.edges_read(&reg);
        assert_eq!(cold.len(), 1, "guard the guard: the one edge the answer involves: {cold:?}");
        assert_eq!(
            cold_reads.read,
            names(&["api", "audit", "billing", "search", "web"]),
            "cold, every member's surface is read — an edge binds the SOLE provider"
        );

        // The answer's two members are the working set (a chat that asked
        // about them through the member-addressed tools leaves them resident).
        reg.engine_for("api").unwrap();
        reg.engine_for("web").unwrap();
        assert_eq!(reg.resident_members(), ["api", "web"]);
        clear_stamp_reads();
        let starts = reg.engine_starts();

        let (warm, warm_reads) = bridge.edges_read(&reg);
        assert!(Arc::ptr_eq(&cold, &warm), "the cached edge set is served");
        assert_eq!(
            stamp_reads(),
            BTreeMap::from([("api".to_string(), 1), ("web".to_string(), 1)]),
            "exactly the two members the answer involves had their stamp read"
        );
        assert_eq!(reg.engine_starts(), starts, "and no engine was started to read one");
        assert_eq!(warm_reads.read, names(&["api", "web"]), "the answer names them");
        assert!(warm_reads.unread.is_empty(), "{warm_reads:?}");
    }

    /// **Checking the stamps does not reorder the eviction queue**
    /// ([NFR-PE-10]): a resident member's stamp is read without marking it
    /// touched, so the members the answers actually used stay the hot ones.
    /// `api` is touched after `web`; a stamp check that touched them in roster
    /// order would leave `web` the most recent, and admitting `billing` would
    /// then evict `api` instead of `web`.
    ///
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    #[test]
    fn a_stamp_check_leaves_the_eviction_order_as_the_answers_left_it() {
        reset();
        let reg = five_member_workspace();
        let bridge = ContractBridge::new();
        let _ = bridge.edges(&reg);
        reg.evict_to_capacity(0);
        reg.engine_for("web").unwrap();
        reg.engine_for("api").unwrap();
        let (_, reads) = bridge.edges_read(&reg);
        assert_eq!(reads.read, names(&["api", "web"]), "guard the guard: both resident stamps were read");

        reg.engine_for("billing").unwrap();
        assert_eq!(reg.resident_members(), ["api", "billing"], "web, the least recently used, was evicted");
    }

    /// The other half of the rule, and why it is sound: a **resident** member
    /// is read whether or not the answer involves it, because a stamp lives in
    /// the engine and only a resident engine can advance one — and its advance
    /// still invalidates the cache, as it did when every member was read.
    #[test]
    fn a_resident_member_is_read_because_only_it_can_have_moved() {
        reset();
        let reg = five_member_workspace();
        let bridge = ContractBridge::new();
        let cold = bridge.edges(&reg);
        reg.evict_to_capacity(0);
        reg.engine_for("billing").unwrap();
        clear_stamp_reads();

        let (warm, reads) = bridge.edges_read(&reg);
        assert!(Arc::ptr_eq(&cold, &warm));
        assert_eq!(stamp_reads(), BTreeMap::from([("billing".to_string(), 1)]));
        assert_eq!(reads.read, names(&["billing"]));

        // `billing` re-syncs while resident: the next read recomputes.
        bump_stamp("billing");
        let surfaces = surface_reads();
        let after = bridge.edges(&reg);
        assert!(surface_reads() > surfaces, "the resident member's advance forces a recompute");
        assert!(!Arc::ptr_eq(&cold, &after));
    }

    /// A member that advanced its stamp and was then evicted is a **miss** —
    /// its restart stamp differs from the one cached — exactly as when the old
    /// check reopened it and read the reset stamp. The narrowing never turns
    /// that miss into a hit.
    #[test]
    fn an_evicted_member_whose_stamp_moved_still_invalidates() {
        reset();
        let reg = five_member_workspace();
        let bridge = ContractBridge::new();
        let _ = bridge.edges(&reg);
        reg.evict_to_capacity(0);
        reg.engine_for("search").unwrap();
        bump_stamp("search"); // resident, re-synced
        let advanced = bridge.edges(&reg);
        assert!(
            bridge.edge_cache.cached_stamps().is_some_and(|s| s.contains(&("search".to_string(), 1))),
            "guard the guard: the advance was read and cached: {:?}",
            bridge.edge_cache.cached_stamps()
        );
        reg.evict_to_capacity(0); // `search` would restart at 0

        let surfaces = surface_reads();
        let after = bridge.edges(&reg);
        assert!(surface_reads() > surfaces, "the restart stamp differs from the cached one");
        assert!(!Arc::ptr_eq(&advanced, &after));
    }

    /// A member whose last open **failed** is opened again by every check —
    /// whether it starts now is not known — and one that recovers adds its
    /// stamp and invalidates the cache, as it did when every member was opened.
    /// It stays failed across warm hits first, so a check that stopped retrying
    /// it after one hit would serve the stale set on recovery.
    #[test]
    fn a_member_whose_last_open_failed_is_retried_and_its_recovery_invalidates() {
        reset();
        RESTART_STAMP.with(|c| c.set(Some(0)));
        set_member("api", 0, vec![route("GET /users/{id}", "local api_route")]);
        set_member("flaky", 0, Vec::new());
        set_consumers("flaky", vec![http_call("GET /users/{id}", "local flaky_call")]);
        FAILS_TO_START.with(|c| c.borrow_mut().insert("flaky".to_string()));
        let reg = registry(&["api", "flaky"]);
        let bridge = ContractBridge::new();
        let (cold, reads) = bridge.edges_read(&reg);
        assert!(cold.is_empty() && reads.unread.contains_key("flaky"), "{reads:?}");
        for _ in 0..2 {
            let (warm, reads) = bridge.edges_read(&reg);
            assert!(warm.is_empty() && reads.unread.contains_key("flaky"), "still failing: {reads:?}");
        }

        FAILS_TO_START.with(|c| c.borrow_mut().clear());
        let (recovered, reads) = bridge.edges_read(&reg);
        assert_eq!(recovered.len(), 1, "the recovered member's call binds: {recovered:?}");
        assert!(reads.read.contains("flaky") && reads.unread.is_empty(), "{reads:?}");
    }

    /// **A member opened before whose store has since gone is opened, not
    /// stated** (review finding A). Its restart cannot be stated without the
    /// store, so the check attempts it; it fails to start, drops out of the
    /// vector, the cache misses, and the answer no longer serves its edges and
    /// names it unread — what opening every member did.
    #[test]
    fn an_evicted_member_whose_store_is_gone_is_opened_and_named_not_served() {
        reset();
        RESTART_STAMP.with(|c| c.set(Some(0)));
        set_member("api", 0, vec![route("GET /users/{id}", "local api_route")]);
        set_member("flaky", 0, Vec::new());
        set_consumers("flaky", vec![http_call("GET /users/{id}", "local flaky_call")]);
        let reg = registry(&["api", "flaky"]);
        let bridge = ContractBridge::new();
        let (cold, _) = bridge.edges_read(&reg);
        assert_eq!(cold.len(), 1, "guard the guard: flaky's call binds api's route");

        reg.evict_to_capacity(0);
        STORE_GONE.with(|c| c.borrow_mut().insert("flaky".to_string()));
        FAILS_TO_START.with(|c| c.borrow_mut().insert("flaky".to_string()));
        let (warm, reads) = bridge.edges_read(&reg);
        assert!(warm.is_empty(), "the gone member's edge is no longer served: {warm:?}");
        assert!(
            reads.unread.get("flaky").is_some_and(|r| r.contains("store is corrupt")),
            "and the member is named with its reason: {reads:?}"
        );
    }

    /// Without a constant restart stamp the check opens every non-resident
    /// member, as before S-484: the narrowing is earned by the engine type, and
    /// one that cannot state its fresh stamp keeps the old read.
    #[test]
    fn an_engine_without_a_restart_stamp_keeps_the_all_member_read() {
        reset();
        let reg = five_member_workspace();
        RESTART_STAMP.with(|c| c.set(None));
        let bridge = ContractBridge::new();
        let _ = bridge.edges(&reg);
        reg.evict_to_capacity(0);
        clear_stamp_reads();
        let (_, reads) = bridge.edges_read(&reg);
        assert_eq!(stamp_reads().len(), 5, "every member opened for its stamp: {:?}", stamp_reads());
        assert_eq!(reads.read.len(), 5);
    }

    /// **A member that could not be read is named with its reason, never
    /// omitted** ([NFR-CC-04]) — on the read that computed the edge set and on
    /// every warm read served from it, since the cached set still lacks what
    /// that member would have contributed. One whose engine will not start is
    /// re-attempted by each check (whether it starts now is not known).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn a_member_that_could_not_be_read_is_named_with_its_reason() {
        reset();
        RESTART_STAMP.with(|c| c.set(Some(0)));
        set_member("api", 0, vec![route("GET /users/{id}", "local api_route")]);
        set_member("web", 0, Vec::new());
        set_member("unreadable", 0, Vec::new());
        let reg = registry(&["api", "web", "unreadable", "broken"]);
        let bridge = ContractBridge::new();

        for (pass, read) in [("cold", bridge.edges_read(&reg).1), ("warm", bridge.edges_read(&reg).1)] {
            assert!(
                read.unread.get("broken").is_some_and(|r| r.contains("store is corrupt")),
                "{pass}: the start failure is named with its diagnostic: {read:?}"
            );
            assert!(
                read.unread.get("unreadable").is_some_and(|r| {
                    r.starts_with("reading its contract surface failed") && r.contains("store read failed")
                }),
                "{pass}: the read failure is named with what failed: {read:?}"
            );
            assert!(!read.read.contains("broken") && !read.read.contains("unreadable"), "{pass}: {read:?}");
        }
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
        fn start(
            root: &Path,
            _read_connections: usize,
            _worker_pool: crate::SharedWorkerPool,
        ) -> Result<Arc<Self>> {
            let member = member_of(root);
            if member == "broken" || FAILS_TO_START.with(|c| c.borrow().contains(&member)) {
                anyhow::bail!("store is corrupt");
            }
            // With a restart stamp the fake keeps its stamp in the engine, as
            // `Engine` does: a start resets it.
            if let Some(stamp) = RESTART_STAMP.with(Cell::get) {
                FIXTURES.with(|f| f.borrow_mut().entry(member.clone()).or_default().stamp = stamp);
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
            if RESYNC_ON_FIRST_READ.with(|c| c.replace(false)) {
                bump_stamp("web");
            }
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
            STAMP_READS.with(|c| *c.borrow_mut().entry(self.member.clone()).or_default() += 1);
            FIXTURES.with(|f| f.borrow().get(&self.member).map(|m| m.stamp).unwrap_or(0))
        }
        fn restart_stamp(root: &Path) -> Option<u64> {
            // A member whose store is gone cannot have its restart stated.
            let gone = STORE_GONE.with(|c| c.borrow().contains(&member_of(root)));
            RESTART_STAMP.with(Cell::get).filter(|_| !gone)
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
        fn config_definitions(&self, keys: &[String]) -> Result<MemberCorpus> {
            CONFIG_READS.with(|c| *c.borrow_mut().entry(self.member.clone()).or_default() += 1);
            if self.member == "unreadable" {
                anyhow::bail!("store read failed");
            }
            Ok(FIXTURES.with(|f| {
                let all = f.borrow();
                let Some(member) = all.get(&self.member) else {
                    return MemberCorpus::new();
                };
                // Only the keys asked for, so a fixture cannot accidentally prove
                // a key the reference never named.
                keys.iter()
                    .filter_map(|k| member.config.get(k).map(|d| (k.clone(), d.clone())))
                    .collect()
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
            warm_concurrency: None,
            member_kinds: Default::default(),
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
    /// A facet-less portable key in `namespace` — the shape the gRPC and broker
    /// namespaces carry (their keys have no method dimension).
    fn pkey(namespace: BridgeNamespace, key: &str) -> PortableKey {
        PortableKey {
            bucket: BucketKey {
                namespace,
                key: key.to_string(),
            },
            method: None,
        }
    }
    fn indexed(
        providers: &[(PortableKey, BridgeEndpoint)],
        consumers: Vec<(PortableKey, BridgeEndpoint)>,
    ) -> Vec<BridgeEdge> {
        let mut index: ProviderIndex = ProviderIndex::new();
        for (key, endpoint) in providers {
            index_provider(&mut index, key.clone(), endpoint.clone(), Provenance::Literal);
        }
        // These match-core tests model invocation-arm consumers (gRPC/broker call
        // sites); neither the intake nor the value provenance changes the match
        // discipline they exercise.
        let tagged = consumers
            .into_iter()
            .map(|(key, endpoint)| {
                (key, endpoint, BridgeIntake::Invocation, Provenance::Literal)
            })
            .collect();
        match_indexed(index, tagged)
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
        let edges = match_indexed(
            HashMap::new(),
            vec![(
                key,
                ep("api", "local stub"),
                BridgeIntake::Invocation,
                Provenance::Literal,
            )],
        );
        assert!(edges.is_empty(), "no provider anywhere → no edge: {edges:?}");
    }

    /// Acceptance (3): an intra-repo invocation binds **locally through the same
    /// relation**, not as a cross-service bridge edge. A consumer whose sole
    /// provider is in its own member yields no bridge edge under either
    /// discipline — the per-repo graph already owns that binding (the same
    /// relation resolves it through the intra-repo artifact binder, unchanged).
    #[test]
    fn an_intra_repo_invocation_is_owned_by_the_local_graph_not_the_bridge() {
        // Exactly-one: a sole same-member provider is intra-repo. Built through
        // `PortableKey::http` rather than the facet-less `pkey`, so this exercises
        // the bucket+facet shape production actually keys HTTP with.
        let http = PortableKey::http("GET".to_string(), "/users/{}".to_string());
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

    /// **S-374 never-fabricate guard: a recorded client-call refusal is not a key.**
    ///
    /// The HTTP arm now writes one **keyless** ledger row per declining declaration
    /// ([CR-120]), and `compute_edges` reads the raw ledger — refusal rows
    /// included — so this is the gate standing between "the refusal was recorded"
    /// and "the refusal bound something". The broker arm needed the same guard in
    /// its own `classify` ([CR-107]); the HTTP arm's lives here because its edges
    /// go through [`consumer_portable_key`].
    ///
    /// Both ends are asserted, because either alone would leave a way in: the
    /// consumer's empty target must not key, **and** an empty-named `Route` must
    /// not classify as a provider — otherwise a member holding one would index a
    /// provider at the same empty key every refusal carries, and every declined
    /// call in the workspace would bind it at once.
    ///
    /// Trimmed, for the reason the broker guard is: an all-whitespace target is no
    /// more of an identity than an absent one.
    ///
    /// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
    /// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    #[test]
    fn a_keyless_client_call_row_is_not_a_key_at_either_end() {
        for keyless in ["", "   "] {
            assert!(
                consumer_portable_key(ArtifactRelation::HttpClientCall, keyless).is_none(),
                "a recorded refusal must never key a consumer ({keyless:?})"
            );
            assert!(
                classify(NodeKind::Route, keyless).is_none(),
                "and no provider may be indexed at the empty key ({keyless:?})"
            );
        }
    }

    // ── CR-109 / S-349: wildcard-method matching with exact-method precedence ──
    //
    // The cross-member half of the shared fixture matrix. The intra-repo binder
    // (`resolve::tests`) and the coverage read-model (`coverage::tests`) drive the
    // *same* rows, so an input that binds at one site and not another fails at one
    // of the three ([FR-CG-09] AC3, [ADR-52]).

    /// Lay a matrix case out across members: the consumer operation in `spec`,
    /// each provider alone in its own `p{i}` — so every binding the bridge could
    /// emit is genuinely cross-member. Returns the member names in registry order.
    fn spread_matrix_case(case: &crate::resolve::route_method::matrix::Case) -> Vec<String> {
        reset();
        set_member("spec", 0, vec![op(case.consumer, "local operation")]);
        let mut members = vec!["spec".to_string()];
        for (i, name) in case.providers.iter().enumerate() {
            let member = format!("p{i}");
            set_member(&member, 0, vec![route(name, &format!("local route_{i}"))]);
            members.push(member);
        }
        members
    }

    /// Every matrix case, driven through the cross-member bridge.
    #[test]
    fn the_wildcard_method_matrix_holds_at_the_cross_member_bridge() {
        for case in crate::resolve::route_method::matrix::MATRIX {
            let members = spread_matrix_case(case);
            let names: Vec<&str> = members.iter().map(String::as_str).collect();
            let edges = ContractBridge::new().edges(&registry(&names));

            match case.expect.bound() {
                Some(i) => {
                    assert_eq!(
                        edges.len(),
                        1,
                        "{}: `{}` must bind exactly one provider, got {edges:?}",
                        case.name,
                        case.consumer
                    );
                    assert_eq!(
                        edges[0].to.member,
                        format!("p{i}"),
                        "{}: `{}` must bind provider {i} (`{}`)",
                        case.name,
                        case.consumer,
                        case.providers[i]
                    );
                }
                None => assert!(
                    edges.is_empty(),
                    "{}: `{}` must emit no edge against {:?}, got {edges:?}",
                    case.name,
                    case.consumer,
                    case.providers
                ),
            }
        }
    }

    /// A client-call consumer reaches a wildcard provider through the very same
    /// bucket an OpenAPI operation does — the HTTP arm inherits the rule without
    /// registering anything of its own ([FR-WS-08]).
    #[test]
    fn a_client_call_binds_a_wildcard_route_in_another_member() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers("web", vec![http_call("DELETE /v1/users/{id}", "local delete_call")]);
        set_member("api", 0, vec![route("ANY /v1/users/{userId}", "local route_any")]);

        let edges = ContractBridge::new().edges(&registry(&["web", "api"]));

        assert_eq!(edges.len(), 1, "the client call binds the wildcard route: {edges:?}");
        assert_eq!(edges[0].to.member, "api");
        assert_eq!(edges[0].to.symbol.as_str(), "local route_any");
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

    // ── S-420 / CR-133: the HTTP arm keys on its COMMITTED target ─────────────
    //
    // The bind side of the same rule the coverage tier applies to these fixtures
    // in `coverage::tests`; the cross-tier walk that asserts the two agree lives
    // there, where both tiers are reachable from one fixture.

    /// The evidence a `config-bound` consumer end carries: `(key, profiles,
    /// values)` per configuration key, in the provenance's own order. The
    /// defining sources are asserted separately where they matter — they are per
    /// *value*, not per key, so folding them in here would flatten the one
    /// distinction [FR-WS-19] AC6 asks the provenance to keep.
    ///
    /// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
    fn evidence(value: &Provenance) -> Vec<(String, Vec<String>, Vec<String>)> {
        let Provenance::ConfigBound { bound } = value else {
            panic!("not a config-bound provenance: {value:?}");
        };
        bound
            .iter()
            .map(|b| {
                (
                    b.key.clone(),
                    b.profiles().into_iter().map(str::to_string).collect(),
                    b.values.iter().map(|v| v.value.clone()).collect(),
                )
            })
            .collect()
    }

    /// [CR-133] AC1: a client call whose path is composed from a committed
    /// configuration value binds the matching route in another member, and the
    /// edge names the key, its defining source and its profile set at the
    /// consumer end while the observed route stays `Literal` at the provider end.
    ///
    /// Before S-420 this fixture drew **zero** edges: `compute_edges` keyed the
    /// consumer on the raw `"GET ${orders.base}/{id}"`, which normalizes to no
    /// portable key — while the coverage tier beside it reported the very same
    /// reference bound.
    ///
    /// [CR-133]: ../../../docs/requests/CR-133-bridge-keys-http-consumer-on-committed-target.md
    #[test]
    fn a_config_bound_client_call_binds_its_committed_target_in_another_member() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers("web", vec![http_call("GET ${orders.base}/{id}", "local fetch_order")]);
        commit_config("web", "orders.base", &[("application.yml", None, "/orders")]);
        set_member("orders", 0, vec![route("GET /orders/{oid}", "local show")]);

        let edges = ContractBridge::new().edges(&registry(&["web", "orders"]));
        assert_eq!(edges.len(), 1, "exactly one edge: {edges:?}");
        let edge = &edges[0];
        assert_eq!(edge.relation, "route");
        assert_eq!(edge.intake, BridgeIntake::Invocation);
        assert_eq!(edge.from.member, "web");
        assert_eq!(edge.to.member, "orders");
        assert_eq!(edge.to.symbol.as_str(), "local show");
        assert_eq!(
            evidence(&edge.from_value),
            vec![(
                "orders.base".to_string(),
                Vec::<String>::new(),
                vec!["/orders".to_string()]
            )],
            "the consumer end names the key and the value its unprofiled source commits"
        );
        let Provenance::ConfigBound { bound } = &edge.from_value else {
            unreachable!("`evidence` above already proved the variant");
        };
        assert_eq!(
            bound[0].values[0].sources,
            ["application.yml"],
            "…and the defining source behind that value ([FR-WS-19] AC6): {bound:?}"
        );
        assert!(
            bound[0].values[0].unprofiled,
            "the unprofiled base proves it, stated rather than inferred from an \
             empty profile list: {bound:?}"
        );
        assert_eq!(
            edge.to_value,
            Provenance::Literal,
            "the route was observed in its own member's source, not admitted"
        );
    }

    /// [CR-133] AC2: with the key undefined in every committed source the bridge
    /// draws nothing — a refusal is never a bind ([NFR-RA-05]).
    #[test]
    fn a_config_bound_client_call_with_an_undefined_key_binds_nothing() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers("web", vec![http_call("GET ${orders.base}/{id}", "local fetch_order")]);
        set_member("orders", 0, vec![route("GET /orders/{oid}", "local show")]);

        let edges = ContractBridge::new().edges(&registry(&["web", "orders"]));
        assert!(
            edges.is_empty(),
            "a key no committed source defines binds nothing: {edges:?}"
        );
    }

    /// [CR-133] AC3: two profiles composing the target two ways, each matching a
    /// sole provider, draw **two** edges — and each carries the profile set that
    /// produced it ([ADR-64] decision point 3), not the site's whole evidence.
    #[test]
    fn two_profiles_draw_one_edge_each_carrying_its_own_profile_set() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers("web", vec![http_call("GET ${orders.base}/{id}", "local fetch_order")]);
        commit_config(
            "web",
            "orders.base",
            &[
                ("application-a.yml", Some("a"), "/orders-a"),
                ("application-b.yml", Some("b"), "/orders-b"),
            ],
        );
        set_member(
            "orders",
            0,
            vec![
                route("GET /orders-a/{oid}", "local show_a"),
                route("GET /orders-b/{oid}", "local show_b"),
            ],
        );

        let edges = ContractBridge::new().edges(&registry(&["web", "orders"]));
        assert_eq!(edges.len(), 2, "one edge per committed composition: {edges:?}");
        let mut seen: Vec<(String, Vec<String>, Vec<String>)> = edges
            .iter()
            .map(|e| {
                let mut ev = evidence(&e.from_value);
                assert_eq!(ev.len(), 1, "one key, one evidence entry: {ev:?}");
                let (key, profiles, values) = ev.remove(0);
                (format!("{}|{key}", e.to.symbol.as_str()), profiles, values)
            })
            .collect();
        seen.sort();
        assert_eq!(
            seen,
            vec![
                (
                    "local show_a|orders.base".to_string(),
                    vec!["a".to_string()],
                    vec!["/orders-a".to_string()]
                ),
                (
                    "local show_b|orders.base".to_string(),
                    vec!["b".to_string()],
                    vec!["/orders-b".to_string()]
                ),
            ],
            "each edge carries only the overlay that produced it"
        );
    }

    /// **A profiled overlay over an unprofiled base attributes each edge to the
    /// value that produced IT** — the estate's dominant idiom, and the shape an
    /// inference from the profile set alone gets wrong.
    ///
    /// `orders.base` is committed once in `application.yml` and again under
    /// `prod`. The composer substitutes the profiled value **instead of** the
    /// base wherever the profile defines the key
    /// ([`crate::resolve::binding`]'s `values_under`), so the `prod` edge is
    /// produced by `/orders-v2` alone. Review found the first version of
    /// [`composition_evidence`] keeping the unprofiled base on every
    /// composition, which put `/orders` — the value behind the *other* edge —
    /// on this one ([ADR-64] decision point 3, [NFR-CC-04]).
    #[test]
    fn a_profiled_overlay_and_its_base_each_name_only_their_own_value() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers("web", vec![http_call("GET ${orders.base}/{id}", "local fetch_order")]);
        commit_config(
            "web",
            "orders.base",
            &[
                ("application.yml", None, "/orders"),
                ("application-prod.yml", Some("prod"), "/orders-v2"),
            ],
        );
        set_member(
            "orders",
            0,
            vec![
                route("GET /orders/{oid}", "local show"),
                route("GET /orders-v2/{oid}", "local show_v2"),
            ],
        );

        let edges = ContractBridge::new().edges(&registry(&["web", "orders"]));
        assert_eq!(edges.len(), 2, "one edge per committed composition: {edges:?}");
        let mut seen: Vec<(String, Vec<String>, Vec<String>)> = edges
            .iter()
            .map(|e| {
                let mut ev = evidence(&e.from_value);
                assert_eq!(ev.len(), 1, "one key, one evidence entry: {ev:?}");
                let (_key, profiles, values) = ev.remove(0);
                (e.to.symbol.as_str().to_string(), profiles, values)
            })
            .collect();
        seen.sort();
        assert_eq!(
            seen,
            vec![
                (
                    "local show".to_string(),
                    Vec::<String>::new(),
                    vec!["/orders".to_string()]
                ),
                (
                    "local show_v2".to_string(),
                    vec!["prod".to_string()],
                    vec!["/orders-v2".to_string()]
                ),
            ],
            "the profiled edge names the profiled value ALONE — the base produced the other edge"
        );
    }

    /// The same exactness where **no profile discriminates at all**: one key,
    /// two unprofiled sources committing two values ([`ConfigBound`] divergence
    /// with an empty profile set on both). A profile-set inference is a no-op
    /// here — both values pass it — so each edge would have named the other's
    /// value. [`ProfiledTemplate::used`] is what separates them.
    #[test]
    fn two_unprofiled_sources_still_name_one_value_per_edge() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers("web", vec![http_call("GET ${orders.base}/{id}", "local fetch_order")]);
        commit_config(
            "web",
            "orders.base",
            &[
                ("application.yml", None, "/orders"),
                ("common.yml", None, "/orders-v2"),
            ],
        );
        set_member(
            "orders",
            0,
            vec![
                route("GET /orders/{oid}", "local show"),
                route("GET /orders-v2/{oid}", "local show_v2"),
            ],
        );

        let edges = ContractBridge::new().edges(&registry(&["web", "orders"]));
        assert_eq!(edges.len(), 2, "one edge per committed composition: {edges:?}");
        let mut seen: Vec<(String, Vec<String>)> = edges
            .iter()
            .map(|e| {
                let mut ev = evidence(&e.from_value);
                let (_key, _profiles, values) = ev.remove(0);
                (e.to.symbol.as_str().to_string(), values)
            })
            .collect();
        seen.sort();
        assert_eq!(
            seen,
            vec![
                ("local show".to_string(), vec!["/orders".to_string()]),
                ("local show_v2".to_string(), vec!["/orders-v2".to_string()]),
            ],
            "neither edge names the value that produced the other"
        );
    }

    /// [CR-133] AC4: a **single** composition matching two providers stays
    /// ambiguous and binds nothing — unchanged from before S-420. The union the
    /// coverage tier performs is across compositions, never within one.
    #[test]
    fn one_composition_matching_two_providers_is_ambiguous_no_edge() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers("web", vec![http_call("GET ${orders.base}/{id}", "local fetch_order")]);
        commit_config("web", "orders.base", &[("application.yml", None, "/orders")]);
        set_member("orders", 0, vec![route("GET /orders/{oid}", "local show")]);
        set_member("legacy", 0, vec![route("GET /orders/{id}", "local legacy_show")]);

        let edges = ContractBridge::new().edges(&registry(&["web", "orders", "legacy"]));
        assert!(
            edges.is_empty(),
            "two providers of one composed key are ambiguous — no edge: {edges:?}"
        );
    }

    /// **One coupling is one edge, however many compositions produce it.** Two
    /// overlays compose the target two ways and one symbol declares both
    /// templates (`@RequestMapping({"/a","/b"})`), so both compositions bind the
    /// *same* provider endpoint. That is one coupling, and the surviving edge
    /// carries the union of what proved it — a second row would be a fabricated
    /// count against the coverage tier's single bound row ([NFR-RA-05], [CR-118]).
    ///
    /// [CR-118]: ../../../docs/requests/CR-118-coverage-names-the-provider-and-records-the-ambiguity-ceiling.md
    #[test]
    fn two_compositions_binding_one_provider_collapse_to_one_edge() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers("web", vec![http_call("GET ${orders.base}/{id}", "local fetch_order")]);
        commit_config(
            "web",
            "orders.base",
            &[
                ("application-a.yml", Some("a"), "/orders-a"),
                ("application-b.yml", Some("b"), "/orders-b"),
            ],
        );
        set_member(
            "orders",
            0,
            vec![
                route("GET /orders-a/{oid}", "local show"),
                route("GET /orders-b/{oid}", "local show"),
            ],
        );

        let edges = ContractBridge::new().edges(&registry(&["web", "orders"]));
        assert_eq!(
            edges.len(),
            1,
            "one coupling, however many compositions reach it: {edges:?}"
        );
        assert_eq!(
            evidence(&edges[0].from_value),
            vec![(
                "orders.base".to_string(),
                vec!["a".to_string(), "b".to_string()],
                vec!["/orders-a".to_string(), "/orders-b".to_string()]
            )],
            "and it carries BOTH overlays, because both produce it"
        );
    }

    /// **A two-key target whose collapse re-states one key's value** — the shape
    /// that reaches [`union_bound`]'s de-duplication.
    ///
    /// `svc.base` is committed once; `svc.path` is committed unprofiled and
    /// again under `p`. Two compositions result, they differ only in `svc.path`,
    /// and one symbol declares both routes — so they collapse, and the merge
    /// meets `svc.base`'s single value **twice**. Review found the collapse's
    /// `sort`/`dedup` undefended: without it the surviving edge names one
    /// committed value twice, which S-419 renders as two sources for one key.
    #[test]
    fn a_collapse_that_meets_one_value_twice_names_it_once() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers(
            "web",
            vec![http_call("GET ${svc.base}${svc.path}/{id}", "local fetch_order")],
        );
        commit_config("web", "svc.base", &[("application.yml", None, "/x")]);
        commit_config(
            "web",
            "svc.path",
            &[
                ("application.yml", None, "/y"),
                ("application-p.yml", Some("p"), "/z"),
            ],
        );
        set_member(
            "orders",
            0,
            vec![
                route("GET /x/y/{oid}", "local show"),
                route("GET /x/z/{oid}", "local show"),
            ],
        );

        let edges = ContractBridge::new().edges(&registry(&["web", "orders"]));
        assert_eq!(edges.len(), 1, "one coupling, two compositions: {edges:?}");
        assert_eq!(
            evidence(&edges[0].from_value),
            vec![
                ("svc.base".to_string(), Vec::<String>::new(), vec!["/x".to_string()]),
                (
                    "svc.path".to_string(),
                    vec!["p".to_string()],
                    vec!["/y".to_string(), "/z".to_string()]
                ),
            ],
            "the key both compositions share is named ONCE; the key they differ on names both"
        );
    }

    /// **The collapse never merges two call sites.** `from` is a `(member,
    /// symbol)` endpoint, so two different targets in one method that bind the
    /// same provider arrive as a collapsible pair. Review found the first
    /// version merging them, which handed the surviving edge a `bound` list
    /// naming `a.base` **and** `b.base` — keys no single target names, which
    /// [`Provenance::ConfigBound`]'s contract forbids and which S-419 would
    /// render on the service map.
    #[test]
    fn two_call_sites_binding_one_provider_are_not_merged() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers(
            "web",
            vec![
                http_call("GET ${a.base}/{id}", "local fetch_order"),
                http_call("GET ${b.base}/{id}", "local fetch_order"),
            ],
        );
        commit_config("web", "a.base", &[("application.yml", None, "/orders")]);
        commit_config("web", "b.base", &[("application.yml", None, "/orders")]);
        set_member("orders", 0, vec![route("GET /orders/{oid}", "local show")]);

        let edges = ContractBridge::new().edges(&registry(&["web", "orders"]));
        assert_eq!(
            edges.len(),
            2,
            "two call sites are two edges, however they collide at the provider: {edges:?}"
        );
        let mut named: Vec<Vec<String>> = edges
            .iter()
            .map(|e| evidence(&e.from_value).into_iter().map(|(key, _, _)| key).collect())
            .collect();
        named.sort();
        assert_eq!(
            named,
            vec![vec!["a.base".to_string()], vec!["b.base".to_string()]],
            "and each edge names ONLY the keys its own target names"
        );
    }

    /// [CR-133] AC6, first half: a workspace whose HTTP targets carry no `${…}`
    /// opens **no** member configuration at all. The gate is a string predicate
    /// over references already in memory, so the corpus read the HTTP arm gained
    /// costs a workspace without placeholders nothing ([NFR-PE-10]).
    #[test]
    fn a_workspace_with_no_placeholder_target_opens_no_member_configuration() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers("web", vec![http_call("GET /orders/{id}", "local fetch_order")]);
        set_member("orders", 0, vec![route("GET /orders/{oid}", "local show")]);

        let edges = ContractBridge::new().edges(&registry(&["web", "orders"]));
        assert_eq!(edges.len(), 1, "the literal call binds as it always did: {edges:?}");
        assert_eq!(edges[0].from_value, Provenance::Literal);
        assert_eq!(config_reads("web"), 0, "no key named, no store opened");
        assert_eq!(config_reads("orders"), 0);
    }

    /// [CR-133] AC6, second half: a key-naming member's configuration is opened
    /// **once** per bridge computation, over every key its own references name —
    /// never once per reference and never once per arm ([NFR-PE-10]).
    #[test]
    fn a_key_naming_member_is_opened_once_per_bridge_computation() {
        reset();
        set_member("web", 0, vec![]);
        set_consumers(
            "web",
            vec![
                http_call("GET ${orders.base}/{id}", "local fetch_order"),
                http_call("GET ${orders.base}/{id}/lines", "local fetch_lines"),
                broker_publish("${orders.topic}", "local emit"),
            ],
        );
        commit_config("web", "orders.base", &[("application.yml", None, "/orders")]);
        commit_config("web", "orders.topic", &[("application.yml", None, "orders-v1")]);
        set_member("orders", 0, vec![route("GET /orders/{oid}", "local show")]);

        let _ = ContractBridge::new().edges(&registry(&["web", "orders"]));
        assert_eq!(
            config_reads("web"),
            1,
            "three key-naming references across two arms, one corpus read"
        );
        assert_eq!(config_reads("orders"), 0, "a member naming no key is never opened");
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
        assert_eq!(key.namespace(), BridgeNamespace::Grpc);
        assert_eq!(key.bucket.key, "example.v1.UserService/GetUser");
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

    // ── S-293 / CR-083: the bridge-edge intake discriminator ──────────────────

    /// CR-083 risk mitigation (§7): every arm carries the **expected intake
    /// discriminator** through the live bridge. An OpenAPI operation → framework
    /// route is a *contract-surface* edge (a spec describing an endpoint); an HTTP
    /// client call, a gRPC stub call, and a broker publish/subscribe are all
    /// *invocation* edges (captured call sites). Table-driven so a mis-tagged arm
    /// fails here rather than silently dropping — or fabricating — a reachability
    /// root downstream. The relation alone cannot witness this: an OpenAPI match
    /// and an HTTP client call both file under `route`, so the discriminator is the
    /// only thing that tells them apart.
    #[test]
    fn every_arm_carries_the_expected_intake_discriminator() {
        fn openapi_operation_to_route() -> Vec<BridgeEdge> {
            reset();
            set_member("api", 0, vec![op("GET /users/{user_id}", "local op_get")]);
            set_member("web", 0, vec![route("GET /users/{id}", "local route_get")]);
            (*ContractBridge::new().edges(&registry(&["api", "web"]))).clone()
        }
        fn http_client_call() -> Vec<BridgeEdge> {
            reset();
            set_member("web", 0, vec![]);
            set_consumers("web", vec![http_call("GET /users/{id}", "local get_user_call")]);
            set_member("api", 0, vec![route("GET /users/{userId}", "local route_get")]);
            (*ContractBridge::new().edges(&registry(&["web", "api"]))).clone()
        }
        fn grpc_stub_call() -> Vec<BridgeEdge> {
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
            (*ContractBridge::new().edges(&registry(&["api", "svc"]))).clone()
        }
        fn broker_publish_subscribe() -> Vec<BridgeEdge> {
            reset();
            set_member("api", 0, vec![]);
            set_consumers("api", vec![broker_publish("orders", "local emit_order")]);
            set_member("billing", 0, vec![]);
            set_consumers("billing", vec![broker_subscribe("orders", "local on_order")]);
            (*ContractBridge::new().edges(&registry(&["api", "billing"]))).clone()
        }

        // (label, one-binding workspace builder, expected intake).
        type IntakeCase = (&'static str, fn() -> Vec<BridgeEdge>, BridgeIntake);
        let cases: [IntakeCase; 4] = [
            (
                "OpenAPI operation → route",
                openapi_operation_to_route,
                BridgeIntake::ContractSurface,
            ),
            ("HTTP client call", http_client_call, BridgeIntake::Invocation),
            ("gRPC stub call", grpc_stub_call, BridgeIntake::Invocation),
            (
                "broker publish/subscribe",
                broker_publish_subscribe,
                BridgeIntake::Invocation,
            ),
        ];

        for (label, build, expected) in cases {
            let edges = build();
            assert_eq!(edges.len(), 1, "{label}: exactly one binding is produced: {edges:?}");
            assert_eq!(
                edges[0].intake, expected,
                "{label}: the edge must carry the {expected:?} intake discriminator"
            );
        }
    }

    /// CR-083 AC: the intake discriminator is an **additive** wire field. The prior
    /// `xservice route-providers` fields (`relation`, `from`, `to`) serialize
    /// exactly as before, and `intake` rides alongside with a stable kebab-case
    /// spelling — so an existing consumer reading the prior three keys is
    /// unaffected.
    #[test]
    fn bridge_edge_serialization_is_backward_compatible() {
        let edge = BridgeEdge {
            relation: "route".to_string(),
            from: ep("api", "local op_get"),
            to: ep("web", "local route_get"),
            intake: BridgeIntake::ContractSurface,
            from_value: Provenance::Literal,
            to_value: Provenance::Literal,
        };
        let value = serde_json::to_value(&edge).unwrap();

        // Prior fields — unchanged shape and content.
        assert_eq!(value["relation"], "route");
        assert_eq!(value["from"]["member"], "api");
        assert_eq!(value["from"]["symbol"], "local op_get");
        assert_eq!(value["to"]["member"], "web");
        assert_eq!(value["to"]["symbol"], "local route_get");

        // The additive fields, with their stable wire spellings.
        assert_eq!(value["intake"], "contract-surface");
        assert_eq!(
            serde_json::to_value(BridgeIntake::Invocation).unwrap(),
            "invocation",
            "the invocation spelling is part of the wire contract"
        );
        // [S-410]'s pair. Internally tagged, so a consumer switches on one key
        // rather than inferring from a nullable sibling — the shape
        // `Provenance` already publishes on a coverage row.
        assert_eq!(value["from_value"]["provenance"], "literal");
        assert_eq!(value["to_value"]["provenance"], "literal");

        // Exactly the prior three keys plus the three additive ones — nothing
        // else leaked onto the wire.
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("a bridge edge serializes to a JSON object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["from", "from_value", "intake", "relation", "to", "to_value"]
        );
    }
}
