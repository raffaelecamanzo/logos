//! The read-only `rig` tool layer (S-167, [agent-core], [ADR-41]).
//!
//! Three domains, each owned by a specialized subagent (S-174):
//!
//! - **graph** — `search` / `context` / `node` / `callers` / `callees` /
//!   `impact` / `explore` / `affected`, each wrapping an existing
//!   [`Engine`](logos_core::Engine) navigation read-model;
//! - **governance** — `scan` / `check_rules` / `hotspots` / `dsm` / `gate` /
//!   `evolution` / `doc_gaps` / `health`, each wrapping an existing
//!   governance/quality read-model;
//! - **source** — net-new, path-sandboxed `read` / `grep` / `glob` confined to
//!   the project root and honoring `ignored_dirs` ([NFR-SE-04]).
//!
//! Under a **federated** backing only, the Graph-Navigator's graph set is joined
//! by the four read-only `xservice` tools (S-431, [FR-WS-29]) over the
//! workspace member registry — an addition to the graph domain, not a fourth
//! domain: no subagent owns them alone, and a single-root roster never sees them
//! ([ADR-52]).
//!
//! The **workspace chat** (S-480, [FR-WS-34]) reaches the same three domains
//! through [`addressed_toolset`]: each member tool with a required `repo`,
//! resolved per call to that member's engine or sandbox through the registry,
//! leaving the member toolsets above untouched. Beside them sit the workspace
//! read-model tools of [`workspace_toolset`] — `workspace_status`,
//! `workspace_reachability`, `workspace_check`, `xservice_build_deps` and
//! `workspace_roster`.
//!
//! Every Engine-backed tool is a thin adapter ([ADR-01]): it deserializes its
//! arguments, runs **one** existing read-model on the blocking pool
//! ([`run_engine`] / [`run_engine_result`], the ADR-03 submit-and-await
//! bridge), and serializes the read-model back. The substrate adds **no new
//! core query** — the tools call only methods the CLI/MCP surfaces already
//! expose.
//!
//! The [`ToolDomain`] enum names each domain's exact tool subset and builds the
//! matching `rig` [`ToolSet`](rig_core::tool::ToolSet); the bounded-dispatch
//! primitive in [`budget`] gates a set by a [`ToolBudget`](budget::ToolBudget).
//!
//! [agent-core]: ../../../docs/specs/architecture/components/agent-core.md
//! [NFR-SE-04]: ../../../docs/specs/requirements/NFR-SE-04.md
//! [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
//! [ADR-03]: ../../../docs/specs/architecture/decisions/ADR-03.md
//! [ADR-41]: the `rig` decision + tool layer + budget primitives.
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
//! [FR-WS-29]: ../../../docs/specs/requirements/FR-WS-29.md
//! [FR-WS-34]: ../../../docs/specs/requirements/FR-WS-34.md

use std::sync::Arc;

use logos_core::Engine;
use rig_core::completion::ToolDefinition;
use rig_core::tool::ToolSet;

mod addressed;
pub mod budget;
mod governance;
mod graph;
mod source;
mod workspace;
mod xservice;

pub use budget::{BoundedDispatcher, BudgetExhausted, DispatchError, ToolBudget};
pub use source::{Sandbox, SandboxError, MAX_FOLLOWED_LINKS};
pub use workspace::{workspace_reading, WORKSPACE_TOOL_NAMES};
pub use xservice::{xservice_reading, XserviceAnswer, XserviceBacking, XSERVICE_TOOL_NAMES};

/// The error every Engine-backed tool surfaces.
///
/// `rig`'s [`Tool::Error`](rig_core::tool::Tool::Error) must be a concrete
/// [`std::error::Error`]; `anyhow::Error` is not one, so the fallible
/// governance read-models are mapped through this enum. The navigation
/// read-models are infallible (they fold failures into a `warnings` field), so
/// only the argument-parsing and runtime arms ever fire for them.
#[derive(Debug, thiserror::Error)]
pub enum ToolCallError {
    /// A caller-supplied argument was malformed (e.g. an unknown node-kind or
    /// dsm-granularity token); names the valid set so the model can retry.
    #[error("{0}")]
    InvalidArgument(String),

    /// The blocking bridge could not run the call to completion (the worker
    /// task was cancelled or panicked) — a runtime fault, never a fabricated
    /// result ([NFR-CC-04]).
    #[error("the agent tool runtime failed to complete the call: {0}")]
    Runtime(String),

    /// A structural failure inside a governance/quality read-model (store
    /// fault, invalid `rules.toml`) — or, for a repo-addressed tool, the
    /// addressed member's engine failing to start or its `config.toml` failing
    /// to load.
    #[error("{0:#}")]
    Engine(#[source] anyhow::Error),
}

/// Run an **infallible** `Engine` read-model on the blocking pool (ADR-03).
///
/// The navigation read-models (`search`, `node`, …) return their value
/// directly — failures ride inside the read-model's `warnings`. `spawn_blocking`
/// keeps the synchronous core off `rig`'s async reactor, the same discipline
/// the mcp/web surfaces use; the core's own pools still own op concurrency
/// (ADR-02), so this only parks the submit-and-await, it is not the bridge
/// ADR-03 rejected.
async fn run_engine<T, F>(engine: Arc<Engine>, call: F) -> Result<T, ToolCallError>
where
    T: Send + 'static,
    F: FnOnce(&Engine) -> T + Send + 'static,
{
    tokio::task::spawn_blocking(move || in_chat_surface(|| call(&engine)))
        .await
        .map_err(|err| ToolCallError::Runtime(err.to_string()))
}

/// Attribute everything `call` emits to [`Surface::Chat`] ([FR-OB-10]).
///
/// This function — together with its twin in [`run_engine_result`] and the
/// federated `run_federated` in `xservice` (which the addressed tools' member
/// resolution also goes through) — is the **entire** chat-surface seam: the
/// three places every agent tool reaches an engine through.
/// Resolution therefore happens once per tool call at this adapter boundary,
/// never inside a chokepoint, so the engine stays unaware the agent exists
/// ([ADR-01]) and the hot path is unchanged ([NFR-OO-02]).
///
/// It must run *inside* the `spawn_blocking` closure, not around the `await`:
/// [`logos_core::observability::in_surface`] scopes per thread, and the blocking
/// pool is where the engine — and so the telemetry event — actually runs.
///
/// # This hardcodes *which* agent, and that is a real assumption
///
/// [`ToolDomain`] partitions the **subagent roster** (S-174), so today this
/// module is the chat agent's tool layer and nothing else reaches it —
/// `wiki-agent` consumes `agent-core`'s provider substrate, not its tools, and
/// carries no `ToolSet` at all. That is the only reason a fixed
/// [`Surface::Chat`] is correct here rather than a surface threaded from the
/// caller.
///
/// A second consumer would silently inherit the wrong attribution, which is the
/// exact conflation [FR-OB-10] exists to prevent — reporting one agent's work as
/// another's. So if a tool-bearing agent is ever added, do not reuse this: give
/// the bridges a surface parameter and let each caller name itself.
///
/// Without it the agent's queries carry the process surface, `web`, and are
/// indistinguishable from a human browsing the dashboard. *"Logos's own agent
/// navigated the graph N times"* and *"a developer did"* are different claims,
/// and a figure that silently sums them answers neither ([NFR-CC-04]).
///
/// [FR-OB-10]: ../../../docs/specs/requirements/FR-OB-10.md
/// [NFR-OO-02]: ../../../docs/specs/requirements/NFR-OO-02.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
fn in_chat_surface<T>(call: impl FnOnce() -> T) -> T {
    logos_core::observability::in_surface(logos_core::observability::Surface::Chat, call)
}

/// Run a **fallible** `Engine` read-model on the blocking pool (ADR-03).
///
/// The governance/quality read-models return `anyhow::Result<T>`: a structural
/// failure maps to [`ToolCallError::Engine`], a worker-task fault to
/// [`ToolCallError::Runtime`] — the run halts honestly either way, never
/// fabricating a tool result ([NFR-CC-04]).
async fn run_engine_result<T, F>(engine: Arc<Engine>, call: F) -> Result<T, ToolCallError>
where
    T: Send + 'static,
    F: FnOnce(&Engine) -> anyhow::Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(move || in_chat_surface(|| call(&engine))).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(ToolCallError::Engine(err)),
        Err(err) => Err(ToolCallError::Runtime(err.to_string())),
    }
}

/// The three least-privilege tool domains the subagent roster partitions over
/// (S-174). Each subagent is built from exactly one domain's [`ToolSet`] — with
/// one extension: under a federated backing the Graph-Navigator's set is
/// [`ToolDomain::Graph`] followed by [`XSERVICE_TOOL_NAMES`]
/// ([`xservice_toolset`], S-431). No other role gains a tool, and a single-root
/// roster is exactly its one domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDomain {
    /// Code-graph navigation tools (Graph-Navigator subagent).
    Graph,
    /// Architecture-governance / quality tools (Governance-Analyst subagent).
    Governance,
    /// Sandboxed filesystem source tools (Source-Reader subagent).
    Source,
}

impl ToolDomain {
    /// Every domain, for exhaustive iteration in partition tests.
    pub const ALL: [ToolDomain; 3] = [
        ToolDomain::Graph,
        ToolDomain::Governance,
        ToolDomain::Source,
    ];

    /// The exact tool-name subset this domain exposes, in registration order.
    ///
    /// These are the `Tool::NAME` constants of the domain's tools; the partition
    /// suite asserts each built [`ToolSet`] contains exactly this subset. The
    /// federated Graph-Navigator's additions are [`XSERVICE_TOOL_NAMES`], not a
    /// fourth domain.
    pub const fn tool_names(self) -> &'static [&'static str] {
        match self {
            ToolDomain::Graph => &[
                graph::Search::NAME,
                graph::Context::NAME,
                graph::Node::NAME,
                graph::Callers::NAME,
                graph::Callees::NAME,
                graph::Impact::NAME,
                graph::Explore::NAME,
                graph::Affected::NAME,
            ],
            ToolDomain::Governance => &[
                governance::Scan::NAME,
                governance::CheckRules::NAME,
                governance::Hotspots::NAME,
                governance::Dsm::NAME,
                governance::Gate::NAME,
                governance::Evolution::NAME,
                governance::DocGaps::NAME,
                governance::Health::NAME,
            ],
            ToolDomain::Source => &[
                source::Read::NAME,
                source::Grep::NAME,
                source::Glob::NAME,
            ],
        }
    }
}

impl ToolDomain {
    /// This domain's member-tool definitions, in [`tool_names`](Self::tool_names)
    /// order — what the domain's toolset registers, read without the engine or
    /// sandbox it wraps, so [`addressed_toolset`] can derive its definitions
    /// before any member is resolved.
    fn member_definitions(self) -> Vec<ToolDefinition> {
        match self {
            ToolDomain::Graph => vec![
                graph::Search::tool_definition(),
                graph::Context::tool_definition(),
                graph::Node::tool_definition(),
                graph::Callers::tool_definition(),
                graph::Callees::tool_definition(),
                graph::Impact::tool_definition(),
                graph::Explore::tool_definition(),
                graph::Affected::tool_definition(),
            ],
            ToolDomain::Governance => vec![
                governance::Scan::tool_definition(),
                governance::CheckRules::tool_definition(),
                governance::Hotspots::tool_definition(),
                governance::Dsm::tool_definition(),
                governance::Gate::tool_definition(),
                governance::Evolution::tool_definition(),
                governance::DocGaps::tool_definition(),
                governance::Health::tool_definition(),
            ],
            ToolDomain::Source => vec![
                source::Read::tool_definition(),
                source::Grep::tool_definition(),
                source::Glob::tool_definition(),
            ],
        }
    }
}

// `Tool::NAME` is a trait const; bring the trait into scope so the
// `tool_names` table above can name the constants.
use rig_core::tool::Tool;

/// Build the **graph** domain's `rig` [`ToolSet`] over a shared [`Engine`].
pub fn graph_toolset(engine: Arc<Engine>) -> ToolSet {
    ToolSet::builder()
        .static_tool(graph::Search::new(engine.clone()))
        .static_tool(graph::Context::new(engine.clone()))
        .static_tool(graph::Node::new(engine.clone()))
        .static_tool(graph::Callers::new(engine.clone()))
        .static_tool(graph::Callees::new(engine.clone()))
        .static_tool(graph::Impact::new(engine.clone()))
        .static_tool(graph::Explore::new(engine.clone()))
        .static_tool(graph::Affected::new(engine))
        .build()
}

/// Build the **governance** domain's `rig` [`ToolSet`] over a shared [`Engine`].
pub fn governance_toolset(engine: Arc<Engine>) -> ToolSet {
    ToolSet::builder()
        .static_tool(governance::Scan::new(engine.clone()))
        .static_tool(governance::CheckRules::new(engine.clone()))
        .static_tool(governance::Hotspots::new(engine.clone()))
        .static_tool(governance::Dsm::new(engine.clone()))
        .static_tool(governance::Gate::new(engine.clone()))
        .static_tool(governance::Evolution::new(engine.clone()))
        .static_tool(governance::DocGaps::new(engine.clone()))
        .static_tool(governance::Health::new(engine))
        .build()
}

/// Build the four read-only `xservice_*` tools over a federated backing
/// (S-431, [FR-WS-29]) — composed onto [`graph_toolset`] for the Graph-Navigator,
/// in [`XSERVICE_TOOL_NAMES`] order, and only when an [`XserviceBacking`] exists,
/// which it never does under a single root ([ADR-52]).
///
/// [FR-WS-29]: ../../../docs/specs/requirements/FR-WS-29.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
pub fn xservice_toolset(xs: XserviceBacking) -> ToolSet {
    ToolSet::builder()
        .static_tool(xservice::XserviceRouteProvidersTool::new(xs.clone()))
        .static_tool(xservice::XserviceCallersTool::new(xs.clone()))
        .static_tool(xservice::XserviceImpactTool::new(xs.clone()))
        .static_tool(xservice::XserviceSearchTool::new(xs))
        .build()
}

/// Build the **source** domain's `rig` [`ToolSet`] over a [`Sandbox`].
pub fn source_toolset(sandbox: Arc<Sandbox>) -> ToolSet {
    ToolSet::builder()
        .static_tool(source::Read::new(sandbox.clone()))
        .static_tool(source::Grep::new(sandbox.clone()))
        .static_tool(source::Glob::new(sandbox))
        .build()
}

/// Build `domain`'s tools **repo-addressed** over a federated backing (S-480,
/// [FR-WS-34]) — in [`ToolDomain::tool_names`] order, each the member tool's
/// definition plus a required `repo`.
///
/// Constructs nothing: a call resolves the addressed member's engine (or, for
/// [`ToolDomain::Source`], its sandbox) through the registry when it runs, so
/// building this set leaves the registry's resident count where it was
/// ([NFR-PE-10]).
///
/// [FR-WS-34]: ../../../docs/specs/requirements/FR-WS-34.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
pub fn addressed_toolset(domain: ToolDomain, xs: XserviceBacking) -> ToolSet {
    let tools = domain
        .member_definitions()
        .into_iter()
        .map(|member| addressed::AddressedTool::new(domain, member, xs.clone()))
        .collect();
    ToolSet::from_tools(tools)
}

/// Build the five workspace read-model tools over a federated backing (S-480,
/// [FR-WS-34]), in [`WORKSPACE_TOOL_NAMES`] order: each runs the read-model its
/// MCP (or, for `workspace_roster`, HTTP) twin runs and returns it beside a
/// deterministic `reading`.
///
/// [FR-WS-34]: ../../../docs/specs/requirements/FR-WS-34.md
pub fn workspace_toolset(xs: XserviceBacking) -> ToolSet {
    ToolSet::builder()
        .static_tool(workspace::WorkspaceStatusTool::new(xs.clone()))
        .static_tool(workspace::WorkspaceReachabilityTool::new(xs.clone()))
        .static_tool(workspace::WorkspaceCheckTool::new(xs.clone()))
        .static_tool(workspace::XserviceBuildDepsTool::new(xs.clone()))
        .static_tool(workspace::WorkspaceRosterTool::new(xs))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use logos_core::observability::Surface;

    /// Both engine bridges install the [`Surface::Chat`] boundary scope
    /// ([FR-OB-10]), and they install it **inside** the blocking closure — where
    /// the engine (and so the telemetry event) actually runs.
    ///
    /// Asserting it here rather than in `logos-core` is the point: the core
    /// proves the scope *works*, this proves the chat agent *enters* it. The
    /// probe closure stands in for a real tool body, which is exactly what these
    /// bridges hand to the engine.
    ///
    /// [FR-OB-10]: ../../../docs/specs/requirements/FR-OB-10.md
    #[tokio::test]
    async fn both_engine_bridges_run_under_the_chat_surface() {
        let dir = tempfile::tempdir().expect("temp project root");
        let engine = Arc::new(Engine::open(dir.path()));

        let seen = run_engine(engine.clone(), |_| {
            logos_core::observability::current_surface_override()
        })
        .await
        .expect("the infallible bridge completes");
        assert_eq!(
            seen,
            Some(Surface::Chat),
            "run_engine attributes the call to the chat surface"
        );

        let seen = run_engine_result(engine, |_| {
            Ok(logos_core::observability::current_surface_override())
        })
        .await
        .expect("the fallible bridge completes");
        assert_eq!(
            seen,
            Some(Surface::Chat),
            "run_engine_result attributes the call to the chat surface"
        );

        assert_eq!(
            logos_core::observability::current_surface_override(),
            None,
            "and the scope does not leak back to the caller's thread"
        );
    }
}
