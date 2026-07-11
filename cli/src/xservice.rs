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
    app_wide_reachability, discover, query, workspace_governance, ContractBridge, EngineRegistry,
    RegistryMode,
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
    Reachability,
    /// Evaluate the workspace governance rules (`[governance]` in
    /// logos.workspace.toml) over the cross-service bridge bindings (FR-WS-13).
    ///
    /// Reported at the WORKSPACE level and **advisory**: this never alters any
    /// member's per-repo quality gate, and always exits 0 — a violation is
    /// reported, not gated (ADR-56). With no rules declared, there is no output
    /// at all (`null`), not a passing report.
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

/// Route one `workspace` subcommand to its read-model ([FR-WS-05], [FR-WS-12], [FR-WS-13]).
///
/// `Check` prints an `Option<WorkspaceGovernance>` and always returns 0: the
/// workspace rule family is **advisory** — it reports cross-service policy
/// breaches without moving any member's gated signal ([ADR-56]). Serialising the
/// `Option` directly is what makes the honest empty machine-readable: no declared
/// rules ⇒ `null`, never a fabricated zero-violation report ([NFR-CC-04]).
pub(crate) fn run_workspace(command: WorkspaceCommands, root: &Path, out: &Output) -> Result<i32> {
    let registry = registry(root)?;
    match command {
        WorkspaceCommands::Status => out.print(&query::workspace_status(&registry))?,
        WorkspaceCommands::Reachability => {
            let bridge = ContractBridge::new();
            let edges = query::edges(&bridge, &registry);
            out.print(&app_wide_reachability(&registry, &edges))?;
        }
        WorkspaceCommands::Check => {
            let bridge = ContractBridge::new();
            let edges = query::edges(&bridge, &registry);
            out.print(&workspace_governance(registry.federation(), &edges)?)?;
        }
    }
    Ok(0)
}
