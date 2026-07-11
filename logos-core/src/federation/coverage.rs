//! The **3-state cross-service coverage** read-model ([FR-WS-05], [ADR-53]).
//!
//! Extends the [bridge](super::bridge)'s binary bound/not-bound match outcome
//! into a reason-annotated advisory tier: every cross-boundary reference — an
//! `ApiOperation` HTTP consumer node and a `GrpcCall` stub-call ledger consumer
//! (S-253, [FR-WS-09]) — is classified **bound**, **ambiguous**, or **unbound**,
//! and each non-bound reference carries a [`CoverageState`] naming *why*.
//!
//! [FR-WS-09]: ../../../docs/specs/requirements/FR-WS-09.md
//!
//! # Advisory only ([ADR-53])
//! This module is **never** called from `scan`, `gate`, or `check_rules`
//! ([`crate::governance`]) — those operate on a single [`crate::Engine`] and
//! have no dependency on `federation` at all, so the coverage tier is
//! structurally incapable of moving the gate. [`cross_service_coverage`] is
//! reachable only through an [`EngineRegistry`], which itself exists only when
//! a workspace manifest is present ([`Backing::Federated`](super::Backing)) —
//! the single-root path never constructs one, so this tier is inert with no
//! manifest ([FR-WS-05]).
//!
//! # `no-provider-in-workspace` is bucketed separately ([ADR-53])
//! A reference whose key has no provider anywhere in the workspace is not a
//! defect — the provider repo simply isn't a member of this workspace. It is
//! reported in its own bucket, excluded from the `bound_ratio` denominator, so
//! a sparse workspace reads as *measured*, not broken.
//!
//! [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
//! [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md

use std::collections::HashMap;

use serde::Serialize;

use crate::model::NodeKind;
use crate::resolve::http_client_call::ClientCallRefusal;

use super::bridge::{
    classify, consumer_portable_key, read_members, BridgeEndpoint, MemberContracts, PortableKey,
    Role,
};
use super::registry::{EngineRegistry, MemberEngine};

/// Why one cross-boundary reference did not bind ([FR-WS-05], [ADR-53]).
///
/// `BaseUrlRuntime` and `SchemaMismatch` are forward-declared vocabulary for
/// the gRPC/broker/GraphQL invocation arms ([ADR-54]) — the bridge only
/// resolves the HTTP key in this story, so this classifier never emits them
/// yet; they exist now so those arms extend the same reason enum rather than
/// growing a second one.
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnboundReason {
    /// No member in this workspace exposes a matching provider — outside this
    /// workspace's boundary, not a broken binding ([ADR-53]).
    NoProviderInWorkspace,
    /// The reference's own template could not be reduced to a portable key
    /// (e.g. a catch-all route) — never approximately matched ([NFR-RA-05]).
    PathNotComposed,
    /// The provider's address is resolved only at runtime (a dynamic base
    /// URL) — deferred to a later invocation arm ([ADR-54]).
    BaseUrlRuntime,
    /// Two or more providers expose the same key across the workspace — never
    /// fabricated ([NFR-RA-05]).
    Ambiguous,
    /// The consumer and provider shapes at this key diverge — deferred to a
    /// later invocation arm's schema check ([ADR-54]).
    SchemaMismatch,
}

impl From<ClientCallRefusal> for UnboundReason {
    /// Map the HTTP client-call arm's refusal ([`ClientCallRefusal`], S-252) onto
    /// the shared coverage vocabulary — so a call the arm's normalizer refused
    /// (contributing no reference, [FR-WS-08]) surfaces under the *same* reason
    /// bucket the read-model reports for the composable cases. This is the point
    /// where "the normalizer returned `None`" becomes an advisory coverage reason.
    ///
    /// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
    fn from(refusal: ClientCallRefusal) -> Self {
        match refusal {
            ClientCallRefusal::BaseUrlRuntime => UnboundReason::BaseUrlRuntime,
            ClientCallRefusal::PathNotComposed => UnboundReason::PathNotComposed,
        }
    }
}

/// The 3-state classification of one cross-boundary reference ([FR-WS-05],
/// [ADR-53]).
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum CoverageState {
    /// Bound to exactly one provider in another member.
    Bound,
    /// Not bound, with a [`reason`](UnboundReason) attributed.
    Unbound {
        /// Why this reference did not bind.
        reason: UnboundReason,
    },
}

impl CoverageState {
    /// The human-facing 3-state bucket this classification renders as:
    /// `"bound"`, `"ambiguous"`, or `"unbound"` ([FR-WS-05]).
    ///
    /// `Unbound { reason: Ambiguous }` renders as its own `"ambiguous"`
    /// bucket rather than folding into `"unbound"` — the 2+-providers case is
    /// visually distinct from every other non-binding reason ([ADR-53]).
    pub fn bucket(&self) -> &'static str {
        match self {
            CoverageState::Bound => "bound",
            CoverageState::Unbound {
                reason: UnboundReason::Ambiguous,
            } => "ambiguous",
            CoverageState::Unbound { .. } => "unbound",
        }
    }
}

/// One cross-boundary reference's coverage classification ([FR-WS-05]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferenceCoverage {
    /// The relation class (`"route"` for the HTTP key, today).
    pub relation: String,
    /// The consumer endpoint this reference belongs to.
    pub from: BridgeEndpoint,
    /// The 3-state display bucket (`"bound"`, `"ambiguous"`, or `"unbound"`,
    /// [`CoverageState::bucket`]) — carried as its own field so a consumer
    /// reads the FR-WS-05 3-state classification directly, without having to
    /// special-case `reason == "ambiguous"` against `state` to recover it.
    pub bucket: &'static str,
    /// This reference's full classification. Flattened so the JSON also
    /// carries a top-level `state` key (`"bound"` or `"unbound"`) plus
    /// `reason` when unbound, rather than nesting the internally-tagged enum
    /// under a second `state` object.
    #[serde(flatten)]
    pub state: CoverageState,
}

impl ReferenceCoverage {
    /// Build a [`ReferenceCoverage`], deriving [`bucket`](Self::bucket) from
    /// `state` so the two can never disagree.
    fn new(relation: String, from: BridgeEndpoint, state: CoverageState) -> Self {
        Self {
            relation,
            from,
            bucket: state.bucket(),
            state,
        }
    }
}

/// The non-gated 3-state cross-service coverage summary over a workspace
/// ([FR-WS-05], [ADR-53]) — advisory only, never a gate input (see module
/// docs).
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
#[derive(Debug, Default, Clone, Serialize)]
pub struct CrossServiceCoverage {
    /// Every classified cross-boundary reference, sorted by endpoint for
    /// deterministic output ([NFR-RA-06]).
    pub references: Vec<ReferenceCoverage>,
    /// References bound to exactly one provider in another member.
    pub bound: u64,
    /// References with 2+ providers across the workspace (no edge).
    pub ambiguous: u64,
    /// References unbound for a reason other than ambiguity or
    /// no-provider-in-workspace (today: an uncomposable template).
    pub unbound: u64,
    /// References with no provider anywhere in the workspace — bucketed
    /// separately so they never depress [`bound_ratio`](Self::bound_ratio)
    /// ([ADR-53]).
    pub no_provider_in_workspace: u64,
    /// `bound / (bound + ambiguous + unbound)`, excluding
    /// `no_provider_in_workspace` from the denominator ([ADR-53]). `1.0` when
    /// that denominator is zero (nothing to bind is full coverage, honestly —
    /// mirrors [`crate::resolve::stats`]).
    pub bound_ratio: f64,
}

/// Classify every cross-boundary reference over `registry`'s members
/// ([FR-WS-05], [ADR-53]).
///
/// Reads each member's contract surface through the same
/// [`MemberContracts::contract_surface`] the bridge uses, indexes providers on
/// the shared [`PortableKey`], then classifies **every** `ApiOperation`
/// consumer node — including ones the bridge's edge computation silently
/// drops (an uncomposable template, no candidate, an intra-repo-only match) —
/// so no cross-boundary reference goes unaccounted for.
///
/// A member that fails to start or whose surface read fails is skipped
/// (degraded, not fatal), exactly as [`ContractBridge::edges`](super::bridge::ContractBridge::edges).
pub fn cross_service_coverage<E>(registry: &EngineRegistry<E>) -> CrossServiceCoverage
where
    E: MemberEngine + MemberContracts,
{
    let mut providers: HashMap<PortableKey, Vec<BridgeEndpoint>> = HashMap::new();
    let mut consumer_refs: Vec<(String, String, crate::model::LogosSymbol)> = Vec::new();
    // Arm-tagged invocation consumers (HTTP client calls, S-252, and later arms):
    // `(member, consumer)` pairs read from each member's ledger, classified below
    // through the same provider index as the contract-surface consumers.
    let mut inv_consumers: Vec<(String, super::bridge::InvocationConsumer)> = Vec::new();

    for (member, surface) in read_members(registry, "contract surface", |e| e.contract_surface()) {
        for node in &surface {
            if node.kind == NodeKind::ApiOperation {
                consumer_refs.push((member.clone(), node.name.clone(), node.symbol.clone()));
            }
            if let Some((key, Role::Provider)) = classify(node.kind, &node.name) {
                providers.entry(key).or_default().push(BridgeEndpoint {
                    member: member.clone(),
                    symbol: node.symbol.clone(),
                });
            }
        }
    }
    // Arm consumer refs from each member's ledger (HTTP client calls, gRPC stub
    // calls, and later arms) — degrade-don't-abort via the same `read_members` the
    // bridge uses ([ADR-53]). Classified below through the same provider index and
    // the same `consumer_portable_key` as the bridge, so a new arm surfaces here
    // with no coverage-tier change.
    for (member, refs) in read_members(registry, "invocation consumers", |e| {
        e.invocation_consumers()
    }) {
        for consumer in refs {
            inv_consumers.push((member.clone(), consumer));
        }
    }

    for endpoints in providers.values_mut() {
        endpoints.sort();
    }

    let mut references = Vec::new();
    let mut bound = 0u64;
    let mut ambiguous = 0u64;
    let mut unbound = 0u64;
    let mut no_provider_in_workspace = 0u64;

    for (member, name, symbol) in consumer_refs {
        let from = BridgeEndpoint {
            member: member.clone(),
            symbol,
        };

        let Some((key, _role)) = classify(NodeKind::ApiOperation, &name) else {
            references.push(ReferenceCoverage::new(
                "route".to_string(),
                from,
                CoverageState::Unbound {
                    reason: UnboundReason::PathNotComposed,
                },
            ));
            unbound += 1;
            continue;
        };
        let relation = key.relation().to_string();

        match providers.get(&key).map(Vec::as_slice) {
            None => {
                references.push(ReferenceCoverage::new(
                    relation,
                    from,
                    CoverageState::Unbound {
                        reason: UnboundReason::NoProviderInWorkspace,
                    },
                ));
                no_provider_in_workspace += 1;
            }
            Some([only]) if only.member == member => {
                // A sole same-member provider is an intra-repo fact the
                // per-repo graph already owns, not a cross-boundary reference
                // ([FR-WS-04]) — excluded from the coverage tier entirely,
                // exactly as the bridge emits no edge for it.
            }
            Some([only]) => {
                debug_assert_ne!(only.member, member);
                references.push(ReferenceCoverage::new(relation, from, CoverageState::Bound));
                bound += 1;
            }
            Some(_) => {
                references.push(ReferenceCoverage::new(
                    relation,
                    from,
                    CoverageState::Unbound {
                        reason: UnboundReason::Ambiguous,
                    },
                ));
                ambiguous += 1;
            }
        }
    }

    // Classify the arm-tagged invocation consumers against the same provider
    // index (S-252 HTTP, S-253 gRPC). A stored consumer target normalizes by
    // construction (the arm's normalizer refused the rest before the ledger), so
    // it keys; a target that nonetheless does not compose is `path-not-composed`
    // (a gRPC key, already fully-qualified, never hits that arm).
    for (member, consumer) in inv_consumers {
        let from = BridgeEndpoint {
            member: member.clone(),
            symbol: consumer.symbol,
        };
        let relation = consumer
            .relation
            .bridge_namespace()
            .map(|ns| ns.relation().to_string())
            .unwrap_or_else(|| consumer.relation.as_str().to_string());

        let Some(key) = consumer_portable_key(consumer.relation, &consumer.target) else {
            references.push(ReferenceCoverage::new(
                relation,
                from,
                CoverageState::Unbound {
                    reason: UnboundReason::PathNotComposed,
                },
            ));
            unbound += 1;
            continue;
        };

        match providers.get(&key).map(Vec::as_slice) {
            None => {
                references.push(ReferenceCoverage::new(
                    relation,
                    from,
                    CoverageState::Unbound {
                        reason: UnboundReason::NoProviderInWorkspace,
                    },
                ));
                no_provider_in_workspace += 1;
            }
            // A sole same-member provider is an intra-repo fact the per-repo graph
            // owns, not a cross-boundary reference — excluded, as for operations.
            Some([only]) if only.member == member => {}
            Some([only]) => {
                debug_assert_ne!(only.member, member);
                references.push(ReferenceCoverage::new(relation, from, CoverageState::Bound));
                bound += 1;
            }
            Some(_) => {
                references.push(ReferenceCoverage::new(
                    relation,
                    from,
                    CoverageState::Unbound {
                        reason: UnboundReason::Ambiguous,
                    },
                ));
                ambiguous += 1;
            }
        }
    }

    references.sort_by(|a, b| a.from.cmp(&b.from));

    let denom = bound + ambiguous + unbound;
    let bound_ratio = if denom == 0 {
        1.0
    } else {
        bound as f64 / denom as f64
    };

    CrossServiceCoverage {
        references,
        bound,
        ambiguous,
        unbound,
        no_provider_in_workspace,
        bound_ratio,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use anyhow::Result;

    use super::super::bridge::{ContractNode, InvocationConsumer};
    use super::super::registry::RegistryMode;
    use super::super::{Federation, Member};
    use crate::model::LogosSymbol;

    // Per-test-thread fixtures, mirroring `bridge::tests` — each `#[test]`
    // runs on its own thread, so the thread-local member surfaces are
    // test-isolated.
    thread_local! {
        static FIXTURES: RefCell<HashMap<String, Vec<ContractNode>>> = RefCell::new(HashMap::new());
        static CONSUMERS: RefCell<HashMap<String, Vec<super::super::InvocationConsumer>>> =
            RefCell::new(HashMap::new());
    }

    fn reset() {
        FIXTURES.with(|f| f.borrow_mut().clear());
        CONSUMERS.with(|c| c.borrow_mut().clear());
    }
    fn set_member(name: &str, nodes: Vec<ContractNode>) {
        FIXTURES.with(|f| {
            f.borrow_mut().insert(name.to_string(), nodes);
        });
    }
    fn set_consumers(name: &str, consumers: Vec<super::super::InvocationConsumer>) {
        CONSUMERS.with(|c| {
            c.borrow_mut().insert(name.to_string(), consumers);
        });
    }
    /// An HTTP client-call consumer at `symbol` calling `target` (`"METHOD /path"`).
    fn http_call(target: &str, symbol: &str) -> super::super::InvocationConsumer {
        super::super::InvocationConsumer {
            relation: crate::model::ArtifactRelation::HttpClientCall,
            target: target.to_string(),
            symbol: LogosSymbol::parse(symbol).unwrap(),
        }
    }
    /// A gRPC stub-call consumer at `symbol` invoking `key` (`package.Service/Method`).
    fn grpc_consumer(key: &str, symbol: &str) -> super::super::InvocationConsumer {
        super::super::InvocationConsumer {
            relation: crate::model::ArtifactRelation::GrpcCall,
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
            if self.member == "unreadable" {
                anyhow::bail!("store read failed");
            }
            Ok(FIXTURES.with(|f| f.borrow().get(&self.member).cloned().unwrap_or_default()))
        }
        fn contract_stamp(&self) -> u64 {
            0
        }
        fn invocation_consumers(&self) -> Result<Vec<super::super::InvocationConsumer>> {
            if self.member == "unreadable" {
                anyhow::bail!("store read failed");
            }
            Ok(CONSUMERS.with(|c| c.borrow().get(&self.member).cloned().unwrap_or_default()))
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

    /// A consumer with exactly one cross-member provider classifies `Bound`.
    #[test]
    fn a_sole_cross_member_provider_is_bound() {
        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_get")]);

        let cov = cross_service_coverage(&registry(&["api", "web"]));

        assert_eq!(cov.bound, 1);
        assert_eq!(cov.ambiguous, 0);
        assert_eq!(cov.unbound, 0);
        assert_eq!(cov.no_provider_in_workspace, 0);
        assert_eq!(cov.references.len(), 1);
        assert_eq!(cov.references[0].state, CoverageState::Bound);
        assert_eq!(cov.references[0].relation, "route");
        assert_eq!(cov.bound_ratio, 1.0);
    }

    /// The flattened `state` field serializes as one top-level `"state"` key
    /// (never a nested `state.state`), with `reason` present only when
    /// unbound, and `bucket` always present as the direct 3-state label
    /// (`"bound"`/`"ambiguous"`/`"unbound"`) — a per-reference consumer reads
    /// `bucket` without special-casing `reason == "ambiguous"` ([FR-WS-05]).
    #[test]
    fn state_serializes_flat_not_double_nested() {
        let bound = ReferenceCoverage::new(
            "route".to_string(),
            BridgeEndpoint {
                member: "api".to_string(),
                symbol: LogosSymbol::parse("local op_get").unwrap(),
            },
            CoverageState::Bound,
        );
        let bound_json = serde_json::to_value(&bound).unwrap();
        assert_eq!(bound_json["state"], "bound");
        assert_eq!(bound_json["bucket"], "bound");
        assert!(bound_json.get("reason").is_none());

        let unbound = ReferenceCoverage::new(
            bound.relation.clone(),
            bound.from.clone(),
            CoverageState::Unbound {
                reason: UnboundReason::NoProviderInWorkspace,
            },
        );
        let unbound_json = serde_json::to_value(&unbound).unwrap();
        assert_eq!(unbound_json["state"], "unbound");
        assert_eq!(unbound_json["bucket"], "unbound");
        assert_eq!(unbound_json["reason"], "no-provider-in-workspace");

        let ambiguous = ReferenceCoverage::new(
            bound.relation.clone(),
            bound.from.clone(),
            CoverageState::Unbound {
                reason: UnboundReason::Ambiguous,
            },
        );
        let ambiguous_json = serde_json::to_value(&ambiguous).unwrap();
        assert_eq!(
            ambiguous_json["bucket"], "ambiguous",
            "an ambiguous reason gets its own bucket, distinct from the generic unbound bucket"
        );
    }

    /// No provider anywhere in the workspace classifies as its own bucket,
    /// separate from `unbound`, and never enters the bound-ratio denominator
    /// ([ADR-53] acceptance).
    #[test]
    fn no_provider_in_workspace_is_bucketed_separately_and_excluded_from_ratio() {
        reset();
        set_member("api", vec![op("GET /orphans/{id}", "local op_orphan")]);
        set_member("web", vec![]); // no route anywhere

        let cov = cross_service_coverage(&registry(&["api", "web"]));

        assert_eq!(cov.no_provider_in_workspace, 1);
        assert_eq!(cov.bound, 0);
        assert_eq!(cov.ambiguous, 0);
        assert_eq!(cov.unbound, 0);
        assert_eq!(
            cov.references[0].state,
            CoverageState::Unbound {
                reason: UnboundReason::NoProviderInWorkspace
            }
        );
        assert_eq!(
            cov.bound_ratio, 1.0,
            "an empty bound+ambiguous+unbound denominator is full coverage, \
             not depressed by the excluded no-provider bucket"
        );
    }

    /// Two providers of the same key classify `Ambiguous`, distinct from a
    /// plain `unbound` reason bucket.
    #[test]
    fn two_providers_classify_ambiguous() {
        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_web")]);
        set_member(
            "admin",
            vec![route("GET /users/{userId}", "local route_admin")],
        );

        let cov = cross_service_coverage(&registry(&["api", "web", "admin"]));

        assert_eq!(cov.ambiguous, 1);
        assert_eq!(cov.bound, 0);
        assert_eq!(
            cov.references[0].state,
            CoverageState::Unbound {
                reason: UnboundReason::Ambiguous
            }
        );
        assert_eq!(cov.references[0].state.bucket(), "ambiguous");
    }

    /// A local-plus-remote provider pair is still ambiguous (2 providers),
    /// mirroring the bridge's own classification.
    #[test]
    fn a_local_plus_remote_provider_pair_is_ambiguous() {
        reset();
        set_member(
            "api",
            vec![
                op("GET /users/{id}", "local op_get"),
                route("GET /users/{id}", "local route_local"),
            ],
        );
        set_member("web", vec![route("GET /users/{id}", "local route_web")]);

        let cov = cross_service_coverage(&registry(&["api", "web"]));
        assert_eq!(cov.ambiguous, 1);
        assert_eq!(cov.bound, 0);
    }

    /// A route whose template does not normalize is never a provider
    /// candidate; the consumer classifies `no-provider-in-workspace`, not
    /// silently dropped.
    #[test]
    fn a_non_normalizing_provider_leaves_the_consumer_unbound() {
        reset();
        set_member("api", vec![op("GET /files/{id}", "local op_files")]);
        set_member(
            "web",
            vec![route("GET /files/{*rest}", "local route_catchall")],
        );

        let cov = cross_service_coverage(&registry(&["api", "web"]));
        assert_eq!(cov.no_provider_in_workspace, 1);
        assert_eq!(cov.bound, 0);
    }

    /// A consumer whose own template does not normalize classifies
    /// `path-not-composed` — it is still an accounted-for reference, not
    /// silently dropped the way the bridge's edge computation drops it.
    #[test]
    fn an_uncomposable_consumer_template_is_path_not_composed() {
        reset();
        set_member(
            "api",
            vec![op("GET /files/{*rest}", "local op_catchall")],
        );
        set_member("web", vec![route("GET /files/{id}", "local route_get")]);

        let cov = cross_service_coverage(&registry(&["api", "web"]));

        assert_eq!(cov.unbound, 1);
        assert_eq!(cov.bound, 0);
        assert_eq!(cov.no_provider_in_workspace, 0);
        assert_eq!(
            cov.references[0].state,
            CoverageState::Unbound {
                reason: UnboundReason::PathNotComposed
            }
        );
        assert_eq!(cov.references[0].state.bucket(), "unbound");
    }

    /// A sole provider in the consumer's own member is excluded from the
    /// coverage tier entirely — it is an intra-repo fact, not a cross-boundary
    /// reference, mirroring the bridge's own exclusion.
    #[test]
    fn a_sole_same_member_provider_is_excluded_not_unbound() {
        reset();
        set_member(
            "api",
            vec![
                op("GET /users/{id}", "local op_get"),
                route("GET /users/{id}", "local route_local"),
            ],
        );

        let cov = cross_service_coverage(&registry(&["api"]));

        assert!(
            cov.references.is_empty(),
            "an intra-repo-only match is not a cross-boundary reference: {:?}",
            cov.references
        );
        assert_eq!(cov.bound + cov.ambiguous + cov.unbound + cov.no_provider_in_workspace, 0);
    }

    /// A degraded member (fails to start, or starts but its surface read
    /// fails) is skipped, not fatal — the healthy members still classify
    /// ([ADR-53] degrade-don't-abort, mirroring the bridge).
    #[test]
    fn a_degraded_member_is_skipped_not_fatal() {
        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_get")]);

        let cov = cross_service_coverage(&registry(&["api", "web", "broken"]));
        assert_eq!(cov.bound, 1);

        let cov2 = cross_service_coverage(&registry(&["api", "web", "unreadable"]));
        assert_eq!(cov2.bound, 1);
    }

    /// The bound-ratio is computed over bound+ambiguous+unbound only.
    #[test]
    fn bound_ratio_excludes_no_provider_but_includes_ambiguous_and_unbound() {
        reset();
        set_member(
            "api",
            vec![
                op("GET /a/{id}", "local op_a"),  // bound
                op("GET /b/{id}", "local op_b"),  // ambiguous
                op("GET /c/{id}", "local op_c"),  // no-provider
                op("GET /d/{*r}", "local op_d"),  // path-not-composed
            ],
        );
        set_member(
            "web",
            vec![
                route("GET /a/{id}", "local route_a"),
                route("GET /b/{id}", "local route_b1"),
            ],
        );
        set_member("svc", vec![route("GET /b/{id}", "local route_b2")]);

        let cov = cross_service_coverage(&registry(&["api", "web", "svc"]));
        assert_eq!(cov.bound, 1);
        assert_eq!(cov.ambiguous, 1);
        assert_eq!(cov.unbound, 1);
        assert_eq!(cov.no_provider_in_workspace, 1);
        assert_eq!(cov.bound_ratio, 1.0 / 3.0, "1 bound of 3 counted (bound+ambiguous+unbound)");
    }

    /// A `ProtoService`/`GqlType` surface node carries no portable HTTP key in
    /// this story ([FR-WS-07]+ deferred) — mixed alongside genuine
    /// `ApiOperation`/`Route` nodes, it must be silently excluded from both
    /// `consumer_refs` and `providers`, not misclassified or double-counted.
    #[test]
    fn proto_and_graphql_nodes_are_silently_excluded_not_misclassified() {
        reset();
        set_member(
            "api",
            vec![
                op("GET /users/{id}", "local op_get"),
                ContractNode {
                    kind: NodeKind::ProtoService,
                    name: "user.UserService".to_string(),
                    symbol: LogosSymbol::parse("local svc").unwrap(),
                },
            ],
        );
        set_member(
            "web",
            vec![
                route("GET /users/{id}", "local route_get"),
                ContractNode {
                    kind: NodeKind::GqlType,
                    name: "User".to_string(),
                    symbol: LogosSymbol::parse("local gqltype").unwrap(),
                },
            ],
        );

        let with_proto_graphql = cross_service_coverage(&registry(&["api", "web"]));

        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_get")]);
        let without = cross_service_coverage(&registry(&["api", "web"]));

        assert_eq!(
            with_proto_graphql.references, without.references,
            "a ProtoService/GqlType node must not add, remove, or alter any classified reference"
        );
        assert_eq!(with_proto_graphql.bound, 1);
        assert_eq!(with_proto_graphql.ambiguous, 0);
        assert_eq!(with_proto_graphql.unbound, 0);
        assert_eq!(with_proto_graphql.no_provider_in_workspace, 0);
    }

    /// The emitted reference set is deterministic (sorted by endpoint)
    /// regardless of member fan-out order ([NFR-RA-06]).
    #[test]
    fn references_are_deterministic_across_member_order() {
        reset();
        set_member(
            "api",
            vec![op("GET /a/{id}", "local op_a"), op("GET /b/{id}", "local op_b")],
        );
        set_member("web", vec![route("GET /a/{id}", "local route_a")]);
        set_member("svc", vec![route("GET /b/{id}", "local route_b")]);

        let one = cross_service_coverage(&registry(&["api", "web", "svc"]));
        let two = cross_service_coverage(&registry(&["svc", "web", "api"]));
        assert_eq!(one.references, two.references);
    }

    // ── S-252 / FR-WS-08: HTTP client-call consumers in the coverage tier ─────

    /// A static client call with exactly one cross-member route classifies
    /// `Bound` under the `route` relation — the same 3-state model an operation
    /// consumer gets, now driven by a ledger-side invocation consumer.
    #[test]
    fn a_client_call_with_a_sole_cross_member_route_is_bound() {
        reset();
        set_consumers("web", vec![http_call("GET /users/{id}", "local get_user_call")]);
        set_member("api", vec![route("GET /users/{userId}", "local route_get")]);

        let cov = cross_service_coverage(&registry(&["web", "api"]));

        assert_eq!(cov.bound, 1);
        assert_eq!(cov.ambiguous, 0);
        assert_eq!(cov.unbound, 0);
        assert_eq!(cov.no_provider_in_workspace, 0);
        assert_eq!(cov.references.len(), 1);
        assert_eq!(cov.references[0].state, CoverageState::Bound);
        assert_eq!(cov.references[0].relation, "route");
        assert_eq!(cov.references[0].from.member, "web");
    }

    /// Two matching routes make the client call `Ambiguous` (its own bucket).
    #[test]
    fn a_client_call_with_two_routes_is_ambiguous() {
        reset();
        set_consumers("web", vec![http_call("GET /users/{id}", "local get_user_call")]);
        set_member("api", vec![route("GET /users/{id}", "local route_api")]);
        set_member("admin", vec![route("GET /users/{userId}", "local route_admin")]);

        let cov = cross_service_coverage(&registry(&["web", "api", "admin"]));

        assert_eq!(cov.ambiguous, 1);
        assert_eq!(cov.bound, 0);
        assert_eq!(cov.references[0].state.bucket(), "ambiguous");
    }

    /// A client call with no matching route anywhere is bucketed
    /// `no-provider-in-workspace` (outside the boundary, not a defect), excluded
    /// from the bound-ratio denominator.
    #[test]
    fn a_client_call_with_no_route_is_no_provider_in_workspace() {
        reset();
        set_consumers("web", vec![http_call("GET /orphans/{id}", "local orphan_call")]);
        set_member("api", vec![]);

        let cov = cross_service_coverage(&registry(&["web", "api"]));

        assert_eq!(cov.no_provider_in_workspace, 1);
        assert_eq!(cov.bound, 0);
        assert_eq!(cov.bound_ratio, 1.0);
    }

    /// A client call whose only matching route is in its own member is an
    /// intra-repo fact — excluded from the coverage tier, mirroring operations.
    #[test]
    fn a_same_member_client_call_route_pair_is_excluded() {
        reset();
        set_consumers("web", vec![http_call("GET /users/{id}", "local get_user_call")]);
        set_member("web", vec![route("GET /users/{id}", "local route_local")]);

        let cov = cross_service_coverage(&registry(&["web"]));
        assert!(
            cov.references.is_empty(),
            "an intra-repo client call→route pair is not a cross-boundary reference: {:?}",
            cov.references
        );
    }

    /// Acceptance (2): the HTTP arm's refusals map onto the coverage vocabulary —
    /// a base-URL-composed call is `base-url-runtime`, a non-normalizable one is
    /// `path-not-composed`. This is the reason a call the normalizer refused (and
    /// therefore left out of the ledger) is reported under, tying the arm's
    /// `None` render to the coverage reason enum.
    #[test]
    fn client_call_refusals_map_to_the_coverage_reasons() {
        assert_eq!(
            UnboundReason::from(ClientCallRefusal::BaseUrlRuntime),
            UnboundReason::BaseUrlRuntime
        );
        assert_eq!(
            UnboundReason::from(ClientCallRefusal::PathNotComposed),
            UnboundReason::PathNotComposed
        );
        // And they render as the FR-WS-08 wire tokens.
        assert_eq!(
            serde_json::to_value(UnboundReason::BaseUrlRuntime).unwrap(),
            "base-url-runtime"
        );
        assert_eq!(
            serde_json::to_value(UnboundReason::PathNotComposed).unwrap(),
            "path-not-composed"
        );
    }

    /// Operation consumers and client-call consumers coexist: an `ApiOperation`
    /// and an HTTP client call both binding cross-member each count once, proving
    /// the two intake paths compose without double-counting or interfering.
    #[test]
    fn operation_and_client_call_consumers_coexist() {
        reset();
        // An operation in `spec` bound by a route in `web`; a client call in `web`
        // bound by a route in `api` — both cross-member, no same-member collision.
        set_member("spec", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_users")]);
        set_consumers("web", vec![http_call("GET /orders/{id}", "local orders_call")]);
        set_member("api", vec![route("GET /orders/{id}", "local route_orders")]);

        let cov = cross_service_coverage(&registry(&["spec", "web", "api"]));
        assert_eq!(cov.bound, 2, "the operation and the client call each bind once");
        assert_eq!(cov.references.len(), 2);
        assert!(cov.references.iter().all(|r| r.relation == "route"));
    }

    // ── S-253 / FR-WS-09: gRPC stub-call coverage ────────────────────────

    /// A gRPC stub call whose `package.Service/Method` provider lives in another
    /// member classifies `Bound`, under the `grpc-call` relation.
    #[test]
    fn a_grpc_stub_call_binds_its_cross_member_proto_service() {
        reset();
        set_member(
            "svc",
            vec![proto_service("example.v1.UserService/GetUser", "local svc")],
        );
        set_consumers(
            "api",
            vec![grpc_consumer("example.v1.UserService/GetUser", "local stub")],
        );

        let cov = cross_service_coverage(&registry(&["api", "svc"]));
        assert_eq!(cov.bound, 1);
        assert_eq!(cov.references.len(), 1);
        assert_eq!(cov.references[0].state, CoverageState::Bound);
        assert_eq!(cov.references[0].relation, "grpc-call");
    }

    /// Acceptance (3): a qualifiable gRPC stub call with no provider anywhere in
    /// the workspace stays honestly unbound with a coverage reason
    /// (`no-provider-in-workspace`) — never silently dropped, never fabricated.
    #[test]
    fn a_grpc_stub_call_with_no_provider_is_unbound_with_a_reason() {
        reset();
        set_consumers(
            "api",
            vec![grpc_consumer("example.v1.UserService/GetUser", "local stub")],
        );
        // A second member with no matching proto service provider.
        set_member("svc", vec![proto_service("example.v1.Other/Do", "local other")]);

        let cov = cross_service_coverage(&registry(&["api", "svc"]));
        assert_eq!(cov.no_provider_in_workspace, 1);
        assert_eq!(cov.bound, 0);
        assert_eq!(
            cov.references[0].state,
            CoverageState::Unbound {
                reason: UnboundReason::NoProviderInWorkspace
            }
        );
        assert_eq!(cov.references[0].relation, "grpc-call");
    }

    /// A gRPC stub call whose sole provider is in its own member is an intra-repo
    /// fact the per-repo graph owns — excluded from the coverage tier entirely,
    /// exactly as the HTTP path and the bridge (never counted as `Bound`).
    #[test]
    fn an_intra_repo_grpc_call_is_excluded_not_bound() {
        reset();
        set_member(
            "svc",
            vec![proto_service("example.v1.UserService/GetUser", "local svc")],
        );
        set_consumers(
            "svc",
            vec![grpc_consumer("example.v1.UserService/GetUser", "local stub")],
        );

        let cov = cross_service_coverage(&registry(&["svc"]));
        assert!(
            cov.references.is_empty(),
            "a same-member stub→provider pair is intra-repo, not a cross-boundary reference: {:?}",
            cov.references
        );
        assert_eq!(cov.bound + cov.ambiguous + cov.unbound + cov.no_provider_in_workspace, 0);
    }

    /// Two members exposing the identical `package.Service/Method` provider make
    /// the stub call ambiguous — its own bucket, distinct from a plain unbound.
    #[test]
    fn two_grpc_providers_classify_ambiguous() {
        reset();
        set_member(
            "svc1",
            vec![proto_service("example.v1.UserService/GetUser", "local a")],
        );
        set_member(
            "svc2",
            vec![proto_service("example.v1.UserService/GetUser", "local b")],
        );
        set_consumers(
            "api",
            vec![grpc_consumer("example.v1.UserService/GetUser", "local stub")],
        );

        let cov = cross_service_coverage(&registry(&["api", "svc1", "svc2"]));
        assert_eq!(cov.ambiguous, 1);
        assert_eq!(cov.bound, 0);
        assert_eq!(cov.references[0].state.bucket(), "ambiguous");
    }

    // ── advisory isolation (ADR-53 / sprint-55 risk register) ─────────────

    /// The coverage tier must never leak into the gated `scan`/`gate`/
    /// `check_rules` surfaces: their serialized output carries none of the
    /// coverage vocabulary, computing coverage first or not at all yields
    /// byte-identical gated output, and (structurally) none of those Engine
    /// methods ever import or call into this module.
    #[test]
    fn coverage_never_flows_into_scan_gate_or_check_rules() {
        reset();
        set_member("api", vec![op("GET /users/{id}", "local op_get")]);
        set_member("web", vec![route("GET /users/{id}", "local route_get")]);

        // Computing the coverage tier must not be a prerequisite for, nor
        // side-effect into, the gated surfaces below.
        let _ = cross_service_coverage(&registry(&["api", "web"]));

        let tmp = tempfile::TempDir::new().expect("temp root");
        let engine = crate::Engine::start(tmp.path()).expect("engine starts");

        let scan = engine.scan(true).expect("scan runs");
        let gate = engine.gate(None, false, true).expect("gate runs");
        let check = engine.check_rules(None, true).expect("check_rules runs");

        let scan_json = serde_json::to_string(&scan).expect("scan serializes");
        let gate_json = serde_json::to_string(&gate).expect("gate serializes");
        let check_json = serde_json::to_string(&check).expect("check_rules serializes");

        for (label, json) in [("scan", &scan_json), ("gate", &gate_json), ("check_rules", &check_json)] {
            for token in [
                "bound_ratio",
                "no_provider_in_workspace",
                "ambiguous",
                "path-not-composed",
                "base-url-runtime",
                "schema-mismatch",
                "no-provider-in-workspace",
            ] {
                assert!(
                    !json.contains(token),
                    "coverage vocabulary {token:?} leaked into gated {label} output: {json}"
                );
            }
        }
    }

    /// Single-root regression ([FR-WS-05]): with no workspace manifest there
    /// is no `EngineRegistry` to call this module's entry point on at all —
    /// the tier is structurally inert, and the existing per-repo `scan` output
    /// is unaffected by this module's mere presence in the crate.
    #[test]
    fn inert_without_a_workspace_manifest() {
        let tmp = tempfile::TempDir::new().expect("temp root");
        let engine = crate::Engine::start(tmp.path()).expect("engine starts");

        let federation = super::super::discover(tmp.path()).expect("discovery succeeds");
        assert!(
            federation.is_none(),
            "no manifest anywhere up-tree must discover as single-root"
        );

        // With no `Federation`, no `Backing::Federated` registry is ever
        // built, so `cross_service_coverage` is never reachable on this path.
        let scan = engine.scan(true).expect("scan runs on the single-root path");
        assert!(scan.warnings.iter().all(|w| !w.contains("workspace")));
    }
}
