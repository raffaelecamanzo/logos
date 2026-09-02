//! `logos xservice` cross-service query group + `logos workspace status`
//! (S-248, [FR-WS-05]).
//!
//! Thin adapter over the thick-core [`logos_core::federation::query`] read-models
//! (NFR-MA-02, ADR-01): each subcommand discovers the workspace, builds a lazy
//! member [`EngineRegistry`], and serialises exactly one `query::*` read-model.
//! The bulk lives here (not `main.rs`/`dispatch.rs`) so those stay under their
//! line budgets, mirroring the [`crate::workspace_init`] precedent.
//!
//! `--json` stays machine-clean: every path routes through [`Output::print`],
//! which emits only the read-model JSON (human notices go to stderr, FR-CL-02).
//!
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md

use std::path::Path;

use anyhow::{Context, Result};
use clap::Subcommand;
use logos_core::federation::{
    app_wide_reachability, degraded, discover, query, workspace_governance, ContractBridge,
    EngineRegistry, ReachabilityScope, RegistryMode,
};
use logos_core::{model::NodeKind, Engine};

use crate::{parse_kind, Output};

/// `xservice` sub-subcommands ([FR-WS-05]): the repo-qualified cross-service
/// query surface. Each carries an optional `--repo` member filter.
#[derive(Subcommand)]
pub(crate) enum XserviceCommands {
    /// Resolved cross-service route bindings: each consumer endpoint → its sole
    /// cross-member provider route. `--repo` scopes to routes that member provides.
    #[command(name = "route-providers", alias = "route_providers")]
    RouteProviders {
        /// Scope to routes provided by this workspace member.
        #[arg(long)]
        repo: Option<String>,
    },
    /// Cross-service callers of a symbol: each member's intra-repo callers plus
    /// the cross-service consumers that reach it over a bridge edge.
    Callers {
        /// Symbol whose cross-service callers to list.
        symbol: String,
        /// Maximum intra-repo callers per member (default 50).
        #[arg(long)]
        limit: Option<usize>,
        /// Scope the intra-repo fan-out to one workspace member.
        #[arg(long)]
        repo: Option<String>,
    },
    /// Cross-service impact of changing a symbol: the seed member's impact plus
    /// the far member's impact stitched across each bridge edge.
    Impact {
        /// Symbol whose cross-service impact to trace.
        symbol: String,
        /// Traversal depth bound per member (default 3).
        #[arg(long)]
        depth: Option<usize>,
        /// Scope the seed impact to one workspace member.
        #[arg(long)]
        repo: Option<String>,
    },
    /// Cross-service full-text search fanned across the workspace members.
    Search {
        /// Search query string.
        query: String,
        /// Filter by node kind (e.g. function, struct, route).
        #[arg(long, value_parser = parse_kind)]
        kind: Option<NodeKind>,
        /// Maximum hits per member (default 20).
        #[arg(long)]
        limit: Option<usize>,
        /// Scope to one workspace member.
        #[arg(long)]
        repo: Option<String>,
    },
}

/// `workspace` sub-subcommands ([FR-WS-05], [FR-WS-12], [FR-WS-13]).
///
/// [FR-WS-12]: ../../docs/specs/requirements/FR-WS-12.md
/// [FR-WS-13]: ../../docs/specs/requirements/FR-WS-13.md
#[derive(Subcommand)]
pub(crate) enum WorkspaceCommands {
    /// Per-member index freshness + the 3-state cross-service coverage summary.
    Status,
    /// App-wide cross-service dead code (FR-WS-12): the union of every member's
    /// call graph plus the bridge's edges as extra live roots. Advisory only —
    /// never a gate input, never alters a repo's own dead-code verdict. Every
    /// claim carries a coverage rider.
    ///
    /// By default the payload is bounded to only the cross-service promotions and
    /// states every applied bound, so a bounded reply is never read as the complete
    /// dead-set ([NFR-CC-04]).
    Reachability {
        /// Scope the union view to one workspace member (its workspace-relative name).
        #[arg(long)]
        repo: Option<String>,
        /// Return the full per-repo-dead set instead of only the cross-service
        /// promotions (the default).
        #[arg(long)]
        all: bool,
    },
    /// Evaluate the workspace governance rules (`[governance]` in
    /// logos.workspace.toml) over the cross-service bridge bindings (FR-WS-13).
    ///
    /// Reported at the WORKSPACE level and **advisory**: this never alters any
    /// member's per-repo quality gate, and a violation never moves the exit code
    /// — it is reported, not gated (ADR-56). With no rules declared, there is no
    /// output at all (`null`), not a passing report.
    ///
    /// A workspace **member that could not be opened** does exit non-zero (1),
    /// for every `workspace` subcommand: that is the answer being incomplete,
    /// not a governance verdict (FR-WS-16).
    Check,
}

/// Discover the workspace and build a lazy member registry (CLI one-shot: an
/// engine is constructed only when a command first touches a member, [NFR-PE-10]).
///
/// # Errors
/// A malformed manifest fails loud (exit 2, [`discover`]); no manifest at all is
/// an actionable usage error naming the remedy.
fn registry(root: &Path) -> Result<EngineRegistry<Engine>> {
    let federation = discover(root)?.context(
        "not a Logos workspace: no logos.workspace.toml found up-tree \
         (run `logos init --workspace` at the parent folder of your repos)",
    )?;
    Ok(EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy))
}

/// Route one `xservice` subcommand to its `query::*` read-model ([FR-WS-05]).
pub(crate) fn run_xservice(command: XserviceCommands, root: &Path, out: &Output) -> Result<i32> {
    let registry = registry(root)?;
    let bridge = ContractBridge::new();
    match command {
        XserviceCommands::RouteProviders { repo } => {
            let edges = query::edges(&bridge, &registry);
            out.print(&query::xservice_route_providers(&edges, repo.as_deref()))?;
        }
        XserviceCommands::Callers {
            symbol,
            limit,
            repo,
        } => {
            let edges = query::edges(&bridge, &registry);
            out.print(&query::xservice_callers(
                &registry,
                &edges,
                &symbol,
                limit,
                repo.as_deref(),
            ))?;
        }
        XserviceCommands::Impact {
            symbol,
            depth,
            repo,
        } => {
            let edges = query::edges(&bridge, &registry);
            out.print(&query::xservice_impact(
                &registry,
                &edges,
                &symbol,
                depth,
                repo.as_deref(),
            ))?;
        }
        XserviceCommands::Search {
            query: q,
            kind,
            limit,
            repo,
        } => {
            out.print(&query::xservice_search(
                &registry,
                &q,
                kind,
                limit,
                repo.as_deref(),
            ))?;
        }
    }
    Ok(0)
}

/// Route one `workspace` subcommand to its read-model, then map the workspace's
/// **degraded members** to the exit code ([FR-WS-05], [FR-WS-12], [FR-WS-13],
/// [FR-WS-16]).
///
/// `Check`'s *governance verdict* still never moves the exit code: the workspace
/// rule family is **advisory** — it reports cross-service policy breaches without
/// moving any member's gated signal ([ADR-56]). Serialising the `Option` directly
/// is what makes the honest empty machine-readable: no declared rules ⇒ `null`,
/// never a fabricated zero-violation report ([NFR-CC-04]).
///
/// # Exit code: an unopenable member is exit 1 ([FR-WS-16], [FR-CL-01])
/// What *does* move the exit code, for all three subcommands alike, is a member
/// whose store could not be **opened**. That is not a governance verdict; it is
/// the answer being incomplete. This function used to return `Ok(0)` whatever
/// happened, so the observed [CR-100] run — 63 of 72 members unopened, every
/// roll-up computed over the 9 that survived — was a *successful* command that
/// passed in CI. Exit 1 is the result-level violation code the governance
/// commands already use ([FR-CL-03], [`crate::violation_code`]): the command
/// ran, and its answer is not the answer it claims to be.
///
/// This is a **breaking change** for a script that tolerated the old exit 0,
/// stated as such in the CLI contract ([FR-CL-01]) and the release notes.
///
/// A member merely skipped by laziness, or reclaimed by the budget's eviction,
/// is **not** degraded and does not move the exit code — the discrimination
/// lives in [`logos_core::federation::degraded`], not here ([BR-45]).
///
/// # The notice goes to stderr
/// So `--json` stdout stays machine-clean ([FR-CL-02]), and so *every*
/// subcommand names its degraded members — including `check`, whose payload is a
/// bare `Option` that must keep serialising as `null` ([NFR-CC-04]).
/// `workspace status` additionally carries the roll-up **inside** its payload,
/// folded into the member table, so a `--json` consumer reads it structurally
/// rather than by parsing a warning.
///
/// [CR-100]: ../../docs/requests/CR-100-workspace-resource-budget.md
/// [FR-CL-01]: ../../docs/specs/requirements/FR-CL-01.md
/// [FR-CL-02]: ../../docs/specs/requirements/FR-CL-02.md
/// [FR-CL-03]: ../../docs/specs/requirements/FR-CL-03.md
/// [FR-WS-16]: ../../docs/specs/requirements/FR-WS-16.md
/// [BR-45]: ../../docs/specs/software-spec.md#327-workspace-federation
pub(crate) fn run_workspace(command: WorkspaceCommands, root: &Path, out: &Output) -> Result<i32> {
    let registry = registry(root)?;
    match command {
        WorkspaceCommands::Status => out.print(&query::workspace_status(&registry))?,
        WorkspaceCommands::Reachability { repo, all } => {
            let bridge = ContractBridge::new();
            let edges = query::edges(&bridge, &registry);
            let scope = ReachabilityScope::new(repo, all);
            out.print(&app_wide_reachability(&registry, &edges).bound(scope))?;
        }
        WorkspaceCommands::Check => {
            let bridge = ContractBridge::new();
            let edges = query::edges(&bridge, &registry);
            out.print(&workspace_governance(registry.federation(), &edges)?)?;
        }
    }
    // Degraded members are read AFTER the read-model, deliberately: the
    // registry's open-state ledger is complete only once the command's own
    // walks have run, and `workspace status` walks every member four times
    // through tiers with no per-member error channel of their own.
    let degraded = degraded::rollup(&registry.open_states());
    if let Some(notice) = degraded.notice().filter(|_| !out.quiet) {
        eprintln!("{notice}");
    }
    Ok(crate::violation_code(degraded.degraded_members.is_empty()))
}
