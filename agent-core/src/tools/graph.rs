//! The **graph** tool domain (S-167): eight `rig` tools, each wrapping one
//! existing [`Engine`] navigation read-model (FR-NV-01..06) — the Graph-Navigator
//! subagent's least-privilege set (S-174). No new core query: every `call`
//! routes to a method the CLI/MCP surfaces already expose.

use std::sync::Arc;

use logos_core::model::NodeKind;
use logos_core::models::{
    AffectedResult, CalleesResult, CallersResult, ContextBundle, ExploreResult, ImpactResult,
    NodeInfo, SearchResult,
};
use logos_core::Engine;
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::json;

use super::{run_engine, ToolCallError};

/// Resolve the optional node-kind filter against the exact wire names; an
/// unknown token is the caller's fault, surfaced with the valid set so the
/// model can retry (mirrors the MCP `parse_kind`).
fn parse_kind(kind: Option<&str>) -> Result<Option<NodeKind>, ToolCallError> {
    match kind {
        None => Ok(None),
        Some(token) => NodeKind::from_wire(token).map(Some).ok_or_else(|| {
            let valid = NodeKind::ALL
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            ToolCallError::InvalidArgument(format!(
                "unknown node kind {token:?}; valid kinds: {valid}"
            ))
        }),
    }
}

// ── search ──────────────────────────────────────────────────────────────────

/// `search` arguments (FR-NV-01).
#[derive(Debug, Deserialize)]
pub struct SearchArgs {
    /// FTS5 query (symbol name or free text).
    pub query: String,
    /// Optional node-kind filter (e.g. "function", "struct", "route").
    #[serde(default)]
    pub kind: Option<String>,
    /// Maximum hits (default 20).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// FTS5 symbol search over the code graph.
#[derive(Clone)]
pub struct Search {
    engine: Arc<Engine>,
}

impl Search {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for Search {
    const NAME: &'static str = "search";
    type Error = ToolCallError;
    type Args = SearchArgs;
    type Output = SearchResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "FTS5 full-text symbol search over the code graph, \
                 optionally filtered by node kind. Returns ranked symbol hits."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Symbol name or free text to search for." },
                    "kind": { "type": "string", "description": "Optional node-kind filter, e.g. \"function\", \"struct\", \"route\"." },
                    "limit": { "type": "integer", "minimum": 1, "description": "Maximum hits (default 20)." }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: SearchArgs) -> Result<SearchResult, ToolCallError> {
        let kind = parse_kind(args.kind.as_deref())?;
        run_engine(self.engine.clone(), move |e| {
            e.search(&args.query, kind, args.limit)
        })
        .await
    }
}

// ── context ───────────────────────────────────────────────────────────────

/// `context` arguments (FR-NV-02).
#[derive(Debug, Deserialize)]
pub struct ContextArgs {
    /// Natural-language task description to build the bundle for.
    pub task: String,
    /// Cap on bundle size in nodes (default 25).
    #[serde(default)]
    pub max_nodes: Option<usize>,
    /// Include source code in the bundle (default false).
    #[serde(default)]
    pub include_code: Option<bool>,
}

/// Deterministic multi-symbol context bundle for a task description.
#[derive(Clone)]
pub struct Context {
    engine: Arc<Engine>,
}

impl Context {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for Context {
    const NAME: &'static str = "context";
    type Error = ToolCallError;
    type Args = ContextArgs;
    type Output = ContextBundle;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Deterministic multi-symbol context bundle for a task \
                 description — one call replaces several speculative file reads."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "Natural-language task description." },
                    "max_nodes": { "type": "integer", "minimum": 1, "description": "Cap on bundle size in nodes (default 25)." },
                    "include_code": { "type": "boolean", "description": "Include source code in the bundle (default false)." }
                },
                "required": ["task"]
            }),
        }
    }

    async fn call(&self, args: ContextArgs) -> Result<ContextBundle, ToolCallError> {
        run_engine(self.engine.clone(), move |e| {
            e.context(&args.task, args.max_nodes, args.include_code.unwrap_or(false))
        })
        .await
    }
}

// ── node ────────────────────────────────────────────────────────────────────

/// `node` arguments (FR-NV-04).
#[derive(Debug, Deserialize)]
pub struct NodeArgs {
    /// Symbol to look up.
    pub symbol: String,
    /// Include the node's source code (default false).
    #[serde(default)]
    pub include_code: Option<bool>,
}

/// Everything about one symbol: kind, location, signature, edges.
#[derive(Clone)]
pub struct Node {
    engine: Arc<Engine>,
}

impl Node {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for Node {
    const NAME: &'static str = "node";
    type Error = ToolCallError;
    type Args = NodeArgs;
    type Output = NodeInfo;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Everything about one symbol: kind, location, signature, \
                 annotations, and immediate edges."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "The symbol to look up." },
                    "include_code": { "type": "boolean", "description": "Include the node's source code (default false)." }
                },
                "required": ["symbol"]
            }),
        }
    }

    async fn call(&self, args: NodeArgs) -> Result<NodeInfo, ToolCallError> {
        run_engine(self.engine.clone(), move |e| {
            e.node(&args.symbol, args.include_code.unwrap_or(false))
        })
        .await
    }
}

// ── callers / callees (shared edge args) ──────────────────────────────────

/// Edge-listing arguments (FR-NV-05): a symbol and an optional result cap.
#[derive(Debug, Deserialize)]
pub struct EdgeArgs {
    /// Symbol whose direct callers/callees to list.
    pub symbol: String,
    /// Maximum results (default 50).
    #[serde(default)]
    pub limit: Option<usize>,
}

fn edge_parameters(verb: &str) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "symbol": { "type": "string", "description": format!("Symbol whose direct {verb} to list.") },
            "limit": { "type": "integer", "minimum": 1, "description": "Maximum results (default 50)." }
        },
        "required": ["symbol"]
    })
}

/// Direct callers of a symbol.
#[derive(Clone)]
pub struct Callers {
    engine: Arc<Engine>,
}

impl Callers {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for Callers {
    const NAME: &'static str = "callers";
    type Error = ToolCallError;
    type Args = EdgeArgs;
    type Output = CallersResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Direct callers of a symbol (who invokes it).".to_string(),
            parameters: edge_parameters("callers"),
        }
    }

    async fn call(&self, args: EdgeArgs) -> Result<CallersResult, ToolCallError> {
        run_engine(self.engine.clone(), move |e| {
            e.callers(&args.symbol, args.limit)
        })
        .await
    }
}

/// Direct callees of a symbol.
#[derive(Clone)]
pub struct Callees {
    engine: Arc<Engine>,
}

impl Callees {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for Callees {
    const NAME: &'static str = "callees";
    type Error = ToolCallError;
    type Args = EdgeArgs;
    type Output = CalleesResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Direct callees of a symbol (what it invokes).".to_string(),
            parameters: edge_parameters("callees"),
        }
    }

    async fn call(&self, args: EdgeArgs) -> Result<CalleesResult, ToolCallError> {
        run_engine(self.engine.clone(), move |e| {
            e.callees(&args.symbol, args.limit)
        })
        .await
    }
}

// ── impact ────────────────────────────────────────────────────────────────

/// `impact` arguments (FR-NV-06).
#[derive(Debug, Deserialize)]
pub struct ImpactArgs {
    /// Symbol whose transitive impact to compute.
    pub symbol: String,
    /// Traversal depth bound (default 3).
    #[serde(default)]
    pub depth: Option<usize>,
}

/// Transitive impact of changing a symbol, both directions labeled.
#[derive(Clone)]
pub struct Impact {
    engine: Arc<Engine>,
}

impl Impact {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for Impact {
    const NAME: &'static str = "impact";
    type Error = ToolCallError;
    type Args = ImpactArgs;
    type Output = ImpactResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Transitive impact of changing a symbol, both directions \
                 labeled: upstream breaks-if-changed, downstream depends-on."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Symbol whose transitive impact to compute." },
                    "depth": { "type": "integer", "minimum": 1, "description": "Traversal depth bound (default 3)." }
                },
                "required": ["symbol"]
            }),
        }
    }

    async fn call(&self, args: ImpactArgs) -> Result<ImpactResult, ToolCallError> {
        run_engine(self.engine.clone(), move |e| {
            e.impact(&args.symbol, args.depth)
        })
        .await
    }
}

// ── explore ───────────────────────────────────────────────────────────────

/// `explore` arguments (FR-NV-03).
#[derive(Debug, Deserialize)]
pub struct ExploreArgs {
    /// Symbol or text to explore around.
    pub query: String,
    /// Cap on file groups returned (default 10).
    #[serde(default)]
    pub max_files: Option<usize>,
}

/// Neighbourhood exploration around a query, grouped by file.
#[derive(Clone)]
pub struct Explore {
    engine: Arc<Engine>,
}

impl Explore {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for Explore {
    const NAME: &'static str = "explore";
    type Error = ToolCallError;
    type Args = ExploreArgs;
    type Output = ExploreResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Neighbourhood exploration around a query, with matching \
                 source grouped by file."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Symbol or text to explore around." },
                    "max_files": { "type": "integer", "minimum": 1, "description": "Cap on file groups returned (default 10)." }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: ExploreArgs) -> Result<ExploreResult, ToolCallError> {
        run_engine(self.engine.clone(), move |e| {
            e.explore(&args.query, args.max_files)
        })
        .await
    }
}

// ── affected ──────────────────────────────────────────────────────────────

/// `affected` arguments (FR-CL-04): the changed file set and a tests-only flag.
#[derive(Debug, Deserialize)]
pub struct AffectedArgs {
    /// Repo-relative changed files whose reverse-transitive closure to compute.
    pub files: Vec<String>,
    /// Narrow the closure to test-marked files (default false).
    #[serde(default)]
    pub tests_only: Option<bool>,
}

/// Reverse-transitive closure of files affected by a changed set.
#[derive(Clone)]
pub struct Affected {
    engine: Arc<Engine>,
}

impl Affected {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for Affected {
    const NAME: &'static str = "affected";
    type Error = ToolCallError;
    type Args = AffectedArgs;
    type Output = AffectedResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Reverse-transitive closure of files affected by a changed \
                 set: every file depending — directly or transitively — on any of \
                 the given files. `tests_only` narrows the closure to test files."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Repo-relative changed files."
                    },
                    "tests_only": { "type": "boolean", "description": "Narrow the closure to test-marked files (default false)." }
                },
                "required": ["files"]
            }),
        }
    }

    async fn call(&self, args: AffectedArgs) -> Result<AffectedResult, ToolCallError> {
        run_engine(self.engine.clone(), move |e| {
            e.affected(&args.files, args.tests_only.unwrap_or(false))
        })
        .await
    }
}
