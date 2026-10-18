//! `logos_cli` — the `logos` CLI as a library: a thin adapter over
//! [`logos_core::Engine`], composed with an [`ExtensionHost`].
//!
//! A binary's `main` builds the host — the one composition root, and the only
//! code that names extension crates ([FR-DS-04], [ADR-74]) — and hands it to
//! [`run`]. A binary's `main` registers whichever extensions its build includes.
//!
//! This crate **must not contain business logic** (NFR-MA-02, ADR-01).
//! Its sole responsibilities are:
//!   1. Parse CLI arguments with clap v4 derive (FR-CL-01, FR-CL-05), plus each
//!      active extension's subcommand.
//!   2. Construct an [`Engine`] and call **exactly one** method per subcommand.
//!   3. Serialise the read-model to stdout (`--json` machine mode, FR-CL-02).
//!   4. Map outcomes to exit codes 0/1/2/3/4/5 — success / violation / usage /
//!      internal / no rules contract loaded / a required security scan missing
//!      (ADR-14, FR-CL-03, FR-EH-01, FR-GV-22, FR-SC-10).
//!
//! [FR-DS-04]: ../../docs/specs/requirements/FR-DS-04.md
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process;

use anyhow::Result;
use clap::{ArgMatches, CommandFactory, FromArgMatches, Parser, Subcommand};
use logos_core::{
    error::{self, CoreError},
    federation::{Backing, SelectedEngine},
    governance::DsmGranularity,
    init::{InitContribution, InitOptions},
    model::NodeKind,
    observability, workspace, Engine,
};
use logos_ext::cli::{CliCommand, CliContext};
pub use logos_ext::ExtensionHost;

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

/// `--repo`, the CLI twin of the MCP `repo` parameter ([FR-WS-42], [CR-225]).
///
/// Flattened into exactly the commands whose MCP twins take `repo` — the
/// read-only single-member reads; the write and gate commands act on the
/// repository they run in and never carry it. One definition, so the flag, its
/// help and its error contract read the same on every command.
///
/// [FR-WS-42]: ../../docs/specs/requirements/FR-WS-42.md
/// [CR-225]: ../../docs/requests/CR-225-per-member-single-root-tools.md
#[derive(clap::Args, Clone, Default)]
pub(crate) struct MemberArg {
    /// Answer for this workspace member (the answer then carries `member`). Under
    /// a workspace root the default member answers without it.
    #[arg(long, value_name = "MEMBER")]
    repo: Option<String>,
}

// ── Subcommands (FR-CL-01) ─────────────────────────────────────────────────

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Initialise `.logos/` (config.toml, rules.toml, .gitignore) and the store.
    Init {
        /// Interactive setup: also inject the MCP server block into .mcp.json,
        /// write the managed CLAUDE.md block and install the session-start
        /// quality-report hook, prompting per step on a TTY (non-TTY takes those
        /// as yes and git hooks as no).
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
        /// Remove everything Logos wrote into the project (FR-IN-12): the
        /// managed blocks, the MCP, hook and plugin-enrolment entries, the
        /// scripts and skill copies, and the managed git hooks. Foreign content
        /// and `.logos/` stay; a second run is a no-op. `-i`/`--hooks` add
        /// nothing: everything is removed.
        #[arg(long, conflicts_with = "workspace")]
        uninstall: bool,
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
    /// Reconcile exactly the given paths into the index.
    ///
    /// With no path it re-reads no file. To fold in every change at once, run
    /// `logos scan` or rebuild with `logos index`.
    Sync {
        /// Paths to reconcile — exactly these and nothing else; with none, no
        /// file is re-read (use `logos scan` or `logos index` for every change).
        paths: Vec<PathBuf>,
    },
    /// Show the current index and sync health.
    Status { #[command(flatten)] member: MemberArg },
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
        #[command(flatten)] member: MemberArg,
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
        #[command(flatten)] member: MemberArg,
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
        #[command(flatten)] member: MemberArg,
    },
    /// Explore a symbol's neighbourhood, source grouped by file.
    Explore {
        /// Symbol or name to anchor the walk on.
        query: String,
        /// Cap the file groups returned (default 10).
        #[arg(long)]
        max_files: Option<usize>,
        #[command(flatten)] member: MemberArg,
    },
    /// Full info for one symbol: metadata, edges, optional code.
    Node {
        /// SCIP symbol string or name to look up.
        symbol: String,
        /// Include the declaration source.
        #[arg(long)]
        code: bool,
        #[command(flatten)] member: MemberArg,
    },
    /// Direct callers of a symbol.
    Callers {
        /// Symbol whose callers to list.
        symbol: String,
        /// Maximum number of results (default 50).
        #[arg(long)]
        limit: Option<usize>,
        #[command(flatten)] member: MemberArg,
    },
    /// Direct callees of a symbol.
    Callees {
        /// Symbol whose callees to list.
        symbol: String,
        /// Maximum number of results (default 50).
        #[arg(long)]
        limit: Option<usize>,
        #[command(flatten)] member: MemberArg,
    },
    /// Transitive impact of changing a symbol, both directions labeled.
    Impact {
        /// Symbol whose impact to trace.
        symbol: String,
        /// Traversal depth bound (default 3).
        #[arg(long)]
        depth: Option<usize>,
        #[command(flatten)] member: MemberArg,
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
        #[command(flatten)] member: MemberArg,
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
        #[command(flatten)] member: MemberArg,
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
        #[command(flatten)] member: MemberArg,
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
    /// Architecture-rules compliance check; error violations exit 1, a missing
    /// required scan 5 (over 1), no rules contract loaded 4 (FR-GV-22).
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
        /// Bless the current offenders of every `ratchet = true` constraint:
        /// write them to the checked-in .logos/rules-baseline.json, then check
        /// against it (FR-GV-23). The only command that writes that file.
        #[arg(long)]
        bless: bool,
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
        #[command(flatten)] member: MemberArg,
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
        #[command(flatten)] member: MemberArg,
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
        #[command(flatten)] member: MemberArg,
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
    /// Every offender the last snapshot recorded, uncapped (FR-QM-19): each
    /// with its symbol id, value and threshold, narrowed by path and dimension.
    /// Reads the last `scan`; writes nothing.
    Offenders {
        /// Only offenders defined in files matching this project-relative glob
        /// (`*` stays in one directory, `**` crosses them).
        #[arg(long, value_name = "GLOB")]
        path: Option<String>,
        /// Only this dimension: nesting, conciseness, cohesion, focus,
        /// uniqueness, acyclicity, depth, equality or redundancy.
        #[arg(long)]
        dimension: Option<String>,
        #[command(flatten)] member: MemberArg,
    },
    /// The branch delta (FR-GV-24): every offender and `check` violation
    /// classified new, worsened, improved, resolved or unchanged per symbol
    /// against the merge-base of HEAD and the base ref, with near-clone groups
    /// the branch introduced. Advisory: exits 0 when it classified, 1 when it
    /// could not measure; never moves `check` or `gate`.
    Review {
        /// The ref to compare against (default: the repository's default
        /// branch — origin/HEAD, else main, else master).
        #[arg(long, value_name = "REF")]
        base: Option<String>,
        #[command(flatten)] member: MemberArg,
    },
    /// Where to start (FR-QM-17): the ranked fix units the last snapshot
    /// recorded, each with its estimated recoverable signal points, and the
    /// per-member outlier labels. Advisory, never gated. Reads the last `scan`;
    /// writes nothing.
    Improvements {
        #[command(flatten)] member: MemberArg,
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
        /// Rank hotspots with a fresh unsuppressed security finding first, highest
        /// severity first (FR-SC-14); the findings column shows with or without it.
        #[arg(long)]
        with_findings: bool,
        #[command(flatten)] member: MemberArg,
    },
    /// Coverage evidence tier: ingest external reports and read freshness-checked
    /// status (the non-gated coverage tier — never moves the gate, BR-28).
    Coverage {
        #[command(subcommand)]
        command: CoverageCommands,
    },
    /// Security evidence tier: run the installed scanners, ingest SARIF, read
    /// per-scanner status and the classified findings (FR-SC-06..08).
    Security {
        #[command(subcommand)]
        command: SecurityCommands,
    },
    /// Install the Claude Code session-start quality-report hook (FR-IN-07,
    /// ADR-49): a marker-tagged hook script plus a non-clobbering merge into
    /// the shared `.claude/settings.json` that surfaces a non-blocking
    /// signal/baseline/violations readout when a session starts, resumes, or is
    /// reopened by `/clear`. Also sweeps the retired SessionEnd hook and its
    /// orphaned script (CR-095). The binary stays offline (NFR-SE-01). Re-emit
    /// with `--force`. CLI-only.
    // An extension's hidden alias of this command is named at composition
    // (`logos_ext::cli::name_aliases`), so only a build composing one names it.
    Hook(HookArgs),
    /// Cross-service workspace queries (federation, FR-WS-05): route-providers,
    /// callers, impact, search, build-deps (a build dependency, never a
    /// runtime coupling) and type-refs (an advisory type reference, never a
    /// coupling) — each with an optional `--repo` member filter.
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
    Status { #[command(flatten)] member: MemberArg },
    /// Run the configured `[coverage_ingest].refresh_cmd` and ingest its output
    /// (FR-CV-10). The lone explicit coverage subprocess — never on the serve
    /// path (ADR-38). Errors if no `refresh_cmd` is configured.
    Refresh,
}

/// `security` sub-subcommands (FR-SC-01, FR-SC-06..08): each one `Engine` call
/// shared with its MCP twin.
#[derive(Subcommand)]
pub(crate) enum SecurityCommands {
    /// Run every installed, enabled scanner and ingest its SARIF (FR-SC-06).
    /// The first scan in a project needs `--consent` (FR-SC-07). `--scanner`
    /// and `--path` narrow it to a re-scan that takes seconds (FR-SC-17).
    Scan {
        /// Record consent for the scanners not yet named in this project's
        /// consent, which may reach the network to refresh rules or databases.
        /// With `--install`, also the install consent for what is installed.
        #[arg(long)]
        consent: bool,
        /// First install the default scanners that are missing and not
        /// disabled here, as `security install` does (FR-SC-22).
        #[arg(long, conflicts_with_all = ["scanners", "paths"])]
        install: bool,
        /// Run only this scanner, by registry name or SARIF driver name;
        /// repeatable.
        #[arg(long = "scanner", value_name = "NAME")]
        scanners: Vec<String>,
        /// Scan only this file or directory, relative to the project root (or
        /// absolute inside it); repeatable. Semgrep, Gitleaks, Bandit, gosec
        /// and Checkov run on these paths and replace only their findings
        /// there (Gitleaks keeps its history findings there); Trivy,
        /// OSV-Scanner and Dependency-Check are whole-manifest
        /// only, CodeQL whole-project only, and each runs unscoped when named
        /// with `--scanner`.
        #[arg(long = "path", value_name = "PATH")]
        paths: Vec<String>,
    },
    /// Download, verify and install scanners into the user-level tools
    /// directory (FR-SC-21/22). Only scanners with no usable copy are
    /// installed: a user-level override or a `PATH` copy at or above the
    /// minimum version is kept. Needs a per-user, per-version consent.
    /// Dependency-Check (with a managed Java runtime) and CodeQL are opt-in:
    /// installed only when named (FR-SC-23).
    Install {
        /// Scanners to install, by registry name.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        scanners: Vec<String>,
        /// Every scanner of the default set: all but Dependency-Check and
        /// CodeQL, which must be named.
        #[arg(long)]
        all: bool,
        /// Also replace a managed copy older than the version this release
        /// pins, removing the old one.
        #[arg(long)]
        update: bool,
        /// Install offline from a directory of pre-downloaded release
        /// artifacts, verified exactly as a download is.
        #[arg(long, value_name = "DIR")]
        from: Option<PathBuf>,
        /// Record the install consent for the versions about to be installed;
        /// it covers them in every project.
        #[arg(long)]
        consent: bool,
        /// Accept the CodeQL CLI's licence, which allows its use on
        /// open-source codebases and for academic research only; recorded
        /// per user for the pinned version. CodeQL is not installed without it.
        #[arg(long = "accept-codeql-licence")]
        accept_codeql_licence: bool,
    },
    /// Remove Logos-managed scanner copies; a copy you installed yourself is
    /// never touched (FR-SC-22). The managed Java runtime goes once no
    /// installed scanner needs it.
    Uninstall {
        /// Scanners to uninstall, by registry name.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        scanners: Vec<String>,
        /// Every scanner Logos installs, Dependency-Check and CodeQL included.
        #[arg(long)]
        all: bool,
    },
    /// Ingest a SARIF 2.1.0 report from any tool into the evidence store (FR-SC-01).
    Ingest {
        /// Path to the SARIF report.
        report: PathBuf,
    },
    /// Each scanner's state — not installed, disabled, never run, stale or
    /// fresh — and the consent state (FR-SC-08).
    Status { #[command(flatten)] member: MemberArg },
    /// The stored findings with OWASP categories and suppressions; the filters
    /// intersect (FR-SC-08).
    Findings {
        /// Least severity listed: critical, high, medium, low or info.
        #[arg(long)]
        severity: Option<String>,
        /// A scanner, by registry name or SARIF driver name.
        #[arg(long)]
        scanner: Option<String>,
        /// An OWASP category, by id (A05:2025) or number (A05).
        #[arg(long)]
        category: Option<String>,
        /// A project-relative file, or a directory holding it.
        #[arg(long)]
        path: Option<String>,
        /// Only `active` or only `suppressed` findings.
        #[arg(long)]
        suppression: Option<String>,
        /// The OWASP edition: 2025 (default) or 2021.
        #[arg(long)]
        edition: Option<String>,
        #[command(flatten)] member: MemberArg,
    },
}

/// The arguments of the session-start quality-report hook install (`logos hook`,
/// S-638). A composed extension that re-exposes it as a hidden subcommand of its
/// own takes the same two flags, and `hook`'s help then names that alias (S-645).
#[derive(clap::Args)]
pub(crate) struct HookArgs {
    /// Emit the hook (the only `hook` operation; required so the verb reads
    /// `hook --emit`).
    #[arg(long, required = true)]
    emit: bool,
    /// Re-emit, replacing an existing managed SessionStart entry.
    #[arg(long)]
    force: bool,
}

// ── Entry point ────────────────────────────────────────────────────────────

/// Run `logos` with `host`'s active extensions composed in, over the process's
/// arguments, and exit with the resulting code — the composition root's one call.
pub fn run(host: ExtensionHost) -> ! {
    let code = run_from(&host, std::env::args_os());
    // `process::exit` runs no destructors; flush what the run printed first.
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    process::exit(code)
}

/// Run `logos` over `args` (the program name first) and return its exit code —
/// [`run`] without the exit, so a test can compose a host and drive the CLI.
pub fn run_from<I, T>(host: &ExtensionHost, args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    // Widen this process's descriptor allowance before anything opens a file
    // (S-324, NFR-PE-11, ADR-63). Best-effort and silent: a workspace's live
    // connections are bounded by the core's WorkspaceBudget, so a kernel that
    // refuses the raise costs residency, never correctness — which is exactly
    // why this is not allowed to report anything.
    logos_core::fdlimit::raise_open_file_limit();

    let invocation = match Composed::new(host).parse(args) {
        Ok(invocation) => invocation,
        // Parse failures, `--help` and `--version` print and exit exactly as
        // `Cli::parse()` would: clap owns usage errors end to end (exit 2).
        Err(e) => {
            let _ = e.print();
            return e.exit_code();
        }
    };
    // The active extensions' process-wide contributions: their telemetry tools,
    // so the stats read-model classifies their events (FR-DS-04), and their
    // config tables and secret keys, so every config load routes them (FR-CF-09).
    if let Err(err) = host.register_with_process() {
        eprintln!("error: {err:#}");
        return CoreError::EXIT_INTERNAL;
    }
    // Defence in depth: a panic crossing the bin boundary is an internal
    // error by definition (FR-CL-03) — the default hook has already printed
    // the payload to stderr.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match invocation {
        Invocation::Core(cli) => run_cli(*cli, host),
        Invocation::Extension(extension) => run_extension(&extension.0, &extension.1),
    }));
    match outcome {
        Ok(Ok(code)) => code,
        Ok(Err(err)) => {
            eprintln!("error: {err:#}");
            error::exit_code(&err)
        }
        Err(_) => CoreError::EXIT_INTERNAL,
    }
}

/// The `logos` clap command with `host`'s active extension subcommands — what
/// `logos --help` renders. Under the public host it is the core command alone.
pub fn command(host: &ExtensionHost) -> clap::Command {
    Composed::new(host).command
}

/// The core command plus the extension subcommands it admits.
struct Composed {
    command: clap::Command,
    extensions: Vec<CliCommand>,
}

/// What one parsed invocation runs: a core subcommand, or an extension's with
/// its own matches.
enum Invocation {
    Core(Box<Cli>),
    Extension(Box<(CliCommand, ArgMatches)>),
}

impl Composed {
    /// Compose `host`'s active extension subcommands onto the core command. One
    /// that [`clash`]es with what is already there is dropped with a notice: an
    /// extension can never shadow a command, and never be silently unreachable.
    fn new(host: &ExtensionHost) -> Self {
        let mut command = Cli::command();
        let mut extensions = Vec::new();
        for extension in host.cli_commands() {
            if let Some(reason) = clash(&command, extension.command()) {
                eprintln!("logos: the extension subcommand `{}` {reason}; not added", extension.name());
                continue;
            }
            command = command.subcommand(extension.command().clone());
            extensions.push(extension);
        }
        let command = logos_ext::cli::name_aliases(place_extensions(command, &extensions), &extensions);
        Self { command, extensions }
    }

    /// Parse `args` against the composed command: an extension subcommand
    /// routes to its extension, anything else parses into the core [`Cli`].
    fn parse<I, T>(self, args: I) -> Result<Invocation, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let mut command = self.command;
        let matches = command.try_get_matches_from_mut(args)?;
        if let Some((name, sub)) = matches.subcommand() {
            if let Some(extension) = self.extensions.into_iter().find(|e| e.name() == name) {
                return Ok(Invocation::Extension(Box::new((extension, sub.clone()))));
            }
        }
        Cli::from_arg_matches(&matches)
            .map(|cli| Invocation::Core(Box::new(cli)))
            .map_err(|e| e.format(&mut command))
    }
}

/// The core subcommand the extensions' subcommands follow in `logos --help`.
///
/// clap lists subcommands in the order they were added, so a composed extension's
/// would trail the integration and serving commands after `serve`. They are listed
/// instead in one fixed slot, in registration order: after the core's evidence
/// tiers (`hotspots`, `coverage`, `security`) and before the integration commands (`hook`,
/// `xservice`, `workspace`, …) — where `wiki` stood while it was a core command
/// (S-644). The slot is the core's, like the Config view's extension panel slot
/// (S-643): an extension says what it adds, not where the core lists it.
const EXTENSION_SLOT_AFTER: &str = "security";

/// `command` with `extensions` listed in the [`EXTENSION_SLOT_AFTER`] slot of its
/// help, and every core subcommand after the slot moved down to make room.
fn place_extensions(command: clap::Command, extensions: &[CliCommand]) -> clap::Command {
    let Some(slot) = command.find_subcommand(EXTENSION_SLOT_AFTER).map(clap::Command::get_display_order)
    else {
        return command;
    };
    let names: Vec<&str> = extensions.iter().map(CliCommand::name).collect();
    command.mut_subcommands(|sub| {
        let order = sub.get_display_order();
        match names.iter().position(|name| *name == sub.get_name()) {
            Some(index) => sub.display_order(slot + 1 + index),
            None if order > slot => sub.display_order(order + names.len()),
            None => sub,
        }
    })
}

/// Why `candidate` cannot join `command`, if it cannot. Asked of the **built**
/// command, so clap's own `help` subcommand counts: a name or alias any existing
/// subcommand answers to, or an argument that redefines one of the global flags
/// every subcommand inherits (`--project`, `--json`, `--quiet`).
fn clash(command: &clap::Command, candidate: &clap::Command) -> Option<&'static str> {
    let mut built = command.clone();
    built.build();
    let mut names = std::iter::once(candidate.get_name()).chain(candidate.get_all_aliases());
    if names.any(|name| built.find_subcommand(name).is_some()) {
        return Some("is named like a registered command");
    }
    let globals: Vec<&str> =
        built.get_arguments().filter(|a| a.is_global_set()).map(|a| a.get_id().as_str()).collect();
    let redefines = |a: &clap::Arg| {
        globals.contains(&a.get_id().as_str()) || a.get_long().is_some_and(|l| globals.contains(&l))
    };
    candidate.get_arguments().any(redefines).then_some("redefines a global flag")
}

/// Run an extension's subcommand over the same root and output mode a core
/// subcommand gets. The global flags propagate into `sub`, wherever on the
/// command line they were given.
fn run_extension(extension: &CliCommand, sub: &ArgMatches) -> Result<i32> {
    let project = sub.get_one::<PathBuf>("project").cloned().unwrap_or_else(|| PathBuf::from("."));
    let root = workspace::resolve_root(&project);
    let _telemetry = observability::init(observability::ProcessSurface::Cli, &root);
    let context = CliContext {
        root: &root,
        json: sub.get_flag("json"),
        quiet: sub.get_flag("quiet"),
    };
    extension.run(sub, &context)
}

/// Dispatch: one Engine method call per subcommand (NFR-MA-02, ADR-01).
fn run_cli(cli: Cli, host: &ExtensionHost) -> Result<i32> {
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
        // stdout for MCP (including the combined one) stamps surface=mcp. This
        // is the *process* stamp only: the web adapter names a surface per
        // request at its route boundary (`web::bridge`, FR-OB-09), so in the
        // combined session its routes are attributed web — and the header's
        // own read shell — over this mcp default.
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
    dispatch::dispatch(cli.command, &root, &out, host)
}

// ── Surface helpers (no business logic) ────────────────────────────────────

/// A started engine for graph commands. Everything except `index` (which
/// bootstraps the store) requires an existing index — or a way to create one:
/// a DB-less linked worktree with a seedable primary DB is served, not
/// refused (`Engine::start` seeds it, FR-WT-03). Otherwise a missing index is
/// an actionable error naming the remedy, mapped to exit 3 (FR-EH-01).
///
/// The guard is [`logos_ext::cli::indexed_engine`] — one body behind the core's
/// graph commands and every extension subcommand ([`CliContext::engine`]).
pub(crate) fn engine(root: &Path, bootstrap: bool) -> Result<Engine> {
    if bootstrap {
        Engine::start(root)
    } else {
        logos_ext::cli::indexed_engine(root)
    }
}

/// The engine a `--repo`-taking command answers from: [`Backing::one_shot`],
/// with [`engine`]'s index guard for the repository it runs in — the selection
/// path and the errors of the MCP `repo` parameter ([FR-WS-42], [CR-225]).
///
/// [FR-WS-42]: ../../docs/specs/requirements/FR-WS-42.md
/// [CR-225]: ../../docs/requests/CR-225-per-member-single-root-tools.md
fn member_engine(root: &Path, member: &MemberArg) -> Result<SelectedEngine<Engine>> {
    Backing::one_shot(root, member.repo.as_deref(), logos_ext::cli::require_index, || engine(root, false))
}

/// Resolve which init steps run (S-023, FR-IN-02/03, FR-IN-07) — pure surface
/// UX, the step logic itself lives in the core. `-i` enables the
/// host-integration steps, prompting per step on a TTY; non-TTY takes the safe
/// defaults (MCP + CLAUDE.md + the session-start quality-report hook — yes,
/// that's what `-i` asks for — git hooks no: they rewire core.hooksPath, so they
/// stay opt-in via --hooks). The PostToolUse wiki-augmentation hook `-i` once
/// also installed here was retired (CR-070).
///
/// `steps` are the init steps the composed extensions contribute. Only such a
/// step reads [`InitOptions::materialize_skill`] — the core materializes no skill
/// of its own — so a build composing none never asks about one ([FR-DS-05]: no
/// question about a surface it does not have). `ask` is the prompt — [`ask`] at
/// both call sites, as `workspace_init::nudge` takes it — so a test can record
/// which questions are asked without a TTY.
///
/// [FR-DS-05]: ../../docs/specs/requirements/FR-DS-05.md
pub(crate) fn init_options(interactive: bool, hooks: bool, steps: &[std::sync::Arc<dyn InitContribution>], mut ask: impl FnMut(&str, bool) -> bool) -> InitOptions {
    InitOptions {
        inject_mcp: interactive && ask("inject the logos MCP server block into .mcp.json?", true),
        write_claude_md: interactive && ask("generate the managed CLAUDE.md block?", true),
        install_hooks: hooks || (interactive && ask("install git hooks (core.hooksPath)?", false)),
        materialize_skill: interactive
            && !steps.is_empty()
            && ask("materialize the agent skills the composed extensions ship?", true),
        install_quality_report_hook: interactive
            && ask("install the Claude Code session-start quality-report hook?", true),
        // The plain single-repo `init` path — never the workspace-member
        // context `federation::enable::enable` sets (CR-119).
        workspace_member: false,
        // FR-IN-12: the core reads the Claude Code settings of the root it
        // resolves, so plugin-aware `-i` asks no question of its own.
        claude_plugin: logos_core::init::ClaudePlugin::Detect,
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

/// Read-model printer: `--json` always emits compact machine output (it IS
/// the essential output); the human rendering (pretty JSON until dedicated
/// formatters land) is what `--quiet` suppresses (FR-CL-02).
pub(crate) struct Output {
    pub(crate) json: bool,
    pub(crate) quiet: bool,
}

impl Output {
    /// The one printing rule, shared with every extension subcommand
    /// ([`logos_ext::cli::print_read_model`]).
    pub(crate) fn print<T: serde::Serialize>(&self, value: &T) -> Result<()> {
        logos_ext::cli::print_read_model(self.json, self.quiet, value)
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

    /// [`Output::query`] for a command whose MCP twin takes `repo`: the engine is
    /// the one [`member_engine`] selects, and under a workspace the answer carries
    /// the `member` it came from — a payload with none is serialised as before.
    pub(crate) fn query_for<T: serde::Serialize>(
        &self,
        root: &Path,
        member: &MemberArg,
        f: impl FnOnce(&Engine) -> T,
    ) -> Result<i32> {
        let selected = member_engine(root, member)?;
        self.print(&selected.answer(f(&selected.engine)))?;
        Ok(0)
    }

    /// [`Output::try_query`] for a command whose MCP twin takes `repo`.
    pub(crate) fn try_query_for<T: serde::Serialize>(
        &self,
        root: &Path,
        member: &MemberArg,
        f: impl FnOnce(&Engine) -> Result<T>,
    ) -> Result<i32> {
        let selected = member_engine(root, member)?;
        self.print(&selected.answer(f(&selected.engine)?))?;
        Ok(0)
    }

    /// [`Output::report_gate`] for `health` and `review`, the verdict commands whose
    /// MCP twins take `repo`: the exit code follows the selected member's verdict.
    pub(crate) fn report_gate_for<T: serde::Serialize>(
        &self,
        root: &Path,
        member: &MemberArg,
        f: impl FnOnce(&Engine) -> Result<T>,
        passed: impl FnOnce(&T) -> bool,
    ) -> Result<i32> {
        let selected = member_engine(root, member)?;
        let report = f(&selected.engine)?;
        let ok = passed(&report);
        self.print(&selected.answer(report))?;
        Ok(violation_code(ok))
    }

    /// Chokepoint for the governance verdict commands (`check`/`gate`/`doctor`/
    /// `verify`): run a fallible engine method, print the report, and map it to an
    /// exit code (FR-GV-03) — 1 on failure. `sync` rides it too: its verdict is
    /// whether the run persisted anything it reached (FR-EH-05, FR-CL-03). `code`
    /// projects the report to its exit code, which differs by command: most map
    /// their verdict field (`.passed` vs `.ok`) through [`violation_code`], while
    /// `gate`/`session_end` use `GateResult::exit_code`, whose exit 5 marks a
    /// `[security]` table's missing required scan (FR-SC-10).
    pub(crate) fn report_gate<T: serde::Serialize>(
        &self,
        root: &Path,
        f: impl FnOnce(&Engine) -> Result<T>,
        code: impl FnOnce(&T) -> i32,
    ) -> Result<i32> {
        let report = f(&engine(root, false)?)?;
        self.print(&report)?;
        Ok(code(&report))
    }

    /// `check` alone (FR-GV-22): the exit code is [`RulesReport::exit_code`]'s
    /// projection of the tri-state verdict, with exit 5 for a missing required
    /// security scan (FR-SC-10); the human surface names the absent-contract
    /// state instead of rendering it as a zero violation count, since
    /// "nothing was evaluated" is not the same claim as a clean evaluation
    /// (NFR-CC-04).
    ///
    /// The CLI's one absence-reporting site. It follows the one taxonomy rather
    /// than restating it — `models::quality::absence` in
    /// `logos-core/src/models/quality.rs` — and which rules it keeps is
    /// recorded beside its occurrence counts in
    /// `logos-core/tests/absence_taxonomy_audit.rs`, so there is one statement
    /// of the rules and one adjudication of this site, not a third copy here
    /// that can drift from either.
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

/// The binary's own composition — the very file `main` composes from, as the
/// surface snapshots read it (`cli/tests/surface_snapshots.rs`) — compiled into
/// the unit tests only, so the CLI↔MCP rosters reconcile against the SHIPPED
/// surface: the core's tools and commands, and the extensions' this build
/// registers (S-644). The library itself names no extension.
#[cfg(test)]
#[path = "composition.rs"]
mod composition;

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// `init -i` asks the skill question only when a composed extension contributes
    /// an init step — the only reader of the answer (S-651, FR-DS-05): with none it
    /// asks the four core questions and never the skill one, and a plain `init`
    /// asks nothing.
    #[test]
    fn init_asks_the_skill_question_only_for_a_composed_step() {
        struct Step;
        impl InitContribution for Step {
            fn run(&self, _: &Path, _: &InitOptions) -> Result<Vec<logos_core::models::pipeline::InitStep>> {
                Ok(Vec::new())
            }
        }
        let run = |interactive: bool, steps: &[std::sync::Arc<dyn InitContribution>]| {
            let mut questions = Vec::new();
            let options = init_options(interactive, false, steps, |question, default| {
                questions.push(question.to_string());
                default
            });
            (options.materialize_skill, questions)
        };
        let (skill, questions) = run(true, &[]);
        assert!(!skill, "nothing reads the answer, so it is no");
        assert_eq!(questions.len(), 4, "the four core questions: {questions:?}");
        assert!(!questions.iter().any(|q| q.contains("skill")), "{questions:?}");

        let composed: [std::sync::Arc<dyn InitContribution>; 1] = [std::sync::Arc::new(Step)];
        let (skill, questions) = run(true, &composed);
        assert!(skill, "a composed step reads the question's default, yes");
        assert_eq!(questions.len(), 5, "{questions:?}");
        assert!(questions.iter().any(|q| q.contains("skill")), "{questions:?}");

        assert_eq!(run(false, &composed), (false, Vec::new()), "a plain `init` asks nothing");
    }

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

    use logos_core::{
        federation::{EngineRegistry, Federation, RegistryMode},
        init::InitOptions,
        Engine,
    };
    use mcp::LogosMcp;

    use crate::composition;

    /// The shipped `logos` command: the core's, with the composed host's
    /// extension subcommands ([`crate::command`]).
    fn shipped_command() -> clap::Command {
        crate::command(&composition::host())
    }

    /// How one MCP tool is reachable from the CLI.
    enum Reach {
        /// The CLI command path that runs the same `Engine` call, space-joined
        /// (`"scan"`, `"coverage status"`, `"xservice callers"`).
        Twin(&'static str),
        /// The twin is a **mode flag** of another tool's command — `bless_rules`
        /// is `logos check --bless` (S-507, [FR-GV-23]). The path stays owned by
        /// that command's own tool; the flag is this tool's.
        ///
        /// [FR-GV-23]: ../../docs/specs/requirements/FR-GV-23.md
        TwinFlag(&'static str, &'static str),
        /// Deliberately MCP-only — the string is the reason, which [FR-CL-06]
        /// requires and this test requires to be non-empty.
        ///
        /// [FR-CL-06]: ../../docs/specs/requirements/FR-CL-06.md
        McpOnly(&'static str),
    }
    use Reach::{McpOnly, Twin, TwinFlag};

    impl Reach {
        /// The CLI command path a twin runs (a flag twin's included), or `None`
        /// for an MCP-only tool.
        fn cli_path(&self) -> Option<&'static str> {
            match self {
                Twin(path) | TwinFlag(path, _) => Some(path),
                McpOnly(_) => None,
            }
        }
    }

    /// Every tool the shipped MCP server registers, and its CLI reachability.
    ///
    /// Ordered as the server registers them (navigation, quality, temporal,
    /// coverage, then the federated `xservice_*`/`workspace_*` family). Read
    /// through the binary's own composition ([`composition`]), so a composed
    /// extension's tools would be reconciled here like the core's; this binary
    /// composes none. **Adding a tool means adding a line here** — that is the whole
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
        ("bless_rules", TwinFlag("check", "bless")),
        ("offenders", Twin("offenders")),
        ("review", Twin("review")),
        ("improvements", Twin("improvements")),
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
        // Security evidence tier (S-667).
        ("security_scan", Twin("security scan")),
        ("security_ingest", Twin("security ingest")),
        ("security_status", Twin("security status")),
        ("security_findings", Twin("security findings")),
        // Cross-service family — registered only by the federated backing.
        ("xservice_route_providers", Twin("xservice route-providers")),
        ("xservice_callers", Twin("xservice callers")),
        ("xservice_impact", Twin("xservice impact")),
        ("xservice_search", Twin("xservice search")),
        ("xservice_build_deps", Twin("xservice build-deps")),
        ("xservice_type_refs", Twin("xservice type-refs")),
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
            "hook",
            "CLI-only: an INSTALL operation (write the Claude Code session-start \
             quality-report hook and merge `.claude/settings.json`), not a \
             question about the code.",
        ),
        (
            "security install",
            "downloads and installs binaries Logos will then run, under a \
             per-user install consent (FR-SC-22, ADR-88 §6). An agent must not \
             grant itself that consent, so MCP has no install tool: \
             `security_status` names this command for the user to run instead.",
        ),
        (
            "security uninstall",
            "the other half of the per-user install lifecycle (FR-SC-22): it \
             changes which scanner binaries exist on the machine, a user's \
             decision, not an agent's. MCP reports the copies through \
             `security_status`.",
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
        walk(&shipped_command(), "", hidden, &mut out);
        out
    }

    /// The PUBLIC command surface — what `logos --help` offers. This is the set
    /// the two-way roster contract is written against.
    ///
    /// **`hide = true` commands are deliberately exempt**, and the exemption is
    /// a decision, not an accident of how the walker was written. Settled at the
    /// Sprint 64 human review, where three review agents split on it: [FR-CL-06]
    /// governs the parity between *tools* and their CLI twins, and a hidden
    /// command is neither a tool nor something a user can be told to run —
    /// today's only one, `internal-warm`, is the [FR-WS-14] bounded-warm
    /// supervisor, spawned solely by `supervisor_argv` and declared not a public
    /// contract by [FR-CL-01]. Requiring it to carry a `CLI_ONLY` reason would
    /// put an internal re-invocation of the binary in a roster whose purpose is
    /// to describe the user-facing surface.
    ///
    /// The dangerous half of the objection is already closed elsewhere: a hidden
    /// command can no longer mask a stale `McpOnly` claim, because that check
    /// reads [`all_cli_command_paths`] instead. What is exempted here is only
    /// the obligation to write down a reason.
    ///
    /// [FR-CL-01]: ../../docs/specs/requirements/FR-CL-01.md
    /// [FR-CL-06]: ../../docs/specs/requirements/FR-CL-06.md
    /// [FR-WS-14]: ../../docs/specs/requirements/FR-WS-14.md
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
            member_kinds: Default::default(),
            exclusion: Default::default(),
        };
        LogosMcp::federated(EngineRegistry::new(federation, RegistryMode::Lazy), &composition::host())
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
                TwinFlag(path, flag) => {
                    let mut root = shipped_command();
                    root.build();
                    let cmd = path.split(' ').fold(&root, |cmd, segment| {
                        cmd.find_subcommand(segment)
                            .unwrap_or_else(|| panic!("`logos {path}` resolves ({segment} missing)"))
                    });
                    assert!(
                        cmd.get_arguments().any(|arg| arg.get_long() == Some(*flag)),
                        "tool `{tool}` claims the CLI twin `logos {path} --{flag}`, which \
                         the shipped clap definition does not accept (FR-CL-06)"
                    );
                }
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
                // A flag twin borrows no path: `check` stays `check_rules`'s.
                TwinFlag(..) | McpOnly(_) => None,
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
        let mut root = shipped_command();
        // Globals are propagated to subcommands at build time, so the check
        // reads what a real invocation would see, not the pre-build derive.
        root.build();

        for (tool, reach) in MCP_SURFACE {
            let Some(path) = reach.cli_path() else { continue };
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

    /// A tool takes `repo` exactly when its CLI twin takes `--repo` ([FR-WS-42]):
    /// a human and an agent ask the same per-member question the same way.
    ///
    /// Read off the real router and the real clap tree, over every twin in
    /// [`MCP_SURFACE`] — not off the two hand-kept lists the MCP and CLI suites
    /// each pin (`MEMBER_SELECTABLE_TOOLS`, `TWINS`), which agree only by
    /// convention. Moving a tool to the default-member set on one surface alone
    /// (dropping `repo` from its schema and re-recording the snapshot) fails here.
    ///
    /// [FR-WS-42]: ../../docs/specs/requirements/FR-WS-42.md
    #[test]
    fn a_tool_takes_repo_exactly_when_its_cli_twin_takes_repo() {
        let federation = Federation {
            name: "w".to_string(),
            root: "/ws".into(),
            members: Vec::new(),
            default: None,
            links: Vec::new(),
            governance: Default::default(),
            warm_concurrency: None,
            member_kinds: Default::default(),
            exclusion: Default::default(),
        };
        let takes_repo: BTreeMap<String, bool> =
            LogosMcp::federated(EngineRegistry::new(federation, RegistryMode::Lazy), &composition::host())
                .list_tools()
                .iter()
                .map(|tool| (tool.name.to_string(), tool.input_schema.get("properties").and_then(|p| p.get("repo")).is_some()))
                .collect();
        let mut root = shipped_command();
        root.build();

        let mut mismatched = Vec::new();
        for (tool, reach) in MCP_SURFACE {
            let Some(path) = reach.cli_path() else { continue };
            let mut cmd = &root;
            for segment in path.split(' ') {
                cmd = cmd
                    .find_subcommand(segment)
                    .unwrap_or_else(|| panic!("`logos {path}` resolves ({segment} missing)"));
            }
            let cli = cmd.get_arguments().any(|arg| arg.get_long() == Some("repo"));
            let mcp = takes_repo[*tool];
            if cli != mcp {
                mismatched.push(format!("{tool} (repo: {mcp}) / logos {path} (--repo: {cli})"));
            }
        }
        assert!(mismatched.is_empty(), "repo and --repo disagree: {mismatched:?}");
        assert!(takes_repo["node"], "the relation is not vacuous: `node` takes `repo`");
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

    /// The markers of the scoping-before-navigation repositioning in one text.
    struct Repositioning {
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
    }

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
        /// The scoping-before-navigation repositioning this text carries, which
        /// [FR-IN-09] governs in three texts. `None` for a page that is only
        /// reconciled against the binary (the plugin page): it states no
        /// repositioning, so it is exempt from the tests about one and still
        /// held to the span walk.
        ///
        /// [FR-IN-09]: ../../docs/specs/requirements/FR-IN-09.md
        repositioning: Option<Repositioning>,
        /// The fewest tool/command claims this text must actually reconcile
        /// against the shipped binary. A floor, not a count: it exists so a text
        /// that stops naming anything — or that the span parser stops reading —
        /// fails instead of passing vacuously. Set below today's real figure so
        /// ordinary prose edits do not trip it.
        min_reconciled_claims: usize,
    }

    /// The repository root — `CARGO_MANIFEST_DIR` is `<root>/cli`.
    ///
    /// Only `docs/` needs this: it belongs to no crate, so there is no constant
    /// to read it from. The other two texts come from the shipped binary itself.
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
                repositioning: Some(Repositioning {
                    scoping_marker: "### Primary — before you decompose the work",
                    navigation_marker: "### Secondary — while you are editing",
                    hypothesis_marker: "hypothesis under measurement, not a settled finding",
                }),
                min_reconciled_claims: 12,
            },
            Guidance {
                label: "the MCP server instructions",
                // The SHIPPED constant, not a path-derived copy: `server.rs`
                // hands this very string to `with_instructions`, so the guard
                // asserts what a host receives — the same reasoning as
                // generating the managed block through the real `init` above.
                text: mcp::INSTRUCTIONS.to_string(),
                bare_names_are_tools: true,
                repositioning: Some(Repositioning {
                    scoping_marker: "## Primary — before the work is decomposed",
                    navigation_marker: "## Secondary — while editing",
                    hypothesis_marker: "hypothesis, not a finding",
                }),
                min_reconciled_claims: 16,
            },
            Guidance {
                // FR-IN-09 says "the README"; no top-level one exists, and this
                // user guide is the README the AC means (CR-143 §3.8, S-444).
                label: "the user guide (docs/howto/README.md)",
                text: read(repo_root().join("docs/howto/README.md")),
                bare_names_are_tools: false,
                repositioning: Some(Repositioning {
                    scoping_marker: "## What it is primarily for: scoping work before you decompose it",
                    navigation_marker: "is the secondary mode",
                    hypothesis_marker: "hypothesis under measurement, not a settled finding",
                }),
                min_reconciled_claims: 10,
            },
            Guidance {
                // The plugin page (S-684) names the commands the plugin's hooks
                // and skill call; every one is reconciled against the binary.
                label: "the plugin guide (docs/howto/plugin.md)",
                text: read(repo_root().join("docs/howto/plugin.md")),
                bare_names_are_tools: false,
                repositioning: None,
                min_reconciled_claims: 30,
            },
        ]
    }

    /// A how-to page held to the shipped binary by the same tool-and-command
    /// walk as the guidance texts, but not to FR-IN-09's scoping, hypothesis and
    /// relational-scope statements, which govern the three channels of one
    /// message and have no place in a topic page.
    struct ManualPage {
        label: &'static str,
        text: String,
        /// The fewest claims the walk must reconcile (see
        /// [`Guidance::min_reconciled_claims`]).
        min_reconciled_claims: usize,
    }

    /// The topic pages of `docs/howto/` the walk covers. A page that names a
    /// command or a tool joins here, so a rename fails a build instead of the page.
    fn shipped_manual_pages() -> Vec<ManualPage> {
        let page = |relative: &str| {
            let path = repo_root().join(relative);
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        };
        vec![ManualPage {
            label: "the security page (docs/howto/security.md)",
            text: page("docs/howto/security.md"),
            min_reconciled_claims: 23,
        }]
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
            let Some(r) = &g.repositioning else { continue };
            let scoping = g.text.find(r.scoping_marker).unwrap_or_else(|| {
                panic!(
                    "{} does not present scoping before decomposition (FR-IN-09): \
                     expected to find {:?}",
                    g.label, r.scoping_marker
                )
            });
            let navigation = g.text.find(r.navigation_marker).unwrap_or_else(|| {
                panic!(
                    "{} does not mark navigate-while-coding as the secondary mode \
                     (FR-IN-09): expected to find {:?}",
                    g.label, r.navigation_marker
                )
            });
            assert!(
                scoping < navigation,
                "{} presents navigation before scoping (FR-IN-09): {:?} must come \
                 before {:?}",
                g.label,
                r.scoping_marker,
                r.navigation_marker
            );

            // Headings alone are a weak anchor: swapping the two sections' BODIES,
            // or inserting a navigation-first section above the scoping heading,
            // leaves both offsets untouched. Anchor to the material as well — the
            // first planning-time capability must be named before the first
            // navigate-while-coding tool.
            // Matched on the OPENING backtick only: a span may wrap across two
            // source lines (`logos impact-intersection\n  --item …`), so a form
            // that also required the closing backtick would miss it.
            let first_capability = CAPABILITIES
                .iter()
                .filter_map(|(tool, command)| {
                    [format!("`{tool}"), format!("`logos:{tool}"), format!("`logos {command}")]
                        .iter()
                        .filter_map(|form| g.text.find(form.as_str()))
                        .min()
                })
                .min()
                .expect("a planning-time capability is named (asserted separately)");
            let first_navigation = ["`callers", "`logos:callers", "`callees", "`logos:callees"]
                .iter()
                .filter_map(|form| g.text.find(form))
                .min()
                .expect("a navigate-while-coding tool is named");
            assert!(
                first_capability < first_navigation,
                "{} names a navigate-while-coding tool (offset {first_navigation}) \
                 before the first planning-time capability (offset \
                 {first_capability}) (FR-IN-09 AC 1). The headings say scoping is \
                 primary; the material must too",
                g.label
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
            let Some(r) = &g.repositioning else { continue };
            assert!(
                g.text.contains(r.hypothesis_marker),
                "{} presents the repositioning as settled (NFR-CC-04): expected \
                 to find {:?}",
                g.label,
                r.hypothesis_marker
            );
        }
    }

    /// The one statement of where the guidance's relational claims hold
    /// ([FR-IN-09] AC 4, [CR-143] §3.8), carried **verbatim** by all three
    /// texts — whitespace aside, since each wraps its lines differently.
    ///
    /// # Why three checked copies and not one derived source
    ///
    /// The three texts spell the same tool three ways (`logos:callers`,
    /// `callers`, `logos callers`), so deriving them whole would need a
    /// templating layer, and the README is a hand-edited document outside every
    /// crate that no constant can generate. What must agree is this one
    /// statement, so it is written spelling-neutral — tools named in prose, only
    /// the denominator's own vocabulary in code spans — and each text is checked
    /// to contain it. That is the guarantee a derived source would give the two
    /// embedded texts, and the only one available for the README: editing one
    /// copy without the others fails here, so "two of three" cannot ship.
    ///
    /// # Why it reads the denominator instead of naming languages
    ///
    /// Which languages resolve a cross-file call is exactly what the resolver's
    /// next increment changes. A statement that named them would be false the
    /// day that lands; one that tells the reader which field to read stays
    /// true, and [`the_relational_scope_speaks_the_denominators_own_vocabulary`]
    /// pins that field to what [S-442] actually serialises.
    ///
    /// [FR-IN-09]: ../../docs/specs/requirements/FR-IN-09.md
    /// [CR-143]: ../../docs/requests/CR-143-a-relational-answer-states-its-resolution-denominator.md
    /// [S-442]: ../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
    const RELATIONAL_SCOPE: &str = "\
        Relational answers hold only on the languages the graph resolves. The answers \
        that traverse resolved edges — callers, callees, impact, affected, precedent, \
        impact intersection and branch overlap — say which: each carries a \
        `resolution_denominator` whose `languages` rows give each anchor language's \
        `calls` resolution. A node answer's edge list carries none, so read it against \
        the project's rows named below. Where a row has a `cross_file_edges` figure, \
        the graph beats grep for that language, as completely as its `bound` share of \
        `references` — a partial ratio is a partial answer. Where its \
        `cross_file_absence` reads `same-file-only` or `no-resolved-edges`, the graph \
        binds no call across a file boundary there: an empty or short answer is the \
        index's reach, not evidence of absence, and grep is the better tool. Callers \
        can live in another language than their target, so read the whole project's \
        rows as well — `resolution_by_language` in the status readout.";

    /// The relational over-claims [CR-143] §3.8 measured false outside Rust. Each
    /// may appear in a guidance text only inside [`RELATIONAL_SCOPE`], which
    /// states the language scope it holds on.
    ///
    /// A lexical tripwire against re-introducing the unscoped sentence beside
    /// the scoped one, not a detector of every possible over-claim: a rewording
    /// escapes it, which is why the positive check — every text carries the
    /// scope statement — is the load-bearing half.
    ///
    /// [CR-143]: ../../docs/requests/CR-143-a-relational-answer-states-its-resolution-denominator.md
    const RELATIONAL_OVERCLAIMS: &[&str] = &["beats grep", "says why"];

    /// `text` with every whitespace run collapsed to one space, so a statement
    /// matches however a text wraps it.
    fn normalise_whitespace(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Every guidance text states the language scope its relational claims hold
    /// on, in the one shared wording, and states none of them unscoped
    /// ([FR-IN-09] AC 4, [CR-143] §3.8).
    ///
    /// Every text is judged before anything fails, and the failure names each
    /// offending text: a surface corrected while its sibling is not is exactly
    /// the outcome this exists to reject ([S-434]'s sibling call site).
    ///
    /// [FR-IN-09]: ../../docs/specs/requirements/FR-IN-09.md
    /// [CR-143]: ../../docs/requests/CR-143-a-relational-answer-states-its-resolution-denominator.md
    /// [S-434]: ../../docs/planning/journal.md#s-434-one-absence-taxonomy-audited-across-the-three-reporting-surfaces
    #[test]
    fn the_shipped_guidance_states_the_language_scope_of_its_relational_claims() {
        let scope = normalise_whitespace(RELATIONAL_SCOPE);
        // The three texts [FR-IN-09] governs: a page with no repositioning (the
        // plugin guide) makes no relational claim to scope.
        let guidance: Vec<Guidance> = shipped_guidance()
            .into_iter()
            .filter(|g| g.repositioning.is_some())
            .collect();
        let mut failures = Vec::new();
        for g in &guidance {
            let text = normalise_whitespace(&g.text);
            let Some(at) = text.find(&scope) else {
                failures.push(format!(
                    "{} does not carry the relational scope statement verbatim",
                    g.label
                ));
                continue;
            };
            let outside = format!("{}{}", &text[..at], &text[at + scope.len()..]);
            for claim in RELATIONAL_OVERCLAIMS {
                if outside.contains(claim) {
                    failures.push(format!(
                        "{} claims {claim:?} outside the scope statement, which \
                         states the languages that claim holds on",
                        g.label
                    ));
                }
            }
        }
        assert_eq!(guidance.len(), 3, "FR-IN-09 governs three texts");
        assert!(
            failures.is_empty(),
            "the shipped guidance overclaims its relational reach (FR-IN-09 AC 4, \
             CR-143 §3.8) — every text must state the language scope, or none of \
             them is corrected:\n  {}",
            failures.join("\n  ")
        );
    }

    /// The object keys and `cause` tags a relational answer and the status
    /// readout actually serialise, over one language row in each cross-file
    /// state — paired, for each tag, with whether [S-442]'s
    /// `unresolved_calls_clause` treats that state as **unresolved**.
    ///
    /// Read off real serialised values rather than listed, so a renamed field
    /// or tag fails the guidance instead of leaving it describing a shape the
    /// binary no longer has. An absence's payload keys are left out: they reuse
    /// row field names, and would otherwise keep a renamed row field "present".
    ///
    /// [S-442]: ../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
    fn denominator_vocabulary() -> (BTreeSet<String>, BTreeMap<String, bool>) {
        use logos_core::models::navigation::{
            CallersResult, LanguageResolution, RelationResolution, ResolutionDenominator,
            StatusInfo,
        };

        // A `cause`-tagged object is an absence, whose payload reuses row field
        // names (`no-resolved-edges` carries `references` and `bound`). Its tag
        // is collected below; its payload is not, so a renamed row field cannot
        // survive on an absence's key of the same name.
        fn keys(value: &serde_json::Value, into: &mut BTreeSet<String>) {
            match value {
                serde_json::Value::Object(map) if map.contains_key("cause") => {}
                serde_json::Value::Object(map) => {
                    for (key, inner) in map {
                        into.insert(key.clone());
                        keys(inner, into);
                    }
                }
                serde_json::Value::Array(items) => items.iter().for_each(|v| keys(v, into)),
                _ => {}
            }
        }

        // (language, references, bound, same-file edges, cross-file edges): a
        // cross-file figure, then each arm of `CrossFileAbsence::classify`.
        let rows: Vec<LanguageResolution> = [
            ("rust", 10, 8, 3, 5),
            ("tsx", 10, 4, 4, 0),
            ("go", 10, 0, 0, 0),
            ("toml", 0, 0, 0, 0),
        ]
        .into_iter()
        .map(|(language, references, bound, same, cross)| LanguageResolution {
            language: language.to_string(),
            files: 1,
            calls: RelationResolution::measured(references, bound, same, cross),
            imports: RelationResolution::measured(0, 0, 0, 0),
            call_residue: None,
        })
        .collect();

        let mut tags = BTreeMap::new();
        for row in &rows {
            let Some(absence) = row.calls.cross_file_absence else {
                continue;
            };
            let tag = serde_json::to_value(absence).expect("serialises")["cause"]
                .as_str()
                .expect("a cross-file absence is tagged by `cause`")
                .to_string();
            let unresolved = ResolutionDenominator::measured(
                vec![row.clone()],
                &[Some(row.language.clone())],
            )
            .unresolved_calls_clause()
            .is_some();
            tags.insert(tag, unresolved);
        }
        assert_eq!(tags.len(), 3, "one row per `CrossFileAbsence` arm");

        let anchors: Vec<Option<String>> = rows.iter().map(|r| Some(r.language.clone())).collect();
        let mut vocabulary = BTreeSet::new();
        keys(
            &serde_json::to_value(CallersResult {
                resolution_denominator: ResolutionDenominator::measured(rows.clone(), &anchors),
                ..CallersResult::default()
            })
            .expect("serialises"),
            &mut vocabulary,
        );
        keys(
            &serde_json::to_value(StatusInfo {
                resolution_by_language: rows,
                ..StatusInfo::default()
            })
            .expect("serialises"),
            &mut vocabulary,
        );
        vocabulary.extend(tags.keys().cloned());
        (vocabulary, tags)
    }

    /// The scope statement speaks the denominator [S-442] ships: every code span
    /// in it is a key or tag the answers really serialise, and the states it
    /// hands to grep are exactly the ones `unresolved_calls_clause` treats as
    /// unresolved ([FR-IN-09] AC 4, [CR-143] §3.8).
    ///
    /// The second half is what "consistent with the denominator" means: were the
    /// statement to send the reader to grep on `no-references-recorded` — a
    /// language that records no call, where an empty answer is true — it would
    /// word the same condition differently from the answer it points at.
    ///
    /// [S-442]: ../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
    /// [FR-IN-09]: ../../docs/specs/requirements/FR-IN-09.md
    /// [CR-143]: ../../docs/requests/CR-143-a-relational-answer-states-its-resolution-denominator.md
    #[test]
    fn the_relational_scope_speaks_the_denominators_own_vocabulary() {
        let (vocabulary, tags) = denominator_vocabulary();
        let spans = inline_code_spans("the relational scope statement", RELATIONAL_SCOPE);
        assert!(spans.len() >= 8, "the statement names the fields it points at");
        for span in &spans {
            assert!(
                vocabulary.contains(span),
                "the relational scope statement names `{span}`, which no relational \
                 answer or status readout serialises (FR-IN-09 AC 4). Serialised: \
                 {vocabulary:?}"
            );
        }
        let named: BTreeSet<&str> = spans
            .iter()
            .map(String::as_str)
            .filter(|span| tags.contains_key(*span))
            .collect();
        let unresolved: BTreeSet<&str> = tags
            .iter()
            .filter(|(_, unresolved)| **unresolved)
            .map(|(tag, _)| tag.as_str())
            .collect();
        assert_eq!(
            named, unresolved,
            "the scope statement sends the reader to grep on a different set of \
             states from the ones the answer itself calls unresolved (S-442's \
             `unresolved_calls_clause`)"
        );
    }

    /// The answers the scope statement says carry a denominator, and the one it
    /// says does not, are the ones whose result types serialise it ([S-442],
    /// [FR-IN-09] AC 4).
    ///
    /// The vocabulary check above pins the statement's code spans; this pins its
    /// prose list, which names answers in words so it can read the same on all
    /// three surfaces. Without it the list was checked by reading — "node" could
    /// join it on every copy at once and every other guard stayed green.
    ///
    /// [S-442]: ../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
    /// [FR-IN-09]: ../../docs/specs/requirements/FR-IN-09.md
    #[test]
    fn the_relational_scope_names_the_answers_that_carry_a_denominator() {
        use logos_core::models::navigation::{
            AffectedResult, BranchOverlapResult, CalleesResult, CallersResult,
            ImpactIntersectionResult, ImpactResult, NodeInfo, PrecedentResult,
        };

        let carries = |value: serde_json::Value| value.get("resolution_denominator").is_some();
        let answers = [
            ("callers", carries(serde_json::to_value(CallersResult::default()).unwrap())),
            ("callees", carries(serde_json::to_value(CalleesResult::default()).unwrap())),
            ("impact", carries(serde_json::to_value(ImpactResult::default()).unwrap())),
            ("affected", carries(serde_json::to_value(AffectedResult::default()).unwrap())),
            ("precedent", carries(serde_json::to_value(PrecedentResult::default()).unwrap())),
            (
                "impact intersection",
                carries(serde_json::to_value(ImpactIntersectionResult::default()).unwrap()),
            ),
            (
                "branch overlap",
                carries(serde_json::to_value(BranchOverlapResult::default()).unwrap()),
            ),
        ];
        for (answer, carried) in answers {
            assert!(
                carried,
                "the relational scope statement says the {answer} answer carries a \
                 `resolution_denominator`, and its result type does not serialise one"
            );
        }
        let (last, rest) = answers.split_last().expect("seven answers");
        let listed = format!(
            "— {} and {} —",
            rest.iter().map(|(a, _)| *a).collect::<Vec<_>>().join(", "),
            last.0
        );
        let scope = normalise_whitespace(RELATIONAL_SCOPE);
        assert!(
            scope.contains(&listed),
            "the relational scope statement's list of answers is not {listed:?} — \
             the list this test checks against the result types"
        );

        assert!(
            scope.contains("A node answer's edge list carries none"),
            "the statement no longer says the node answer carries no denominator — \
             drop this half with it"
        );
        assert!(
            !carries(serde_json::to_value(NodeInfo::default()).unwrap()),
            "the node answer now carries a `resolution_denominator`: the statement's \
             \"A node answer's edge list carries none\" is false — move node into the list"
        );
    }

    /// The three planning-time capabilities ([FR-NV-11], [FR-NV-12], [FR-NV-13])
    /// — tool name and CLI command spelling.
    const CAPABILITIES: &[(&str, &str)] = &[
        ("impact_intersection", "impact-intersection"),
        ("precedent", "precedent"),
        ("branch_overlap", "branch-overlap"),
    ];

    /// Each text names the three planning-time capabilities **as capabilities**,
    /// in whichever spelling that surface uses ([FR-IN-09] AC 2).
    ///
    /// These are the capabilities the repositioning points at — guidance that
    /// says "scope first" without naming what to scope with is a slogan.
    ///
    /// # Why this matches code spans and not the raw text
    ///
    /// "precedent" is an ordinary English word, and the lede of two of these
    /// texts uses it in prose ("finding the precedent to follow"). A raw
    /// `contains` therefore passed even with the `precedent` bullet deleted —
    /// a false green on the AC this story exists to satisfy. A capability is
    /// named when it appears as a code span, so that is what is asserted.
    ///
    /// [FR-IN-09]: ../../docs/specs/requirements/FR-IN-09.md
    /// [FR-NV-11]: ../../docs/specs/requirements/FR-NV-11.md
    /// [FR-NV-12]: ../../docs/specs/requirements/FR-NV-12.md
    /// [FR-NV-13]: ../../docs/specs/requirements/FR-NV-13.md
    #[test]
    fn the_shipped_guidance_names_the_planning_time_capabilities() {
        for g in shipped_guidance() {
            if g.repositioning.is_none() {
                continue;
            }
            let named: BTreeSet<String> = inline_code_spans(g.label, &g.text)
                .iter()
                .filter_map(|span| span.split(' ').next().map(str::to_string))
                .collect();
            for (tool, command) in CAPABILITIES {
                // The three accepted spellings: the MCP wire name bare
                // (`precedent`), namespaced (`logos:precedent`), and the CLI
                // command (`logos precedent`, whose first token is `logos`, so
                // match the whole span's leading two words instead).
                let named_as_capability = named.contains(*tool)
                    || named.contains(&format!("logos:{tool}"))
                    || inline_code_spans(g.label, &g.text)
                        .iter()
                        .any(|span| span == &format!("logos {command}")
                            || span.starts_with(&format!("logos {command} ")));
                assert!(
                    named_as_capability,
                    "{} does not name the planning-time capability `{tool}` as a \
                     code span (FR-IN-09 AC 2). Prose that merely uses the word is \
                     not naming the capability — the guidance must say what to \
                     scope WITH, not just that scoping comes first",
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
        // The scope statement's spans are field names and tags, reconciled
        // against the serialised denominator by
        // `the_relational_scope_speaks_the_denominators_own_vocabulary`, not
        // against the router — where tools are named bare they would otherwise
        // read as tool claims. Only the statement's own run of spans is exempt:
        // the same words anywhere else in a text are still claims to check.
        let scope_spans = inline_code_spans("the relational scope statement", RELATIONAL_SCOPE);

        // The three FR-IN-09 texts, then the how-to pages held to the same walk.
        let pages = shipped_manual_pages();
        let texts: Vec<_> = shipped_guidance()
            .into_iter()
            .map(|g| (g.label, g.text, g.bare_names_are_tools, g.min_reconciled_claims))
            .chain(pages.into_iter().map(|p| (p.label, p.text, false, p.min_reconciled_claims)))
            .collect();
        // Dropping the pages from the walk must fail here, not only as a
        // dead-code warning: the three FR-IN-09 texts, plus at least the security page.
        assert!(
            texts.len() > 3 && texts.iter().any(|t| t.0.contains("security page")),
            "the manual pages are no longer walked: {:?}",
            texts.iter().map(|t| t.0).collect::<Vec<_>>()
        );
        for (label, text, bare_names_are_tools, min_reconciled_claims) in texts {
            // How many spans this text actually RECONCILED against the binary.
            // Without a floor, a text the parser mis-reads (or a future edit that
            // drops every backticked claim) checks nothing and still passes.
            let mut reconciled = 0_usize;
            let spans = inline_code_spans(label, &text);
            // Where the statement sits in this text's span sequence; a text that
            // lacks it exempts nothing, and fails the scope test instead.
            let statement = spans
                .windows(scope_spans.len())
                .position(|run| run == scope_spans.as_slice())
                .map_or(0..0, |at| at..at + scope_spans.len());
            for (index, span) in spans.into_iter().enumerate() {
                // `logos:*` is the tool NAMESPACE, not a tool.
                if span == "logos:*" || statement.contains(&index) {
                    continue;
                }
                if let Some(rest) = span.strip_prefix("logos:") {
                    let tool = rest.split(' ').next().expect("split yields one item");
                    assert!(
                        tools.contains(tool),
                        "{} names the MCP tool `logos:{tool}`, which the shipped \
                         server does not register (FR-IN-09). Span: `{span}`",
                        label
                    );
                    reconciled += 1;
                } else if let Some(rest) = span.strip_prefix("logos ") {
                    reconciled += usize::from(assert_command_path_exists(
                        &commands, rest, &span, label,
                    ));
                } else if bare_names_are_tools
                    && is_plain_token(&span)
                    && !MCP_BARE_NON_TOOLS.contains(&span.as_str())
                {
                    assert!(
                        tools.contains(span.as_str()),
                        "{} names `{span}` as a tool, which the shipped server does \
                         not register (FR-IN-09). If it is not a tool claim, add it \
                         to MCP_BARE_NON_TOOLS with the reason",
                        label
                    );
                    reconciled += 1;
                }
            }
            assert!(
                reconciled >= min_reconciled_claims,
                "{} reconciled only {reconciled} claims against the shipped binary, \
                 below its floor of {} (FR-IN-09 AC 4). Either the text stopped \
                 naming tools and commands, or the span parser stopped seeing them \
                 — a guard that checks nothing passes for the wrong reason",
                label,
                min_reconciled_claims
            );
        }
    }

    /// The user guide's subcommand COUNT matches the shipped clap surface
    /// ([FR-IN-09] AC 4).
    ///
    /// This story had to correct `docs/howto/README.md` from "27-subcommand" to
    /// 38 — proof that a hand-counted cardinality drifts silently. The spans walk
    /// checks that every *named* command exists; it says nothing about how many
    /// there are, so the number needs its own guard. `docs/howto/commands.md`
    /// carries the same figure in its opening line and is checked with it.
    ///
    /// [FR-IN-09]: ../../docs/specs/requirements/FR-IN-09.md
    #[test]
    fn the_user_guide_subcommand_count_matches_the_shipped_surface() {
        // Top-level, public, clap's generated `help` excluded — the set a reader
        // of `logos --help` counts. `command_paths` returns leaves, so a group
        // (`coverage ingest`) would be counted once per leaf; go to clap directly.
        let shipped = shipped_command()
            .get_subcommands()
            .filter(|sub| !sub.is_hide_set() && sub.get_name() != "help")
            .count();

        // (file, the phrase whose preceding integer is the claim)
        for (relative, phrase) in [
            ("docs/howto/README.md", "-subcommand reference"),
            ("docs/howto/commands.md", " top-level commands"),
        ] {
            let path = repo_root().join(relative);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let at = text.find(phrase).unwrap_or_else(|| {
                panic!("{relative} no longer states a subcommand count near {phrase:?} — \
                        update or drop this guard")
            });
            let claimed: usize = text[..at]
                .rsplit(|c: char| !c.is_ascii_digit())
                .next()
                .filter(|digits| !digits.is_empty())
                .unwrap_or_else(|| panic!("{relative} states no number before {phrase:?}"))
                .parse()
                .expect("digits parse");
            assert_eq!(
                claimed, shipped,
                "{relative} claims {claimed} top-level commands but the shipped \
                 clap definition has {shipped} (FR-IN-09 AC 4)"
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
    /// A *group* (`coverage`, `xservice`) is accepted only as an intermediate
    /// step. clap rejects `logos coverage` on its own, so a span that RUNS OUT of
    /// tokens on a group is a false claim — unless the walk was cut short by a
    /// placeholder or an alternation (`logos coverage ingest|status|…`), where the
    /// group is as far as this parser can honestly get.
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

    /// The `logos` command lines a markdown text shows, as `(inline, fenced)`:
    /// the inline spans that start `logos `, and the fenced lines that do, with a
    /// trailing `# comment` cut and a chained line (`a && b`, `a | b`, `a; b`)
    /// split so each command is read on its own.
    fn logos_invocations(markdown: &str) -> (Vec<String>, Vec<String>) {
        let mut fenced = Vec::new();
        let mut in_fence = false;
        for line in markdown.lines() {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
            } else if in_fence {
                let line = line.split(" #").next().unwrap_or("");
                fenced.extend(
                    line.replace("&&", "|")
                        .replace("||", "|")
                        .replace(';', "|")
                        .split('|')
                        .map(str::trim)
                        .filter(|segment| segment.starts_with("logos "))
                        .map(str::to_string),
                );
            }
        }
        let inline = inline_code_spans("a markdown text", markdown)
            .into_iter()
            .filter(|span| span.starts_with("logos "))
            .collect();
        (inline, fenced)
    }

    /// Check one `logos …` command line against the shipped clap definition and
    /// return how many flags it reconciled. The command path is as deep as the
    /// tokens name subcommands, starting at the root, so a global flag may come
    /// first (`logos --json quality-report`); the first word that is neither a
    /// flag nor a subcommand of the root names no command and fails. After the
    /// command, a word that is not a subcommand is a value and ends the descent.
    /// A flag is checked against the command reached so far: `--long`, `--long=v`,
    /// and every letter of a short run (`-zq`); a bracketed synopsis
    /// (`[--base <ref>]`) is read through its brackets.
    fn reconcile_invocation(root: &clap::Command, invocation: &str) -> usize {
        let mut cmd = root;
        let mut descending = true;
        let mut flags = 0_usize;
        for token in invocation.split_whitespace().skip(1) {
            let token = token.trim_matches(|c| matches!(c, '[' | ']' | '(' | ')' | ','));
            let name = token.split('=').next().unwrap_or("");
            if let Some(long) = name.strip_prefix("--").filter(|n| !n.is_empty()) {
                assert!(
                    cmd.get_arguments().any(|arg| arg.get_long() == Some(long)),
                    "plugin.md shows `{invocation}`, but `{}` accepts no `{name}` \
                     (FR-IN-09 AC 4)",
                    cmd.get_name()
                );
                flags += 1;
            } else if let Some(shorts) = name.strip_prefix('-').filter(|n| !n.is_empty()) {
                for short in shorts.chars() {
                    assert!(
                        cmd.get_arguments().any(|arg| arg.get_short() == Some(short)),
                        "plugin.md shows `{invocation}`, but `{}` accepts no `-{short}` \
                         (FR-IN-09 AC 4)",
                        cmd.get_name()
                    );
                    flags += 1;
                }
            } else if let Some(sub) = descending.then(|| cmd.find_subcommand(token)).flatten() {
                cmd = sub;
            } else {
                assert!(
                    !std::ptr::eq(cmd, root) || token.starts_with(['<', '$']),
                    "plugin.md shows `{invocation}`, but `{token}` is no `logos` command \
                     (FR-IN-09 AC 4)"
                );
                descending = false;
            }
        }
        flags
    }

    /// The plugin guide names only commands and flags the shipped binary accepts
    /// ([FR-IN-09] AC 4, S-684). The span walk proves each *inline* command
    /// exists and says nothing of fenced blocks or of the `--flag` after a
    /// command, which is where a renamed option goes stale. Every `logos …`
    /// command the guide shows, inline or fenced, resolves to a command, and
    /// every flag after it to an argument of that command (or a global one).
    ///
    /// [FR-IN-09]: ../../docs/specs/requirements/FR-IN-09.md
    #[test]
    fn the_plugin_guide_names_only_flags_the_binary_accepts() {
        // The page is a text beside FR-IN-09's three: dropping its entry would
        // silently drop its span walk too.
        assert!(
            shipped_guidance().iter().any(|g| g.label.contains("plugin guide")),
            "the plugin guide is no longer one of the shipped guidance texts"
        );
        let text = std::fs::read_to_string(repo_root().join("docs/howto/plugin.md"))
            .expect("read docs/howto/plugin.md");
        let mut root = shipped_command();
        root.build();
        let (inline, fenced) = logos_invocations(&text);
        let inline_flags: usize = inline.iter().map(|i| reconcile_invocation(&root, i)).sum();
        let fenced_flags: usize = fenced.iter().map(|i| reconcile_invocation(&root, i)).sum();
        // One floor per half: either can die silently while the other keeps a
        // single floor satisfied.
        assert!(
            inline_flags >= 20 && fenced_flags >= 6,
            "plugin.md reconciled {inline_flags} inline and {fenced_flags} fenced flags \
             against the shipped CLI, below the floors of 20 and 6; a guard that \
             checks nothing passes for the wrong reason"
        );
    }

    /// The `LOGOS_*` name at the start of `text`, or an empty string.
    fn leading_variable(text: &str) -> String {
        let name: String = text
            .chars()
            .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
            .collect();
        if name.starts_with("LOGOS_") { name } else { String::new() }
    }

    /// Every file under `dir`, recursively.
    fn files_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read a directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                files_under(&path, out);
            } else {
                out.push(path);
            }
        }
    }

    /// Every environment variable the plugin guide names is read by a shipped
    /// script or source file, and every `LOGOS_*` variable a plugin script reads
    /// is named in the guide (S-684). Without the second half a variable added to
    /// a script ships undocumented; without the first, a renamed one stays
    /// documented. Names match whole: `LOGOS_STOP_HOOK_BUDGET` is not
    /// `LOGOS_STOP_HOOK_BUDGET_MS`. The first half reads comments as well as
    /// code, so it proves a name exists in the shipped tree, not that it is live.
    #[test]
    fn the_plugin_guide_names_exactly_the_variables_the_plugin_reads() {
        // Shell variables a plugin script sets for itself, not environment input.
        const INTERNAL: &[&str] = &["LOGOS_BIN", "LOGOS_PLUGIN_ROOT"];

        let root = repo_root();
        let guide = std::fs::read_to_string(root.join("docs/howto/plugin.md"))
            .expect("read docs/howto/plugin.md");
        let documented: BTreeSet<String> = guide
            .split("LOGOS_")
            .skip(1)
            .map(|piece| leading_variable(&format!("LOGOS_{piece}")))
            .filter(|name| name.len() > "LOGOS_".len())
            .collect();

        // What every plugin shell script reads: `$LOGOS_X` and `${LOGOS_X…}`.
        let mut plugin_files = Vec::new();
        files_under(&root.join("plugins/logos"), &mut plugin_files);
        let read_by_scripts: BTreeSet<String> = plugin_files
            .iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "sh"))
            .flat_map(|path| {
                let text = std::fs::read_to_string(path).expect("read a plugin script");
                text.split('$')
                    .skip(1)
                    .map(|piece| leading_variable(piece.strip_prefix("{").unwrap_or(piece)))
                    .filter(|name| !name.is_empty() && !INTERNAL.contains(&name.as_str()))
                    .collect::<Vec<_>>()
            })
            .collect();
        for pinned in [
            "LOGOS_STOP_HOOK_DISABLE",
            "LOGOS_STOP_HOOK_BUDGET_MS",
            "LOGOS_QUALITY_REPORT_DISABLE",
            "LOGOS_PROJECT_DIR",
        ] {
            assert!(
                read_by_scripts.contains(pinned),
                "the plugin scan stopped seeing `{pinned}`: {read_by_scripts:?}"
            );
        }
        let undocumented: Vec<&String> = read_by_scripts.difference(&documented).collect();
        assert!(
            undocumented.is_empty(),
            "plugin scripts read these variables and plugin.md does not name them: {undocumented:?}"
        );

        // Every documented variable appears, whole, in a shipped script or source.
        let mut sources = plugin_files;
        files_under(&root.join("logos-core/src"), &mut sources);
        let shipped: String = sources
            .iter()
            .filter_map(|path| std::fs::read_to_string(path).ok())
            .collect();
        for name in &documented {
            let whole = shipped.match_indices(name.as_str()).any(|(at, _)| {
                !shipped[at + name.len()..]
                    .starts_with(|c: char| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            });
            assert!(
                whole,
                "plugin.md names `{name}`, which no shipped script or source reads"
            );
        }
    }
}
