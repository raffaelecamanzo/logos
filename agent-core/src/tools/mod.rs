//! The read-only `rig` tool layer (S-167, [agent-core], [ADR-41]).
//!
//! Three domains, each owned by a specialized subagent (S-174):
//!
//! - **graph** — `search` / `context` / `node` / `callers` / `callees` /
//!   `impact` / `explore` / `affected`, each wrapping an existing
//!   [`Engine`](logos_core::Engine) navigation read-model;
//! - **governance** — `scan` / `check_rules` / `hotspots` / `test_gaps` /
//!   `dsm` / `gate` / `evolution` / `doc_gaps` / `health`, each wrapping an
//!   existing governance/quality read-model;
//! - **source** — net-new, path-sandboxed `read` / `grep` / `glob` confined to
//!   the project root and honoring `ignored_dirs` ([NFR-SE-04]).
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

use std::sync::Arc;

use logos_core::Engine;
use rig_core::tool::ToolSet;

pub mod budget;
mod governance;
mod graph;
mod source;

pub use budget::{BoundedDispatcher, BudgetExhausted, DispatchError, ToolBudget};
pub use source::{Sandbox, SandboxError};

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
    /// fault, invalid `rules.toml`).
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
    tokio::task::spawn_blocking(move || call(&engine))
        .await
        .map_err(|err| ToolCallError::Runtime(err.to_string()))
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
    match tokio::task::spawn_blocking(move || call(&engine)).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(ToolCallError::Engine(err)),
        Err(err) => Err(ToolCallError::Runtime(err.to_string())),
    }
}

/// The three least-privilege tool domains the subagent roster partitions over
/// (S-174). Each subagent is built from exactly one domain's [`ToolSet`].
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
    /// suite asserts each built [`ToolSet`] contains exactly this subset.
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
                governance::TestGaps::NAME,
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
        .static_tool(governance::TestGaps::new(engine.clone()))
        .static_tool(governance::Dsm::new(engine.clone()))
        .static_tool(governance::Gate::new(engine.clone()))
        .static_tool(governance::Evolution::new(engine.clone()))
        .static_tool(governance::DocGaps::new(engine.clone()))
        .static_tool(governance::Health::new(engine))
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
