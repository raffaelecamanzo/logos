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
use super::topics::{workspace_topics, MemberTopics};
use super::warm_state::{self, MemberWarmState, WarmEvidence, WarmRollup};

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

/// Per-member index freshness with **both** failure channels folded into
/// [`MemberResult::error`] ([FR-WS-05], [BR-44]).
///
/// [`fan`] cannot serve this: it flattens only the engine-*start* `Err`, and the
/// freshness read has a second failure mode of its own. [`Engine::status`] hides
/// that one — it degrades to a defaulted [`StatusInfo`] whose `indexed: false`
/// is indistinguishable from an honestly empty graph, so a member whose read
/// failed would be labeled `deferred` ("never attempted") instead of `degraded`
/// ([BR-44]). Fanning [`Engine::try_status`] and flattening both `Result`s keeps
/// `MemberResult`'s exactly-one-channel contract while making the second failure
/// visible to [`MemberStatus`].
///
/// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
fn fan_status(registry: &EngineRegistry<Engine>) -> Vec<MemberResult<StatusInfo>> {
    registry
        .fan_out(|_, engine| engine.try_status())
        .into_iter()
        .map(flatten_status)
        .collect()
}

/// Collapse the two nested failure channels of a fanned [`Engine::try_status`]
/// onto [`MemberResult`]'s single `error` ([BR-44]).
///
/// Outer `Err`: the member's engine failed to start. Inner `Err`: it started and
/// the freshness read itself failed. Split out from [`fan_status`] so both are
/// assertable without a corrupt on-disk store — the inner channel is the one
/// this story added, and the point of it is that it must NOT arrive as a
/// defaulted `StatusInfo`.
///
/// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
fn flatten_status(
    scoped: MemberScoped<anyhow::Result<anyhow::Result<StatusInfo>>>,
) -> MemberResult<StatusInfo> {
    MemberResult::from_scoped(MemberScoped {
        member: scoped.member,
        value: scoped.value.and_then(|status| status),
    })
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

/// One member's row in a [`WorkspaceStatus`]: its index freshness and its warm
/// state, in **one** record ([FR-WS-05], [FR-WS-15]).
///
/// Both halves are flattened, so a row is the flat table a reader expects —
/// `{"member": "api", "result": {…}, "warm_state": "warm"}` — rather than a
/// freshness list a caller has to join a state list against by name. That
/// matters for what comes next as much as for now: [FR-WS-16]'s degraded
/// reporting extends this same row.
///
/// Only `workspace status` carries a warm state; the `xservice` fan-outs keep
/// the plain [`MemberResult`], which is why the state lives here rather than on
/// the generic envelope.
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
/// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
#[derive(Debug, Serialize)]
pub struct MemberStatus {
    /// The member's index freshness, or why it could not be read.
    #[serde(flatten)]
    pub status: MemberResult<StatusInfo>,
    /// This member's warm state ([FR-WS-15]).
    ///
    /// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
    #[serde(flatten)]
    pub warm: MemberWarmState,
}

impl MemberStatus {
    /// Build one labelled row from a member's already-read freshness
    /// ([FR-WS-15]).
    ///
    /// The state itself is derived by
    /// [`warm_state::derive_state`](super::warm_state::derive_state), which owns
    /// the vocabulary AND its precedence rules; the only work here is projecting
    /// [`MemberResult`]'s two channels onto that function's
    /// `Result<bool, &str>` input. Deliberately not restated — a second copy of
    /// the table would go quietly stale the first time the precedence changed.
    ///
    /// No engine is constructed, opened, or touched here ([NFR-PE-10]).
    ///
    /// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    fn labelled(status: MemberResult<StatusInfo>, evidence: &WarmEvidence) -> Self {
        // `result` is the only channel that can yield `Ok`; absent freshness is
        // never evidence of an un-attempted member, so it reads unreadable
        // rather than silently `deferred` — including the `error`-less case,
        // which `MemberResult::from_scoped` cannot actually produce
        // ([BR-44], [NFR-CC-04]).
        let indexed = match &status.result {
            Some(info) => Ok(info.indexed),
            None => Err(status
                .error
                .as_deref()
                .unwrap_or("the member reported no index freshness")),
        };
        let warm = warm_state::derive_state(&status.member, indexed, evidence);
        Self { status, warm }
    }
}

/// The `logos workspace status` read-model ([FR-WS-05]): each member's index
/// freshness and warm state, the workspace warm roll-up, the 3-state
/// cross-service coverage summary, and the promoted topic inventory.
#[derive(Debug, Serialize)]
pub struct WorkspaceStatus {
    /// The workspace name (`[workspace] name`).
    pub workspace: String,
    /// Per-member index freshness ([`Engine::status`]) and warm state,
    /// repo-qualified.
    pub members: Vec<MemberStatus>,
    /// The workspace-wide warm roll-up over [`members`](Self::members)
    /// ([FR-WS-15]). Purely derived from the rows above — it can never disagree
    /// with them.
    ///
    /// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
    pub warm_rollup: WarmRollup,
    /// The non-gated 3-state cross-service coverage summary from [S-247]
    /// ([`cross_service_coverage`], [ADR-53]).
    ///
    /// [S-247]: ../coverage/index.html
    pub coverage: CrossServiceCoverage,
    /// Each member's promoted broker topics (S-256, [`workspace_topics`],
    /// [FR-WS-11]).
    ///
    /// Read from the per-repo topic **graph**, not from the cross-member bind, so a
    /// topic one member publishes and nobody consumes yet is still reported — the
    /// service map draws it, and its `consumers: 0` is an honest fact rather than an
    /// absence ([FR-WS-11]).
    ///
    /// [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
    pub topics: Vec<MemberTopics>,
}

/// The workspace **roster** — the manifest, and nothing but the manifest
/// ([FR-WS-06], [NFR-PE-10]).
///
/// Deliberately engine-free. [`workspace_status`] fans out over every member (it
/// reads each one's index freshness and the cross-service coverage), which
/// *constructs and watches every member's engine*. That is right for the coverage
/// dashboard and wrong for the thing the web shell needs on **every page load**:
/// the member names, so it can render its selector. Serving the selector from
/// `workspace_status` would eagerly warm all N members on first paint and undo
/// [NFR-PE-10]'s warm-only-the-default policy.
///
/// This read touches no engine at all: it projects the already-discovered
/// [`Federation`](super::Federation).
#[derive(Debug, Serialize)]
pub struct WorkspaceRoster {
    /// The workspace name (`[workspace] name`).
    pub workspace: String,
    /// The default member's name, when the manifest named one that survived
    /// resolution — the member an unscoped request answers from, so the shell can
    /// open on it rather than guessing at the roster's first entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Every member's repo-qualified name, in manifest order.
    pub members: Vec<String>,
}

/// The engine-free workspace roster ([`WorkspaceRoster`], [FR-WS-06], [NFR-PE-10]).
pub fn workspace_roster<E>(registry: &EngineRegistry<E>) -> WorkspaceRoster
where
    E: super::registry::MemberEngine,
{
    let federation = registry.federation();
    WorkspaceRoster {
        workspace: federation.name.clone(),
        default: federation.default.clone(),
        members: federation.members.iter().map(|m| m.name.clone()).collect(),
    }
}

/// Per-member freshness and warm state, the warm roll-up, the 3-state coverage
/// summary, and the promoted topic inventory ([FR-WS-05], [FR-WS-11],
/// [FR-WS-15]).
///
/// # The warm labelling costs nothing
/// [`WarmEvidence::none`] is passed because no durable per-member warm record
/// exists yet (see [`super::warm_state`]), so the labels are derived
/// **entirely** from the freshness rows the first fan-out already produced. This
/// adds no all-member walk, no engine construction, and no store read, which is
/// what keeps the resident-engine ceiling of a `status` exactly what it was
/// ([NFR-PE-10], [NFR-PE-11]) — asserted at N = 72 in
/// `tests/workspace_connection_budget.rs`. `warming` is consequently absent from
/// the roll-up rather than reported as `0` ([NFR-CC-04]).
///
/// [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
/// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
/// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
pub fn workspace_status(registry: &EngineRegistry<Engine>) -> WorkspaceStatus {
    let evidence = WarmEvidence::none();
    let members: Vec<MemberStatus> = fan_status(registry)
        .into_iter()
        .map(|status| MemberStatus::labelled(status, &evidence))
        .collect();

    WorkspaceStatus {
        workspace: registry.federation().name.clone(),
        warm_rollup: warm_state::rollup(members.iter().map(|m| &m.warm), &evidence),
        members,
        coverage: cross_service_coverage(registry),
        topics: workspace_topics(registry),
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
            intake: crate::federation::bridge::BridgeIntake::Invocation,
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

    // ── warm state on the member row (FR-WS-15, BR-44, NFR-CC-04) ──────────

    /// A member row carrying `indexed` freshness.
    fn fresh(member: &str, indexed: bool) -> MemberResult<StatusInfo> {
        MemberResult {
            member: member.to_string(),
            result: Some(StatusInfo {
                indexed,
                ..StatusInfo::default()
            }),
            error: None,
        }
    }

    /// A member row whose engine failed to start.
    fn unopenable(member: &str, error: &str) -> MemberResult<StatusInfo> {
        MemberResult {
            member: member.to_string(),
            result: None,
            error: Some(error.to_string()),
        }
    }

    /// The freshness row and the warm label serialise into **one flat member
    /// row** — the coherent table [FR-WS-16] extends, not two lists to join.
    #[test]
    fn a_member_row_carries_freshness_and_the_warm_label_in_one_record() {
        let row = MemberStatus::labelled(fresh("api", true), &WarmEvidence::none());
        let value = serde_json::to_value(&row).unwrap();

        assert_eq!(value["member"], "api");
        assert_eq!(value["result"]["indexed"], true);
        assert_eq!(value["warm_state"], "warm", "one row, both facts: {value}");
    }

    /// The member row's **exact key set**, pinned against literals.
    ///
    /// `MemberStatus` flattens two structs into one JSON object, and
    /// `serde_json`'s flatten merge is last-write-wins and **silent**: a key
    /// emitted by both halves produces no compile error, no runtime error and no
    /// failing test — one value simply disappears. [FR-WS-16] extends this same
    /// row in [S-326], and the half most likely to want a `reason` is precisely
    /// the degraded half, so the collision surface is real and imminent. Pinning
    /// the key set turns a future collision into a failing test instead of a
    /// dropped field.
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [S-326]: ../../../docs/planning/journal.md#s-326-degraded-member-reporting-and-non-zero-exit-for-workspace-commands
    #[test]
    fn a_member_row_has_exactly_the_keys_it_is_meant_to() {
        let keys = |row: &MemberStatus| {
            let value = serde_json::to_value(row).unwrap();
            let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };

        let healthy = MemberStatus::labelled(fresh("api", true), &WarmEvidence::none());
        assert_eq!(
            keys(&healthy),
            ["member", "result", "warm_state"],
            "a healthy row: no `error`, no `reason`, nothing shadowed"
        );

        let degraded =
            MemberStatus::labelled(unopenable("web", "store is corrupt"), &WarmEvidence::none());
        assert_eq!(
            keys(&degraded),
            ["error", "member", "reason", "warm_state"],
            "a degraded row: no `result`, and `error`/`reason` both survive the \
             double flatten"
        );
    }

    /// Index presence is the whole derivation for the two derivable states: an
    /// indexed member is `warm`, an empty one `deferred`.
    #[test]
    fn index_presence_decides_warm_versus_deferred() {
        let evidence = WarmEvidence::none();
        assert_eq!(
            MemberStatus::labelled(fresh("api", true), &evidence).warm,
            MemberWarmState::Warm
        );
        assert_eq!(
            MemberStatus::labelled(fresh("web", false), &evidence).warm,
            MemberWarmState::Deferred
        );
    }

    /// [BR-44]: a member whose open was attempted and **failed** reports
    /// `degraded` carrying the fan-out's own reason — never `deferred`, which
    /// would claim it was never attempted.
    ///
    /// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[test]
    fn an_unopenable_member_is_degraded_not_deferred() {
        let row = MemberStatus::labelled(
            unopenable("web", "starting the engine for workspace member \"web\""),
            &WarmEvidence::none(),
        );

        assert!(matches!(row.warm, MemberWarmState::Degraded { .. }));
        assert_ne!(row.warm, MemberWarmState::Deferred);
        let value = serde_json::to_value(&row).unwrap();
        assert_eq!(value["warm_state"], "degraded");
        assert!(
            value["reason"].as_str().unwrap().contains("web"),
            "the fan-out's reason rides the label: {value}"
        );
        // The freshness channel is untouched — the row still reports the error
        // it always did, so no existing reader loses anything.
        assert!(value["error"].as_str().is_some());
    }

    /// The finding this fix closes: a member whose engine STARTED but whose
    /// **freshness read failed** must land in the `error` channel — and so read
    /// `degraded` — not arrive as a defaulted `StatusInfo` whose `indexed:
    /// false` would read `deferred` ("never attempted", [BR-44]).
    ///
    /// [`Engine::status`] hides exactly that case by design (ADR-14 degradation),
    /// which is why [`fan_status`] fans [`Engine::try_status`]. Asserted over
    /// [`flatten_status`] rather than a corrupted on-disk store, so all three
    /// channels are pinned deterministically.
    ///
    /// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[test]
    fn a_failed_freshness_read_becomes_an_error_channel_not_an_empty_graph() {
        let scoped = |value| MemberScoped {
            member: "api".to_string(),
            value,
        };

        // Inner Err — the engine started, `navigate::status` failed. THE case.
        let row = flatten_status(scoped(Ok(Err(anyhow::anyhow!("no such table: edges")))));
        assert!(
            row.result.is_none(),
            "a failed read must not surface as a defaulted, apparently-empty graph"
        );
        assert_eq!(row.error.as_deref(), Some("no such table: edges"));
        assert!(
            matches!(
                MemberStatus::labelled(row, &WarmEvidence::none()).warm,
                MemberWarmState::Degraded { .. }
            ),
            "BR-44: the read was attempted and failed"
        );

        // Outer Err — the engine never started. Still degraded, as before.
        let row = flatten_status(scoped(Err(anyhow::anyhow!("starting the engine"))));
        assert!(row.result.is_none());
        assert_eq!(row.error.as_deref(), Some("starting the engine"));

        // Ok — the read succeeded and reports an honestly empty graph.
        let row = flatten_status(scoped(Ok(Ok(StatusInfo::default()))));
        assert!(row.error.is_none(), "a successful read carries no error");
        assert_eq!(
            MemberStatus::labelled(row, &WarmEvidence::none()).warm,
            MemberWarmState::Deferred,
            "an honestly empty graph is deferred, which is the whole distinction"
        );
    }

    /// A row with neither channel populated (unreachable through
    /// `from_scoped`, but representable) reads `degraded`, not `deferred`:
    /// missing freshness is not evidence that nothing was attempted
    /// ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn a_row_with_no_freshness_at_all_is_degraded_not_deferred() {
        let row = MemberStatus::labelled(
            MemberResult {
                member: "ghost".to_string(),
                result: None,
                error: None,
            },
            &WarmEvidence::none(),
        );
        assert!(matches!(row.warm, MemberWarmState::Degraded { .. }));
    }

    /// The roll-up is derived from the very rows it accompanies, so the two can
    /// never disagree — and with no live signal the `warming` key is absent
    /// from the roll-up entirely ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn the_rollup_agrees_with_the_rows_and_omits_warming() {
        let evidence = WarmEvidence::none();
        let rows: Vec<MemberStatus> = vec![
            MemberStatus::labelled(fresh("api", true), &evidence),
            MemberStatus::labelled(fresh("web", true), &evidence),
            MemberStatus::labelled(fresh("svc", false), &evidence),
            MemberStatus::labelled(unopenable("old", "store is corrupt"), &evidence),
        ];
        let rollup = warm_state::rollup(rows.iter().map(|r| &r.warm), &evidence);

        assert_eq!(rollup.members, rows.len());
        assert_eq!((rollup.warm, rollup.deferred, rollup.degraded), (2, 1, 1));

        let value = serde_json::to_value(&rollup).unwrap();
        assert!(
            value.get("warming").is_none(),
            "no live signal ⇒ no warming key, not a fabricated 0: {value}"
        );

        // The roll-up really is a projection of the rows, not a parallel count.
        let count = |want: &MemberWarmState| {
            rows.iter()
                .filter(|r| std::mem::discriminant(&r.warm) == std::mem::discriminant(want))
                .count()
        };
        assert_eq!(count(&MemberWarmState::Warm), rollup.warm);
        assert_eq!(count(&MemberWarmState::Deferred), rollup.deferred);
        assert_eq!(
            count(&MemberWarmState::Degraded {
                reason: String::new()
            }),
            rollup.degraded
        );
        assert_eq!(
            count(&MemberWarmState::Warming),
            0,
            "the vocabulary is present but `warming` is unreachable today"
        );
    }
}
