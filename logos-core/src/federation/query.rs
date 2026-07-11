//! The **repo-qualified cross-service query read-models** ([FR-WS-05], [ADR-53]).
//!
//! This module is the thick-core home of the `xservice` surface the
//! [mcp-surface] and [cli-surface] adapters expose: `route-providers`,
//! `callers`, `impact`, and `search`, plus [`workspace_status`]. Every answer
//! is **repo-qualified** — each per-member value is tagged with the
//! [`Member::name`](super::Member::name) that produced it ([FR-WS-03]) — and an
//! optional `repo` filter scopes any fan-out to a single member.
//!
//! # Advisory only, never a gate input ([ADR-53])
//! These read-models are reachable only through an [`EngineRegistry`], which
//! itself exists only when a workspace manifest is present
//! ([`Backing::Federated`](super::Backing)). `scan`/`gate`/`check_rules` operate
//! on a single [`Engine`] and never construct a registry, so nothing here can
//! move a member's gated signal — the single-root path is byte-for-byte
//! unchanged ([FR-WS-05]).
//!
//! # Cross-service impact ([FR-WS-05])
//! [`xservice_impact`] fans per-member [`Engine::impact`] across the
//! [bridge](super::bridge) edges: the seed member's impact, plus — for every
//! [`BridgeEdge`] the queried symbol is an endpoint of — the far member's
//! impact of the opposite endpoint, tagged with the edge it was reached
//! through. A member whose engine fails to start surfaces as a per-member error
//! rather than aborting the whole answer ([ADR-53]).
//!
//! [mcp-surface]: ../../../docs/specs/architecture/components/mcp-surface.md
//! [cli-surface]: ../../../docs/specs/architecture/components/cli-surface.md
//! [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
//! [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
//! [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md

use std::sync::Arc;

use serde::Serialize;

use crate::model::NodeKind;
use crate::models::{CallersResult, ImpactResult, SearchResult, StatusInfo};
use crate::Engine;

use super::bridge::BridgeEdge;
use super::coverage::{cross_service_coverage, CrossServiceCoverage};
use super::registry::{EngineRegistry, MemberScoped};

/// One member's outcome for a repo-qualified fan-out query ([FR-WS-03]).
///
/// The per-member value rides `result`; a member whose engine failed to start
/// carries a human-readable `error` instead — a partly-degraded workspace still
/// answers for its healthy members ([ADR-53]). Exactly one of the two is
/// populated.
///
/// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
#[derive(Debug, Serialize)]
pub struct MemberResult<T> {
    /// The owning member's name (its workspace-relative path).
    pub member: String,
    /// The per-member read-model, when the member's engine started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    /// Why this member produced no result (engine start / read failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T> MemberResult<T> {
    /// Project a fan-out [`MemberScoped<Result<T>>`] onto the wire shape,
    /// flattening the start-failure `Err` into the `error` channel.
    fn from_scoped(scoped: MemberScoped<anyhow::Result<T>>) -> Self {
        match scoped.value {
            Ok(value) => Self {
                member: scoped.member,
                result: Some(value),
                error: None,
            },
            Err(err) => Self {
                member: scoped.member,
                result: None,
                error: Some(format!("{err:#}")),
            },
        }
    }
}

/// Run `f` over the members in scope, tagged repo-qualified ([FR-WS-03]).
///
/// `repo = Some(name)` scopes to that one member (constructing only its engine,
/// [NFR-PE-10]); `repo = None` fans over every member in discovery order. An
/// unknown `repo` surfaces as a single per-member error, not a panic.
fn fan<T>(
    registry: &EngineRegistry<Engine>,
    repo: Option<&str>,
    f: impl Fn(&Engine) -> T,
) -> Vec<MemberResult<T>> {
    match repo {
        Some(member) => vec![MemberResult::from_scoped(MemberScoped {
            member: member.to_string(),
            value: registry.engine_for(member).map(|engine| f(&engine)),
        })],
        None => registry
            .fan_out(|_, engine| f(engine))
            .into_iter()
            .map(MemberResult::from_scoped)
            .collect(),
    }
}

/// Repo-qualified cross-service full-text search ([FR-WS-05]): [`Engine::search`]
/// fanned across the members in scope.
#[derive(Debug, Serialize)]
pub struct XserviceSearch {
    /// The search text as given.
    pub query: String,
    /// The `--repo` scope, if one was applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Per-member search results, repo-qualified.
    pub members: Vec<MemberResult<SearchResult>>,
}

/// Search every member (or the `repo`-scoped one) for `query` ([FR-WS-05]).
pub fn xservice_search(
    registry: &EngineRegistry<Engine>,
    query: &str,
    kind: Option<NodeKind>,
    limit: Option<usize>,
    repo: Option<&str>,
) -> XserviceSearch {
    XserviceSearch {
        query: query.to_string(),
        scope: repo.map(str::to_string),
        members: fan(registry, repo, |engine| engine.search(query, kind, limit)),
    }
}

/// Repo-qualified cross-service callers ([FR-WS-05]): each member's intra-repo
/// [`Engine::callers`], plus the cross-service consumers that reach the symbol
/// over a [bridge](super::bridge) edge.
#[derive(Debug, Serialize)]
pub struct XserviceCallers {
    /// The symbol text as given.
    pub query: String,
    /// The `--repo` scope, if one was applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Per-member intra-repo callers, repo-qualified.
    pub members: Vec<MemberResult<CallersResult>>,
    /// Cross-service callers: bridge edges whose provider endpoint is the
    /// queried symbol — the consumer endpoint (`from`) is the cross-boundary
    /// caller. Never fabricated (exactly-one, [NFR-RA-05]).
    pub cross_service: Vec<BridgeEdge>,
}

/// Callers of `symbol` across the workspace ([FR-WS-05]).
pub fn xservice_callers(
    registry: &EngineRegistry<Engine>,
    edges: &[BridgeEdge],
    symbol: &str,
    limit: Option<usize>,
    repo: Option<&str>,
) -> XserviceCallers {
    XserviceCallers {
        query: symbol.to_string(),
        scope: repo.map(str::to_string),
        members: fan(registry, repo, |engine| engine.callers(symbol, limit)),
        // The cross-service tier matches on the canonical symbol *string* alone
        // (not member+symbol): a `LogosSymbol` is a database-portable identity
        // and bridge edges are already exactly-one resolved cross-member, so a
        // provider symbol identifies its consumers unambiguously.
        cross_service: edges
            .iter()
            .filter(|edge| edge.to.symbol.as_str() == symbol)
            .cloned()
            .collect(),
    }
}

/// One far-side impact reached by stitching across a [`BridgeEdge`] ([FR-WS-05]).
#[derive(Debug, Serialize)]
pub struct CrossServiceImpact {
    /// The bridge edge the far member was reached through.
    pub via: BridgeEdge,
    /// The far member whose impact this is.
    pub member: String,
    /// The far endpoint's transitive impact within its own member.
    pub impact: ImpactResult,
}

/// Repo-qualified cross-service impact ([FR-WS-05]): the seed member(s)' impact
/// plus each far-side impact stitched across a bridge edge.
#[derive(Debug, Serialize)]
pub struct XserviceImpact {
    /// The symbol text as given.
    pub query: String,
    /// The `--repo` scope, if one was applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// The seed impact, per member in scope, repo-qualified.
    pub seed: Vec<MemberResult<ImpactResult>>,
    /// Far-side impacts reached by fanning across the bridge edges the queried
    /// symbol is an endpoint of.
    pub cross_service: Vec<CrossServiceImpact>,
}

/// Transitive impact of `symbol`, stitched across bridge edges ([FR-WS-05]).
///
/// The seed is `symbol`'s impact in each member in scope; the cross-service tier
/// follows every [`BridgeEdge`] the symbol is an endpoint of to the opposite
/// endpoint's member and computes *its* impact — literally fanning per-member
/// impact across the bridge. A far member whose engine fails to start is
/// skipped (degraded, not fatal, [ADR-53]).
pub fn xservice_impact(
    registry: &EngineRegistry<Engine>,
    edges: &[BridgeEdge],
    symbol: &str,
    depth: Option<usize>,
    repo: Option<&str>,
) -> XserviceImpact {
    let cross_service = edges
        .iter()
        .filter_map(|edge| {
            let far = if edge.from.symbol.as_str() == symbol {
                &edge.to
            } else if edge.to.symbol.as_str() == symbol {
                &edge.from
            } else {
                return None;
            };
            // A far member that fails to start is skipped, not fatal ([ADR-53]).
            let engine = registry.engine_for(&far.member).ok()?;
            Some(CrossServiceImpact {
                via: edge.clone(),
                member: far.member.clone(),
                impact: engine.impact(far.symbol.as_str(), depth),
            })
        })
        .collect();

    XserviceImpact {
        query: symbol.to_string(),
        scope: repo.map(str::to_string),
        seed: fan(registry, repo, |engine| engine.impact(symbol, depth)),
        cross_service,
    }
}

/// The resolved cross-service route bindings ([FR-WS-05]): the [bridge](super::bridge)
/// edges, optionally scoped to routes a single member *provides*.
#[derive(Debug, Serialize)]
pub struct XserviceRouteProviders {
    /// The `--repo` scope, if one was applied (routes provided by that member).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Each resolved cross-service binding: `from` (the consumer endpoint) →
    /// `to` (the provider endpoint), both repo-qualified `(member, symbol)`.
    pub providers: Vec<BridgeEdge>,
}

/// The cross-service route bindings, scoped to a provider member when `repo` is
/// given ([FR-WS-05]).
pub fn xservice_route_providers(
    edges: &[BridgeEdge],
    repo: Option<&str>,
) -> XserviceRouteProviders {
    XserviceRouteProviders {
        scope: repo.map(str::to_string),
        providers: edges
            .iter()
            .filter(|edge| repo.is_none_or(|member| edge.to.member == member))
            .cloned()
            .collect(),
    }
}

/// The `logos workspace status` read-model ([FR-WS-05]): each member's index
/// freshness plus the 3-state cross-service coverage summary.
#[derive(Debug, Serialize)]
pub struct WorkspaceStatus {
    /// The workspace name (`[workspace] name`).
    pub workspace: String,
    /// Per-member index freshness ([`Engine::status`]), repo-qualified.
    pub members: Vec<MemberResult<StatusInfo>>,
    /// The non-gated 3-state cross-service coverage summary from [S-247]
    /// ([`cross_service_coverage`], [ADR-53]).
    ///
    /// [S-247]: ../coverage/index.html
    pub coverage: CrossServiceCoverage,
}

/// Per-member freshness plus the 3-state coverage summary ([FR-WS-05]).
pub fn workspace_status(registry: &EngineRegistry<Engine>) -> WorkspaceStatus {
    WorkspaceStatus {
        workspace: registry.federation().name.clone(),
        members: fan(registry, None, Engine::status),
        coverage: cross_service_coverage(registry),
    }
}

/// The bridge edge set the query surface stitches over, resolved once per call
/// so a CLI one-shot and the serve loop share the same entry point ([FR-WS-04]).
///
/// A thin re-export of [`ContractBridge::edges`](super::bridge::ContractBridge::edges)
/// kept here so callers reach the whole query surface through this module.
pub fn edges(
    bridge: &super::bridge::ContractBridge,
    registry: &EngineRegistry<Engine>,
) -> Arc<Vec<BridgeEdge>> {
    bridge.edges(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::federation::BridgeEndpoint;
    use crate::model::LogosSymbol;

    fn endpoint(member: &str, symbol: &str) -> BridgeEndpoint {
        // The `local <name>` scheme is the smallest valid SCIP symbol the
        // bridge fixtures use — a bare name is not a parseable symbol.
        BridgeEndpoint {
            member: member.to_string(),
            symbol: LogosSymbol::parse(&format!("local {symbol}")).unwrap(),
        }
    }

    fn edge(from_member: &str, from_symbol: &str, to_member: &str, to_symbol: &str) -> BridgeEdge {
        BridgeEdge {
            relation: "route".to_string(),
            from: endpoint(from_member, from_symbol),
            to: endpoint(to_member, to_symbol),
        }
    }

    /// Unscoped, `route-providers` returns every resolved binding verbatim.
    #[test]
    fn route_providers_unscoped_returns_every_edge() {
        let edges = [edge("api", "op1", "web", "r1"), edge("api", "op2", "svc", "r2")];
        let out = xservice_route_providers(&edges, None);
        assert_eq!(out.providers.len(), 2);
        assert!(out.scope.is_none(), "no scope when repo is None");
    }

    /// `--repo` scopes to bindings whose PROVIDER (`to`) is that member — routes
    /// that member exposes to the rest of the workspace ([FR-WS-05]).
    #[test]
    fn route_providers_repo_scopes_to_the_provider_member() {
        let edges = [edge("api", "op1", "web", "r1"), edge("api", "op2", "svc", "r2")];
        let out = xservice_route_providers(&edges, Some("web"));
        assert_eq!(out.providers.len(), 1, "only web-provided routes survive");
        assert_eq!(out.providers[0].to.member, "web");
        assert_eq!(out.scope.as_deref(), Some("web"));

        // A member that provides nothing (only consumes) scopes to empty.
        assert!(xservice_route_providers(&edges, Some("api")).providers.is_empty());
    }

    /// [`MemberResult`] serialises the ok and error channels **mutually
    /// exclusively** — a healthy member carries `result` (no `error`), a
    /// degraded one carries `error` (no `result`), so the wire is machine-clean.
    #[test]
    fn member_result_serialises_exactly_one_channel() {
        let ok = MemberResult::from_scoped(MemberScoped {
            member: "api".to_string(),
            value: Ok(7u32),
        });
        let value = serde_json::to_value(&ok).unwrap();
        assert_eq!(value["member"], "api");
        assert_eq!(value["result"], 7);
        assert!(value.get("error").is_none(), "an ok result omits the error key");

        let degraded: MemberResult<u32> = MemberResult::from_scoped(MemberScoped {
            member: "web".to_string(),
            value: Err(anyhow::anyhow!("store is corrupt")),
        });
        let value = serde_json::to_value(&degraded).unwrap();
        assert_eq!(value["member"], "web");
        assert!(value.get("result").is_none(), "an error omits the result key");
        assert_eq!(value["error"], "store is corrupt");
    }
}
