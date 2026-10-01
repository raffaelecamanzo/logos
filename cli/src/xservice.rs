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
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Subcommand;
use logos_core::federation::{
    app_wide_reachability, cross_service_coverage, discover, open_state, query, workspace_governance,
    xservice_build_deps, BuildDependencies, ContractBridge, EngineRegistry, ReachabilityScope,
    RegistryMode, TypeReferenceIndex, TypeReferences,
};
use logos_core::{model::NodeKind, Engine};

use crate::Output;

/// `xservice` sub-subcommands ([FR-WS-05]): the repo-qualified cross-service
/// query surface. Each carries an optional `--repo` member filter.
#[derive(Subcommand)]
pub(crate) enum XserviceCommands {
    /// Resolved cross-service route bindings: each consumer endpoint's binding
    /// across the `route`, `grpc-call` and `broker-topic` relations.
    ///
    /// This is **not** one edge per consumer endpoint, so a call site's
    /// providers must never be de-duplicated: since S-420 a `route` target
    /// that committed overlays compose several ways is keyed on EVERY
    /// composition, each composition binding its own sole provider and carrying
    /// its own profile set, and a `broker-topic` binding fans out — one publish
    /// binds every subscriber. (`grpc-call` reads its target verbatim and is
    /// unaffected.) Each edge carries `intake` plus `from_value`/`to_value`
    /// provenance (`literal`, `config-bound`, `config-unresolved`) naming the
    /// consumer's and the provider's evidence, so an admitted binding is never
    /// emitted as though it had been observed (ADR-64). Worded to match the
    /// `xservice_route_providers` MCP twin, which states the same fact.
    /// `--repo` scopes to routes that member provides.
    ///
    /// Beside the bindings, never among them ([BR-57]): `declared_contracts` is
    /// the declared-contract relation — a spec document a member holds and does
    /// not implement, naming the member whose own spec it is or a named external
    /// — and `bound_external` the external join, each `no-provider-in-workspace`
    /// REST call judged against the externals its own member declares, under a
    /// committed base path. Both are DECLARED by vendored specs, not observed
    /// calls; each carries its headline beside its denominator, both are
    /// workspace-wide under `--repo` (`declared_scope_note` says so), and each
    /// is absent when there is nothing to report — no vendored or `mock`-held
    /// spec, no named external a member declares.
    ///
    /// [BR-57]: ../../docs/specs/software-spec.md#327-workspace-federation
    #[command(name = "route-providers", alias = "route_providers")]
    RouteProviders {
        /// Scope to routes provided by this workspace member.
        #[arg(long)]
        repo: Option<String>,
    },
    /// Cross-service callers of a symbol: each member's intra-repo callers plus
    /// the cross-service consumers that reach it over a bridge edge.
    ///
    /// Apart from those, never merged with them, `via_type_reference` lists the
    /// importers an advisory type reference reaches ([FR-WS-35], [BR-60]) — for
    /// a type's node or its dotted name — each tagged `via type reference` with
    /// the reference, whose importer (member, file, line) is the class-grain
    /// caller; absent when none.
    ///
    /// [FR-WS-35]: ../../docs/specs/requirements/FR-WS-35.md
    /// [BR-60]: ../../docs/specs/software-spec.md#327-workspace-federation
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
    ///
    /// Apart from those, never merged with them, `via_type_reference` carries
    /// each importing file reached through an advisory type reference
    /// ([FR-WS-35], [BR-60]) — for a type's node or its dotted name — tagged
    /// `via type reference` with the reference, and the files depending on it
    /// in its member; absent when none.
    ///
    /// [FR-WS-35]: ../../docs/specs/requirements/FR-WS-35.md
    /// [BR-60]: ../../docs/specs/software-spec.md#327-workspace-federation
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
        #[arg(long)]
        kind: Option<NodeKind>,
        /// Maximum hits per member (default 20).
        #[arg(long)]
        limit: Option<usize>,
        /// Scope to one workspace member.
        #[arg(long)]
        repo: Option<String>,
    },
    /// What each member builds against, and what builds against it
    /// ([FR-WS-33]): `builds_against` and `built_against_by` rows joined from
    /// the members' Maven/Gradle manifests, each naming kind (`parent`,
    /// `dependency`, `managed`, `bom-import`), scope and artifact.
    ///
    /// A **build** dependency, never a runtime coupling ([BR-58]): nothing here
    /// is a bridge edge or enters a coverage figure. The workspace headline rides
    /// beside the rows with its denominator, and `cross_context` lists members
    /// depending on two or more contexts' model libraries (`<group>.<context>:kafka-models`
    /// or `<context>-kafka-models`) — a hint, never an edge. `--repo` scopes the rows to one member; a name that is
    /// not a member read says so in `scope_note`.
    ///
    /// [FR-WS-33]: ../../docs/specs/requirements/FR-WS-33.md
    /// [BR-58]: ../../docs/specs/software-spec.md#327-workspace-federation
    #[command(name = "build-deps", alias = "build_deps")]
    BuildDeps {
        /// Scope to one workspace member.
        #[arg(long)]
        repo: Option<String>,
    },
    /// Cross-member type references ([FR-WS-35]): per provider member, the
    /// types other members import — an import of a type exactly one other
    /// member declares, in main-tree source or an Avro schema, between members
    /// the build relation relates — each importer with its file and line, under
    /// the `type_reference` headline beside its denominators.
    ///
    /// An **advisory type reference, never a coupling** ([BR-60]): no row is a
    /// bridge edge or enters a coverage or build figure. `--repo` scopes the
    /// listing to one provider member while the headline stays workspace-wide;
    /// a name that is not a member read says so in `scope_note`.
    ///
    /// [FR-WS-35]: ../../docs/specs/requirements/FR-WS-35.md
    /// [BR-60]: ../../docs/specs/software-spec.md#327-workspace-federation
    #[command(name = "type-refs", alias = "type_refs")]
    TypeRefs {
        /// Scope to one provider member.
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
    ///
    /// A member the manifest declares `kind = "documentation" | "mock"`
    /// ([FR-WS-32]) has its contract-surface rows reported apart, under
    /// `coverage.declared_apart`, with their count and denominator;
    /// `kind_candidates` lists undeclared members holding API documents and no
    /// runnable source, as a hint that classifies nothing.
    ///
    /// When any member holds a build manifest, `build_dependency` states the
    /// build-dependency pairs by kind beside the references they were joined
    /// from — its own section, apart from every runtime figure above ([FR-WS-33],
    /// [BR-58]); `xservice build-deps` lists the rows.
    ///
    /// When any member is Java, Kotlin or Avro, `type_reference` states
    /// `type_reference_pairs` — member pairs bound by an import of a type
    /// exactly one other member declares, admitted only between members the
    /// build relation relates — beside the rows considered, bound,
    /// ambiguous-owner and type-only and the members read: an advisory type
    /// reference, never a coupling, apart from both sections above ([FR-WS-35],
    /// [BR-60]).
    ///
    /// When a member holds a vendored spec, `coverage.declared_contracts` states
    /// `declared_contract_pairs` beside the spec documents read, and
    /// `coverage.bound_external` the calls bound to a named external beside the
    /// `no-provider-in-workspace` REST rows they were judged from — beside the
    /// invocation and contract-surface headlines, never inside them: DECLARED by
    /// vendored specs, not observed calls ([BR-51], [BR-57]).
    ///
    /// [BR-51]: ../../docs/specs/software-spec.md#327-workspace-federation
    /// [BR-57]: ../../docs/specs/software-spec.md#327-workspace-federation
    /// [FR-WS-32]: ../../docs/specs/requirements/FR-WS-32.md
    /// [FR-WS-33]: ../../docs/specs/requirements/FR-WS-33.md
    /// [FR-WS-35]: ../../docs/specs/requirements/FR-WS-35.md
    /// [BR-58]: ../../docs/specs/software-spec.md#327-workspace-federation
    /// [BR-60]: ../../docs/specs/software-spec.md#327-workspace-federation
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
            let coverage = cross_service_coverage(&registry.answer());
            out.print(&query::xservice_route_providers(&edges, repo.as_deref()).with_declared(coverage))?;
        }
        XserviceCommands::Callers {
            symbol,
            limit,
            repo,
        } => {
            let (edges, residue) = query::reachability_inputs(&bridge, &registry);
            out.print(
                &query::xservice_callers(&registry, &edges, &residue, &symbol, limit, repo.as_deref())
                    .with_type_references(&registry, &type_references(&registry)),
            )?;
        }
        XserviceCommands::Impact {
            symbol,
            depth,
            repo,
        } => {
            let (edges, residue) = query::reachability_inputs(&bridge, &registry);
            out.print(
                &query::xservice_impact(&registry, &edges, &residue, &symbol, depth, repo.as_deref())
                    .with_type_references(&registry, &type_references(&registry)),
            )?;
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
        XserviceCommands::BuildDeps { repo } => {
            let relation = BuildDependencies::new().relation(&registry);
            out.print(&xservice_build_deps(&relation, repo.as_deref()))?;
        }
        XserviceCommands::TypeRefs { repo } => {
            out.print(&query::xservice_type_refs(&type_references(&registry), repo.as_deref()))?;
        }
    }
    Ok(0)
}

/// The type-reference overlay over `registry` ([FR-WS-35]), built once for a
/// CLI one-shot: its build relation comes from a fresh holder, exactly as
/// `build-deps` joins it.
///
/// [FR-WS-35]: ../../docs/specs/requirements/FR-WS-35.md
fn type_references(registry: &EngineRegistry<Engine>) -> Arc<TypeReferenceIndex> {
    TypeReferences::new().index(registry, &BuildDependencies::new())
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
/// lives in [`logos_core::federation::open_state`], not here ([BR-45]).
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
    let opens = registry.open_states();
    let degraded = open_state::rollup(&opens);
    if let Some(notice) = degraded.notice(&opens).filter(|_| !out.quiet) {
        eprintln!("{notice}");
    }
    Ok(crate::violation_code(degraded.all_opened()))
}
