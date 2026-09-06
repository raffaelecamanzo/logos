//! `logos` binary — thin CLI adapter over [`logos_core::Engine`].
//!
//! This crate **must not contain business logic** (NFR-MA-02, ADR-01).
//! Its sole responsibilities are:
//!   1. Parse CLI arguments with clap v4 derive (FR-CL-01, FR-CL-05).
//!   2. Construct an [`Engine`] and call **exactly one** method per subcommand.
//!   3. Serialise the read-model to stdout (`--json` machine mode, FR-CL-02).
//!   4. Map outcomes to exit codes 0/1/2/3/4 — success / violation / usage /
//!      internal / no rules contract loaded (ADR-14, FR-CL-03, FR-EH-01,
//!      FR-GV-22).

use std::path::{Path, PathBuf};
use std::process;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use logos_core::{
    error::{self, CoreError},
    governance::DsmGranularity,
    init::InitOptions,
    model::NodeKind,
    observability, workspace, Engine,
};

mod dispatch;
mod workspace_init;
mod xservice;

use xservice::{WorkspaceCommands, XserviceCommands};

// ── Exit codes (FR-CL-03 / BR-09) ──────────────────────────────────────────

// Usage errors (2) are clap-owned end to end: parse failures by clap itself,
// config faults via the core's `ConfigError::EXIT_CODE` contract. Result-level
// violations (1) and the error-boundary mapping (2/3) are owned by the core's
// `Severity` classification (ADR-14); this surface only projects it.
const EXIT_VIOLATION: i32 = 1;

// ── Top-level CLI ──────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "logos",
    version,
    about = "Logos — structural code intelligence for AI-assisted development",
    long_about = None,
)]
struct Cli {
    /// Override the project root (defaults to the current directory).
    #[arg(long, global = true, value_name = "PATH")]
    project: Option<PathBuf>,

    /// Output results as machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,

    /// Suppress non-essential output (exit codes and --json still apply).
    #[arg(long, global = true)]
    quiet: bool,

    #[command(subcommand)]
    command: Commands,
}

// ── Subcommands (FR-CL-01) ─────────────────────────────────────────────────

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Initialise `.logos/` (config.toml, rules.toml, .gitignore) and the store.
    Init {
        /// Interactive setup: also inject the MCP server block into .mcp.json,
        /// write the managed CLAUDE.md block, and materialize the logos-wiki
        /// generation skill, prompting per step on a TTY (non-TTY takes those
        /// as yes and hooks as no).
        #[arg(short = 'i', long = "interactive")]
        interactive: bool,
        /// Install git hooks (core.hooksPath) syncing on commit/checkout/merge.
        #[arg(long)]
        hooks: bool,
        /// Turn a parent folder of sibling repos into a Logos workspace
        /// (FR-WS-02): discover member repos, gate their inclusion, run the
        /// non-clobber per-member `init`, write `logos.workspace.toml`, and
        /// inject one workspace MCP entry at the parent. Indexing is hybrid
        /// (background-warmed, lazy-fallback) — this never blocks on it.
        #[arg(long)]
        workspace: bool,
        /// With `--workspace`: skip the interactive approval gate and include
        /// every discovered candidate not dropped by `--exclude`.
        #[arg(long, requires = "workspace")]
        yes: bool,
        /// With `--workspace`: drop a candidate member whose name matches this
        /// glob (repeatable).
        #[arg(long, value_name = "GLOB", requires = "workspace")]
        exclude: Vec<String>,
    },
    /// Build or rebuild the full code-graph index.
    Index,
    /// **Internal, not a public CLI contract** (FR-CL-01): the bounded
    /// workspace warm supervisor `init --workspace` spawns detached
    /// (FR-WS-14, BR-44). Hidden from help output — it exists because the
    /// warm must outlive its parent, so it has to be a process, and the
    /// binary re-invokes itself for it. Do not script against it.
    #[command(name = crate::workspace_init::SUPERVISOR_COMMAND, hide = true)]
    InternalWarm {
        /// The bound K the parent resolved; absent falls back to the
        /// core-derived default.
        #[arg(long)]
        concurrency: Option<usize>,
        /// The member repository roots to warm, in queue order.
        members: Vec<PathBuf>,
    },
    /// Incrementally sync changed files into the index.
    Sync {
        /// Paths to sync (defaults to all changed files).
        paths: Vec<PathBuf>,
    },
    /// Show the current index and sync health.
    Status,
    /// Full-text search over the code graph.
    Search {
        /// Search query string.
        query: String,
        /// Filter by node kind (e.g. function, struct, route).
        #[arg(long)]
        kind: Option<NodeKind>,
        /// Maximum number of results (default 20).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Query a symbol — a façade over search/callers/callees (FR-CL-05).
    Query {
        /// Symbol or name to query.
        symbol: String,
        /// Filter the search by node kind.
        #[arg(long, conflicts_with_all = ["callers", "callees"])]
        kind: Option<NodeKind>,
        /// List the symbol's direct callers instead of searching.
        #[arg(long, conflicts_with = "callees")]
        callers: bool,
        /// List the symbol's direct callees instead of searching.
        #[arg(long)]
        callees: bool,
        /// Maximum number of results.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Deterministic context bundle for a task (the token-saving tool).
    Context {
        /// Task description (multiple words allowed).
        #[arg(required = true)]
        task: Vec<String>,
        /// Cap the bundle size (default 25).
        #[arg(long)]
        max_nodes: Option<usize>,
        /// Omit declaration source from the bundle.
        #[arg(long)]
        no_code: bool,
    },
    /// Explore a symbol's neighbourhood, source grouped by file.
    Explore {
        /// Symbol or name to anchor the walk on.
        query: String,
        /// Cap the file groups returned (default 10).
        #[arg(long)]
        max_files: Option<usize>,
    },
    /// Full info for one symbol: metadata, edges, optional code.
    Node {
        /// SCIP symbol string or name to look up.
        symbol: String,
        /// Include the declaration source.
        #[arg(long)]
        code: bool,
    },
    /// Direct callers of a symbol.
    Callers {
        /// Symbol whose callers to list.
        symbol: String,
        /// Maximum number of results (default 50).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Direct callees of a symbol.
    Callees {
        /// Symbol whose callees to list.
        symbol: String,
        /// Maximum number of results (default 50).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Transitive impact of changing a symbol, both directions labeled.
    Impact {
        /// Symbol whose impact to trace.
        symbol: String,
        /// Traversal depth bound (default 3).
        #[arg(long)]
        depth: Option<usize>,
    },
    /// Which planned work items collide, and on what (FR-NV-11): given work
    /// items each naming the symbols it intends to change, the pairs whose
    /// transitive impact sets intersect, the pairs that are safely parallel,
    /// and the coverage limits of that verdict.
    #[command(name = "impact-intersection", alias = "impact_intersection")]
    ImpactIntersection {
        /// A work item, repeatable: `<id>=<symbol>[,<symbol>...]`. Repeating an
        /// id accumulates its symbols (also the escape hatch for a symbol that
        /// contains a comma).
        #[arg(long = "item", value_name = "SPEC", required = true)]
        items: Vec<String>,
        /// Traversal depth bound for every impact set (default 3).
        #[arg(long)]
        depth: Option<usize>,
    },
    /// Structurally analogous code — the sibling that already does this
    /// (FR-NV-12): nodes sharing a supertype, a registration, or a call shape
    /// with the target, each naming why. Ranked by counted graph facts, never
    /// a score; an empty answer states its reason.
    Precedent {
        /// Symbol or project-relative file whose structural precedents to find.
        target: String,
        /// Maximum number of precedents (default 20, capped at 100).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Which git refs collide, and what a merge did not carry (FR-NV-13):
    /// given the refs about to be merged, the symbols more than one of them
    /// modifies; given `--merge`, the symbols and files a ref changed that the
    /// stated merge result does not. A clean merge is not a complete merge.
    #[command(name = "branch-overlap", alias = "branch_overlap")]
    BranchOverlap {
        /// A git ref, repeatable: a branch, tag or commit — anything
        /// `git rev-parse` accepts.
        #[arg(long = "ref", value_name = "REF", required = true)]
        refs: Vec<String>,
        /// Comparison point (default: the merge-base of the supplied refs).
        #[arg(long)]
        base: Option<String>,
        /// A stated merge result to check the refs' work against.
        #[arg(long)]
        merge: Option<String>,
    },
    /// Files affected by a changed set — whole reverse-transitive closure.
    Affected {
        /// Changed files (project-relative paths).
        #[arg(required = true)]
        files: Vec<String>,
        /// Narrow the closure to test-marked files.
        #[arg(long)]
        tests_only: bool,
    },
    /// Code that implements a documentation node or requirement (FR-NV-10):
    /// the code symbols a doc node points at over doc→code edges.
    Implements {
        /// Documentation node, requirement, or heading whose implementing
        /// code to list (canonical symbol or human-facing name).
        doc: String,
    },
    /// Documentation sections that reference a symbol (FR-NV-10): the docs a
    /// change to the symbol may oblige updating.
    #[command(name = "referencing-docs", alias = "referencing_docs")]
    ReferencingDocs {
        /// Symbol whose referencing docs to list.
        symbol: String,
    },
    /// Full architecture-quality scan (reconcile-then-score).
    Scan {
        /// Restrict the scan to a path (not supported yet — whole project).
        path: Option<PathBuf>,
        /// Skip the pre-evaluation reconcile (tight inner loops, FR-RC-04);
        /// the freshness line marks the result assumed-fresh.
        #[arg(long, alias = "assume-fresh")]
        no_reconcile: bool,
    },
    /// Architecture-rules compliance check; error violations exit 1, no
    /// rules contract loaded exits 4 (FR-GV-22).
    Check {
        /// Alternate rules file (defaults to .logos/rules.toml).
        #[arg(long, value_name = "FILE")]
        rules: Option<PathBuf>,
        /// Skip the pre-evaluation reconcile (FR-RC-04).
        #[arg(long, alias = "assume-fresh")]
        no_reconcile: bool,
        /// Restore exit 0 when no rules.toml contract was loaded (FR-GV-22),
        /// for callers that have deliberately authored no contract yet.
        #[arg(long)]
        allow_no_rules: bool,
    },
    /// Non-blocking quality readout for the report tier (FR-IN-07, CR-095):
    /// the freshly computed signal, the blessed baseline and their delta, plus
    /// the last recorded `check` findings — computed **without writing**, so it
    /// never appends to the evolution series or takes the graph write lock.
    /// Always exits 0; it reports, it never gates.
    #[command(name = "quality-report", alias = "quality_report")]
    QualityReport {
        /// Emit the agent-host session-start hook payload instead of the plain
        /// read-model. This is what the installed hook script runs; the shape is
        /// the host's, so it is built here rather than assembled in shell.
        #[arg(long)]
        hook_json: bool,
    },
    /// CI gate: regression vs the saved baseline (or under --threshold) exits 1.
    Gate {
        /// Required signal floor (0–10000); below it the gate fails.
        #[arg(long)]
        threshold: Option<u32>,
        /// Save this run's snapshot as the new baseline instead of gating.
        #[arg(long)]
        save: bool,
        /// Label for the saved snapshot.
        #[arg(long, requires = "save")]
        label: Option<String>,
        /// Skip the pre-evaluation reconcile (FR-RC-04).
        #[arg(long, alias = "assume-fresh")]
        no_reconcile: bool,
    },
    /// ARCHITECTURE health: DB integrity, schema version, FTS coherence,
    /// structural integrity, admission-tripwire drift (FR-GV-18/FR-GV-20) and
    /// graph counts; an unhealthy graph exits 1. For INDEX freshness use
    /// `status`.
    Health {
        /// Skip the pre-evaluation reconcile (FR-RC-04).
        #[arg(long, alias = "assume-fresh")]
        no_reconcile: bool,
    },
    /// Begin a quality session (FR-GV-04): record the baseline BEFORE edits —
    /// the CLI half of the session gate the MCP instructions call mandatory.
    #[command(name = "session-start", alias = "session_start")]
    SessionStart,
    /// End the quality session (FR-GV-05): re-score and compare to the
    /// baseline; an aggregate regression beyond epsilon exits 1.
    #[command(name = "session-end", alias = "session_end")]
    SessionEnd,
    /// Fast graph structural-integrity check (CR-052, FR-GV-18): asserts one
    /// node per symbol_id and zero orphan rows; drift exits 1.
    Doctor,
    /// Deep graph consistency check (CR-052, FR-GV-19): reindex into a throwaway
    /// shadow store and diff node/edge/file counts + symbol sets against the live
    /// graph; reports leaked/orphaned symbols and drift, exits 1 on drift.
    Verify,
    /// Signal evolution over stored snapshots.
    Evolution {
        /// Snapshot window size (default 30).
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Dependency structure matrix (module rollup by default).
    Dsm {
        /// Matrix granularity: module (default) or file.
        /// An unknown value is a clap value error → usage exit 2 (FR-CL-03).
        #[arg(long, value_parser = <DsmGranularity as std::str::FromStr>::from_str)]
        granularity: Option<DsmGranularity>,
        /// Skip the pre-evaluation reconcile (FR-RC-04).
        #[arg(long, alias = "assume-fresh")]
        no_reconcile: bool,
    },
    /// Undocumented exported functions (static doc-gap analysis, FR-GV-14).
    #[command(name = "doc-gaps", alias = "doc_gaps")]
    DocGaps {
        /// Cap on listed gaps (default 50).
        #[arg(long)]
        limit: Option<u32>,
        /// Skip the pre-evaluation reconcile (FR-RC-04).
        #[arg(long, alias = "assume-fresh")]
        no_reconcile: bool,
    },
    /// Hotspot ranking: files high in both churn and structural complexity
    /// (the non-gated temporal tier — never moves the gate, FR-GH-06/BR-26).
    Hotspots {
        /// Cap the ranked files returned (default: all).
        #[arg(long)]
        limit: Option<usize>,
        /// Rank only untested hotspots (no fresh execution coverage); falls back
        /// to the labeled static-reachability signal when no coverage is ingested.
        #[arg(long)]
        untested: bool,
        /// Drop whole test files (`is_test`-only) from the candidate set before
        /// ranking (CR-076); default off — the whole-repo board is unchanged.
        #[arg(long)]
        production_scope: bool,
    },
    /// Coverage evidence tier: ingest external reports and read freshness-checked
    /// status (the non-gated coverage tier — never moves the gate, BR-28).
    Coverage {
        #[command(subcommand)]
        command: CoverageCommands,
    },
    /// Source wiki: write/read/search/status/delete agent-generated pages in
    /// the gate-immune `.logos/wiki.db` store (CR-008). Every read carries
    /// mandatory provenance (generator, written-at HEAD, per-anchor freshness,
    /// the fixed generated-content marker).
    Wiki {
        #[command(subcommand)]
        command: WikiCommands,
    },
    /// Cross-service workspace queries (federation, FR-WS-05): route-providers,
    /// callers, impact, search — each with an optional `--repo` member filter.
    Xservice {
        #[command(subcommand)]
        command: XserviceCommands,
    },
    /// Workspace-level commands (federation, FR-WS-05): `status` reports
    /// per-member freshness + the 3-state cross-service coverage summary.
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommands,
    },
    /// Aggregated usage/performance statistics.
    Stats {
        /// Trailing window in days (default 7).
        #[arg(long, value_name = "DAYS")]
        window: Option<u32>,
    },
    /// List registered language grammars.
    Languages,
    /// Start a long-lived server: the stdio MCP surface and/or — in `ui`
    /// builds — the localhost web dashboard, over one Engine and one watcher.
    Serve {
        /// Run the stdio MCP server. In the slim build (`--no-default-features
        /// --features lang-all`) this is the only serve mode (required); in a
        /// `ui` build (the default, S-287) it may pair with `--ui`.
        #[cfg_attr(not(feature = "ui"), arg(long, required = true))]
        #[cfg_attr(feature = "ui", arg(long, required_unless_present = "ui"))]
        mcp: bool,
        /// Serve the localhost web dashboard on `127.0.0.1` (CR-012, ADR-27).
        /// Combine with `--mcp` to run both surfaces in one process.
        #[cfg(feature = "ui")]
        #[arg(long, required_unless_present = "mcp")]
        ui: bool,
        /// Web dashboard port (default 4983); loopback bind is not overridable.
        #[cfg(feature = "ui")]
        #[arg(long, default_value_t = web::DEFAULT_PORT)]
        port: u16,
        /// Force single-repo focus even under a workspace manifest (FR-WS-06):
        /// serve this repo alone, byte-for-byte as a plain single-root serve,
        /// skipping workspace discovery and the `/api/v1/workspace/*` surface.
        #[cfg(feature = "ui")]
        #[arg(long)]
        standalone: bool,
    },
}

/// `coverage` sub-subcommands (FR-CV-05/06): ingest external reports, read status.
#[derive(Subcommand)]
pub(crate) enum CoverageCommands {
    /// Ingest an LCOV/Cobertura report into the evidence store (FR-CV-01).
    Ingest {
        /// Path to the coverage report file.
        report: PathBuf,
        /// Force the report format ("lcov" or "cobertura"); default auto-detects.
        #[arg(long)]
        format: Option<String>,
    },
    /// Per-file coverage freshness + the overall fraction (FR-CV-05/06).
    Status,
    /// Run the configured `[coverage_ingest].refresh_cmd` and ingest its output
    /// (FR-CV-10). The lone explicit coverage subprocess — never on the serve
    /// path (ADR-38). Errors if no `refresh_cmd` is configured.
    Refresh,
}

/// `wiki` sub-subcommands (FR-WK-02/04/05/06/07): the read/write/search surface
/// over the wiki store. `write`/`read`/`search`/`status`/`materialize` have
/// payload-identical MCP twins (FR-WK-09); `delete` is CLI-only (destructive,
/// off the agent surface).
#[derive(Subcommand)]
pub(crate) enum WikiCommands {
    /// Upsert a page by slug: byte-verbatim body (1 MiB cap), write-time anchor
    /// resolution, mandatory generator label (FR-WK-02).
    Write {
        /// The page slug (path-like: lowercase/digit/`-`/`_` segments).
        slug: String,
        /// The page title.
        #[arg(long, short = 't')]
        title: String,
        /// The mandatory generator label (e.g. the model/tool that wrote it).
        #[arg(long, short = 'g')]
        generator: String,
        /// Anchor entity id, repeatable: `file:<path>` or `symbol:<symbol>`.
        #[arg(long = "anchor", value_name = "ID")]
        anchors: Vec<String>,
        /// Read the markdown body from a file (`-` reads stdin); otherwise pass
        /// the body as the positional argument.
        #[arg(long, value_name = "PATH", conflicts_with = "body")]
        body_file: Option<PathBuf>,
        /// The markdown body (when `--body-file` is not used).
        body: Option<String>,
    },
    /// Read a page by slug with mandatory provenance + per-anchor freshness
    /// (FR-WK-04). A miss (or an all-anchors-gone auto-prune) exits non-zero.
    Read {
        /// The slug to read.
        slug: String,
    },
    /// FTS5 bm25 search over page titles + bodies, staleness-flagged (FR-WK-05).
    Search {
        /// The search query (omit with `--list`).
        query: Option<String>,
        /// Enumerate all pages instead of searching.
        #[arg(long)]
        list: bool,
    },
    /// Store summary + regeneration work-list: stale, missing-anchor, pruned,
    /// and page-worthy entities without a page (FR-WK-06).
    Status,
    /// Format the `wiki status` work-list into an ordered, offline generation
    /// queue (FR-WK-13): a human-readable prompt block by default, or `--json`
    /// for machines. Each item carries its target slug and a runnable `wiki
    /// write` skeleton. A pure read — no `wiki.db` write, no LLM, no network.
    Generate,
    /// Deterministically assemble the presented tier (FR-WK-20, ADR-57): in SRS
    /// mode, present each Design/Specs category (and the single-file
    /// Architecture page) from the project's authored `docs/specs/**` sources
    /// into `wiki.db` with `generator = "logos:doc-present"`, then run the
    /// reconciliation sweep. A pure deterministic write — no LLM, no network
    /// (NFR-SE-01); byte-identical on re-run. Outside SRS mode (Case 2) this is
    /// a no-op. Run automatically by the UI-gated generation flow ahead of the
    /// LLM queue (FR-WK-18); safe to run manually.
    Materialize,
    /// Explicitly delete a page by slug (FR-WK-07); an unknown slug exits non-zero.
    Delete {
        /// The slug to delete.
        slug: String,
    },
    /// Materialize the embedded wiki-generation skill (FR-WK-08): the canonical
    /// `.agents/skills/logos-wiki/` directory plus the `.claude/skills/logos-wiki`
    /// symlink. Refreshes an existing install or a post-upgrade skill. CLI-only.
    Skill {
        /// Emit the embedded skill (the only `skill` operation; required so the
        /// verb reads `wiki skill --emit`).
        #[arg(long, required = true)]
        emit: bool,
        /// Target base directory (defaults to the project root).
        dir: Option<PathBuf>,
        /// Overwrite an existing install, restoring the embedded content.
        #[arg(long)]
        force: bool,
    },
    /// Install the Claude Code session-start quality-report hook (FR-IN-07,
    /// ADR-49): a marker-tagged hook script plus a non-clobbering merge into
    /// the shared `.claude/settings.json` that surfaces a non-blocking
    /// signal/baseline/violations readout when a session starts, resumes, or is
    /// reopened by `/clear`. Also sweeps the retired SessionEnd hook and its
    /// orphaned script (CR-095). The binary stays offline (NFR-SE-01). Re-emit
    /// with `--force`. CLI-only. (The PostToolUse wiki-augmentation hook this
    /// once also installed was retired — CR-070.)
    Hook {
        /// Emit the hook (the only `hook` operation; required so the verb reads
        /// `wiki hook --emit`).
        #[arg(long, required = true)]
        emit: bool,
        /// Re-emit, replacing an existing managed SessionStart entry.
        #[arg(long)]
        force: bool,
    },
}

// ── Entry point ────────────────────────────────────────────────────────────

fn main() {
    // Widen this process's descriptor allowance before anything opens a file
    // (S-324, NFR-PE-11, ADR-63). Best-effort and silent: a workspace's live
    // connections are bounded by the core's WorkspaceBudget, so a kernel that
    // refuses the raise costs residency, never correctness — which is exactly
    // why this is not allowed to report anything.
    logos_core::fdlimit::raise_open_file_limit();

    let cli = Cli::parse();
    // Defence in depth: a panic crossing the bin boundary is an internal
    // error by definition (FR-CL-03) — the default hook has already printed
    // the payload to stderr.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(cli)));
    process::exit(match outcome {
        Ok(Ok(code)) => code,
        Ok(Err(err)) => {
            eprintln!("error: {err:#}");
            error::exit_code(&err)
        }
        Err(_) => CoreError::EXIT_INTERNAL,
    });
}

/// Dispatch: one Engine method call per subcommand (NFR-MA-02, ADR-01).
fn run(cli: Cli) -> Result<i32> {
    // Resolve the hint (cwd or --project) to the working-tree root ONCE, so
    // telemetry, the index guard, and the engine all agree on where `.logos/`
    // lives (FR-WT-01, NFR-CC-02). Delegated to the core (NFR-MA-02); outside
    // git the hint is used verbatim.
    let root = workspace::resolve_root(&cli.project.unwrap_or_else(|| PathBuf::from(".")));

    // Logs → stderr only, telemetry → telemetry.db; guard flushes on exit
    // (S-019, ADR-13). The surface stamp feeds the per-surface stats
    // breakdown (FR-OB-04): the serve path IS the MCP surface.
    let surface = match &cli.command {
        // A web-only serve session stamps surface=web; any session that owns
        // stdout for MCP (including the combined one) stamps surface=mcp.
        #[cfg(feature = "ui")]
        Commands::Serve { mcp: false, ui: true, .. } => observability::ProcessSurface::Web,
        Commands::Serve { .. } => observability::ProcessSurface::Mcp,
        _ => observability::ProcessSurface::Cli,
    };
    // The warm supervisor (FR-WS-14) is the one arm that initialises NOTHING:
    // it emits no telemetry event (no `traced` span is reachable from it — it
    // opens no Engine), yet unlike every other command it lives for the whole
    // serialized warm, minutes on a large workspace. Initialising here would
    // open — and migrate, and hold for that whole lifetime — a connection to
    // `.logos/telemetry.db` at whatever root the detached child inherited,
    // creating a store file at a parent-of-repos root that has no index
    // (the CR-098 state) and costing two `git` subprocesses per warm. Skipping
    // it is what makes "the supervisor holds no `.logos` store" literally
    // true rather than nearly true.
    let _telemetry = match &cli.command {
        Commands::InternalWarm { .. } => None,
        _ => Some(observability::init(surface, &root)),
    };

    let out = Output {
        json: cli.json,
        quiet: cli.quiet,
    };

    // Dispatch is delegated to per-domain handlers in `dispatch` so this
    // entry point stays a thin setup wrapper (NFR-MA-02); the split also keeps
    // each function under the max_cc / max_fn_lines gates.
    dispatch::dispatch(cli.command, &root, &out)
}

// ── Surface helpers (no business logic) ────────────────────────────────────

/// A started engine for graph commands. Everything except `index` (which
/// bootstraps the store) requires an existing index — or a way to create one:
/// a DB-less linked worktree with a seedable primary DB is served, not
/// refused (`Engine::start` seeds it, FR-WT-03). Otherwise a missing index is
/// an actionable error naming the remedy, mapped to exit 3 (FR-EH-01).
pub(crate) fn engine(root: &Path, bootstrap: bool) -> Result<Engine> {
    if !bootstrap
        && !root.join(".logos").join("logos.db").exists()
        && workspace::seed_source(root).is_none()
    {
        // A typed Correctness fault (ADR-14): `error::exit_code` maps it to the
        // internal exit 3, and the message names the remedy (FR-EH-01).
        return Err(CoreError::NoIndex {
            root: root.to_path_buf(),
        }
        .into());
    }
    Engine::start(root)
}

/// Resolve which init steps run (S-023, FR-IN-02/03, FR-WK-08, FR-IN-07) —
/// pure surface UX, the step logic itself lives in the core. `-i` enables the
/// host-integration steps, prompting per step on a TTY; non-TTY takes the safe
/// defaults (MCP + CLAUDE.md + the wiki skill + the session-start
/// quality-report hook — yes, that's what `-i` asks for — git hooks no: they rewire
/// core.hooksPath, so they stay opt-in via --hooks). The PostToolUse
/// wiki-augmentation hook `-i` once also installed here was retired (CR-070).
pub(crate) fn init_options(interactive: bool, hooks: bool) -> InitOptions {
    InitOptions {
        inject_mcp: interactive && ask("inject the logos MCP server block into .mcp.json?", true),
        write_claude_md: interactive && ask("generate the managed CLAUDE.md block?", true),
        install_hooks: hooks || (interactive && ask("install git hooks (core.hooksPath)?", false)),
        materialize_skill: interactive && ask("materialize the logos-wiki generation skill?", true),
        install_quality_report_hook: interactive
            && ask("install the Claude Code session-start quality-report hook?", true),
    }
}

/// One y/n prompt on stderr (stdout stays machine-clean, FR-CL-02); a
/// non-TTY stdin or a read failure resolves to `default` without prompting.
/// `pub(crate)`: also the `logos init --workspace` per-candidate approval
/// gate ([`crate::workspace_init`], FR-WS-02).
pub(crate) fn ask(question: &str, default: bool) -> bool {
    use std::io::{BufRead, IsTerminal, Write};
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return default;
    }
    eprint!("{question} {} ", if default { "[Y/n]" } else { "[y/N]" });
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if stdin.lock().read_line(&mut line).is_err() {
        return default;
    }
    match line.trim().to_ascii_lowercase().as_str() {
        "" => default,
        s => s == "y" || s == "yes",
    }
}

/// Resolve the `wiki write` body from either `--body-file` (a path, or `-` for
/// stdin) or the positional argument — exactly one source. Reading a file/stdin
/// is surface I/O, not business logic (the core takes the resolved `&str`); a
/// large markdown body would otherwise blow past the shell's argv limit.
///
/// This is the external-agent write surface the content-validity guard
/// (FR-WK-19) protects: the resolved body is handed unchanged to
/// [`Engine::wiki_write`](logos_core::Engine::wiki_write), whose façade rejects
/// agent-noise before it reaches the store — so the guard applies identically
/// here and to the in-process run, with no separate check needed on this path.
pub(crate) fn read_wiki_body(body: Option<String>, body_file: Option<PathBuf>) -> Result<String> {
    match (body, body_file) {
        (_, Some(path)) if path.as_os_str() == "-" => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            Ok(buf)
        }
        (_, Some(path)) => Ok(std::fs::read_to_string(&path)?),
        (Some(body), None) => Ok(body),
        (None, None) => {
            bail!("wiki write needs a body: pass it as an argument or via --body-file <PATH|->")
        }
    }
}

/// Read-model printer: `--json` always emits compact machine output (it IS
/// the essential output); the human rendering (pretty JSON until dedicated
/// formatters land) is what `--quiet` suppresses (FR-CL-02).
pub(crate) struct Output {
    pub(crate) json: bool,
    pub(crate) quiet: bool,
}

impl Output {
    pub(crate) fn print<T: serde::Serialize>(&self, value: &T) -> Result<()> {
        if self.json {
            println!("{}", serde_json::to_string(value)?);
        } else if !self.quiet {
            println!("{}", serde_json::to_string_pretty(value)?);
        }
        Ok(())
    }

    /// Dispatch chokepoint for the dominant arm shape: index-guarded engine →
    /// **infallible** read-model → print → success. Mirrors the MCP adapter's
    /// `run` delegator (mcp/src/server.rs) so each dispatch arm carries no
    /// error-propagation of its own — keeping the arms' cyclomatic complexity at
    /// zero (the `?`/exit-code logic lives here, once).
    pub(crate) fn query<T: serde::Serialize>(
        &self,
        root: &Path,
        f: impl FnOnce(&Engine) -> T,
    ) -> Result<i32> {
        self.print(&f(&engine(root, false)?))?;
        Ok(0)
    }

    /// As [`Output::query`] for a **fallible** engine method (the MCP `run_result`
    /// twin): the inner `?` propagates the engine fault to the exit-code boundary.
    pub(crate) fn try_query<T: serde::Serialize>(
        &self,
        root: &Path,
        f: impl FnOnce(&Engine) -> Result<T>,
    ) -> Result<i32> {
        self.print(&f(&engine(root, false)?)?)?;
        Ok(0)
    }

    /// Chokepoint for the governance verdict commands (`check`/`gate`/`doctor`/
    /// `verify`): run a fallible engine method, print the report, and map it to an
    /// exit code (FR-GV-03) — 1 on failure. `passed` reads the report's verdict
    /// field, which differs by command (`.passed` vs `.ok`), so the caller
    /// supplies it.
    pub(crate) fn report_gate<T: serde::Serialize>(
        &self,
        root: &Path,
        f: impl FnOnce(&Engine) -> Result<T>,
        passed: impl FnOnce(&T) -> bool,
    ) -> Result<i32> {
        let report = f(&engine(root, false)?)?;
        self.print(&report)?;
        Ok(violation_code(passed(&report)))
    }

    /// `check` alone (FR-GV-22): the exit code is [`RulesReport::exit_code`]'s
    /// three-state projection; the human surface names the absent-contract
    /// state instead of rendering it as a zero violation count, since
    /// "nothing was evaluated" is not the same claim as a clean evaluation
    /// (NFR-CC-04).
    pub(crate) fn report_check(
        &self,
        root: &Path,
        f: impl FnOnce(&Engine) -> Result<logos_core::models::RulesReport>,
        allow_absent: bool,
    ) -> Result<i32> {
        let report = f(&engine(root, false)?)?;
        if !self.json && !self.quiet && report.passed.is_none() {
            println!("no rules contract found — nothing was evaluated");
        } else {
            self.print(&report)?;
        }
        Ok(report.exit_code(allow_absent))
    }
}

/// Map a result-level violation to its exit code: rule/gate failure → 1
/// (FR-CL-03, UAT-CL-02).
const fn violation_code(passed: bool) -> i32 {
    if passed {
        0
    } else {
        EXIT_VIOLATION
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// clap's own self-check: argument ids, conflicts, and requirements are
    /// consistent across the whole derive (catches a bad `conflicts_with` at
    /// test time instead of first invocation).
    #[test]
    fn cli_definition_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    /// The violation path of FR-CL-03/UAT-CL-02: rule/gate failure → exit 1.
    /// End-to-end coverage activates when S-020 wires `gate`/`check`; the
    /// mapping itself is pinned here.
    #[test]
    fn violations_map_to_exit_one() {
        assert_eq!(violation_code(true), 0);
        assert_eq!(violation_code(false), EXIT_VIOLATION);
    }

    /// ADR-14 boundary mapping is owned by the core (`error::exit_code`); the
    /// surface only delegates. A typed missing-index fault is internal (3); an
    /// untyped failure is internal too; the core unit tests pin config → 2.
    #[test]
    fn errors_map_through_the_core_classifier() {
        let no_index = anyhow::Error::new(CoreError::NoIndex {
            root: PathBuf::from("/nowhere"),
        });
        assert_eq!(error::exit_code(&no_index), CoreError::EXIT_INTERNAL);
        assert_eq!(error::exit_code(&anyhow::anyhow!("boom")), CoreError::EXIT_INTERNAL);
    }

    /// `--kind` is wired to the ontology's own `FromStr` (no adapter-side
    /// parser): a canonical wire name parses, and garbage is a clap value
    /// error → usage exit 2 (FR-CL-03). The round-trip over all 37 kinds and
    /// the enumerating message are asserted where the vocabulary lives,
    /// `logos_core::model::kinds`.
    #[test]
    fn kind_flag_parses_through_the_ontology_from_str() {
        let cli = Cli::try_parse_from(["logos", "search", "q", "--kind", "function"])
            .expect("a canonical wire name parses");
        let Commands::Search { kind, .. } = cli.command else {
            panic!("expected Search");
        };
        assert_eq!(kind, Some(NodeKind::Function));

        // `match`, not `expect_err`: `Cli` derives no `Debug`.
        let err = match Cli::try_parse_from(["logos", "search", "q", "--kind", "nonsense"]) {
            Err(err) => err,
            Ok(_) => panic!("an unknown kind is a usage error"),
        };
        assert!(
            err.to_string().contains("function"),
            "clap surfaces the ontology's enumerating message: {err}"
        );
    }
}

// ── FR-CL-06 / CR-114: the two surfaces, enumerated against each other ──────

/// The shipped-surface reconciliation gate ([FR-CL-06]).
///
/// # Why this exists as an enumeration and not as a count
///
/// Every roster guard in this workspace used to be a hard-coded *number*
/// (`assert_eq!(tools.len(), 30)`). Sprint 64 iteration 4 showed what that
/// costs: two dev sessions each appended one tool at the same registration
/// points and each wrote the new roster count as `29`. Git auto-merged the two
/// identical edits **without a conflict**, so eleven guard assertions claimed
/// 29 while the shipped set was 30, and a human had to notice.
///
/// A *list* does not have that failure mode. Two sessions appending one line
/// each to [`MCP_SURFACE`] merge cleanly into two lines, and the truth this
/// test compares against is read live off the shipped router and the shipped
/// clap definition — there is no number for a merge to get wrong. What a
/// session must not forget is instead named for it: a tool added to one surface
/// and not the other fails here with the tool's own name in the message.
///
/// # What it asserts
///
/// [FR-CL-06] wants every tool to be reachable from both surfaces *or* to be
/// documented as deliberately absent with its reason. So:
///
/// 1. [`MCP_SURFACE`] covers exactly the tools the shipped server registers —
///    both directions, so neither a new tool nor a retired one can slip past.
/// 2. Every [`Reach::Twin`] names a CLI command path that really exists, and
///    every [`Reach::McpOnly`] carries a reason and really has no CLI command.
/// 3. Every CLI command that is *not* some tool's twin is listed in
///    [`CLI_ONLY`] with its reason — the same contract, the other way round.
/// 4. Every twin accepts `--json` ([FR-CL-02]).
/// 5. The managed `CLAUDE.md` block's CLI-twin sentence ([FR-IN-02]) names
///    exactly the MCP-only tools — so the claim shipped into every project's
///    agent memory cannot drift from the roster above it.
/// 6. All three shipped guidance texts ([FR-IN-09]) order scoping before
///    navigation, name the planning-time capabilities, and — the reason this
///    lives *here* rather than beside the prose — name no tool and no command
///    the shipped binary does not have.
///
/// [FR-CL-06]: ../../docs/specs/requirements/FR-CL-06.md
/// [FR-CL-02]: ../../docs/specs/requirements/FR-CL-02.md
/// [FR-IN-02]: ../../docs/specs/requirements/FR-IN-02.md
/// [FR-IN-09]: ../../docs/specs/requirements/FR-IN-09.md
#[cfg(test)]
mod surface_parity {
    use std::collections::{BTreeMap, BTreeSet};

    use clap::CommandFactory;
    use logos_core::{
        federation::{EngineRegistry, Federation, RegistryMode},
        init::InitOptions,
        Engine,
    };
    use mcp::LogosMcp;

    use super::Cli;

    /// How one MCP tool is reachable from the CLI.
    enum Reach {
        /// The CLI command path that runs the same `Engine` call, space-joined
        /// (`"scan"`, `"wiki read"`, `"xservice callers"`).
        Twin(&'static str),
        /// Deliberately MCP-only — the string is the reason, which [FR-CL-06]
        /// requires and this test requires to be non-empty.
        ///
        /// [FR-CL-06]: ../../docs/specs/requirements/FR-CL-06.md
        McpOnly(&'static str),
    }
    use Reach::{McpOnly, Twin};

    /// Every tool the shipped MCP server registers, and its CLI reachability.
    ///
    /// Ordered as `mcp/src/server.rs` registers them (navigation, quality,
    /// temporal, coverage, wiki, then the federated `xservice_*`/`workspace_*`
    /// family). **Adding a tool means adding a line here** — that is the whole
    /// contract, and the assertions below name the tool you forgot.
    const MCP_SURFACE: &[(&str, Reach)] = &[
        // Navigation.
        ("search", Twin("search")),
        ("context", Twin("context")),
        ("explore", Twin("explore")),
        ("node", Twin("node")),
        ("callers", Twin("callers")),
        ("callees", Twin("callees")),
        ("impact", Twin("impact")),
        ("impact_intersection", Twin("impact-intersection")),
        ("precedent", Twin("precedent")),
        ("branch_overlap", Twin("branch-overlap")),
        ("status", Twin("status")),
        // Quality / governance.
        ("scan", Twin("scan")),
        (
            "rescan",
            McpOnly(
                "replays the parameters of the LAST scan, which are held in \
                 process-lifetime `GovernanceState` — a one-shot CLI invocation \
                 has no earlier scan to replay, so `logos rescan` could only ever \
                 mean `logos scan`. The CLI spelling is `scan`.",
            ),
        ),
        ("check_rules", Twin("check")),
        ("evolution", Twin("evolution")),
        ("dsm", Twin("dsm")),
        ("health", Twin("health")),
        ("doctor", Twin("doctor")),
        ("verify", Twin("verify")),
        ("session_start", Twin("session-start")),
        ("session_end", Twin("session-end")),
        // Temporal + coverage evidence tiers.
        ("hotspots", Twin("hotspots")),
        ("coverage_ingest", Twin("coverage ingest")),
        ("coverage_status", Twin("coverage status")),
        ("coverage_refresh", Twin("coverage refresh")),
        // Source wiki.
        ("wiki_write", Twin("wiki write")),
        ("wiki_read", Twin("wiki read")),
        ("wiki_search", Twin("wiki search")),
        ("wiki_status", Twin("wiki status")),
        ("wiki_materialize", Twin("wiki materialize")),
        // Cross-service family — registered only by the federated backing.
        ("xservice_route_providers", Twin("xservice route-providers")),
        ("xservice_callers", Twin("xservice callers")),
        ("xservice_impact", Twin("xservice impact")),
        ("xservice_search", Twin("xservice search")),
        ("workspace_status", Twin("workspace status")),
        ("workspace_reachability", Twin("workspace reachability")),
        ("workspace_check", Twin("workspace check")),
    ];

    /// Every CLI command path that is deliberately **not** an MCP tool, and why.
    ///
    /// The mirror of [`MCP_SURFACE`]'s [`Reach::McpOnly`] arm: [FR-CL-06] asks
    /// for a documented reason on either side of the asymmetry, not just the
    /// MCP-only side.
    ///
    /// [FR-CL-06]: ../../docs/specs/requirements/FR-CL-06.md
    const CLI_ONLY: &[(&str, &str)] = &[
        (
            "init",
            "the bootstrap that INSTALLS the MCP server block, the managed \
             CLAUDE.md block and the git hooks (FR-IN-01/02/03) — it cannot be a \
             tool of the server it wires up.",
        ),
        (
            "index",
            "a full, always-purge graph rebuild measured in seconds-to-minutes. \
             MCP callers never need it: every tool auto-indexes through \
             `ensure_indexed`.",
        ),
        (
            "sync",
            "incremental freshening for git hooks and CI. Inside `serve --mcp` \
             the core-owned debounced watcher (FR-SY-04) already does it, so an \
             agent has nothing to issue.",
        ),
        (
            "query",
            "a CLI-ergonomics facade that picks ONE of search/callers/callees \
             from flags (FR-CL-05). MCP hosts call those three tools directly, \
             so exposing the facade would add a second spelling of each.",
        ),
        (
            "affected",
            "outside the FR-MC-01 tool roster: the changed-file-set query CI and \
             pre-push tooling run over a git diff. No CR has proposed a tool.",
        ),
        (
            "implements",
            "outside the FR-MC-01 tool roster — the FR-NV-10 traceability pair \
             ships on the CLI only. No CR has proposed a tool.",
        ),
        (
            "referencing-docs",
            "outside the FR-MC-01 tool roster — the other half of the FR-NV-10 \
             traceability pair. No CR has proposed a tool.",
        ),
        (
            "quality-report",
            "the non-blocking readout the installed session-start hook execs \
             (FR-IN-07, CR-095). It writes nothing and always exits 0; the \
             agent-facing gate is session_start/session_end.",
        ),
        (
            "gate",
            "the CI gate and the release-only `--save` bless. Its agent-facing \
             spellings ARE tools: `session_end` is literally `gate` with no save \
             (`Engine::session_end` calls `governance::gate(.., None, false, \
             true)`), and `session_start` is the separate \
             `Engine::session_start` baseline write next to it (FR-GV-04/05) — \
             a different read-model and a different telemetry tool, not a \
             spelling of `gate --save`.",
        ),
        (
            "doc-gaps",
            "outside the FR-MC-01 tool roster — the FR-GV-14 read-only \
             documentation-gap report. No CR has proposed a tool.",
        ),
        (
            "stats",
            "deliberately deferred by FR-MC-05: no `logos:stats` tool is \
             registered in v1. Counting a self-referential read from the agent \
             surface would also inflate the very figure it reports (FR-OB-09).",
        ),
        (
            "languages",
            "outside the FR-MC-01 tool roster — the compiled-in grammar registry \
             readout. No CR has proposed a tool.",
        ),
        (
            "serve",
            "the command that STARTS the MCP server. A tool for it would be the \
             server asking itself to exist.",
        ),
        (
            "wiki generate",
            "CLI-only by FR-WK-07: it formats the regeneration work-list into an \
             offline prompt block for a human or an agent session to act on — \
             the queue, not a store operation.",
        ),
        (
            "wiki delete",
            "CLI-only: a destructive store operation kept off the agent surface \
             (CR-008, the same rule that keeps `wiki skill`/`wiki hook` off it).",
        ),
        (
            "wiki skill",
            "CLI-only: an INSTALL operation (materialize the embedded skill into \
             `.agents/`/`.claude/`), not a question about the code.",
        ),
        (
            "wiki hook",
            "CLI-only: an INSTALL operation (write the Claude Code session-start \
             quality-report hook and merge `.claude/settings.json`).",
        ),
    ];

    /// Every command path the shipped CLI accepts — leaves only, space-joined,
    /// clap's generated `help` excluded. `hidden` selects whether `hide = true`
    /// commands are included.
    fn command_paths(hidden: bool) -> BTreeSet<String> {
        fn walk(cmd: &clap::Command, prefix: &str, hidden: bool, out: &mut BTreeSet<String>) {
            for sub in cmd.get_subcommands() {
                if (!hidden && sub.is_hide_set()) || sub.get_name() == "help" {
                    continue;
                }
                let path = if prefix.is_empty() {
                    sub.get_name().to_string()
                } else {
                    format!("{prefix} {}", sub.get_name())
                };
                if sub.get_subcommands().next().is_some() {
                    walk(sub, &path, hidden, out);
                } else {
                    out.insert(path);
                }
            }
        }
        let mut out = BTreeSet::new();
        walk(&Cli::command(), "", hidden, &mut out);
        out
    }

    /// The PUBLIC command surface — what `logos --help` offers. This is the set
    /// the two-way roster contract is written against.
    fn cli_command_paths() -> BTreeSet<String> {
        command_paths(false)
    }

    /// Every command the binary accepts, `hide = true` ones included. A hidden
    /// command is still shipped, so it can still falsify an "MCP-only" claim
    /// even though it is not part of the public roster contract.
    fn all_cli_command_paths() -> BTreeSet<String> {
        command_paths(true)
    }

    /// Every tool the shipped server registers, read off the real router.
    ///
    /// The **federated** backing, because it is the superset: it registers the
    /// single-root roster byte-identically plus the `xservice_*`/`workspace_*`
    /// family (asserted by `mcp/tests/xservice_roster.rs`), and both halves have
    /// CLI commands to reconcile against. Lazy mode starts no engine, so the
    /// empty member list costs nothing.
    fn mcp_tool_names() -> BTreeSet<String> {
        let federation = Federation {
            name: "w".to_string(),
            root: "/ws".into(),
            members: Vec::new(),
            default: None,
            links: Vec::new(),
            governance: Default::default(),
            warm_concurrency: None,
        };
        LogosMcp::federated(EngineRegistry::new(federation, RegistryMode::Lazy))
            .list_tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    /// The roster covers exactly the shipped tool set — in both directions.
    ///
    /// This is the assertion that fires on the merge shape described in the
    /// module docs: a tool registered in `mcp/src/server.rs` and not declared
    /// here is named in the failure message, and so is a declaration left
    /// behind by a retired tool.
    #[test]
    fn the_roster_covers_exactly_the_shipped_mcp_tool_set() {
        let declared: BTreeSet<String> = MCP_SURFACE.iter().map(|(t, _)| t.to_string()).collect();
        assert_eq!(
            declared.len(),
            MCP_SURFACE.len(),
            "a tool is declared twice in MCP_SURFACE"
        );

        let registered = mcp_tool_names();
        let undeclared: Vec<&String> = registered.difference(&declared).collect();
        assert!(
            undeclared.is_empty(),
            "these tools are registered by the MCP server but declare no CLI \
             reachability — add a Twin(..) or McpOnly(reason) line to \
             MCP_SURFACE (FR-CL-06): {undeclared:?}"
        );
        let retired: Vec<&String> = declared.difference(&registered).collect();
        assert!(
            retired.is_empty(),
            "MCP_SURFACE declares tools the server no longer registers: {retired:?}"
        );
    }

    /// Every declared twin is a real CLI command, and every MCP-only tool
    /// really has none — with a reason.
    #[test]
    fn every_tool_is_a_real_cli_command_or_a_reasoned_absence() {
        let cli = cli_command_paths();

        for (tool, reach) in MCP_SURFACE {
            match reach {
                Twin(path) => assert!(
                    cli.contains(*path),
                    "tool `{tool}` claims the CLI twin `logos {path}`, which the \
                     shipped clap definition does not accept (FR-CL-06)"
                ),
                McpOnly(reason) => {
                    assert!(
                        !reason.trim().is_empty(),
                        "tool `{tool}` is declared MCP-only with no reason (FR-CL-06)"
                    );
                    // The default CLI spelling of a tool name: `_` → `-` flat,
                    // or the first `_` as a subcommand group. If either exists,
                    // the tool is reachable and the McpOnly claim is stale.
                    //
                    // Checked against the UNFILTERED command set on purpose: a
                    // hidden command still ships, so it can still falsify an
                    // "MCP-only" claim, and this check must not inherit the
                    // walker's `hide` exemption.
                    let flat = tool.replace('_', "-");
                    let grouped = tool.replacen('_', " ", 1).replace('_', "-");
                    let reachable = all_cli_command_paths();
                    let found = [&flat, &grouped]
                        .into_iter()
                        .find(|spelling| reachable.contains(*spelling));
                    assert!(
                        found.is_none(),
                        "tool `{tool}` is declared MCP-only but `logos {}` \
                         exists — declare it a Twin instead (FR-CL-06)",
                        found.map_or("", String::as_str)
                    );
                }
            }
        }
    }

    /// Every CLI command is some tool's twin, or is listed as CLI-only with a
    /// reason — the same contract in the other direction.
    #[test]
    fn every_cli_command_is_a_twin_or_a_reasoned_cli_only() {
        let twins: BTreeMap<&str, &str> = MCP_SURFACE
            .iter()
            .filter_map(|(tool, reach)| match reach {
                Twin(path) => Some((*path, *tool)),
                McpOnly(_) => None,
            })
            .collect();
        // A `BTreeMap` would silently swallow a second tool claiming the same
        // path — and that is the shortcut this roster invites: writing
        // `("rescan", Twin("scan"))` to "resolve" the asymmetry would satisfy
        // every other assertion here while certifying a `logos rescan` that
        // does not exist.
        let twin_count = MCP_SURFACE
            .iter()
            .filter(|(_, reach)| matches!(reach, Twin(_)))
            .count();
        assert_eq!(
            twins.len(),
            twin_count,
            "two tools declare the same CLI twin path — one of them is borrowing \
             another tool's command instead of having its own (FR-CL-06)"
        );
        let cli_only: BTreeMap<&str, &str> = CLI_ONLY.iter().copied().collect();

        for (path, reason) in &cli_only {
            assert!(
                !reason.trim().is_empty(),
                "CLI command `logos {path}` is listed CLI-only with no reason (FR-CL-06)"
            );
            assert!(
                !twins.contains_key(path),
                "`logos {path}` is listed CLI-only but is also declared the twin \
                 of `{}` — one of the two declarations is wrong",
                twins[path]
            );
        }

        let cli = cli_command_paths();
        for path in &cli {
            let key = path.as_str();
            assert!(
                twins.contains_key(key) || cli_only.contains_key(key),
                "`logos {path}` is on the CLI and on neither surface roster — \
                 give it an MCP tool, or add it to CLI_ONLY with the reason it \
                 is deliberately absent (FR-CL-06)"
            );
        }

        let stale: Vec<&&str> = cli_only
            .keys()
            .filter(|path| !cli.contains(**path))
            .collect();
        assert!(
            stale.is_empty(),
            "CLI_ONLY lists commands the CLI no longer has: {stale:?}"
        );
    }

    /// The global `--json` reaches every CLI twin ([FR-CL-02]).
    ///
    /// # What this does and does not guard
    ///
    /// `--json` is declared `global = true` on the root, so `build()` propagates
    /// it to every subcommand at every depth. This test therefore **cannot fail
    /// for one newly added twin** — it fails only if the global itself is
    /// removed or stops propagating, which is the structural half of
    /// [FR-CL-02]. Read it as "the mechanism that gives every twin `--json` is
    /// still in place", not as per-twin coverage.
    ///
    /// The per-twin half is asserted through the real executable, where a
    /// missing `--json` shows up as unparseable stdout rather than an absent
    /// clap arg: `cli/tests/cli_surface.rs::non_stub_subcommands_emit_valid_json_
    /// with_json_flag` (which the three S-361 commands joined) and
    /// `every_subcommand_parses_with_global_flags`.
    ///
    /// [FR-CL-02]: ../../docs/specs/requirements/FR-CL-02.md
    #[test]
    fn every_cli_twin_carries_json() {
        let mut root = Cli::command();
        // Globals are propagated to subcommands at build time, so the check
        // reads what a real invocation would see, not the pre-build derive.
        root.build();

        for (tool, reach) in MCP_SURFACE {
            let Twin(path) = reach else { continue };
            let mut cmd = &root;
            for segment in path.split(' ') {
                cmd = cmd
                    .find_subcommand(segment)
                    .unwrap_or_else(|| panic!("`logos {path}` resolves ({segment} missing)"));
            }
            assert!(
                cmd.get_arguments().any(|arg| arg.get_id() == "json"),
                "the CLI twin `logos {path}` of tool `{tool}` does not accept \
                 --json (FR-CL-02)"
            );
        }
    }

    /// The managed `CLAUDE.md` block's CLI-twin sentence names exactly the
    /// MCP-only tools ([FR-CL-06], [FR-IN-02]).
    ///
    /// The block is written into every initialised project's agent memory, so
    /// "every tool has a CLI twin" is a claim shipped to agents. Binding it to
    /// the roster is what keeps it from becoming false the next time a tool
    /// lands on one surface only.
    ///
    /// [FR-CL-06]: ../../docs/specs/requirements/FR-CL-06.md
    /// [FR-IN-02]: ../../docs/specs/requirements/FR-IN-02.md
    #[test]
    fn the_managed_claude_md_block_names_exactly_the_mcp_only_tools() {
        let tmp = tempfile::tempdir().expect("tempdir");
        Engine::init_with(
            tmp.path(),
            &InitOptions {
                write_claude_md: true,
                ..InitOptions::default()
            },
        )
        .expect("init writes the managed block");
        let block = std::fs::read_to_string(tmp.path().join("CLAUDE.md")).expect("CLAUDE.md");

        let paragraph = block
            .split("\n\n")
            .find(|para| para.contains("CLI twin"))
            .expect("the managed block states the CLI-twin claim (FR-IN-02)");

        // Backticked spans are the sentence's only tool-naming form; `logos
        // context` and friends are prefixed, so only a bare tool name matches.
        let tools: BTreeSet<&str> = MCP_SURFACE.iter().map(|(t, _)| *t).collect();
        let named: BTreeSet<&str> = paragraph
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|span| tools.contains(span))
            .collect();
        let mcp_only: BTreeSet<&str> = MCP_SURFACE
            .iter()
            .filter_map(|(tool, reach)| matches!(reach, McpOnly(_)).then_some(*tool))
            .collect();

        assert_eq!(
            named, mcp_only,
            "the managed CLAUDE.md block's CLI-twin sentence must name exactly \
             the MCP-only tools (FR-CL-06). It says:\n{paragraph}"
        );
    }

    // ── FR-IN-09 (S-362/CR-114): the shipped guidance, all three texts ───────
    //
    // The managed block, the MCP server instructions and the README are one
    // message delivered through three channels, so they are checked as one set.
    // The tool/command claims are checked against the SHIPPED router and clap
    // definition for the same reason the CLI-twin sentence above is: prose that
    // names a tool the binary does not have is worse than no prose at all.

    /// One shipped guidance text.
    struct Guidance {
        /// How the failure message refers to it.
        label: &'static str,
        /// The rendered markdown.
        text: String,
        /// Whether a bare backticked `snake_case` span in this text is a tool
        /// name. True for the MCP instructions, whose naming convention IS the
        /// wire name; false for the other two, which prefix (`logos:context`,
        /// `logos context`) and so are free to backtick ordinary identifiers.
        bare_names_are_tools: bool,
        /// The heading that opens the scoping-before-decomposition material.
        scoping_marker: &'static str,
        /// The marker that opens (or names) the navigate-while-coding material,
        /// which must come after `scoping_marker` ([FR-IN-09]).
        ///
        /// [FR-IN-09]: ../../docs/specs/requirements/FR-IN-09.md
        navigation_marker: &'static str,
        /// Where the text says the repositioning is a hypothesis rather than a
        /// finding ([NFR-CC-04]). CR-114 R4 is explicit that the evidence is one
        /// project's telemetry; guidance that asserted it as settled would be
        /// exactly the over-claim that NFR forbids.
        ///
        /// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
        hypothesis_marker: &'static str,
        /// The fewest tool/command claims this text must actually reconcile
        /// against the shipped binary. A floor, not a count: it exists so a text
        /// that stops naming anything — or that the span parser stops reading —
        /// fails instead of passing vacuously. Set below today's real figure so
        /// ordinary prose edits do not trip it.
        min_reconciled_claims: usize,
    }

    /// The repository root — `CARGO_MANIFEST_DIR` is `<root>/cli`.
    fn repo_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the cli crate has a parent directory")
            .to_path_buf()
    }

    /// The three texts [FR-IN-09] governs. The managed block is generated by the
    /// real `init` rather than read from source, so what is asserted is what a
    /// project actually receives.
    ///
    /// [FR-IN-09]: ../../docs/specs/requirements/FR-IN-09.md
    fn shipped_guidance() -> Vec<Guidance> {
        let tmp = tempfile::tempdir().expect("tempdir");
        Engine::init_with(
            tmp.path(),
            &InitOptions {
                write_claude_md: true,
                ..InitOptions::default()
            },
        )
        .expect("init writes the managed block");
        let read = |path: std::path::PathBuf| {
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        };
        vec![
            Guidance {
                label: "the managed CLAUDE.md block",
                text: read(tmp.path().join("CLAUDE.md")),
                bare_names_are_tools: false,
                scoping_marker: "### Primary — before you decompose the work",
                navigation_marker: "### Secondary — while you are editing",
                hypothesis_marker: "hypothesis under measurement, not a settled finding",
                min_reconciled_claims: 12,
            },
            Guidance {
                label: "the MCP server instructions",
                text: read(repo_root().join("mcp/src/instructions.md")),
                bare_names_are_tools: true,
                scoping_marker: "## Primary — before the work is decomposed",
                navigation_marker: "## Secondary — while editing",
                hypothesis_marker: "hypothesis, not a finding",
                min_reconciled_claims: 16,
            },
            Guidance {
                label: "the README",
                text: read(repo_root().join("docs/howto/README.md")),
                bare_names_are_tools: false,
                scoping_marker: "## What it is primarily for: scoping work before you decompose it",
                navigation_marker: "is the secondary mode",
                hypothesis_marker: "hypothesis under measurement, not a settled finding",
                min_reconciled_claims: 10,
            },
        ]
    }

    /// The inline `` `code` `` spans of a markdown text, whitespace-normalised so
    /// a span broken across two source lines reads as one, and with fenced code
    /// blocks removed (their content is not backtick-delimited, and the fences
    /// themselves would unbalance the split).
    ///
    /// # Why this panics instead of returning what it managed to parse
    ///
    /// The split takes odd-indexed pieces, which is only the span set while the
    /// backtick count is even. One stray backtick flips the parity of every span
    /// after it, so the walk then inspects the *prose between* the spans — which
    /// never starts `logos ` or `logos:` and is therefore silently skipped. The
    /// guard would stop guarding and stay green. An unterminated fence does the
    /// same to the tail of the file. Both are malformed input, not licence to
    /// check less, so both fail loudly and name the text.
    fn inline_code_spans(label: &str, markdown: &str) -> Vec<String> {
        let mut unfenced = String::new();
        let mut fenced = false;
        for line in markdown.lines() {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
            } else if !fenced {
                unfenced.push_str(line);
                unfenced.push('\n');
            }
        }
        assert!(
            !fenced,
            "{label} has an unterminated ``` fence — everything after it was not \
             checked. Close the fence."
        );
        assert_eq!(
            unfenced.matches('`').count() % 2,
            0,
            "{label} has an odd number of backticks outside its fenced blocks — \
             the span parser cannot be trusted and would silently check nothing. \
             Balance them."
        );
        unfenced
            .split('`')
            .skip(1)
            .step_by(2)
            .map(|span| span.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect()
    }

    /// A bare command or tool token — no punctuation, so a placeholder
    /// (`<file>`), a flag (`--json`) or an alternation (`read|write`) ends the
    /// walk instead of being mistaken for a subcommand.
    fn is_plain_token(token: &str) -> bool {
        !token.is_empty()
            && !token.starts_with('-')
            && token
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    }

    /// Scoping-before-decomposition is presented first, navigation second, in
    /// every one of the three texts ([FR-IN-09] AC 1).
    ///
    /// [FR-IN-09]: ../../docs/specs/requirements/FR-IN-09.md
    #[test]
    fn the_shipped_guidance_puts_scoping_before_navigation() {
        for g in shipped_guidance() {
            let scoping = g.text.find(g.scoping_marker).unwrap_or_else(|| {
                panic!(
                    "{} does not present scoping before decomposition (FR-IN-09): \
                     expected to find {:?}",
                    g.label, g.scoping_marker
                )
            });
            let navigation = g.text.find(g.navigation_marker).unwrap_or_else(|| {
                panic!(
                    "{} does not mark navigate-while-coding as the secondary mode \
                     (FR-IN-09): expected to find {:?}",
                    g.label, g.navigation_marker
                )
            });
            assert!(
                scoping < navigation,
                "{} presents navigation before scoping (FR-IN-09): {:?} must come \
                 before {:?}",
                g.label,
                g.scoping_marker,
                g.navigation_marker
            );
        }
    }

    /// Each text presents the repositioning as a hypothesis, not a finding
    /// ([NFR-CC-04], CR-114 R4).
    ///
    /// The whole change rests on one project's telemetry. Guidance that states
    /// it as settled would be the over-claim [NFR-CC-04] exists to prevent, and
    /// it ships into every initialised project — so it is asserted, not trusted.
    ///
    /// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn the_shipped_guidance_states_the_repositioning_as_a_hypothesis() {
        for g in shipped_guidance() {
            assert!(
                g.text.contains(g.hypothesis_marker),
                "{} presents the repositioning as settled (NFR-CC-04): expected \
                 to find {:?}",
                g.label,
                g.hypothesis_marker
            );
        }
    }

    /// Each text names the three planning-time capabilities, in whichever
    /// spelling that surface uses ([FR-IN-09] AC 2).
    ///
    /// These are the capabilities the repositioning points at — guidance that
    /// says "scope first" without naming what to scope with is a slogan.
    ///
    /// [FR-IN-09]: ../../docs/specs/requirements/FR-IN-09.md
    #[test]
    fn the_shipped_guidance_names_the_planning_time_capabilities() {
        // (tool name, CLI command spelling) — FR-NV-11, FR-NV-12, FR-NV-13.
        const CAPABILITIES: &[(&str, &str)] = &[
            ("impact_intersection", "impact-intersection"),
            ("precedent", "precedent"),
            ("branch_overlap", "branch-overlap"),
        ];
        for g in shipped_guidance() {
            for (tool, command) in CAPABILITIES {
                assert!(
                    g.text.contains(tool) || g.text.contains(command),
                    "{} does not name the planning-time capability `{tool}` \
                     (FR-IN-09): the repositioned guidance must say what to scope \
                     with, not just that scoping comes first",
                    g.label
                );
            }
        }
    }

    /// No guidance text names a tool or a command the shipped binary does not
    /// have ([FR-IN-09] AC 4, the load-bearing one).
    ///
    /// The CLI-twin claim in the managed block was FALSE until S-361 corrected
    /// it, and it was false in prose that nothing read. This walks every inline
    /// code span in all three texts and reconciles it against the live router
    /// and the live clap definition, so the next such claim fails a build rather
    /// than shipping into every initialised project's agent memory.
    ///
    /// [FR-IN-09]: ../../docs/specs/requirements/FR-IN-09.md
    /// Bare backticked words in the MCP instructions that are deliberately NOT
    /// tool claims. The instructions name tools bare (`context`, `precedent`),
    /// so every other bare span has to be listed here rather than filtered by a
    /// heuristic — an exception a reviewer can read beats a rule that quietly
    /// exempts sixteen real claims.
    const MCP_BARE_NON_TOOLS: &[&str] = &[
        // The bare-name ambiguity example in the "disambiguate by symbol" bullet.
        "new", "map", "severity", // `doctor`'s verdict value.
        "ok",
    ];

    #[test]
    fn the_shipped_guidance_names_only_tools_and_commands_the_binary_has() {
        let tools = mcp_tool_names();
        // Hidden commands included: a hidden command is still shipped, so naming
        // one is not a false claim (the same reasoning as the McpOnly check).
        let commands = all_cli_command_paths();

        for g in shipped_guidance() {
            // How many spans this text actually RECONCILED against the binary.
            // Without a floor, a text the parser mis-reads (or a future edit that
            // drops every backticked claim) checks nothing and still passes.
            let mut reconciled = 0_usize;
            for span in inline_code_spans(g.label, &g.text) {
                // `logos:*` is the tool NAMESPACE, not a tool.
                if span == "logos:*" {
                    continue;
                }
                if let Some(rest) = span.strip_prefix("logos:") {
                    let tool = rest.split(' ').next().expect("split yields one item");
                    assert!(
                        tools.contains(tool),
                        "{} names the MCP tool `logos:{tool}`, which the shipped \
                         server does not register (FR-IN-09). Span: `{span}`",
                        g.label
                    );
                    reconciled += 1;
                } else if let Some(rest) = span.strip_prefix("logos ") {
                    reconciled += usize::from(assert_command_path_exists(
                        &commands, rest, &span, g.label,
                    ));
                } else if g.bare_names_are_tools
                    && is_plain_token(&span)
                    && !MCP_BARE_NON_TOOLS.contains(&span.as_str())
                {
                    assert!(
                        tools.contains(span.as_str()),
                        "{} names `{span}` as a tool, which the shipped server does \
                         not register (FR-IN-09). If it is not a tool claim, add it \
                         to MCP_BARE_NON_TOOLS with the reason",
                        g.label
                    );
                    reconciled += 1;
                }
            }
            assert!(
                reconciled >= g.min_reconciled_claims,
                "{} reconciled only {reconciled} claims against the shipped binary, \
                 below its floor of {} (FR-IN-09 AC 4). Either the text stopped \
                 naming tools and commands, or the span parser stopped seeing them \
                 — a guard that checks nothing passes for the wrong reason",
                g.label,
                g.min_reconciled_claims
            );
        }
    }

    /// The MCP instructions' negative claim — that `affected` has no tool on the
    /// MCP surface — is true of the shipped router ([FR-IN-09] AC 4).
    ///
    /// The spans walk above only checks that a *named* tool exists. A claim that
    /// something is absent needs the opposite assertion, and this is exactly the
    /// shape that went stale for three sprints in the CLI-twin sentence.
    ///
    /// [FR-IN-09]: ../../docs/specs/requirements/FR-IN-09.md
    #[test]
    fn the_mcp_instructions_absence_claim_holds_against_the_router() {
        let instructions = &shipped_guidance()
            .into_iter()
            .find(|g| g.label == "the MCP server instructions")
            .expect("the instructions are one of the three guidance texts")
            .text;
        assert!(
            instructions.contains("it has no tool on this surface"),
            "the instructions no longer carry the `affected` absence claim — drop \
             this test with it, or update the sentence it guards"
        );
        assert!(
            !mcp_tool_names().contains("affected"),
            "the MCP instructions claim `logos affected` has no tool on this \
             surface, but the shipped server now registers one (FR-IN-09)"
        );
    }

    /// The leading command tokens of `rest` name a real command path — either a
    /// leaf, or a group that some leaf lives under. The walk stops at the first
    /// token that is not a bare command word, so flags, placeholders and
    /// alternations end it rather than failing it.
    ///
    /// Returns whether a command was actually reconciled, so the caller can tell
    /// a checked claim from a span it merely declined to check. A span that is
    /// only global flags (`logos --version`) reconciles nothing and is not a
    /// failure — it names no subcommand by design.
    ///
    /// A *group* (`wiki`, `coverage`) is accepted only as an intermediate step.
    /// clap rejects `logos wiki` on its own, so a span that RUNS OUT of tokens on
    /// a group is a false claim — unless the walk was cut short by a placeholder
    /// or an alternation (`logos wiki write|read|…`), where the group is as far
    /// as this parser can honestly get.
    fn assert_command_path_exists(
        commands: &BTreeSet<String>,
        rest: &str,
        span: &str,
        label: &str,
    ) -> bool {
        let mut path = String::new();
        let mut cut_short = false;
        for token in rest.split(' ') {
            if !is_plain_token(token) {
                cut_short = true;
                break;
            }
            let candidate = if path.is_empty() {
                token.to_string()
            } else {
                format!("{path} {token}")
            };
            let group_prefix = format!("{candidate} ");
            assert!(
                commands
                    .iter()
                    .any(|cmd| *cmd == candidate || cmd.starts_with(&group_prefix)),
                "{label} names `logos {candidate}`, which the shipped CLI does not \
                 accept (FR-IN-09). Span: `{span}`"
            );
            path = candidate;
        }
        if path.is_empty() {
            return false;
        }
        assert!(
            cut_short || commands.contains(&path),
            "{label} names `logos {path}`, which is a command GROUP — clap requires \
             a subcommand after it, so the span as written is not a runnable \
             invocation (FR-IN-09). Span: `{span}`"
        );
        true
    }
}
