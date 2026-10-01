//! The rmcp `ServerHandler` — the `logos:*` tools, each a thin delegator
//! (S-017, FR-MC-01, FR-MC-02, NFR-MA-02). The single-root backing exposes the
//! 30-tool `single_tool_router` roster; the federated workspace backing composes
//! the `xservice_*`/`workspace_*` cross-service tools on top, leaving every
//! single-root tool byte-identical (FR-WS-05, S-248).
//!
//! Tool names are registered BARE (`search`, not `logos:search`): MCP hosts
//! namespace tools by *server identity* — this server identifies as `logos`
//! in [`ServerHandler::get_info`], so hosts render `logos:<tool>` (FR-MC-01,
//! "the host namespaces them"; also the MCP tool-name SEP forbids `:`).

use std::sync::Arc;
use std::time::Instant;

use anyhow::Context as _;
use logos_core::federation::{
    self, query, workspace_governance, Backing, BuildDependencies, ContractBridge, EngineRegistry,
    TypeReferences,
};
use logos_core::{governance::DsmGranularity, model::NodeKind, Engine};
use rmcp::{
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ErrorData, Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router, ServerHandler,
};
use serde::Deserialize;

/// `server-instructions` steering graph-first usage, the session-gate
/// protocol, and status-vs-health disambiguation (FR-MC-03, NFR-UX-04).
/// Prose is data, not logic — it lives in Markdown beside this module.
/// Public so guards assert against the string the server SERVES rather than a
/// path-derived copy of the file (S-362, FR-IN-09).
pub const INSTRUCTIONS: &str = include_str!("instructions.md");

/// The Logos MCP server — a pure protocol adapter (ADR-01): every tool
/// delegates to one [`Engine`] method (or, for `xservice_*`, one [`query`]
/// read-model over the member registry); no business logic lives here (FR-MC-02).
///
/// [`new`](Self::new) is the single-root server (one [`Engine`], the 30-tool
/// `single_tool_router`); [`federated`](Self::federated) composes the
/// `xservice_*` tools on top and runs the shared tools against the default
/// member. The single-root roster carries no `repo` dimension, so its
/// `tools/list` is byte-identical whether or not federation exists (FR-WS-05).
#[derive(Clone)]
pub struct LogosMcp {
    /// The serve backing (ADR-52): the single engine, or the member registry.
    backing: Arc<Backing<Engine>>,
    /// The cross-service bridge, cached on member sync-stamps ([FR-WS-04]);
    /// inert under [`Backing::Single`].
    bridge: Arc<ContractBridge>,
    /// The build-dependency relation beside the bridge, joined on first query
    /// and cached on member sync-stamps ([FR-WS-33]); never a runtime coupling.
    build_deps: Arc<BuildDependencies>,
    /// The type-reference overlay, built on first query over the relation above
    /// and cached on member sync-stamps ([FR-WS-35]); never a coupling.
    type_refs: Arc<TypeReferences>,
    tool_router: ToolRouter<Self>,
}

impl LogosMcp {
    /// Wrap a started (long-lived) [`Engine`] — one per worktree root
    /// (ADR-04, ADR-15, FR-WT-04). The single-root roster is byte-for-byte
    /// today's (FR-WS-05).
    pub fn new(engine: impl Into<Arc<Engine>>) -> Self {
        Self {
            backing: Arc::new(Backing::Single(engine.into())),
            bridge: Arc::new(ContractBridge::new()),
            build_deps: Arc::new(BuildDependencies::new()),
            type_refs: Arc::new(TypeReferences::new()),
            tool_router: Self::single_tool_router(),
        }
    }

    /// Wrap a workspace member [`EngineRegistry`] — the federated backing
    /// ([FR-WS-05], [ADR-52]): the single-root roster (against the default
    /// member) plus the `xservice_*` tools. Only constructed when a manifest is
    /// present, so single-root never pays for the extra roster.
    pub fn federated(registry: EngineRegistry<Engine>) -> Self {
        Self {
            backing: Arc::new(Backing::Federated(Box::new(registry))),
            bridge: Arc::new(ContractBridge::new()),
            build_deps: Arc::new(BuildDependencies::new()),
            type_refs: Arc::new(TypeReferences::new()),
            tool_router: Self::single_tool_router() + Self::xservice_tool_router(),
        }
    }

    /// This backing's registered tool roster — introspection for the roster
    /// byte-identity test (FR-WS-05 acceptance).
    pub fn list_tools(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router.list_all()
    }

    /// The ADR-03 submit-and-await bridge: run one blocking [`Engine`] call
    /// on tokio's blocking pool (tokio never enters logos-core) and serialise
    /// the read-model as the tool result. Emits the per-call telemetry event
    /// (surface=mcp, tool, duration, ok) through the tracing chokepoint
    /// (ADR-13, FR-OB-01).
    ///
    /// `spawn_blocking` is NOT the bridge ADR-03 rejected: it only parks this
    /// task's blocking submit-and-await off the reactor (which must stay free
    /// for protocol I/O and the future watcher). Concurrency policy — op
    /// concurrency and write serialization — stays in the core-owned
    /// `Runtime` pools that every `Engine` method submits to (ADR-02);
    /// nothing about how many core ops run at once is delegated to tokio.
    ///
    /// # Errors (ADR-14 severity mapping at the MCP boundary)
    /// `Degraded` conditions never reach this error path — the core embeds
    /// them as `warnings` inside the read-model (fail-soft). A panic escaping
    /// the core is a `Correctness` failure: it surfaces here as a `JoinError`
    /// and becomes a structured internal error — the server stays alive,
    /// never crashes (FR-MC-06, NFR-RA-12). The fallible quality methods go
    /// through [`run_result`](Self::run_result) instead; when the typed
    /// `CoreError` lands (S-026), its `severity()` maps to an MCP error
    /// there.
    async fn run<T, F>(&self, tool: &'static str, call: F) -> Result<CallToolResult, ErrorData>
    where
        T: serde::Serialize + Send + 'static,
        F: FnOnce(&Engine) -> T + Send + 'static,
    {
        self.run_result(tool, move |engine| Ok(call(engine))).await
    }

    /// The fallible body behind [`run`](Self::run), used directly by the
    /// quality/governance tools (S-020): the core returns `Result<T>` so a
    /// *structural* failure (store fault, invalid rules.toml — ADR-14
    /// Correctness) maps to a structured MCP error with the server still
    /// alive (FR-MC-06, NFR-RA-12). Degraded conditions never reach the
    /// error path — they ride inside the read-model (`INCOMPLETE` freshness
    /// line + warnings, NFR-RA-11).
    async fn run_result<T, F>(
        &self,
        tool: &'static str,
        call: F,
    ) -> Result<CallToolResult, ErrorData>
    where
        T: serde::Serialize + Send + 'static,
        F: FnOnce(&Engine) -> anyhow::Result<T> + Send + 'static,
    {
        let backing = Arc::clone(&self.backing);
        self.run_blocking(tool, move || {
            let engine = backing.default_engine()?;
            call(&engine)
        })
        .await
    }

    /// Run one `xservice_*` read-model over the member registry (FR-WS-05):
    /// resolve the federated registry and hand it, with the workspace's
    /// [`ContractBridge`], to the thick-core [`query`] fn (no surface logic,
    /// NFR-MA-02).
    ///
    /// The **bridge**, not a pre-computed edge slice: its derived read-models are
    /// now two (edges, and S-401's unresolved egress residue) and only the
    /// reachability verbs want the second, so each tool pulls exactly what it
    /// renders and `search` never pays for a walk it discards (NFR-PE-01). The
    /// shape `api_v1::workspace_fan` already uses.
    async fn run_xservice<T, F>(
        &self,
        tool: &'static str,
        call: F,
    ) -> Result<CallToolResult, ErrorData>
    where
        T: serde::Serialize + Send + 'static,
        F: FnOnce(&EngineRegistry<Engine>, &ContractBridge) -> T + Send + 'static,
    {
        self.run_xservice_result(tool, move |registry, bridge| Ok(call(registry, bridge)))
            .await
    }

    /// The fallible body behind [`run_xservice`](Self::run_xservice) — the
    /// [`run_result`](Self::run_result) twin for the cross-service surface
    /// (S-258, FR-WS-13).
    ///
    /// `workspace_check` needs it: the workspace rule family compiles
    /// user-authored globs, and a malformed rule must fail **loud** as a
    /// structured MCP error (ADR-14) rather than silently matching nothing — a
    /// governance rule that quietly never fires would report a false all-clear.
    async fn run_xservice_result<T, F>(
        &self,
        tool: &'static str,
        call: F,
    ) -> Result<CallToolResult, ErrorData>
    where
        T: serde::Serialize + Send + 'static,
        F: FnOnce(&EngineRegistry<Engine>, &ContractBridge) -> anyhow::Result<T> + Send + 'static,
    {
        let backing = Arc::clone(&self.backing);
        let bridge = Arc::clone(&self.bridge);
        self.run_blocking(tool, move || {
            let registry = backing
                .as_federated()
                .context("xservice tools require a federated workspace backing")?;
            call(registry, &bridge)
        })
        .await
    }

    /// The shared submit-and-await bridge (ADR-03): run one blocking job, emit
    /// the per-call telemetry event, and map the outcome to the tool result with
    /// the ADR-14 severity tags — the error mapping lives here once, for both the
    /// per-engine and `xservice_*` tools.
    async fn run_blocking<T, F>(
        &self,
        tool: &'static str,
        job: F,
    ) -> Result<CallToolResult, ErrorData>
    where
        T: serde::Serialize + Send + 'static,
        F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    {
        let started = Instant::now();
        let outcome = tokio::task::spawn_blocking(job).await;
        tracing::info!(
            target: "logos::mcp",
            surface = "mcp",
            tool,
            duration_ms = started.elapsed().as_millis() as u64,
            ok = matches!(outcome, Ok(Ok(_))),
            "tool call",
        );
        // ADR-14 severity tags: a panic (JoinError) is Correctness by
        // definition; otherwise the core classifies once and this surface only
        // stamps the tag, never re-deciding it (FR-EH-02, NFR-RA-12).
        let read_model = outcome
            .map_err(|err| {
                ErrorData::internal_error(
                    format!("logos:{tool} failed inside the core: {err}"),
                    Some(serde_json::json!({
                        "tool": tool,
                        "severity": logos_core::Severity::Correctness.as_str(),
                    })),
                )
            })?
            .map_err(|err| {
                ErrorData::internal_error(
                    format!("logos:{tool} failed: {err:#}"),
                    Some(serde_json::json!({
                        "tool": tool,
                        "severity": logos_core::error::classify(&err).as_str(),
                    })),
                )
            })?;
        Ok(CallToolResult::success(vec![Content::json(read_model)?]))
    }
}

/// Parse the optional node-kind filter token against the exact wire names
/// ([`NodeKind::as_str`]); an unknown token is the caller's fault →
/// `invalid_params` naming the valid set (FR-MC-06 structured errors).
fn parse_kind(kind: Option<&str>) -> Result<Option<NodeKind>, ErrorData> {
    let Some(token) = kind else { return Ok(None) };
    NodeKind::ALL
        .iter()
        .copied()
        .find(|k| k.as_str() == token)
        .map(Some)
        .ok_or_else(|| {
            ErrorData::invalid_params(
                format!("unknown node kind {token:?}"),
                Some(serde_json::json!({
                    "valid_kinds": NodeKind::ALL.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
                })),
            )
        })
}

/// Parse the optional dsm granularity token ("module"/"file"); an unknown
/// token is the caller's fault → `invalid_params` (FR-MC-06).
fn parse_granularity(token: Option<&str>) -> Result<Option<DsmGranularity>, ErrorData> {
    token
        .map(|t| {
            t.parse::<DsmGranularity>().map_err(|reason| {
                ErrorData::invalid_params(
                    reason,
                    Some(serde_json::json!({ "valid_granularities": ["module", "file"] })),
                )
            })
        })
        .transpose()
}

// ── Tool parameter schemas (FR-NV-01..07 wire contracts) ───────────────────

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SearchParams {
    /// FTS5 search query (symbol name or free text).
    pub query: String,
    /// Optional node-kind filter, e.g. "function", "struct", "route".
    pub kind: Option<String>,
    /// Maximum number of hits (default 20).
    pub limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ContextParams {
    /// Natural-language task description to build the context bundle for.
    pub task: String,
    /// Cap on bundle size in nodes (default 25).
    pub max_nodes: Option<usize>,
    /// Include source code in the bundle (default false).
    pub include_code: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ExploreParams {
    /// Symbol or text to explore around.
    pub query: String,
    /// Cap on file groups returned (default 10).
    pub max_files: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct NodeParams {
    /// Symbol to look up.
    pub symbol: String,
    /// Include the node's source code (default false).
    pub include_code: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct EdgeParams {
    /// Symbol whose direct callers/callees to list.
    pub symbol: String,
    /// Maximum results (default 50).
    pub limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ImpactParams {
    /// Symbol whose transitive impact to compute.
    pub symbol: String,
    /// Traversal depth bound (default 3).
    pub depth: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ImpactIntersectionParams {
    /// The work items, each `<id>=<symbol>[,<symbol>...]`. Repeating an id
    /// accumulates its symbols.
    pub items: Vec<String>,
    /// Traversal depth bound for every impact set (default 3).
    pub depth: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct PrecedentParams {
    /// Symbol or project-relative file whose structural precedents to find.
    pub target: String,
    /// Maximum number of precedents (default 20, capped at 100).
    pub limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct BranchOverlapParams {
    /// The git refs about to be merged (or just merged) — branches, tags or
    /// commits; anything `git rev-parse` accepts.
    pub refs: Vec<String>,
    /// Comparison point (default: the merge-base of the supplied refs).
    pub base: Option<String>,
    /// A stated merge result to check the refs' work against.
    pub merge: Option<String>,
}

// ── Quality tool parameter schemas (S-020, FR-GV / FR-RC wire contracts) ────

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReconcileParams {
    /// Skip the pre-evaluation reconcile for tight inner loops; the
    /// freshness line marks the result assumed-fresh (FR-RC-04).
    pub no_reconcile: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct EvolutionParams {
    /// Snapshot window size (default 30, FR-GV-06).
    pub limit: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DsmParams {
    /// Matrix granularity: "module" (default) or "file" (FR-GV-07).
    pub granularity: Option<String>,
    /// Skip the pre-evaluation reconcile (FR-RC-04).
    pub no_reconcile: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct HotspotsParams {
    /// Cap the ranked files returned (default: all, FR-GH-06).
    pub limit: Option<usize>,
    /// Rank only untested hotspots (no fresh execution coverage); falls back to
    /// the labeled static-reachability signal when no coverage is ingested
    /// (default false, FR-CV-07).
    pub untested: Option<bool>,
    /// Drop whole test files (`is_test`-only) from the candidate set before
    /// ranking (default false — whole-repo board unchanged, CR-076).
    pub production_scope: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CoverageIngestParams {
    /// Path to the LCOV/Cobertura coverage report to ingest (FR-CV-01).
    pub report: String,
    /// Force the report format ("lcov" or "cobertura"); default auto-detects.
    pub format: Option<String>,
}

// ── Wiki tool parameter schemas (CR-008, FR-WK-02/04/05 wire contracts) ─────

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct WikiWriteParams {
    /// The page slug (path-like: lowercase/digit/`-`/`_` segments, FR-WK-02).
    pub slug: String,
    /// The page title.
    pub title: String,
    /// The markdown body, stored byte-verbatim (1 MiB cap, FR-WK-02).
    pub body: String,
    /// Anchor entity ids: `file:<path>` or `symbol:<symbol>` (default none).
    #[serde(default)]
    pub anchors: Vec<String>,
    /// The mandatory non-empty generator label (FR-WK-02).
    pub generator: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct WikiReadParams {
    /// The slug to read.
    pub slug: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct WikiSearchParams {
    /// The search query (omit with `list: true` to enumerate all pages).
    pub query: Option<String>,
    /// Enumerate all pages instead of searching (default false).
    pub list: Option<bool>,
}

// ── The 30 single-root tools (FR-MC-01) ─────────────────────────────────────
//
// Named `single_tool_router` (not the default `tool_router`) so the federated
// backing can compose it with `xservice_tool_router`; under `Backing::Single`
// this roster is used alone, byte-for-byte as today (FR-WS-05 acceptance).

#[tool_router(router = single_tool_router)]
impl LogosMcp {
    // — Navigation (8): wired, one Engine method each (FR-MC-02, S-013) —

    #[tool(
        description = "FTS5 full-text symbol search over the code graph, optionally filtered by node kind (FR-NV-01)."
    )]
    async fn search(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let kind = parse_kind(p.kind.as_deref())?;
        self.run("search", move |e| e.search(&p.query, kind, p.limit))
            .await
    }

    #[tool(
        description = "Deterministic multi-symbol context bundle for a task description — one call replaces several file reads (FR-NV-02)."
    )]
    async fn context(
        &self,
        Parameters(p): Parameters<ContextParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("context", move |e| {
            e.context(&p.task, p.max_nodes, p.include_code.unwrap_or(false))
        })
        .await
    }

    #[tool(
        description = "Neighbourhood exploration around a query, source grouped by file (FR-NV-03)."
    )]
    async fn explore(
        &self,
        Parameters(p): Parameters<ExploreParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("explore", move |e| e.explore(&p.query, p.max_files))
            .await
    }

    #[tool(
        description = "Everything about one symbol: kind, location, signature, annotations, immediate edges (FR-NV-04)."
    )]
    async fn node(
        &self,
        Parameters(p): Parameters<NodeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("node", move |e| {
            e.node(&p.symbol, p.include_code.unwrap_or(false))
        })
        .await
    }

    #[tool(description = "Direct callers of a symbol (FR-NV-05). Every answer carries `resolution_denominator`: the per-language resolved edge set it was computed over, so an empty set in a language whose cross-file calls are not resolved reads as unresolved, not as nothing (FR-NV-14).")]
    async fn callers(
        &self,
        Parameters(p): Parameters<EdgeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("callers", move |e| e.callers(&p.symbol, p.limit))
            .await
    }

    #[tool(description = "Direct callees of a symbol (FR-NV-05). Every answer carries `resolution_denominator`: the per-language resolved edge set it was computed over, so an empty set in a language whose cross-file calls are not resolved reads as unresolved, not as nothing (FR-NV-14).")]
    async fn callees(
        &self,
        Parameters(p): Parameters<EdgeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("callees", move |e| e.callees(&p.symbol, p.limit))
            .await
    }

    #[tool(
        description = "Transitive impact of changing a symbol, both directions labeled: upstream breaks-if-changed, downstream depends-on (FR-NV-06). Every answer carries `resolution_denominator`: the per-language resolved edge set it was computed over, so an empty set in a language whose cross-file calls are not resolved reads as unresolved, not as nothing (FR-NV-14)."
    )]
    async fn impact(
        &self,
        Parameters(p): Parameters<ImpactParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("impact", move |e| e.impact(&p.symbol, p.depth))
            .await
    }

    #[tool(
        description = "Which planned work items collide, and on what (FR-NV-11). Give work items as `<id>=<symbol>[,<symbol>...]`; returns the pairs whose transitive impact sets intersect (naming the shared symbols), the pairs that are safely parallel, and the coverage limits of that verdict. Ask BEFORE scheduling work in parallel, not after. Every answer carries `resolution_denominator`: the per-language resolved edge set it was computed over, so an empty set in a language whose cross-file calls are not resolved reads as unresolved, not as nothing (FR-NV-14)."
    )]
    async fn impact_intersection(
        &self,
        Parameters(p): Parameters<ImpactIntersectionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("impact_intersection", move |e| {
            e.impact_intersection(&p.items, p.depth)
        })
        .await
    }

    #[tool(
        description = "Structural precedent (FR-NV-12): nodes analogous to a symbol or a project-relative file — those sharing a trait/interface implementation, a registration edge (the same registry, dispatcher or factory names both), or a call shape. Each result names WHY it is analogous and through which nodes; ranking is counted graph facts, never a score, and an empty answer states its reason, and names the resolution denominator where the reason is the index's reach rather than the code (`resolution_denominator`, FR-NV-14). Ask BEFORE writing new code, to find the sibling that already does this."
    )]
    async fn precedent(
        &self,
        Parameters(p): Parameters<PrecedentParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("precedent", move |e| e.precedent(&p.target, p.limit))
            .await
    }

    #[tool(
        description = "Which git refs collide, and what a merge did not carry (FR-NV-13). Give the refs about to be merged; returns the symbols more than one of them modifies (naming the refs, and naming the refs that do NOT touch a shared symbol — the silent-drop shape), plus, with `merge`, the symbols and files a ref changed that the stated merge result does not. Ask BEFORE integrating parallel branches, and again after: a clean merge is not a complete merge. Every answer carries `resolution_denominator`: the per-language resolved edge set it was computed over, so an empty set in a language whose cross-file calls are not resolved reads as unresolved, not as nothing (FR-NV-14)."
    )]
    async fn branch_overlap(
        &self,
        Parameters(p): Parameters<BranchOverlapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("branch_overlap", move |e| {
            e.branch_overlap(&p.refs, p.base.as_deref(), p.merge.as_deref())
        })
        .await
    }

    #[tool(
        description = "INDEX health: file/node/edge counts, store size, freshness of the index vs the working tree (FR-NV-07). For ARCHITECTURE health use the health tool."
    )]
    async fn status(&self) -> Result<CallToolResult, ErrorData> {
        self.run("status", |e| e.status()).await
    }

    // — Quality (9): wired to the governance engine (S-020, FR-MC-01). Each
    //   is a guaranteed-fresh aggregate run (reconcile-then-score, ADR-11)
    //   whose result carries the FR-RC-03 freshness line; a structural core
    //   failure becomes a structured MCP error, never a crash (FR-MC-06,
    //   NFR-RA-12). —

    #[tool(
        description = "Full architecture-quality scan, reconcile-then-score (ADR-11): the 0-10000 signal, rule violations, and a persisted snapshot. The freshness line reports what was reconciled."
    )]
    async fn scan(
        &self,
        Parameters(p): Parameters<ReconcileParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let reconcile = !p.no_reconcile.unwrap_or(false);
        self.run_result("scan", move |e| e.scan(reconcile)).await
    }

    #[tool(description = "Re-scan with the same parameters as the last scan (ADR-11).")]
    async fn rescan(&self) -> Result<CallToolResult, ErrorData> {
        self.run_result("rescan", |e| e.rescan()).await
    }

    #[tool(
        description = "Architecture-rules compliance report against rules.toml (FR-GV-02): constraints, layer ordering (unassigned files exempt), and boundary checks."
    )]
    async fn check_rules(
        &self,
        Parameters(p): Parameters<ReconcileParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let reconcile = !p.no_reconcile.unwrap_or(false);
        self.run_result("check_rules", move |e| e.check_rules(None, reconcile))
            .await
    }

    #[tool(
        description = "Signal evolution over stored snapshots with per-metric deltas (FR-GV-06, default window 30)."
    )]
    async fn evolution(
        &self,
        Parameters(p): Parameters<EvolutionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_result("evolution", move |e| e.evolution(p.limit))
            .await
    }

    #[tool(
        description = "Dependency structure matrix (FR-GV-07): cell (i,j) counts dep edges i->j; rows ordered by layer order then name; module granularity by default."
    )]
    async fn dsm(&self, Parameters(p): Parameters<DsmParams>) -> Result<CallToolResult, ErrorData> {
        let granularity = parse_granularity(p.granularity.as_deref())?;
        let reconcile = !p.no_reconcile.unwrap_or(false);
        self.run_result("dsm", move |e| e.dsm(granularity, reconcile))
            .await
    }

    // — Temporal tier (1): the non-gated git-history surface (CR-006,
    //   FR-GH-06). Behind the SAME `api` method as the CLI `hotspots`
    //   subcommand, so payloads are byte-identical (NFR-CC-01). Advisory only:
    //   the temporal tier never moves the gate (BR-26). —

    #[tool(
        description = "Hotspot ranking (FR-GH-06): indexed files ranked by churn-rank × structural-complexity-rank — a Rust-side join of git-history churn and per-file cyclomatic complexity, with a per-file coverage column (fresh/stale/n-a). The NON-GATED temporal+coverage tier (BR-26/BR-28); the defect-history column is a labeled heuristic. `untested:true` ranks only files with no fresh coverage (labeled static-reachability fallback when none is ingested). `production_scope:true` drops whole test files (is_test-only) from the candidate set before ranking (default false, CR-076). Non-git/shallow repos return n/a + a notice."
    )]
    async fn hotspots(
        &self,
        Parameters(p): Parameters<HotspotsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let untested = p.untested.unwrap_or(false);
        let production_scope = p.production_scope.unwrap_or(false);
        self.run_result("hotspots", move |e| {
            e.hotspots(p.limit, untested, production_scope)
        })
        .await
    }

    // — Coverage evidence tier (2): the non-gated coverage surface (CR-007,
    //   FR-CV-05/06/07). Each behind the SAME `api` method as its CLI twin, so
    //   payloads are byte-identical (NFR-CC-01). Advisory only: coverage never
    //   moves the gate (BR-28). —

    #[tool(
        description = "Ingest an LCOV/Cobertura coverage report into the evidence store (FR-CV-01): auto-detects the format (override with `format`), maps report paths to indexed files, and anchors each file by content hash. The NON-GATED coverage tier (BR-28). Fails loud on an unreadable/unrecognized/malformed report; per-file outcomes (unmatched, stale-rejected) ride inside the summary."
    )]
    async fn coverage_ingest(
        &self,
        Parameters(p): Parameters<CoverageIngestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_result("coverage_ingest", move |e| {
            e.coverage_ingest(std::path::Path::new(&p.report), p.format.as_deref())
        })
        .await
    }

    #[tool(
        description = "Coverage status (FR-CV-05/06): per-file freshness (fresh value / stale label / n-a, hash-based against the ingest anchor), the overall freshness fraction, snapshot provenance, and an artifact-vs-HEAD staleness prompt when the coverage lags the current commit (FR-CV-10). Raw numbers only, no grading (BR-28). With no coverage ingested, reports n/a + a notice."
    )]
    async fn coverage_status(&self) -> Result<CallToolResult, ErrorData> {
        self.run_result("coverage_status", |e| e.coverage_status())
            .await
    }

    #[tool(
        description = "Coverage refresh (FR-CV-10): explicitly run the configured [coverage_ingest].refresh_cmd as a subprocess (the ONLY place Logos ever spawns a coverage command, never on the serve/watcher path, ADR-38), then ingest the artifact it produced. Errors loud if no refresh_cmd is configured, the command fails, or it produced no recognizable artifact. The NON-GATED coverage tier (BR-28)."
    )]
    async fn coverage_refresh(&self) -> Result<CallToolResult, ErrorData> {
        self.run_result("coverage_refresh", |e| e.coverage_refresh())
            .await
    }

    // — Source wiki (5): the agent-generated wiki surface (CR-008,
    //   FR-WK-02/04/05/06/09) plus the CR-062 deterministic presented tier
    //   (FR-WK-20). Each behind the SAME `api` method as its CLI twin, so
    //   payloads are byte-identical (NFR-CC-01). Gate-immune: never read by the
    //   metric path (BR-29). `wiki delete`/`wiki skill` stay CLI-only —
    //   destructive/install ops off the agent surface. —

    #[tool(
        description = "Write (upsert) a source-wiki page by slug (FR-WK-02): byte-verbatim markdown body (1 MiB cap), write-time anchor resolution to content hashes, write-time HEAD tag, and a MANDATORY non-empty generator label. Anchors are `file:<repo-relative-path>` or `symbol:<canonical-symbol>`; an unknown anchor / empty generator / over-cap body is rejected loudly with the store left byte-identical. The gate-immune wiki store (BR-29) — never moves the quality signal."
    )]
    async fn wiki_write(
        &self,
        Parameters(p): Parameters<WikiWriteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_result("wiki_write", move |e| {
            e.wiki_write(&p.slug, &p.title, &p.body, &p.anchors, &p.generator)
        })
        .await
    }

    #[tool(
        description = "Read a source-wiki page by slug (FR-WK-04) with MANDATORY provenance no surface may omit: the generator label, the written-at HEAD commit, per-anchor freshness (fresh/stale/missing, computed against the current tree — no sync needed), and the fixed 'generated content — not extracted by Logos' marker. A miss (or an all-anchors-gone auto-prune) returns null. Wiki prose is generated content, never extracted fact."
    )]
    async fn wiki_read(
        &self,
        Parameters(p): Parameters<WikiReadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_result("wiki_read", move |e| e.wiki_read(&p.slug))
            .await
    }

    #[tool(
        description = "FTS5 bm25 search over source-wiki page titles and bodies (FR-WK-05), indexed inside wiki.db so it survives `index`. Every hit carries its staleness flag and provenance summary (generator, HEAD). `list: true` enumerates all pages (slug-ordered) instead of searching. Offline; no vectors (NFR-SE-01). A pure read — never prunes."
    )]
    async fn wiki_search(
        &self,
        Parameters(p): Parameters<WikiSearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let list = p.list.unwrap_or(false);
        self.run_result("wiki_search", move |e| {
            e.wiki_search(p.query.as_deref().unwrap_or(""), list)
        })
        .await
    }

    #[tool(
        description = "Source-wiki store summary + regeneration work-list (FR-WK-06): page/stale/missing counts and the freshness fraction, the pruned-orphan log, and the work-list driving regeneration — stale pages, missing-anchor pages, and page-worthy entities lacking a page (modules, top-level files, and — only when the swe-skills doc graph is present — Requirement/Adr/Story nodes). Logos discovers deterministically; the agent writes the pages."
    )]
    async fn wiki_status(&self) -> Result<CallToolResult, ErrorData> {
        self.run_result("wiki_status", |e| e.wiki_status()).await
    }

    #[tool(
        description = "Deterministically materialize the presented tier (FR-WK-20, CR-062): in SRS mode, assembles each present Design/Specs category and the single-file Architecture page from the project's authored `docs/specs/**` sources into `wiki.db` with `generator = \"logos:doc-present\"`, then runs the reconciliation sweep. Pure local-FS reads + `wiki.db` writes — no LLM, no network (NFR-SE-01); byte-identical on re-run. Outside SRS mode (Case 2) this is a no-op returning the empty summary."
    )]
    async fn wiki_materialize(&self) -> Result<CallToolResult, ErrorData> {
        self.run_result("wiki_materialize", |e| e.wiki_materialize())
            .await
    }

    #[tool(
        description = "ARCHITECTURE health: DB integrity, schema version, FTS coherence, structural integrity, admission-tripwire drift, graph counts. For INDEX freshness use the status tool."
    )]
    async fn health(&self) -> Result<CallToolResult, ErrorData> {
        self.run_result("health", |e| e.health(true)).await
    }

    #[tool(
        description = "Fast graph structural-integrity check (FR-GV-18/FR-GV-20, NFR-RA-13): asserts one node per symbol_id and zero orphan rows (dangling file/edge/shingle), and flags every indexed file the current admission rules (gitignore, nested-.git boundary, ignored_dirs, globs) would reject — in a handful of indexed queries plus O(files) matcher work, no reindex. `ok:false` with named faults and a capped `unadmitted_sample` on drift — the always-on guard that also hard-fails session_end/check_rules. For a deep reindex-diff see verify."
    )]
    async fn doctor(&self) -> Result<CallToolResult, ErrorData> {
        self.run_result("doctor", |e| e.doctor()).await
    }

    #[tool(
        description = "Deep graph consistency check (FR-GV-19, NFR-RA-06): reindexes the project into a throwaway shadow store via the always-purge index path, then diffs node/edge/file counts and symbol sets against the live graph. Reports `ok:false` with live-vs-reindex deltas and a capped sample of leaked (live-only) / orphaned (reindex-only) symbols, and embeds the fast structural + admission check (FR-GV-20). Catches Channel-B orphans (files the live store retains but a fresh index drops) that doctor cannot. On-demand only — a full reindex is seconds-to-minutes; the live store is read-only and the shadow store is torn down on completion."
    )]
    async fn verify(&self) -> Result<CallToolResult, ErrorData> {
        self.run_result("verify", |e| e.verify()).await
    }

    #[tool(
        description = "Begin a quality session (FR-GV-04): records the quality baseline before edits — call this BEFORE making changes."
    )]
    async fn session_start(&self) -> Result<CallToolResult, ErrorData> {
        self.run_result("session_start", |e| e.session_start())
            .await
    }

    #[tool(
        description = "End the quality session (FR-GV-05): re-score and compare to the baseline; fails on aggregate regression beyond epsilon."
    )]
    async fn session_end(&self) -> Result<CallToolResult, ErrorData> {
        self.run_result("session_end", |e| e.session_end()).await
    }
}

// ── xservice cross-service tool parameter schemas (FR-WS-05 wire contracts) ──
// Each carries the optional `repo` member filter; the shared navigation tools
// deliberately do NOT (their single-root schema stays byte-identical).

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct XserviceSearchParams {
    /// FTS5 search query (symbol name or free text).
    pub query: String,
    /// Optional node-kind filter, e.g. "function", "route".
    pub kind: Option<String>,
    /// Maximum hits per member (default 20).
    pub limit: Option<usize>,
    /// Scope to one workspace member (its workspace-relative name); omit to fan
    /// across every member.
    pub repo: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct XserviceCallersParams {
    /// Symbol whose cross-service callers to list.
    pub symbol: String,
    /// Maximum intra-repo callers per member (default 50).
    pub limit: Option<usize>,
    /// Scope the intra-repo fan-out to one workspace member.
    pub repo: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct XserviceImpactParams {
    /// Symbol whose cross-service impact to trace.
    pub symbol: String,
    /// Traversal depth bound per member (default 3).
    pub depth: Option<usize>,
    /// Scope the seed impact to one workspace member.
    pub repo: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct XserviceRepoParams {
    /// Scope to one workspace member (its workspace-relative name).
    pub repo: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct XserviceReachabilityParams {
    /// Scope the union view to one workspace member (its workspace-relative name);
    /// omit to project every member.
    pub repo: Option<String>,
    /// Return the full per-repo-dead set instead of only the (usually tiny)
    /// cross-service promotions. Default false: the response carries only promoted
    /// nodes and states `promotions_only`, so a bounded reply is never misread as
    /// the complete dead-set (NFR-CC-04).
    pub all: Option<bool>,
}

// ── The xservice cross-service tools (FR-WS-05) ─────────────────────────────
// Registered ONLY on the federated backing, so single-root `tools/list` never
// sees them. Each is a thin delegator to one thick-core `query::*` read-model.

#[tool_router(router = xservice_tool_router)]
impl LogosMcp {
    #[tool(
        description = "Cross-service resolved route bindings (FR-WS-05, FR-WS-10): each consumer endpoint's binding across the `route`, `grpc-call` and `broker-topic` relations, both repo-qualified (member, symbol). `route`/`grpc-call` bind the SOLE provider of each key — but that is NOT one edge per consumer endpoint, and `providers` must never be de-duplicated on the consumer: SINCE S-420 a `route` target whose committed overlays compose it several ways is keyed on EVERY composition, and each composition that binds its own sole provider is its own edge carrying its own profile set, so one call site can carry several `route` edges naming different providers (`grpc-call` reads its target verbatim and is unaffected). A `broker-topic` binding FANS OUT — one publish binds every subscribe on the topic, so `providers` carries one edge per subscriber, never a sole provider chosen from the set. Each edge carries `intake` (how it entered the overlay) plus `from_value`/`to_value` provenance (`literal`, `config-bound` or `config-unresolved`) naming the consumer's and the provider's evidence respectively. `repo` scopes to routes that member provides. BESIDE `providers`, never inside it (BR-57): `declared_contracts` is the DECLARED-CONTRACT relation — `declares-contract(holder → target)` from a spec document the holder vendors and does not implement, `target` either the member whose own spec it is (`kind: member`, with `shared` of `total` operations: the identity score) or a named external (`kind: external`, keyed by `external` id; names are not unique) — and `bound_external` is the external join: each invocation `no-provider-in-workspace` REST row judged against the externals its own member declares, a bound row naming the external, the matched `operation` and the committed `base` path with its `origin` and `sources` (file and key). BOTH ARE DECLARED, NOT OBSERVED: no declared contract or bound external is a binding, a bridge edge or a resolved call, and the bound row stays `no-provider-in-workspace` in every coverage count. Each carries a `headline` whose `summary` states it beside its denominator. Both are workspace-wide under `repo` (`declared_scope_note` says so); `declared_contracts` is absent when no member holds a vendored or mock-held spec, `bound_external` when no member declares a named external."
    )]
    async fn xservice_route_providers(
        &self,
        Parameters(p): Parameters<XserviceRepoParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_xservice("xservice_route_providers", move |reg, bridge| {
            let coverage = federation::cross_service_coverage(&reg.answer());
            query::xservice_route_providers(&query::edges(bridge, reg), p.repo.as_deref())
                .with_declared(coverage)
        })
        .await
    }

    #[tool(
        description = "Cross-service callers of a symbol (FR-WS-05): each member's intra-repo callers (repo-qualified) plus the cross-service consumers that reach it over a bridge edge. `repo` scopes the intra-repo fan-out to one member. EVERY ANSWER CARRIES ITS UNRESOLVED RESIDUE (CR-125, BR-53): `unresolved_egress` reports the captured outbound call sites in scope that did NOT resolve — `unresolved_sites` (the count), `measured_sites` (its denominator, the same `bound+ambiguous+unbound` egress population `workspace_status`'s `egress_resolution` is taken over, so `unresolved_sites = measured_sites - bound`), `by_reason` (the per-reason breakdown, summing exactly to `unresolved_sites`), `no_provider_in_workspace` (sites whose provider is outside this workspace — bucketed APART, never inside the residue), `members_in_scope` (how many members the unresolved sites are SPREAD ACROSS — not the workspace roster size), `covers_all_members` and `summary`, the one composed line carrying all of it. THE FIELD IS ABSENT EXACTLY WHEN THE RESIDUE IS ZERO, and only then: an EMPTY `cross_service` list WITH an `unresolved_egress` block does NOT mean \"nothing reaches this\" — it means the question was answered over a graph missing that many outbound calls. Do not read it as an absence. `repo` scopes the residue to that member's egress, exactly as it scopes the per-member fan-out — but NOT the cross-service tier, which matches on the symbol alone; under a `repo` scope the `summary` says so, marking the resolved count workspace-wide and naming the member the residue covers. APART FROM `cross_service`, never merged with it: `via_type_reference` lists the importers an ADVISORY TYPE REFERENCE reaches (FR-WS-35, BR-60) when the symbol is a type's node or its dotted name (an Avro-declared type has only the name) — each entry carries `reached: \"via type reference\"` and `via`, the reference (`fqn`, `importer` member, file, line and declaration, `owner`, `evidence`), whose importer IS the caller at class grain: an import of a type, not a call of one of its methods. It is NOT a coupling, never a bridge edge, and never counted in `unresolved_egress`; the key is absent when no bound type reference names the symbol. Advisory only, never a gate input."
    )]
    async fn xservice_callers(
        &self,
        Parameters(p): Parameters<XserviceCallersParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let (deps, types) = (Arc::clone(&self.build_deps), Arc::clone(&self.type_refs));
        self.run_xservice("xservice_callers", move |reg, bridge| {
            let (edges, residue) = query::reachability_inputs(bridge, reg);
            query::xservice_callers(reg, &edges, &residue, &p.symbol, p.limit, p.repo.as_deref())
                .with_type_references(reg, &types.index(reg, &deps))
        })
        .await
    }

    #[tool(
        description = "Cross-service impact of changing a symbol (FR-WS-05): the seed member's impact plus the far member's impact stitched across every bridge edge the symbol is an endpoint of, all repo-qualified. `repo` scopes the seed to one member. EVERY ANSWER CARRIES ITS UNRESOLVED RESIDUE (CR-125, BR-53): `unresolved_egress` reports the captured outbound call sites in scope that did NOT resolve — `unresolved_sites` (the count), `measured_sites` (its denominator, the same `bound+ambiguous+unbound` egress population `workspace_status`'s `egress_resolution` is taken over, so `unresolved_sites = measured_sites - bound`), `by_reason` (the per-reason breakdown, summing exactly to `unresolved_sites`), `no_provider_in_workspace` (sites whose provider is outside this workspace — bucketed APART, never inside the residue), `members_in_scope` (how many members the unresolved sites are SPREAD ACROSS — not the workspace roster size), `covers_all_members` and `summary`, the one composed line carrying all of it. THE FIELD IS ABSENT EXACTLY WHEN THE RESIDUE IS ZERO, and only then: an EMPTY `cross_service` list WITH an `unresolved_egress` block does NOT mean \"nothing reaches this\" — it means the question was answered over a graph missing that many outbound calls. Do not read it as an absence. `repo` scopes the residue to that member's egress, exactly as it scopes the per-member fan-out — but NOT the cross-service tier, which matches on the symbol alone; under a `repo` scope the `summary` says so, marking the resolved count workspace-wide and naming the member the residue covers. APART FROM `cross_service`, never merged with it: `via_type_reference` carries each importer's impact reached through an ADVISORY TYPE REFERENCE (FR-WS-35, BR-60) when the symbol is a type's node or its dotted name (an Avro-declared type has only the name) — every importing FILE of the type, since a reference is an import, not a call — each entry carrying `reached: \"via type reference\"`, `via` (the reference: `fqn`, `importer` member, file, line and declaration, `owner`, `evidence`), `member`, and as `result` the importing member's affected-file closure of the importing file (`changed` is that file, `affected` every file depending on it directly or transitively), or its `error` when that member will not open. It is NOT a coupling, never a bridge edge, and never counted in `unresolved_egress`; the key is absent when no bound type reference names the symbol. Advisory only, never a gate input."
    )]
    async fn xservice_impact(
        &self,
        Parameters(p): Parameters<XserviceImpactParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let (deps, types) = (Arc::clone(&self.build_deps), Arc::clone(&self.type_refs));
        self.run_xservice("xservice_impact", move |reg, bridge| {
            let (edges, residue) = query::reachability_inputs(bridge, reg);
            query::xservice_impact(reg, &edges, &residue, &p.symbol, p.depth, p.repo.as_deref())
                .with_type_references(reg, &types.index(reg, &deps))
        })
        .await
    }

    #[tool(
        description = "Workspace build dependencies (FR-WS-33): per member, `builds_against` (what it builds against) and `built_against_by` (what builds against it), joined from the members' Maven/Gradle manifests; each row names `kind` (`parent`, `dependency`, `managed`, `bom-import`), `scope` (null when undeclared, never defaulted) and `artifact` (`groupId:artifactId`). THIS IS A BUILD DEPENDENCY, NOT A RUNTIME COUPLING (BR-58): no row is a bridge edge, a resolved call or a coverage figure, and none may be read as one. `headline` states `build_dependency_pairs` by kind beside its denominator (`references`, every referenced fact by bucket) and `members` read; a declared `platform` member's inbound pairs sit apart under `platform_apart`, and those rows carry `platform: true`. `cross_context` lists members depending on two or more bounded contexts' model libraries (an artifactId `kafka-models`, the context the groupId's last segment, or `<context>-kafka-models`), each library named: a hint, never an edge. `repo` scopes the rows to one member; a name that is not a member read yields an empty list with `scope_note`."
    )]
    async fn xservice_build_deps(
        &self,
        Parameters(p): Parameters<XserviceRepoParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let deps = Arc::clone(&self.build_deps);
        self.run_xservice("xservice_build_deps", move |reg, _bridge| {
            federation::xservice_build_deps(&deps.relation(reg), p.repo.as_deref())
        })
        .await
    }

    #[tool(
        description = "Cross-member type references (FR-WS-35), advisory and not a coupling: per provider member (`providers`), the types other members import from it (`types`, each with its `fqn` and `owner` — the declaring source file and node, or the Avro schema, which has no node) and every importer (`importers`: `member`, `file`, `line`, the importing declaration's `symbol`, `naming` exact or enclosing, `form` import or type-use, and `evidence`). A row binds only when EXACTLY ONE other member declares the type in main-tree source or an Avro schema, and only where the build relation relates the pair (`evidence.via: build`) or the importer references a colliding artifact the owner produces (`via: collision`, the `artifacts` named). THIS IS AN ADVISORY TYPE REFERENCE, NOT A COUPLING (BR-60): no row is a bridge edge, a resolved call, a build dependency or a coverage figure, and none may be read as one. `headline` is `workspace_status`'s `type_reference` section, workspace-wide whatever the scope: `type_reference_pairs` (split `build_pairs`/`collision_backed_pairs`) beside its denominator `rows` (every row considered, filed into exactly one bucket) and the `members` read — a member whose declared types could not be read or are not yet extracted is named in `members.unread` with `members.unread_reasons`, never counted as a member declaring nothing. `type_only` and `ambiguous_owner` there list the matches never bound. `repo` scopes `providers` to one provider member, listed even when nothing imports its types; a name that is not a member read yields an empty list with `scope_note` stating why."
    )]
    async fn xservice_type_refs(
        &self,
        Parameters(p): Parameters<XserviceRepoParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let (deps, types) = (Arc::clone(&self.build_deps), Arc::clone(&self.type_refs));
        self.run_xservice("xservice_type_refs", move |reg, _bridge| {
            query::xservice_type_refs(&types.index(reg, &deps), p.repo.as_deref())
        })
        .await
    }

    #[tool(
        description = "Cross-service full-text search (FR-WS-05): FTS5 symbol search fanned across the workspace members, each hit repo-qualified. `repo` scopes to one member; `kind` filters by node kind."
    )]
    async fn xservice_search(
        &self,
        Parameters(p): Parameters<XserviceSearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let kind = parse_kind(p.kind.as_deref())?;
        self.run_xservice("xservice_search", move |reg, _bridge| {
            query::xservice_search(reg, &p.query, kind, p.limit, p.repo.as_deref())
        })
        .await
    }

    #[tool(
        description = "Workspace status (FR-WS-05): each member's index freshness and warm state, the workspace warm roll-up, plus the 3-state (bound/ambiguous/unbound-with-reasons) cross-service coverage summary. Each member row carries `warm_state`: `warm` (its graph holds at least one indexed file), `deferred` (no index yet and none attempted — honest and NON-alarming, it indexes lazily on first query, FR-IX-07), or `degraded` (attempted and FAILED, with the reason). `warming` is in the vocabulary but is never reported without a live signal from the warm supervisor, and the roll-up then OMITS the `warming` key entirely rather than sending 0 — an absent `warming` means not-knowable, never none (FR-WS-15, NFR-CC-04). Each row ALSO carries `open_state` on a SEPARATE axis (FR-WS-16): `opened` (its store was opened — a member later evicted to stay inside the workspace connection budget still reads `opened`, because eviction reclaims a success), `not-attempted` (nothing needed this member, so nothing was opened — not a failure), or `degraded` (opening was attempted and FAILED, carrying `degraded_reason` and, when the diagnostic identifies one, `degraded_cause` — `host-resource-limit` means the store is intact and the process ran out of file descriptors, so a re-index is NOT the remedy). `warm_state` is about index presence and `open_state` about store openability: different questions, never merge them. The `degraded_rollup` names every unopenable member and its `covers_all_members: false` marks the warm roll-up, the coverage summary and the topic inventory as computed over fewer than all members. THE HEADLINE IS `coverage.resolved_cross_service_edges` — cross-service edges resolved from a captured `invocation` (a caller→callee HTTP client call, a producer→consumer broker publish, a gRPC stub call) — and it is NEVER read without `coverage.egress_resolution` beside it, the rate at which captured egress sites resolve at all (BR-51). Read `coverage.resolved_edges_summary` rather than recomposing the two: both halves are counted over ONE population by ONE walk since S-403, and before it they were filtered differently and the sentence contradicted itself (CR-127). The headline counts what the coverage tier RESOLVED, which is not the same as what the bridge DREW, and the two are never folded together: a fan-out publish resolves once and draws one edge per subscriber, while an ambiguous or refused composition resolves nothing the bridge could draw. Until S-420 the HTTP arm added a third difference — a target composed from committed configuration resolved here and seeded no reachability root, because the bridge keyed a consumer on its raw ledger target — and on the reference estate that gap was the whole difference: 15 resolved against 0 drawn (2026-09-13). Both arms now key on the committed value, and on the same estate re-measured 2026-09-18 the two read 51 and 51. The drawn count still rides on `workspace_reachability` under its own name, `coverage.bridge_invocation_edges`. `coverage.resolved_edges_summary` carries both in one line, and `coverage.egress_resolution_measured` is that rate's explicit denominator. `egress_resolution` is ABSENT when no egress site was captured, never a fabricated 1.0 — an absent rate with `egress_resolution_measured: 0` means NOTHING OUTBOUND WAS CAPTURED, which is a different statement from `0.0` (captured and none resolved). The edge count counts EDGES, not sites: one broker publish binds every cross-member subscriber and the bridge emits one edge per subscriber, so it is NOT `egress_resolution`'s numerator. `coverage.spec_conformance_ratio` is `bound / (bound+ambiguous+unbound)` — the RETIRED `bound_ratio`'s formula under the name of what it measures: how far this workspace's DECLARATIONS line up with its controllers. It is dominated by contract-surface intake and is NEVER a measure of cross-service coupling; read `resolved_cross_service_edges` for that. It is likewise ABSENT when nothing was measured (bound+ambiguous+unbound = 0), never a fabricated 1.0, and never bare: `coverage.spec_conformance_measured` is its denominator and `coverage.spec_conformance_summary` the composed line (CR-111). `bound_ratio` IS NO LONGER SENT (CR-120) — a reader of it gets nothing, deliberately, rather than a renamed figure. No warm state affects the CLI exit code; an unopenable member makes `logos workspace status` exit 1 (FR-WS-16) — MCP calls report it in the payload instead. The coverage tier is advisory only, never a gate input. Each row in `coverage.references` NAMES THE OTHER END (CR-118): a bound row carries `to` (the provider's member+symbol — the same pair `xservice route-providers` reports for it); an ambiguous row carries `candidates`, the providers it tied between. Read `candidates.disposition`, never the row's bucket: `tied-between` means NONE of them is bound (no edge; naming a candidate is not binding to it), `bound-to` means ALL of them are — the broker fan-out, where one publish reaches every cross-member subscriber, and SINCE S-420 also an exactly-one `route` row whose committed overlays compose its target several ways, each composition binding its own sole provider; in both cases there is no single `to`, so do NOT read a bound `route` row as always carrying one. The set is capped at 8 with `total` and `omitted` disclosing any truncation. Both fields are OPTIONAL and absent when there is nothing to name — an older store has neither. EVERY row also carries `intake` (`contract-surface` for a declared endpoint, `invocation` for a captured call site), in every state — bound, ambiguous and unbound alike — and it is NOT optional (S-377/CR-120). Do not read its presence as \"this row bound something\": that is `bucket`/`state`. `coverage.by_intake` reports the same four classification counters split by it — `by_intake.contract_surface` and `by_intake.invocation`, each with `bound`/`ambiguous`/`unbound`/`no_provider_in_workspace` — and the two populations SUM to the four top-level counters, so the split can never report less than the headline beside it. READ THE SPLIT before drawing any conclusion from `bound`: it counts two different things as one. On the 84-member reference estate the split is 81 `contract-surface` and 45 `invocation` bound rows (2026-09-18; it was 81 and 15 on 2026-09-13, before the broker arm admitted committed values), so a bare `bound: 126` says nothing about whether ANY outbound call site resolves — and it read 81 and 0 for as long as none did (CR-120, NFR-CC-04). No floor is asserted on any of these figures. A high `ambiguous` count is usually NOT a matching defect: where several members legitimately serve one template (the aggregator pattern), the exactly-one rule is refusing correctly and no path-normalisation or method precedence can resolve it — that ambiguity is call-site-gated, not match-gated (FR-CG-09). A member the manifest declares `kind = \"documentation\"` or `\"mock\"` (FR-WS-32) has its contract-surface rows moved OUT of these counts and `spec_conformance_ratio` into `coverage.declared_apart`, whose `summary` states them over their denominator — read it beside the headline. `kind_candidates` lists undeclared members holding API documents and no runnable source: a hint, never a classification; their rows stay in the headline. Both keys are absent when there is nothing to report. `build_dependency`, present when any member holds a Maven/Gradle manifest (or a member's build facts could not be read, or are not yet extracted — a store upgraded across migration 22 until its first full re-read; each such member is named in `members.unread` with its reason in `members.unread_reasons`, never counted as a member with no manifests), states `build_dependency_pairs` by kind beside its `references` denominator and the `members` read: a BUILD dependency, NOT a runtime coupling (BR-58) — it is apart from every figure above and none of them counts it; `xservice_build_deps` lists its rows. `type_reference`, present when any member is Java, Kotlin or Avro (or a member's declared types could not be read, or are not yet extracted — named in `members.unread` with `members.unread_reasons`), states `type_reference_pairs` — directed member pairs bound by an unresolved import or type use of a type EXACTLY ONE other member declares (main-tree source or an Avro schema), admitted only where the build relation relates the pair (`build_pairs`) or the importer references a colliding artifact the owner produces (`collision_backed_pairs`, each listed with its `artifacts`) — beside its denominator `rows` (`considered`, split `imports`/`type_uses`, filed into exactly one of `bound`, `type_only`, `pair_unread`, `ambiguous_owner`, `self_owned`, `unqualified` (a bare type use naming no package, never looked up — NOT a claim that no member declares it), `no_owner`) and the `members` read. `type_only` lists the matched pairs the build relation does not relate (never bound) and `ambiguous_owner` the types several members declare, owners named (never bound). AN ADVISORY TYPE REFERENCE, NOT A COUPLING (BR-60): it is apart from `coverage` and `build_dependency`, never a bridge edge, and none of their figures counts it. `coverage.declared_contracts`, present when a member holds a vendored spec, is the DECLARED-CONTRACT relation: `headline.declared_contract_pairs` (split `to_member` by document identity and `to_external`) beside its denominator `headline.documents` (every spec document read, by bucket), each contract naming its `document` and `target`, and the named-external registry. `coverage.bound_external` states `headline.bound_external` beside `headline.no_provider_rows`, the invocation no-provider REST rows it was judged from, with every refusal counted. BOTH ARE DECLARED, NOT OBSERVED (BR-57): read them beside the invocation and contract-surface headlines, never inside — no figure above counts either, a bound-external row stays `no-provider-in-workspace`, and a tie a declared contract resolves stays ambiguous."
    )]
    async fn workspace_status(&self) -> Result<CallToolResult, ErrorData> {
        self.run_xservice("workspace_status", |reg, _bridge| {
            query::workspace_status(reg)
        })
        .await
    }

    #[tool(
        description = "App-wide cross-service dead code (FR-WS-12): reachability over the union of every member's call graph plus the bridge's cross-service edges as extra live roots. Reports callables their own repo calls dead that a cross-service call keeps alive, and the ones still dead app-wide. Additive and monotone toward live — a missing invocation edge never marks anything dead. ADVISORY ONLY: never a gate input, and it never alters a repo's own dead-code verdict. Every claim carries a coverage rider stating how much of the invocation graph bound. ITS HEADLINE IS `coverage.resolved_cross_service_edges` — the edges resolved from a captured invocation; `0` says NO call site in the workspace resolved. A `live-via-cross-service` promotion rests on `coverage.bridge_invocation_edges` BESIDE it, NOT on the headline: that is the count of invocation edges the bridge actually drew and therefore the roots this view was seeded from, and the two need not agree — one fan-out publish resolves once and draws one edge per subscriber, and an ambiguous composition resolves nothing drawable. Until S-420 they also differed wherever an HTTP target was composed from committed configuration, because the coverage tier resolved the placeholder and the bridge did not: on the reference estate the headline read 15 against a seeded count of 0 (2026-09-13), so a reader who took the headline for the view's basis over-read it (CR-127). Both arms now key on the committed value; re-measured on the same estate 2026-09-18 the two read 51 and 51 (CR-133). Read the seeded count anyway — the reason they can diverge is unchanged. It rides with `coverage.egress_resolution` (ABSENT when no egress site was captured, never a fabricated 1.0) and `coverage.egress_resolution_measured`, its denominator (BR-51). `coverage.spec_conformance_ratio` is the retired `bound_ratio`'s formula renamed (CR-120): dominated by declared-contract matches, NOT a coupling measure, likewise ABSENT when nothing was measured, and never bare — read it beside `coverage.spec_conformance_measured` (the denominator) and `coverage.no_provider_in_workspace` (the excluded count), because a correct-looking ratio can describe a negligible fraction of the evidence (CR-111). The rider deliberately does NOT carry `by_intake`: that split is on `workspace_status`'s coverage summary. `coverage.members_read` counts the REACHABILITY walk's reads, which can differ from the coverage summary's. `skipped_members` names the members the view could not read; a skipped member suppresses promotions but never causes a demotion. `repo` scopes the view to one member. By default the payload is bounded to only the cross-service promotions (the usually-tiny interesting set) and states `promotions_only`; pass `all: true` for the full per-repo-dead set. Every applied bound is stated in the response, so a bounded reply is never the complete dead-set (NFR-CC-04)."
    )]
    async fn workspace_reachability(
        &self,
        Parameters(p): Parameters<XserviceReachabilityParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let scope = federation::ReachabilityScope::new(p.repo, p.all.unwrap_or(false));
        self.run_xservice("workspace_reachability", move |reg, bridge| {
            federation::app_wide_reachability(reg, &query::edges(bridge, reg)).bound(scope)
        })
        .await
    }

    #[tool(
        description = "Workspace governance (FR-WS-13): evaluate the workspace rule family ([governance] in logos.workspace.toml — service-layer boundaries and no-cross-service-callers contracts) over the cross-service bridge bindings. Reported at the workspace level and ADVISORY: it never alters any member's per-repo quality gate. Returns null when no workspace rules are declared (honest empty), never a fabricated passing report."
    )]
    async fn workspace_check(&self) -> Result<CallToolResult, ErrorData> {
        self.run_xservice_result("workspace_check", |reg, bridge| {
            workspace_governance(reg.federation(), &query::edges(bridge, reg))
        })
        .await
    }
}

// `router = self.tool_router`: dispatch through the router built once in
// `new()` instead of the macro's default `Self::tool_router()`-per-request.
#[tool_handler(router = self.tool_router)]
impl ServerHandler for LogosMcp {
    fn get_info(&self) -> ServerInfo {
        // `logos` is the namespace authority: hosts derive `logos:<tool>`
        // from this identity (FR-MC-01); the instructions ride the
        // initialize response (FR-MC-03).
        // This macro expands to THIS crate's version, so it is the product
        // version only while every member inherits `version.workspace` (root
        // Cargo.toml says why). Give `mcp` its own version again and hosts see
        // a number nobody can install; `cli/tests/cli_surface.rs` fails on it.
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("logos", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}
