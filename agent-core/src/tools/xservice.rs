//! The **xservice** tool domain (S-431, [FR-WS-29]): four read-only `rig` tools
//! over the federated query backing — `xservice_route_providers`,
//! `xservice_callers`, `xservice_impact`, `xservice_search` — each a thin adapter
//! over one [`federation::query`](logos_core::federation::query) read-model, the
//! same functions the MCP surface composes into its `xservice_*` router
//! ([mcp-surface], [FR-WS-05]). No new core query.
//!
//! # Only under a federated backing ([ADR-52])
//! Every tool holds an [`XserviceBacking`], which [`XserviceBacking::federated`]
//! mints **only** over [`Backing::Federated`]: a single-root backing yields
//! `None`, so there is nothing to build the toolset from. Under any backing the
//! tools belong to the workspace roster's Workspace-Analyst (S-481), never to a
//! member roster.
//!
//! # Lazy, like the registry it wraps ([NFR-PE-10])
//! Constructing the tools touches no engine. A member is started only when a
//! dispatched call reaches it, and the resident-engine ceiling stays the
//! registry's own budget. A `repo`-scoped `xservice_search` starts that one
//! member, and so does the per-member tier of a `repo`-scoped `xservice_callers`
//! (its intra-repo fan-out) or `xservice_impact` (its seed); unscoped, those
//! tiers open every member. All three bridge-backed tools also read the
//! contract bridge, exactly as `logos xservice …` and the MCP tools do: its
//! **first** answer reads every member (an edge binds the *sole* provider of a
//! key, which only every member's surface can establish); after that a read
//! checks the sync-stamps of the members that can have changed — the resident
//! ones — opens any member not opened yet, or that failed to open, or whose
//! store file has gone, and starts no other for the check; when one of those
//! stamps has moved, the read recomputes over every member (S-484,
//! `ContractBridge::edges_read`). Each answer names the members it read in
//! `member_reads`, beside the read-model; the `reading` line is unchanged by it.
//!
//! # The residue rides the answer ([BR-53], [NFR-CC-04])
//! Each output is an [`XserviceAnswer`]: the read-model verbatim, plus one
//! deterministic `reading` line composed from it. The reading is repo-qualified
//! (`member:name`, so a symbol present in two members is two entries, never one)
//! and — for `callers`/`impact` — carries the unresolved egress residue: an
//! **empty** answer over a **non-zero** residue reads `UNRESOLVED`, naming the
//! count and its reasons through the residue's own composed
//! [`summary`](logos_core::federation::EgressResidue::summary); over a zero
//! residue it stands unqualified. The roster lifts that line out of the tool
//! output with [`xservice_reading`] and carries it into the observation the
//! planner and Synthesizer read, so the qualification does not depend on the
//! model choosing to repeat it.
//!
//! [FR-WS-29]: ../../../docs/specs/requirements/FR-WS-29.md
//! [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
//! [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [BR-53]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [mcp-surface]: ../../../docs/specs/architecture/components/mcp-surface.md

use std::sync::Arc;

use logos_core::federation::query::{
    self, MemberResult, XserviceCallers, XserviceImpact, XserviceRouteProviders, XserviceSearch,
};
use logos_core::federation::{
    Backing, BridgeEdge, BridgeEndpoint, BuildDependencies, ContractBridge, EgressResidue,
    EngineRegistry,
};
use logos_core::model::LogosSymbol;
use logos_core::models::SymbolRef;
use logos_core::Engine;
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::graph::parse_kind;
use super::ToolCallError;

/// How many repo-qualified entries a `reading` names per list before it
/// summarizes the rest as `+N more`. The full list is in the read-model beside
/// it; the reading only has to be an honest, bounded index into it.
const READING_ENTRIES: usize = 5;

/// The federated query backing the `xservice_*` tools run over: the member
/// registry and the workspace's cached [`ContractBridge`] ([FR-WS-04]).
///
/// Only constructible over [`Backing::Federated`] ([`federated`](Self::federated)),
/// which is what makes "compose the tools only when a federated backing is
/// supplied" structural rather than a check someone has to remember.
///
/// The same backing carries the workspace read-model tools and the
/// repo-addressed member tools (S-480): one value, so the workspace roster's
/// tools all stitch over one bridge and resolve members through one registry.
///
/// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
#[derive(Clone)]
pub struct XserviceBacking {
    pub(super) backing: Arc<Backing<Engine>>,
    pub(super) bridge: Arc<ContractBridge>,
    /// The build-dependency relation `xservice_build_deps` reads, joined on its
    /// first query and cached on member sync-stamps ([FR-WS-33]) — this
    /// backing's own cache unless [`with_build_deps`](Self::with_build_deps)
    /// shares a surface's, as the MCP server holds its own.
    ///
    /// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
    pub(super) build_deps: Arc<BuildDependencies>,
}

impl XserviceBacking {
    /// The federated backing, or `None` under [`Backing::Single`] — a single root
    /// has no registry to fan over and gets no `xservice_*` tools ([ADR-52]).
    ///
    /// `bridge` should be the surface's shared bridge, so the chat answer
    /// stitches over the same cached edge set the workspace endpoints do.
    ///
    /// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
    pub fn federated(backing: Arc<Backing<Engine>>, bridge: Arc<ContractBridge>) -> Option<Self> {
        backing.as_federated()?;
        Some(Self {
            backing,
            bridge,
            build_deps: Arc::new(BuildDependencies::new()),
        })
    }

    /// Read the build-dependency relation through `build_deps` rather than a
    /// cache of this backing's own — the surface's shared cache, so the
    /// workspace chat's `xservice_build_deps` and the `/api/v1/workspace/build-deps`
    /// route join the relation once between them ([FR-WS-33]).
    ///
    /// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
    #[must_use]
    pub fn with_build_deps(mut self, build_deps: Arc<BuildDependencies>) -> Self {
        self.build_deps = build_deps;
        self
    }

    /// The member registry every tool on this backing resolves members through
    /// (and tests assert residency on, [NFR-PE-10]). Always present:
    /// [`federated`](Self::federated) refuses a single-root backing.
    ///
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    pub fn registry(&self) -> &EngineRegistry<Engine> {
        self.backing
            .as_federated()
            .expect("XserviceBacking is only ever minted over a federated backing")
    }
}

/// An `xservice_*` tool's output — and a workspace read-model tool's (S-480):
/// one deterministic, repo-qualified `reading` line, then the read-model's own
/// fields verbatim ([FR-WS-05] wire shape).
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
#[derive(Debug, Serialize)]
pub struct XserviceAnswer<T> {
    /// The answer in one line — repo-qualified, and (for `callers`/`impact`)
    /// qualified by the unresolved residue ([BR-53]). Serialized first, so the
    /// model reads it before the detail.
    ///
    /// [BR-53]: ../../../docs/specs/software-spec.md#327-workspace-federation
    pub reading: String,
    /// The read-model, unchanged.
    #[serde(flatten)]
    pub answer: T,
}

/// The `reading` of an `xservice_*` tool's serialized output, or `None` when
/// `tool` is not an `xservice_*` tool (or its output carries no reading).
///
/// The roster calls this on every successful dispatch; the member roster
/// registers no `xservice_*` tool under any backing, so there it never returns
/// `Some` and the observation is byte-for-byte what it was ([ADR-52]).
///
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
pub fn xservice_reading(tool: &str, output: &str) -> Option<String> {
    if !XSERVICE_TOOL_NAMES.contains(&tool) {
        return None;
    }
    reading_of(output)
}

/// The `reading` field of a serialized [`XserviceAnswer`], whichever tool
/// produced it.
pub(super) fn reading_of(output: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(output).ok()?;
    value.get("reading")?.as_str().map(str::to_string)
}

/// The `xservice_*` tool names, in registration order.
pub const XSERVICE_TOOL_NAMES: &[&str] = &[
    XserviceRouteProvidersTool::NAME,
    XserviceCallersTool::NAME,
    XserviceImpactTool::NAME,
    XserviceSearchTool::NAME,
];

/// Run one `xservice_*` read-model on the blocking pool under the chat surface —
/// the federated twin of [`run_engine`](super::run_engine) ([ADR-03],
/// [FR-OB-10]). The registry's fan-out is sequential on this thread, so the
/// surface scope covers every member it reaches.
///
/// [ADR-03]: ../../../docs/specs/architecture/decisions/ADR-03.md
/// [FR-OB-10]: ../../../docs/specs/requirements/FR-OB-10.md
pub(super) async fn run_federated<T, F>(
    xs: XserviceBacking,
    call: F,
) -> Result<T, ToolCallError>
where
    T: Send + 'static,
    F: FnOnce(&EngineRegistry<Engine>, &ContractBridge) -> T + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        super::in_chat_surface(|| call(xs.registry(), &xs.bridge))
    })
    .await
    .map_err(|err| ToolCallError::Runtime(err.to_string()))
}

// ── readings ────────────────────────────────────────────────────────────────

/// `member:name (file:line)` — a hit named with the member that produced it.
fn qualified_ref(member: &str, hit: &SymbolRef) -> String {
    match (&hit.file, hit.line) {
        (Some(file), Some(line)) => format!("{member}:{} ({file}:{line})", hit.name),
        (Some(file), None) => format!("{member}:{} ({file})", hit.name),
        _ => format!("{member}:{}", hit.name),
    }
}

/// `member:symbol` — a bridge endpoint, repo-qualified.
pub(super) fn qualified_endpoint(endpoint: &BridgeEndpoint) -> String {
    format!("{}:{}", endpoint.member, endpoint.symbol.as_str())
}

/// `consumer → provider [relation]` — one resolved cross-service binding.
pub(super) fn edge_line(edge: &BridgeEdge) -> String {
    format!(
        "{} → {} [{}]",
        qualified_endpoint(&edge.from),
        qualified_endpoint(&edge.to),
        edge.relation
    )
}

/// Join at most [`READING_ENTRIES`] entries, naming how many were left out.
pub(super) fn bounded_list(entries: Vec<String>) -> String {
    let total = entries.len();
    let mut shown: Vec<String> = entries.into_iter().take(READING_ENTRIES).collect();
    if total > READING_ENTRIES {
        shown.push(format!("+{} more", total - READING_ENTRIES));
    }
    shown.join("; ")
}

/// One member's slice of a fan-out, rendered `member: <what>` — or its error.
fn member_line<T>(member: &MemberResult<T>, render: impl Fn(&T) -> String) -> String {
    match (&member.result, &member.error) {
        (Some(result), _) => format!("{}: {}", member.member, render(result)),
        (None, Some(error)) => format!("{}: no answer — {error}", member.member),
        (None, None) => format!("{}: no answer", member.member),
    }
}

/// The residue clause of a reachability reading ([BR-53], [NFR-CC-04]).
///
/// `resolved` is the length of the answer's `cross_service` list and `noun`
/// what it counts. The count and the reasons come from the residue's own
/// composed [`summary`](EgressResidue::summary) — the one line the MCP, CLI and
/// web renderings already share — so this adds only the verdict word, never a
/// second composition of the figures.
///
/// [BR-53]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
fn residue_clause(resolved: usize, noun: &str, residue: Option<&EgressResidue>) -> String {
    match (resolved, residue) {
        // The case the whole requirement exists for: silence here would read as
        // "nothing reaches this", and the graph is missing outbound calls.
        (0, Some(residue)) => format!("UNRESOLVED, not an absence — {}", residue.summary),
        // A zero residue is the one case in which silence is honest.
        (0, None) => format!("no {noun}s"),
        // The summary already states the resolved count beside the residue.
        (_, Some(residue)) => format!("incomplete — {}", residue.summary),
        (n, None) => format!("{n} {noun}(s)"),
    }
}

/// Whether `query` is a canonical symbol string — the only form the
/// cross-service tier matches a bridge endpoint on (`federation::query` compares
/// `edge.to.symbol.as_str() == symbol`).
fn is_canonical(query: &str) -> bool {
    LogosSymbol::parse(query).is_ok_and(|symbol| symbol.as_str() == query)
}

/// The cross-service verdict of a `callers`/`impact` reading.
///
/// A bare name (`get_user`, a route template) matches no bridge endpoint, so its
/// empty cross-service tier says nothing about what reaches the symbol: stated
/// as such rather than as "none", even over a zero residue ([BR-53],
/// [NFR-CC-04]). A canonical query takes [`residue_clause`] unchanged.
///
/// [BR-53]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
///
/// Under a `repo` scope the residue is that member's **own** outbound egress
/// ([FR-WS-05]), while the cross-service tier stays workspace-wide — so for a
/// provider's callers the unresolved sites that might reach it live in the
/// *other* members. An empty scoped answer over a zero scoped residue therefore
/// says whose residue it measured, rather than standing as a workspace-wide
/// absence.
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
fn cross_service_verdict(
    query: &str,
    scope: Option<&str>,
    resolved: usize,
    noun: &str,
    residue: Option<&EgressResidue>,
) -> String {
    let mut clause = residue_clause(resolved, noun, residue);
    if let (0, None, Some(member)) = (resolved, residue, scope) {
        clause.push_str(&format!(
            " (residue measured over {member}'s own outbound calls only — unscoped, other \
             members' unresolved calls may reach it)"
        ));
    }
    if resolved == 0 && !is_canonical(query) {
        format!(
            "NOT CHECKED, not an absence — {query:?} is not a canonical symbol, and the \
             cross-service tier matches canonical symbols only; retry with a hit's `symbol` \
             (from xservice_search or xservice_route_providers) [{clause}]"
        )
    } else {
        clause
    }
}

fn read_search(answer: &XserviceSearch) -> String {
    let members: Vec<String> = answer
        .members
        .iter()
        .map(|member| {
            member_line(member, |result| {
                if result.hits.is_empty() {
                    "no hits".to_string()
                } else {
                    let hits = result
                        .hits
                        .iter()
                        .map(|hit| qualified_ref(&member.member, hit))
                        .collect();
                    format!("{} hit(s): {}", result.hits.len(), bounded_list(hits))
                }
            })
        })
        .collect();
    format!(
        "xservice_search {:?} over {} member(s) — {}",
        answer.query,
        answer.members.len(),
        members.join(" | ")
    )
}

fn read_callers(answer: &XserviceCallers) -> String {
    let cross = cross_service_verdict(
        &answer.query,
        answer.scope.as_deref(),
        answer.cross_service.len(),
        "resolved cross-service caller",
        answer.unresolved_egress.as_ref(),
    );
    let edges = answer.cross_service.iter().map(edge_line).collect::<Vec<_>>();
    let intra: Vec<String> = answer
        .members
        .iter()
        .map(|member| {
            member_line(member, |result| {
                let callers = result
                    .callers
                    .iter()
                    .map(|caller| qualified_ref(&member.member, caller))
                    .collect::<Vec<_>>();
                if callers.is_empty() {
                    "0 intra-repo callers".to_string()
                } else {
                    format!("{} intra-repo caller(s): {}", result.total, bounded_list(callers))
                }
            })
        })
        .collect();
    let mut reading = format!("xservice_callers {:?} — cross-service: {cross}", answer.query);
    if !edges.is_empty() {
        reading.push_str(&format!(" ({})", bounded_list(edges)));
    }
    reading.push_str(&format!(" | per member — {}", intra.join(" | ")));
    reading
}

fn read_impact(answer: &XserviceImpact) -> String {
    let cross = cross_service_verdict(
        &answer.query,
        answer.scope.as_deref(),
        answer.cross_service.len(),
        "cross-service impact",
        answer.unresolved_egress.as_ref(),
    );
    let far: Vec<String> = answer
        .cross_service
        .iter()
        .map(|hop| {
            format!(
                "{} via {} ({} upstream, {} downstream)",
                hop.member,
                edge_line(&hop.via),
                hop.impact.upstream.len(),
                hop.impact.downstream.len()
            )
        })
        .collect();
    let seed: Vec<String> = answer
        .seed
        .iter()
        .map(|member| {
            member_line(member, |impact| {
                format!(
                    "{} upstream, {} downstream",
                    impact.upstream.len(),
                    impact.downstream.len()
                )
            })
        })
        .collect();
    let mut reading = format!("xservice_impact {:?} — cross-service: {cross}", answer.query);
    if !far.is_empty() {
        reading.push_str(&format!(" ({})", bounded_list(far)));
    }
    reading.push_str(&format!(" | seed per member — {}", seed.join(" | ")));
    reading
}

fn read_route_providers(answer: &XserviceRouteProviders) -> String {
    let scope = answer
        .scope
        .as_deref()
        .map(|member| format!(" provided by {member}"))
        .unwrap_or_default();
    if answer.providers.is_empty() {
        return format!("xservice_route_providers — no resolved cross-service bindings{scope}");
    }
    format!(
        "xservice_route_providers — {} resolved cross-service binding(s){scope}: {}",
        answer.providers.len(),
        bounded_list(answer.providers.iter().map(edge_line).collect())
    )
}

// ── tool argument shapes (the MCP `xservice_*` wire contracts) ──────────────

/// `xservice_route_providers` arguments.
#[derive(Debug, Deserialize)]
pub struct RouteProvidersArgs {
    /// Scope to routes one member provides.
    #[serde(default)]
    pub repo: Option<String>,
}

/// `xservice_callers` arguments.
#[derive(Debug, Deserialize)]
pub struct XserviceCallersArgs {
    /// Symbol whose cross-service callers to list.
    pub symbol: String,
    /// Maximum intra-repo callers per member (default 50).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Scope the intra-repo fan-out to one member.
    #[serde(default)]
    pub repo: Option<String>,
}

/// `xservice_impact` arguments.
#[derive(Debug, Deserialize)]
pub struct XserviceImpactArgs {
    /// Symbol whose cross-service impact to trace.
    pub symbol: String,
    /// Traversal depth bound per member (default 3).
    #[serde(default)]
    pub depth: Option<usize>,
    /// Scope the seed impact to one member.
    #[serde(default)]
    pub repo: Option<String>,
}

/// `xservice_search` arguments.
#[derive(Debug, Deserialize)]
pub struct XserviceSearchArgs {
    /// Symbol name or free text.
    pub query: String,
    /// Optional node-kind filter.
    #[serde(default)]
    pub kind: Option<String>,
    /// Maximum hits per member (default 20).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Scope to one member.
    #[serde(default)]
    pub repo: Option<String>,
}

/// What a `repo` scope costs a tool, stated in its schema ([NFR-PE-10]).
///
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
#[derive(Clone, Copy)]
enum RepoCost {
    /// `xservice_search`: only the scoped member's engine is started.
    OpensOne,
    /// `xservice_callers` / `xservice_impact`: the per-member tier opens only
    /// the scoped member, and the bridge reads what it reads either way.
    TierAndBridge,
    /// `xservice_route_providers`: no per-member tier, so the scope narrows the
    /// answer and changes nothing that is read.
    BridgeOnly,
}

/// What the bridge reads, whatever `repo` says — one sentence shared by the
/// bridge-backed tools' schemas.
const BRIDGE_READS: &str = "the cross-service bridge reads every member on its first \
    answer and, after that, only the members that can have changed (every member again \
    when one has), and `member_reads` names what was read";

/// The `repo` property every `xservice_*` schema carries, with what scoping
/// costs that tool.
fn repo_property(scoped: &str, cost: RepoCost) -> serde_json::Value {
    let cost = match cost {
        RepoCost::OpensOne => "which starts only that member's engine".to_string(),
        RepoCost::TierAndBridge => format!("which also opens only that member for this tier; {BRIDGE_READS}"),
        RepoCost::BridgeOnly => format!("which narrows the answer but not what is read: {BRIDGE_READS}"),
    };
    json!({
        "type": "string",
        "description": format!(
            "Workspace member name (its workspace-relative path). Scopes {scoped} to that \
             one member, {cost}; omit to fan across the whole workspace."
        )
    })
}

/// The `symbol` argument of the reachability tools: the cross-service tier
/// matches the canonical symbol string exactly, never a bare name.
const SYMBOL_ARGUMENT: &str = "The CANONICAL symbol string — the `symbol` field of an \
    xservice_search / search hit, or an endpoint of xservice_route_providers. A bare name \
    matches no cross-service edge (the reading then says NOT CHECKED).";

/// What every reachability tool tells the model about the residue it returns.
const RESIDUE_CONTRACT: &str = "The `reading` field states the answer in one \
    repo-qualified line. An EMPTY `cross_service` list with an `unresolved_egress` \
    block is NOT an absence: the question was answered over a graph missing that many \
    outbound calls, and the reading says UNRESOLVED — report it that way, with the \
    count. Results name their member; the same symbol in two members is two results.";

// ── xservice_route_providers ────────────────────────────────────────────────

/// The resolved cross-service route bindings.
#[derive(Clone)]
pub struct XserviceRouteProvidersTool {
    xs: XserviceBacking,
}

impl XserviceRouteProvidersTool {
    /// Wrap the federated backing.
    pub fn new(xs: XserviceBacking) -> Self {
        Self { xs }
    }
}

impl Tool for XserviceRouteProvidersTool {
    const NAME: &'static str = "xservice_route_providers";
    type Error = ToolCallError;
    type Args = RouteProvidersArgs;
    type Output = XserviceAnswer<XserviceRouteProviders>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Cross-service resolved bindings across the workspace: each \
                 consumer endpoint → the provider endpoint it binds (route, gRPC call, \
                 broker topic), both repo-qualified (member, symbol). `repo` scopes to \
                 bindings that member provides."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "repo": repo_property("the bindings", RepoCost::BridgeOnly) }
            }),
        }
    }

    async fn call(&self, args: RouteProvidersArgs) -> Result<Self::Output, ToolCallError> {
        run_federated(self.xs.clone(), move |registry, bridge| {
            let answer =
                query::xservice_route_providers(&query::bridge_read(bridge, registry), args.repo.as_deref());
            XserviceAnswer {
                reading: read_route_providers(&answer),
                answer,
            }
        })
        .await
    }
}

// ── xservice_callers ────────────────────────────────────────────────────────

/// Cross-service callers of a symbol, with the residue the answer could not reach.
#[derive(Clone)]
pub struct XserviceCallersTool {
    xs: XserviceBacking,
}

impl XserviceCallersTool {
    /// Wrap the federated backing.
    pub fn new(xs: XserviceBacking) -> Self {
        Self { xs }
    }
}

impl Tool for XserviceCallersTool {
    const NAME: &'static str = "xservice_callers";
    type Error = ToolCallError;
    type Args = XserviceCallersArgs;
    type Output = XserviceAnswer<XserviceCallers>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: format!(
                "Cross-service callers of a symbol: each member's intra-repo callers plus \
                 the consumers in OTHER members that reach it over a resolved bridge edge, \
                 all repo-qualified. {RESIDUE_CONTRACT}"
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": SYMBOL_ARGUMENT },
                    "limit": { "type": "integer", "minimum": 1, "description": "Maximum intra-repo callers per member (default 50)." },
                    "repo": repo_property("the intra-repo fan-out and the residue", RepoCost::TierAndBridge)
                },
                "required": ["symbol"]
            }),
        }
    }

    async fn call(&self, args: XserviceCallersArgs) -> Result<Self::Output, ToolCallError> {
        run_federated(self.xs.clone(), move |registry, bridge| {
            let inputs = query::reachability_inputs(bridge, registry);
            let answer = query::xservice_callers(
                registry,
                &inputs, &args.symbol,
                args.limit,
                args.repo.as_deref(),
            );
            XserviceAnswer {
                reading: read_callers(&answer),
                answer,
            }
        })
        .await
    }
}

// ── xservice_impact ─────────────────────────────────────────────────────────

/// Cross-service impact of changing a symbol, with its residue.
#[derive(Clone)]
pub struct XserviceImpactTool {
    xs: XserviceBacking,
}

impl XserviceImpactTool {
    /// Wrap the federated backing.
    pub fn new(xs: XserviceBacking) -> Self {
        Self { xs }
    }
}

impl Tool for XserviceImpactTool {
    const NAME: &'static str = "xservice_impact";
    type Error = ToolCallError;
    type Args = XserviceImpactArgs;
    type Output = XserviceAnswer<XserviceImpact>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: format!(
                "Cross-service impact of changing a symbol: its impact in each member in \
                 scope, plus the far member's impact stitched across every bridge edge the \
                 symbol is an endpoint of, all repo-qualified. {RESIDUE_CONTRACT}"
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": SYMBOL_ARGUMENT },
                    "depth": { "type": "integer", "minimum": 1, "description": "Traversal depth bound per member (default 3)." },
                    "repo": repo_property("the seed impact and the residue", RepoCost::TierAndBridge)
                },
                "required": ["symbol"]
            }),
        }
    }

    async fn call(&self, args: XserviceImpactArgs) -> Result<Self::Output, ToolCallError> {
        run_federated(self.xs.clone(), move |registry, bridge| {
            let inputs = query::reachability_inputs(bridge, registry);
            let answer = query::xservice_impact(
                registry,
                &inputs, &args.symbol,
                args.depth,
                args.repo.as_deref(),
            );
            XserviceAnswer {
                reading: read_impact(&answer),
                answer,
            }
        })
        .await
    }
}

// ── xservice_search ─────────────────────────────────────────────────────────

/// Full-text symbol search fanned across the workspace members.
#[derive(Clone)]
pub struct XserviceSearchTool {
    xs: XserviceBacking,
}

impl XserviceSearchTool {
    /// Wrap the federated backing.
    pub fn new(xs: XserviceBacking) -> Self {
        Self { xs }
    }
}

impl Tool for XserviceSearchTool {
    const NAME: &'static str = "xservice_search";
    type Error = ToolCallError;
    type Args = XserviceSearchArgs;
    type Output = XserviceAnswer<XserviceSearch>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Full-text symbol search fanned across the workspace members, \
                 each hit repo-qualified — the same name in two members is two hits, never \
                 one. `repo` scopes to one member; `kind` filters by node kind."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Symbol name or free text to search for." },
                    "kind": { "type": "string", "description": "Optional node-kind filter, e.g. \"function\", \"route\"." },
                    "limit": { "type": "integer", "minimum": 1, "description": "Maximum hits per member (default 20)." },
                    "repo": repo_property("the search", RepoCost::OpensOne)
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: XserviceSearchArgs) -> Result<Self::Output, ToolCallError> {
        let kind = parse_kind(args.kind.as_deref())?;
        run_federated(self.xs.clone(), move |registry, _bridge| {
            let answer = query::xservice_search(
                registry,
                &args.query,
                kind,
                args.limit,
                args.repo.as_deref(),
            );
            XserviceAnswer {
                reading: read_search(&answer),
                answer,
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    //! The readings over hand-built read-models — the residue verdict table and
    //! repo qualification, independent of any fixture workspace.

    use super::*;
    use logos_core::model::{LogosSymbol, NodeKind};
    use logos_core::federation::CrossServiceImpact;
    use logos_core::models::SearchResult;

    fn endpoint(member: &str, symbol: &str) -> BridgeEndpoint {
        BridgeEndpoint {
            member: member.to_string(),
            symbol: LogosSymbol::parse(&format!("local {symbol}")).expect("valid symbol"),
        }
    }

    fn residue(unresolved: u64, summary: &str) -> EgressResidue {
        EgressResidue {
            scope: None,
            members_in_scope: 1,
            measured_sites: unresolved + 1,
            unresolved_sites: unresolved,
            no_provider_in_workspace: 0,
            by_reason: Vec::new(),
            covers_all_members: true,
            summary: summary.to_string(),
        }
    }

    fn hit(name: &str) -> SymbolRef {
        SymbolRef {
            symbol: format!("local {name}"),
            name: name.to_string(),
            kind: NodeKind::Function,
            file: Some("src/lib.rs".to_string()),
            line: Some(1),
        }
    }

    /// [`XserviceBacking::with_build_deps`] replaces the backing's own cache with
    /// the one it is handed — the web surface's, so the workspace chat and the
    /// `/api/v1/workspace/build-deps` route share one join ([FR-WS-33]).
    ///
    /// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
    #[test]
    fn a_shared_build_dependency_cache_replaces_the_backings_own() {
        use logos_core::federation::{EngineRegistry, Federation, RegistryMode};
        let federation = Federation {
            name: "shop".to_string(),
            root: std::path::PathBuf::from("/ws"),
            members: Vec::new(),
            default: None,
            links: Vec::new(),
            governance: Default::default(),
            warm_concurrency: None,
            member_kinds: Default::default(),
        };
        let backing = Arc::new(Backing::Federated(Box::new(EngineRegistry::<Engine>::new(
            federation,
            RegistryMode::Lazy,
        ))));
        let shared = Arc::new(BuildDependencies::new());
        let own = XserviceBacking::federated(Arc::clone(&backing), Arc::new(ContractBridge::new()))
            .expect("federated");
        assert!(!Arc::ptr_eq(&own.build_deps, &shared), "a fresh backing holds its own cache");
        let xs = own.with_build_deps(Arc::clone(&shared));
        assert!(Arc::ptr_eq(&xs.build_deps, &shared), "the shared cache is the one read");
    }

    #[test]
    fn an_empty_answer_over_a_non_zero_residue_reads_unresolved_with_the_core_summary() {
        let summary = "no resolved cross-service callers; 3 of 4 captured outbound sites in \
                       scope did not resolve across 1 member (base-url-runtime 3)";
        let clause = residue_clause(0, "resolved cross-service caller", Some(&residue(3, summary)));
        // The verdict word, then the core's one composed line verbatim — the count
        // and the reasons are the summary's, never recomposed here.
        assert_eq!(clause, format!("UNRESOLVED, not an absence — {summary}"));
    }

    #[test]
    fn an_empty_answer_over_a_zero_residue_stands_unqualified() {
        let clause = residue_clause(0, "resolved cross-service caller", None);
        assert_eq!(clause, "no resolved cross-service callers");
        assert!(!clause.to_lowercase().contains("unresolved"), "{clause}");
    }

    #[test]
    fn a_non_empty_answer_over_a_residue_is_marked_incomplete_not_unresolved() {
        let summary = "2 cross-service impacts; 1 of 3 captured outbound sites in scope did \
                       not resolve across 1 member (ambiguous 1)";
        let clause = residue_clause(2, "cross-service impact", Some(&residue(1, summary)));
        assert_eq!(clause, format!("incomplete — {summary}"));
        assert_eq!(residue_clause(2, "cross-service impact", None), "2 cross-service impact(s)");
    }

    #[test]
    fn a_bare_name_never_reads_as_a_clean_absence() {
        // The near miss: a bare name and a route template are not canonical; the
        // smallest valid symbol is.
        assert!(!is_canonical("get_user"));
        assert!(!is_canonical("GET /users/{user_id}"));
        assert!(is_canonical("local get_user"));

        let bare = cross_service_verdict("get_user", None, 0, "resolved cross-service caller", None);
        assert!(bare.starts_with("NOT CHECKED, not an absence"), "{bare}");
        let canonical =
            cross_service_verdict("local get_user", None, 0, "resolved cross-service caller", None);
        assert_eq!(canonical, "no resolved cross-service callers");
        // A scoped empty answer names whose residue it measured.
        let scoped = cross_service_verdict(
            "local get_user",
            Some("api"),
            0,
            "resolved cross-service caller",
            None,
        );
        assert!(scoped.starts_with("no resolved cross-service callers (residue measured over api's own"), "{scoped}");
        // A resolved answer is never re-labelled, whatever the query looked like.
        assert_eq!(
            cross_service_verdict("get_user", None, 1, "resolved cross-service caller", None),
            "1 resolved cross-service caller(s)"
        );
    }

    #[test]
    fn a_symbol_in_two_members_is_two_repo_qualified_hits() {
        let answer = XserviceSearch {
            query: "shared".to_string(),
            scope: None,
            members: ["api", "web"]
                .into_iter()
                .map(|member| MemberResult {
                    member: member.to_string(),
                    result: Some(SearchResult {
                        query: "shared".to_string(),
                        hits: vec![hit("shared")],
                        suggestions: Vec::new(),
                        warnings: Vec::new(),
                    }),
                    error: None,
                })
                .collect(),
        };
        let reading = read_search(&answer);
        assert!(reading.contains("api:shared (src/lib.rs:1)"), "{reading}");
        assert!(reading.contains("web:shared (src/lib.rs:1)"), "{reading}");
    }

    fn bound_edge() -> BridgeEdge {
        BridgeEdge {
            relation: "route".to_string(),
            from: endpoint("web", "fetch_user"),
            to: endpoint("api", "get_user"),
            intake: logos_core::federation::BridgeIntake::Invocation,
            from_value: logos_core::resolve::binding::Provenance::Literal,
            to_value: logos_core::resolve::binding::Provenance::Literal,
        }
    }

    #[test]
    fn a_non_empty_answer_over_a_residue_cites_its_edges_and_reads_incomplete() {
        let summary = "1 resolved cross-service caller; 2 of 3 captured outbound sites in \
                       scope did not resolve across 1 member (base-url-runtime 2)";
        let callers = XserviceCallers {
            query: "local get_user".to_string(),
            scope: None,
            members: Vec::new(),
            cross_service: vec![bound_edge()],
            via_type_reference: Vec::new(),
            type_reference_unread: Default::default(),
            unresolved_egress: Some(residue(2, summary)),
            member_reads: Default::default(),
        };
        let reading = read_callers(&callers);
        assert!(
            reading.contains(&format!(
                "cross-service: incomplete — {summary} (web:local fetch_user → api:local get_user [route])"
            )),
            "{reading}"
        );

        let impact = XserviceImpact {
            query: "local get_user".to_string(),
            scope: None,
            seed: Vec::new(),
            cross_service: vec![CrossServiceImpact {
                via: bound_edge(),
                member: "web".to_string(),
                impact: logos_core::models::ImpactResult::default(),
            }],
            via_type_reference: Vec::new(),
            type_reference_unread: Default::default(),
            unresolved_egress: None,
            member_reads: Default::default(),
        };
        let reading = read_impact(&impact);
        assert!(
            reading.contains(
                "cross-service: 1 cross-service impact(s) (web via web:local fetch_user → \
                 api:local get_user [route] (0 upstream, 0 downstream))"
            ),
            "{reading}"
        );
    }

    #[test]
    fn route_providers_name_both_endpoints_with_their_member() {
        let answer = XserviceRouteProviders {
            scope: None,
            providers: vec![bound_edge()],
            declared_contracts: None,
            bound_external: None,
            declared_scope_note: None,
            member_reads: Default::default(),
        };
        let reading = read_route_providers(&answer);
        assert!(reading.contains("1 resolved cross-service binding(s)"), "{reading}");
        assert!(
            reading.contains("web:local fetch_user → api:local get_user [route]"),
            "{reading}"
        );
    }

    #[test]
    fn the_reading_is_lifted_only_from_an_xservice_tool() {
        let output = r#"{"reading":"xservice_search \"x\" over 2 member(s) — …","query":"x"}"#;
        assert_eq!(
            xservice_reading("xservice_search", output).as_deref(),
            Some("xservice_search \"x\" over 2 member(s) — …")
        );
        // The near miss: the same field on a non-xservice tool is not lifted.
        assert_eq!(xservice_reading("search", output), None);
        assert_eq!(xservice_reading("xservice_searc", output), None);
        assert_eq!(xservice_reading("xservice_search", "not json"), None);
    }

    #[test]
    fn bounded_lists_name_what_they_leave_out() {
        let list = bounded_list((0..7).map(|i| format!("e{i}")).collect());
        assert_eq!(list, "e0; e1; e2; e3; e4; +2 more");
    }
}
