//! `logos` binary — thin CLI adapter over [`logos_core::Engine`].
//!
//! This crate **must not contain business logic** (NFR-MA-02, ADR-01).
//! Its sole responsibilities are:
//!   1. Parse CLI arguments with clap v4 derive (FR-CL-01, FR-CL-05).
//!   2. Construct an [`Engine`] and call **exactly one** method per subcommand.
//!   3. Serialise the read-model to stdout (`--json` machine mode, FR-CL-02).
//!   4. Map outcomes to exit codes 0/1/2/3 — success / violation / usage /
//!      internal (ADR-14, FR-CL-03, FR-EH-01).

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
        #[arg(long, value_parser = parse_kind)]
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
        #[arg(long, value_parser = parse_kind, conflicts_with_all = ["callers", "callees"])]
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
    /// Architecture-rules compliance check; error violations exit 1.
    Check {
        /// Alternate rules file (defaults to .logos/rules.toml).
        #[arg(long, value_name = "FILE")]
        rules: Option<PathBuf>,
        /// Skip the pre-evaluation reconcile (FR-RC-04).
        #[arg(long, alias = "assume-fresh")]
        no_reconcile: bool,
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
    /// Install the Claude Code SessionEnd quality-report hook (FR-IN-07,
    /// ADR-49): a marker-tagged hook script plus a non-clobbering merge into
    /// the shared `.claude/settings.json` that prints a non-blocking
    /// signal/baseline/violations readout at session end. The binary stays
    /// offline (NFR-SE-01). Re-emit with `--force`. CLI-only. (The PostToolUse
    /// wiki-augmentation hook this once also installed was retired — CR-070.)
    Hook {
        /// Emit the hook (the only `hook` operation; required so the verb reads
        /// `wiki hook --emit`).
        #[arg(long, required = true)]
        emit: bool,
        /// Re-emit, replacing an existing managed SessionEnd entry.
        #[arg(long)]
        force: bool,
    },
}

// ── Entry point ────────────────────────────────────────────────────────────

fn main() {
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
        Commands::Serve { mcp: false, ui: true, .. } => observability::Surface::Web,
        Commands::Serve { .. } => observability::Surface::Mcp,
        _ => observability::Surface::Cli,
    };
    let _telemetry = observability::init(surface, &root);

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
/// defaults (MCP + CLAUDE.md + the wiki skill + the SessionEnd quality-report
/// hook — yes, that's what `-i` asks for — git hooks no: they rewire
/// core.hooksPath, so they stay opt-in via --hooks). The PostToolUse
/// wiki-augmentation hook `-i` once also installed here was retired (CR-070).
pub(crate) fn init_options(interactive: bool, hooks: bool) -> InitOptions {
    InitOptions {
        inject_mcp: interactive && ask("inject the logos MCP server block into .mcp.json?", true),
        write_claude_md: interactive && ask("generate the managed CLAUDE.md block?", true),
        install_hooks: hooks || (interactive && ask("install git hooks (core.hooksPath)?", false)),
        materialize_skill: interactive && ask("materialize the logos-wiki generation skill?", true),
        install_quality_report_hook: interactive
            && ask("install the Claude Code SessionEnd quality-report hook?", true),
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

/// Parse a `--kind` filter against the canonical ontology's wire names; an
/// unknown kind is a clap value error → usage exit 2 (FR-CL-03).
pub(crate) fn parse_kind(s: &str) -> Result<NodeKind, String> {
    NodeKind::ALL
        .into_iter()
        .find(|k| k.as_str() == s)
        .ok_or_else(|| {
            let names: Vec<&str> = NodeKind::ALL.iter().map(|k| k.as_str()).collect();
            format!("unknown node kind {s:?} (one of: {})", names.join(", "))
        })
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

    /// `--kind` accepts every canonical wire name and rejects garbage with an
    /// enumerating message (NFR-UX-02).
    #[test]
    fn kind_parser_round_trips_the_ontology() {
        for kind in NodeKind::ALL {
            assert_eq!(parse_kind(kind.as_str()), Ok(kind));
        }
        let err = parse_kind("nonsense").unwrap_err();
        assert!(err.contains("function"), "names the valid kinds: {err}");
    }
}
