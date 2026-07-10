//! The rmcp `ServerHandler` — 28 `logos:*` tools, each a thin delegator
//! (S-017, FR-MC-01, FR-MC-02, NFR-MA-02).
//!
//! Tool names are registered BARE (`search`, not `logos:search`): MCP hosts
//! namespace tools by *server identity* — this server identifies as `logos`
//! in [`ServerHandler::get_info`], so hosts render `logos:<tool>` (FR-MC-01,
//! "the host namespaces them"; also the MCP tool-name SEP forbids `:`).

use std::sync::Arc;
use std::time::Instant;

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
const INSTRUCTIONS: &str = include_str!("instructions.md");

/// The Logos MCP server — a pure protocol adapter (ADR-01): every tool
/// delegates to exactly one [`Engine`] method and returns its `Serialize`
/// read-model as JSON content; no business logic lives here (FR-MC-02).
#[derive(Clone)]
pub struct LogosMcp {
    /// The long-lived engine (ADR-04), shared with the blocking pool.
    engine: Arc<Engine>,
    tool_router: ToolRouter<Self>,
}

impl LogosMcp {
    /// Wrap a started (long-lived) [`Engine`] — one per worktree root
    /// (ADR-04, ADR-15, FR-WT-04). Accepts an `Arc` so the serve path can
    /// share the engine with the S-022 watcher it hosts alongside.
    pub fn new(engine: impl Into<Arc<Engine>>) -> Self {
        Self {
            engine: engine.into(),
            tool_router: Self::tool_router(),
        }
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
        let engine = Arc::clone(&self.engine);
        let started = Instant::now();
        let outcome = tokio::task::spawn_blocking(move || call(&engine)).await;
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

// ── The 28 tools (FR-MC-01) ────────────────────────────────────────────────

#[tool_router]
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

    #[tool(description = "Direct callers of a symbol (FR-NV-05).")]
    async fn callers(
        &self,
        Parameters(p): Parameters<EdgeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("callers", move |e| e.callers(&p.symbol, p.limit))
            .await
    }

    #[tool(description = "Direct callees of a symbol (FR-NV-05).")]
    async fn callees(
        &self,
        Parameters(p): Parameters<EdgeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("callees", move |e| e.callees(&p.symbol, p.limit))
            .await
    }

    #[tool(
        description = "Transitive impact of changing a symbol, both directions labeled: upstream breaks-if-changed, downstream depends-on (FR-NV-06)."
    )]
    async fn impact(
        &self,
        Parameters(p): Parameters<ImpactParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("impact", move |e| e.impact(&p.symbol, p.depth))
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

// `router = self.tool_router`: dispatch through the router built once in
// `new()` instead of the macro's default `Self::tool_router()`-per-request.
#[tool_handler(router = self.tool_router)]
impl ServerHandler for LogosMcp {
    fn get_info(&self) -> ServerInfo {
        // `logos` is the namespace authority: hosts derive `logos:<tool>`
        // from this identity (FR-MC-01); the instructions ride the
        // initialize response (FR-MC-03).
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("logos", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}
