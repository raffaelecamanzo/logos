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
    /// graph counts. For INDEX freshness use `status`.
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
///
/// [FR-CL-06]: ../../docs/specs/requirements/FR-CL-06.md
/// [FR-CL-02]: ../../docs/specs/requirements/FR-CL-02.md
/// [FR-IN-02]: ../../docs/specs/requirements/FR-IN-02.md
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
             spellings ARE tools: session_start is `gate --save`, session_end is \
             a bare `gate` (FR-GV-04/05).",
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
    /// hidden commands and clap's generated `help` excluded.
    fn cli_command_paths() -> BTreeSet<String> {
        fn walk(cmd: &clap::Command, prefix: &str, out: &mut BTreeSet<String>) {
            for sub in cmd.get_subcommands() {
                if sub.is_hide_set() || sub.get_name() == "help" {
                    continue;
                }
                let path = if prefix.is_empty() {
                    sub.get_name().to_string()
                } else {
                    format!("{prefix} {}", sub.get_name())
                };
                if sub.get_subcommands().next().is_some() {
                    walk(sub, &path, out);
                } else {
                    out.insert(path);
                }
            }
        }
        let mut out = BTreeSet::new();
        walk(&Cli::command(), "", &mut out);
        out
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
                    let flat = tool.replace('_', "-");
                    let grouped = tool.replacen('_', " ", 1).replace('_', "-");
                    assert!(
                        !cli.contains(&flat) && !cli.contains(&grouped),
                        "tool `{tool}` is declared MCP-only but `logos {flat}` \
                         exists — declare it a Twin instead (FR-CL-06)"
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

    /// Every CLI twin carries `--json` ([FR-CL-02]) — the machine-readable mode
    /// an agent directed at the CLI depends on.
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
}
