//! The **governance** tool domain (S-167): nine `rig` tools, each wrapping one
//! existing [`Engine`] governance/quality read-model (FR-GV-02..08/14,
//! FR-GH-06) — the Governance-Analyst subagent's least-privilege set (S-174).
//!
//! None of the nine mutates source or policy files, and none writes outside
//! `.logos/` ([NFR-SE-04]). Two carry the engine's normal index bookkeeping:
//! `scan` persists a metric snapshot + the violation set, and `check_rules`
//! replaces the cached violations — both inside `.logos/logos.db`, byte-for-byte
//! the same writes the CLI/MCP `scan`/`check_rules` perform (so an agent run
//! does not diverge from a user run). `gate` runs with `save = false`, so it
//! computes the verdict without persisting a baseline snapshot. No new core
//! query: every `call` routes to a method the CLI/MCP surfaces already expose
//! (using a non-persisting `scan`/`check_rules` variant would require a new core
//! method, deferred — see [S-167] notes). A structural failure halts honestly
//! through [`ToolCallError::Engine`] ([NFR-CC-04]).
//!
//! [S-167]: ../../../docs/planning/journal.md#s-167-agent-tool-layer-graph-governance-and-sandboxed-source-tools-with-a-bounded-dispatch-loop
//!
//! [agent-core]: ../../../docs/specs/architecture/components/agent-core.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md

use std::sync::Arc;

use logos_core::governance::DsmGranularity;
use logos_core::history::HotspotReport;
use logos_core::models::{
    DocGapsReport, DsmReport, EvolutionReport, GateResult, HealthInfo, RulesReport, ScanResult,
    TestGapsReport,
};
use logos_core::Engine;
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::json;

use super::{run_engine_result, ToolCallError};

/// `reconcile` defaults to true (guaranteed-fresh aggregate run); an explicit
/// `no_reconcile: true` skips the pre-evaluation reconcile for tight loops.
fn reconcile(no_reconcile: Option<bool>) -> bool {
    !no_reconcile.unwrap_or(false)
}

/// The shared `{ no_reconcile? }` parameter schema for the reconcile-gated tools.
fn reconcile_only_parameters() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "no_reconcile": { "type": "boolean", "description": "Skip the pre-evaluation reconcile for a tight loop (default false)." }
        }
    })
}

/// The reconcile-gated tools share this argument shape.
#[derive(Debug, Default, Deserialize)]
pub struct ReconcileArgs {
    /// Skip the pre-evaluation reconcile (default false).
    #[serde(default)]
    pub no_reconcile: Option<bool>,
}

// ── scan ────────────────────────────────────────────────────────────────────

/// Full architecture-quality scan (reconcile-then-score).
#[derive(Clone)]
pub struct Scan {
    engine: Arc<Engine>,
}

impl Scan {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for Scan {
    const NAME: &'static str = "scan";
    type Error = ToolCallError;
    type Args = ReconcileArgs;
    type Output = ScanResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Full architecture-quality scan (reconcile-then-score): the \
                 0-10000 quality signal, rule violations, and a persisted snapshot. \
                 The freshness line reports what was reconciled."
                .to_string(),
            parameters: reconcile_only_parameters(),
        }
    }

    async fn call(&self, args: ReconcileArgs) -> Result<ScanResult, ToolCallError> {
        let reconcile = reconcile(args.no_reconcile);
        run_engine_result(self.engine.clone(), move |e| e.scan(reconcile)).await
    }
}

// ── check_rules ─────────────────────────────────────────────────────────────

/// Architecture-rules compliance report against `rules.toml`.
#[derive(Clone)]
pub struct CheckRules {
    engine: Arc<Engine>,
}

impl CheckRules {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for CheckRules {
    const NAME: &'static str = "check_rules";
    type Error = ToolCallError;
    type Args = ReconcileArgs;
    type Output = RulesReport;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Architecture-rules compliance report against rules.toml: \
                 constraints, layer ordering (unassigned files exempt), and boundary \
                 checks."
                .to_string(),
            parameters: reconcile_only_parameters(),
        }
    }

    async fn call(&self, args: ReconcileArgs) -> Result<RulesReport, ToolCallError> {
        let reconcile = reconcile(args.no_reconcile);
        // `rules_path = None` → the project's default `.logos/rules.toml`.
        run_engine_result(self.engine.clone(), move |e| e.check_rules(None, reconcile)).await
    }
}

// ── hotspots ────────────────────────────────────────────────────────────────

/// `hotspots` arguments (FR-GH-06).
#[derive(Debug, Default, Deserialize)]
pub struct HotspotsArgs {
    /// Cap the ranked files returned (default: all).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Rank only untested hotspots (default false).
    #[serde(default)]
    pub untested: Option<bool>,
}

/// Hotspot ranking: churn × structural complexity, with a coverage column.
#[derive(Clone)]
pub struct Hotspots {
    engine: Arc<Engine>,
}

impl Hotspots {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for Hotspots {
    const NAME: &'static str = "hotspots";
    type Error = ToolCallError;
    type Args = HotspotsArgs;
    type Output = HotspotReport;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Hotspot ranking: indexed files ranked by churn-rank × \
                 structural-complexity-rank, with a per-file coverage column. The \
                 advisory temporal+coverage tier — never moves the gate. Non-git or \
                 shallow repos return n/a with a notice."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "description": "Cap the ranked files returned (default: all)." },
                    "untested": { "type": "boolean", "description": "Rank only files with no fresh coverage (default false)." }
                }
            }),
        }
    }

    async fn call(&self, args: HotspotsArgs) -> Result<HotspotReport, ToolCallError> {
        let untested = args.untested.unwrap_or(false);
        run_engine_result(self.engine.clone(), move |e| {
            e.hotspots(args.limit, untested, false)
        })
        .await
    }
}

// ── test_gaps ───────────────────────────────────────────────────────────────

/// `test_gaps` arguments (FR-GV-08).
#[derive(Debug, Default, Deserialize)]
pub struct GapsArgs {
    /// Cap on listed gaps (default 50).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Skip the pre-evaluation reconcile (default false).
    #[serde(default)]
    pub no_reconcile: Option<bool>,
}

fn gaps_parameters(noun: &str) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "description": format!("Cap on listed {noun} (default 50).") },
            "no_reconcile": { "type": "boolean", "description": "Skip the pre-evaluation reconcile (default false)." }
        }
    })
}

/// Test-gap analysis: functions unreachable from any test node.
#[derive(Clone)]
pub struct TestGaps {
    engine: Arc<Engine>,
}

impl TestGaps {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for TestGaps {
    const NAME: &'static str = "test_gaps";
    type Error = ToolCallError;
    type Args = GapsArgs;
    type Output = TestGapsReport;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Test-gap analysis: non-test functions unreachable from any \
                 test node via calls BFS — static reachability, not execution \
                 coverage (carries the heuristic caveat). Ranked by blast radius."
                .to_string(),
            parameters: gaps_parameters("gaps"),
        }
    }

    async fn call(&self, args: GapsArgs) -> Result<TestGapsReport, ToolCallError> {
        let reconcile = reconcile(args.no_reconcile);
        run_engine_result(self.engine.clone(), move |e| e.test_gaps(args.limit, reconcile)).await
    }
}

// ── dsm ─────────────────────────────────────────────────────────────────────

/// `dsm` arguments (FR-GV-07).
#[derive(Debug, Default, Deserialize)]
pub struct DsmArgs {
    /// Matrix granularity: "module" (default) or "file".
    #[serde(default)]
    pub granularity: Option<String>,
    /// Skip the pre-evaluation reconcile (default false).
    #[serde(default)]
    pub no_reconcile: Option<bool>,
}

/// Dependency structure matrix.
#[derive(Clone)]
pub struct Dsm {
    engine: Arc<Engine>,
}

impl Dsm {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for Dsm {
    const NAME: &'static str = "dsm";
    type Error = ToolCallError;
    type Args = DsmArgs;
    type Output = DsmReport;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Dependency structure matrix: cell (i,j) counts dependency \
                 edges i->j; rows ordered by layer order then name. Module \
                 granularity by default."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "granularity": { "type": "string", "enum": ["module", "file"], "description": "Matrix granularity (default module)." },
                    "no_reconcile": { "type": "boolean", "description": "Skip the pre-evaluation reconcile (default false)." }
                }
            }),
        }
    }

    async fn call(&self, args: DsmArgs) -> Result<DsmReport, ToolCallError> {
        let granularity = match args.granularity.as_deref() {
            None => None,
            Some(token) => Some(token.parse::<DsmGranularity>().map_err(|reason| {
                ToolCallError::InvalidArgument(format!(
                    "{reason}; valid granularities: module, file"
                ))
            })?),
        };
        let reconcile = reconcile(args.no_reconcile);
        run_engine_result(self.engine.clone(), move |e| e.dsm(granularity, reconcile)).await
    }
}

// ── gate ────────────────────────────────────────────────────────────────────

/// `gate` arguments (FR-GV-04): an optional threshold override.
#[derive(Debug, Default, Deserialize)]
pub struct GateArgs {
    /// Override the pass/fail threshold (default: the configured gate threshold).
    #[serde(default)]
    pub threshold: Option<u32>,
    /// Skip the pre-evaluation reconcile (default false).
    #[serde(default)]
    pub no_reconcile: Option<bool>,
}

/// Quality-gate verdict (computed, never persisted by this read-only tool).
#[derive(Clone)]
pub struct Gate {
    engine: Arc<Engine>,
}

impl Gate {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for Gate {
    const NAME: &'static str = "gate";
    type Error = ToolCallError;
    type Args = GateArgs;
    type Output = GateResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Quality-gate verdict: the 0-10000 signal compared to the \
                 pass/fail threshold, pass/fail, and the contributing breakdown. \
                 Read-only — computes the verdict without persisting a snapshot."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "threshold": { "type": "integer", "minimum": 0, "maximum": 10000, "description": "Override the pass/fail threshold (default: configured)." },
                    "no_reconcile": { "type": "boolean", "description": "Skip the pre-evaluation reconcile (default false)." }
                }
            }),
        }
    }

    async fn call(&self, args: GateArgs) -> Result<GateResult, ToolCallError> {
        let reconcile = reconcile(args.no_reconcile);
        // `save = false`: the agent tool never persists a baseline snapshot —
        // it reports the verdict only (read-only, NFR-SE-04).
        run_engine_result(self.engine.clone(), move |e| {
            e.gate(args.threshold, false, reconcile)
        })
        .await
    }
}

// ── evolution ─────────────────────────────────────────────────────────────

/// `evolution` arguments (FR-GV-06).
#[derive(Debug, Default, Deserialize)]
pub struct EvolutionArgs {
    /// Snapshot window size (default 30).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Signal evolution over stored snapshots with per-metric deltas.
#[derive(Clone)]
pub struct Evolution {
    engine: Arc<Engine>,
}

impl Evolution {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for Evolution {
    const NAME: &'static str = "evolution";
    type Error = ToolCallError;
    type Args = EvolutionArgs;
    type Output = EvolutionReport;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Quality-signal evolution over stored snapshots with \
                 per-metric deltas (default window 30)."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "description": "Snapshot window size (default 30)." }
                }
            }),
        }
    }

    async fn call(&self, args: EvolutionArgs) -> Result<EvolutionReport, ToolCallError> {
        run_engine_result(self.engine.clone(), move |e| e.evolution(args.limit)).await
    }
}

// ── doc_gaps ────────────────────────────────────────────────────────────────

/// Documentation-gap analysis: exported symbols referenced by no doc section.
#[derive(Clone)]
pub struct DocGaps {
    engine: Arc<Engine>,
}

impl DocGaps {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for DocGaps {
    const NAME: &'static str = "doc_gaps";
    type Error = ToolCallError;
    type Args = GapsArgs;
    type Output = DocGapsReport;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Documentation-gap analysis: exported functions/methods \
                 referenced by no documentation section over doc->code edges \
                 (carries the reference-presence caveat)."
                .to_string(),
            parameters: gaps_parameters("gaps"),
        }
    }

    async fn call(&self, args: GapsArgs) -> Result<DocGapsReport, ToolCallError> {
        let reconcile = reconcile(args.no_reconcile);
        run_engine_result(self.engine.clone(), move |e| e.doc_gaps(args.limit, reconcile)).await
    }
}

// ── health ──────────────────────────────────────────────────────────────────

/// Architecture health: DB integrity, schema version, FTS coherence, counts.
#[derive(Clone)]
pub struct Health {
    engine: Arc<Engine>,
}

impl Health {
    /// Wrap a shared engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

impl Tool for Health {
    const NAME: &'static str = "health";
    type Error = ToolCallError;
    type Args = ReconcileArgs;
    type Output = HealthInfo;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Architecture health: DB integrity, schema version, FTS \
                 coherence, and graph counts. For INDEX freshness use the graph \
                 tools' status instead."
                .to_string(),
            parameters: reconcile_only_parameters(),
        }
    }

    async fn call(&self, args: ReconcileArgs) -> Result<HealthInfo, ToolCallError> {
        let reconcile = reconcile(args.no_reconcile);
        run_engine_result(self.engine.clone(), move |e| e.health(reconcile)).await
    }
}
