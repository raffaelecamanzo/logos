//! The **workspace read-model** tools (S-480, [FR-WS-34]): `workspace_status`,
//! `workspace_reachability`, `workspace_check`, `xservice_build_deps` and
//! `workspace_roster` — the Workspace-Analyst's view of the workspace as a whole.
//!
//! # The MCP twins' read-models, not a re-derivation ([ADR-01])
//! Each tool runs the **same** core call its twin runs — the MCP tool of the same
//! name in `mcp/src/server.rs`, or for `workspace_roster`, which has no MCP twin,
//! `GET /api/v1/workspace/roster` — and returns that read-model verbatim beside
//! a `reading` line. No figure is recomposed here: the reading quotes the
//! read-model's own composed summaries (`resolved_edges_summary`, the build
//! headline's `summary`) wherever one exists, and otherwise only counts and
//! names what the payload beside it carries.
//!
//! # Read-only, and only under a federated backing ([ADR-52])
//! Every tool holds an [`XserviceBacking`], minted only over a federated
//! backing, and calls only read-models: nothing here saves a baseline, writes a
//! store or touches a manifest.
//!
//! # What each one opens ([NFR-PE-10])
//! `workspace_roster` is engine-free — the manifest and nothing else — so it is
//! the cheap first move for a roster question. The other four fan over the
//! members exactly as their twins do.
//!
//! [FR-WS-34]: ../../../docs/specs/requirements/FR-WS-34.md
//! [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
//! [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md

use logos_core::federation::query::{self, WorkspaceRoster};
use logos_core::federation::{
    self, workspace_governance, BoundedReachability, ReachabilityScope, WorkspaceGovernance,
    WorkspaceStatus, XserviceBuildDeps,
};
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::json;

use super::xservice::{bounded_list, qualified_endpoint, reading_of, run_federated};
use super::{ToolCallError, XserviceAnswer, XserviceBacking};

/// The workspace read-model tool names, in registration order.
pub const WORKSPACE_TOOL_NAMES: &[&str] = &[
    WorkspaceStatusTool::NAME,
    WorkspaceReachabilityTool::NAME,
    WorkspaceCheckTool::NAME,
    XserviceBuildDepsTool::NAME,
    WorkspaceRosterTool::NAME,
];

/// The `reading` of a workspace read-model tool's serialized output, or `None`
/// when `tool` is not one of [`WORKSPACE_TOOL_NAMES`] (or its output carries no
/// reading) — the [`xservice_reading`](super::xservice_reading) twin for this
/// family, so a roster can carry the line into its observation.
pub fn workspace_reading(tool: &str, output: &str) -> Option<String> {
    if !WORKSPACE_TOOL_NAMES.contains(&tool) {
        return None;
    }
    reading_of(output)
}

/// The argument shape of a tool that takes none.
#[derive(Debug, Deserialize)]
pub struct NoArgs {}

/// The schema of a tool that takes no argument.
fn no_parameters() -> serde_json::Value {
    json!({ "type": "object", "properties": {} })
}

/// The `repo` property of the two scopable tools — optional here, unlike the
/// repo-addressed member tools: omitting it answers for the whole workspace.
fn scope_property(scoped: &str) -> serde_json::Value {
    json!({
        "type": "string",
        "description": format!(
            "Workspace member name (its workspace-relative path). Scopes {scoped} to that \
             one member; omit for the whole workspace."
        )
    })
}

// ── readings ────────────────────────────────────────────────────────────────

fn read_roster(roster: &WorkspaceRoster) -> String {
    let default = roster
        .default
        .as_deref()
        .map(|member| format!("; default member: {member}"))
        .unwrap_or_default();
    format!(
        "workspace_roster {:?} — {} member(s): {}{default}",
        roster.workspace,
        roster.members.len(),
        bounded_list(roster.members.clone())
    )
}

fn read_status(status: &WorkspaceStatus) -> String {
    let warm = &status.warm_rollup;
    let warming = warm
        .warming
        .map(|n| format!(", {n} warming"))
        .unwrap_or_default();
    let open = &status.degraded_rollup;
    let degraded = if open.degraded_members.is_empty() {
        String::new()
    } else {
        format!(" ({})", bounded_list(open.degraded_members.clone()))
    };
    let coverage = &status.coverage;
    let partial = if coverage.covers_all_members {
        String::new()
    } else {
        format!(
            " — over {} of {} members, not all",
            coverage.members_read, coverage.members_total
        )
    };
    let build = status
        .build_dependency
        .as_ref()
        .map(|headline| {
            format!(
                " | build dependency (not a runtime coupling): {}",
                headline.summary
            )
        })
        .unwrap_or_default();
    format!(
        "workspace_status {:?} — {} member(s): {} warm, {} deferred, {} degraded{warming} | \
         store open: {} opened, {} not attempted, {} degraded{degraded} | coverage: {}{partial}{build}",
        status.workspace,
        warm.members,
        warm.warm,
        warm.deferred,
        warm.degraded,
        open.opened,
        open.not_attempted,
        open.degraded_members.len(),
        coverage.resolved_edges_summary,
    )
}

fn read_reachability(view: &BoundedReachability) -> String {
    let scope = view
        .scope
        .repo
        .as_deref()
        .map(|member| format!(" for {member}"))
        .unwrap_or_default();
    let rider = &view.coverage;
    let promotions: Vec<String> = view
        .live_via_cross_service
        .iter()
        .map(|claim| format!("{}:{}", claim.member, claim.symbol.as_str()))
        .collect();
    let promoted = if promotions.is_empty() {
        "no callable live via cross-service".to_string()
    } else {
        format!(
            "{} callable(s) live via cross-service: {}",
            promotions.len(),
            bounded_list(promotions)
        )
    };
    let dead = match &view.dead {
        Some(dead) => format!("{} dead app-wide", dead.len()),
        None => "dead set withheld (promotions only — pass all: true for it)".to_string(),
    };
    let skipped = if view.skipped_members.is_empty() {
        String::new()
    } else {
        format!(
            " | skipped (suppresses promotions, never demotes): {}",
            bounded_list(view.skipped_members.clone())
        )
    };
    format!(
        "workspace_reachability{scope} — ADVISORY: {promoted}; {dead} | read {} of {} member(s) | \
         seeded by {} bridge invocation edge(s) beside a headline of {} resolved{skipped}",
        rider.members_read,
        rider.members_total,
        rider.bridge_invocation_edges,
        rider.resolved_cross_service_edges,
    )
}

fn read_check(report: Option<&WorkspaceGovernance>) -> String {
    let Some(report) = report else {
        return "workspace_check — no workspace rules declared ([governance] in \
                logos.workspace.toml): nothing was checked, which is not a pass"
            .to_string();
    };
    let violations: Vec<String> = report
        .violations
        .iter()
        .map(|v| {
            format!(
                "{}: {} → {} [{}]",
                v.rule,
                qualified_endpoint(&v.from),
                qualified_endpoint(&v.to),
                v.relation
            )
        })
        .collect();
    let verdict = if violations.is_empty() {
        "no violations".to_string()
    } else {
        format!(
            "{} violation(s): {}",
            violations.len(),
            bounded_list(violations)
        )
    };
    let unknown = if report.unknown_member_refs.is_empty() {
        String::new()
    } else {
        format!(
            " | rules name unknown members: {}",
            bounded_list(report.unknown_member_refs.clone())
        )
    };
    format!(
        "workspace_check {:?} — ADVISORY: {} rule(s) over {} cross-service binding(s), \
         {verdict}{unknown}",
        report.workspace, report.rules_checked, report.bindings_checked,
    )
}

fn read_build_deps(deps: &XserviceBuildDeps) -> String {
    let scope = deps
        .scope
        .as_deref()
        .map(|member| format!(" for {member}"))
        .unwrap_or_default();
    let note = deps
        .scope_note
        .as_deref()
        .map(|note| format!(" | {note}"))
        .unwrap_or_default();
    let hints = if deps.cross_context.is_empty() {
        String::new()
    } else {
        format!(
            " | {} member(s) depend on two or more contexts' model libraries (a hint, never an \
             edge)",
            deps.cross_context.len()
        )
    };
    format!(
        "xservice_build_deps{scope} — BUILD DEPENDENCY, NOT A RUNTIME COUPLING: {} | {} member \
         row(s){hints}{note}",
        deps.headline.summary,
        deps.members.len(),
    )
}

// ── workspace_status ────────────────────────────────────────────────────────

/// Per-member freshness, warm and open state, and the cross-service coverage.
#[derive(Clone)]
pub struct WorkspaceStatusTool {
    xs: XserviceBacking,
}

impl WorkspaceStatusTool {
    /// Wrap the federated backing.
    pub fn new(xs: XserviceBacking) -> Self {
        Self { xs }
    }
}

impl Tool for WorkspaceStatusTool {
    const NAME: &'static str = "workspace_status";
    type Error = ToolCallError;
    type Args = NoArgs;
    type Output = XserviceAnswer<WorkspaceStatus>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Workspace status: each member's index freshness, `warm_state` \
                 (index presence) and `open_state` (store openability) — two different \
                 questions, never merged — the warm and degraded roll-ups, and the \
                 cross-service coverage. Read `coverage.resolved_edges_summary` rather than \
                 recomposing its figures; a figure marked over fewer than all members \
                 covers fewer than all members. Opens every member."
                .to_string(),
            parameters: no_parameters(),
        }
    }

    async fn call(&self, _args: NoArgs) -> Result<Self::Output, ToolCallError> {
        run_federated(self.xs.clone(), |registry, _bridge| {
            let answer = query::workspace_status(registry);
            XserviceAnswer {
                reading: read_status(&answer),
                answer,
            }
        })
        .await
    }
}

// ── workspace_reachability ──────────────────────────────────────────────────

/// `workspace_reachability` arguments.
#[derive(Debug, Deserialize)]
pub struct ReachabilityArgs {
    /// Scope the view to one member.
    #[serde(default)]
    pub repo: Option<String>,
    /// Return the full per-repo-dead set, not only the promotions.
    #[serde(default)]
    pub all: Option<bool>,
}

/// App-wide cross-service dead code, advisory only.
#[derive(Clone)]
pub struct WorkspaceReachabilityTool {
    xs: XserviceBacking,
}

impl WorkspaceReachabilityTool {
    /// Wrap the federated backing.
    pub fn new(xs: XserviceBacking) -> Self {
        Self { xs }
    }
}

impl Tool for WorkspaceReachabilityTool {
    const NAME: &'static str = "workspace_reachability";
    type Error = ToolCallError;
    type Args = ReachabilityArgs;
    type Output = XserviceAnswer<BoundedReachability>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "App-wide cross-service dead code, ADVISORY ONLY: callables their own \
                 repo calls dead that a resolved cross-service call keeps alive \
                 (`live_via_cross_service`), and — with `all: true` — the ones still dead \
                 app-wide. A promotion rests on `coverage.bridge_invocation_edges`, not on the \
                 headline beside it. A skipped member suppresses promotions and never causes a \
                 demotion. Never a gate input."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "repo": scope_property("the view"),
                    "all": { "type": "boolean", "description": "Return the full per-repo-dead set rather than only the cross-service promotions (default false)." }
                }
            }),
        }
    }

    async fn call(&self, args: ReachabilityArgs) -> Result<Self::Output, ToolCallError> {
        let scope = ReachabilityScope::new(args.repo, args.all.unwrap_or(false));
        run_federated(self.xs.clone(), move |registry, bridge| {
            let answer =
                federation::app_wide_reachability(registry, &query::edges(bridge, registry))
                    .bound(scope);
            XserviceAnswer {
                reading: read_reachability(&answer),
                answer,
            }
        })
        .await
    }
}

// ── workspace_check ─────────────────────────────────────────────────────────

/// The workspace rule family evaluated over the cross-service bindings.
#[derive(Clone)]
pub struct WorkspaceCheckTool {
    xs: XserviceBacking,
}

impl WorkspaceCheckTool {
    /// Wrap the federated backing.
    pub fn new(xs: XserviceBacking) -> Self {
        Self { xs }
    }
}

impl Tool for WorkspaceCheckTool {
    const NAME: &'static str = "workspace_check";
    type Error = ToolCallError;
    type Args = NoArgs;
    type Output = XserviceAnswer<Option<WorkspaceGovernance>>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Workspace governance, ADVISORY: the `[governance]` rule family of \
                 logos.workspace.toml (service-layer boundaries, no-cross-service-callers \
                 contracts) evaluated over the cross-service bindings. It never alters a \
                 member's own quality gate. With no rules declared the answer carries only its \
                 reading: nothing was checked, which is not a pass."
                .to_string(),
            parameters: no_parameters(),
        }
    }

    async fn call(&self, _args: NoArgs) -> Result<Self::Output, ToolCallError> {
        run_federated(self.xs.clone(), |registry, bridge| {
            let answer =
                workspace_governance(registry.federation(), &query::edges(bridge, registry))?;
            Ok(XserviceAnswer {
                reading: read_check(answer.as_ref()),
                answer,
            })
        })
        .await?
        .map_err(ToolCallError::Engine)
    }
}

// ── xservice_build_deps ─────────────────────────────────────────────────────

/// `xservice_build_deps` arguments.
#[derive(Debug, Deserialize)]
pub struct BuildDepsArgs {
    /// Scope the rows to one member.
    #[serde(default)]
    pub repo: Option<String>,
}

/// The members' build dependencies — never a runtime coupling.
#[derive(Clone)]
pub struct XserviceBuildDepsTool {
    xs: XserviceBacking,
}

impl XserviceBuildDepsTool {
    /// Wrap the federated backing.
    pub fn new(xs: XserviceBacking) -> Self {
        Self { xs }
    }
}

impl Tool for XserviceBuildDepsTool {
    const NAME: &'static str = "xservice_build_deps";
    type Error = ToolCallError;
    type Args = BuildDepsArgs;
    type Output = XserviceAnswer<XserviceBuildDeps>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Workspace build dependencies from the members' Maven/Gradle \
                 manifests: per member, what it builds against and what builds against it. \
                 THIS IS A BUILD DEPENDENCY, NOT A RUNTIME COUPLING: no row is a bridge edge, a \
                 resolved call or a coverage figure. `headline.summary` states the pairs beside \
                 their denominator; `cross_context` is a hint, never an edge."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "repo": scope_property("the rows") }
            }),
        }
    }

    async fn call(&self, args: BuildDepsArgs) -> Result<Self::Output, ToolCallError> {
        let deps = self.xs.build_deps.clone();
        run_federated(self.xs.clone(), move |registry, _bridge| {
            let answer =
                federation::xservice_build_deps(&deps.relation(registry), args.repo.as_deref());
            XserviceAnswer {
                reading: read_build_deps(&answer),
                answer,
            }
        })
        .await
    }
}

// ── workspace_roster ────────────────────────────────────────────────────────

/// The member roster — the manifest, and nothing but the manifest.
#[derive(Clone)]
pub struct WorkspaceRosterTool {
    xs: XserviceBacking,
}

impl WorkspaceRosterTool {
    /// Wrap the federated backing.
    pub fn new(xs: XserviceBacking) -> Self {
        Self { xs }
    }
}

impl Tool for WorkspaceRosterTool {
    const NAME: &'static str = "workspace_roster";
    type Error = ToolCallError;
    type Args = NoArgs;
    type Output = XserviceAnswer<WorkspaceRoster>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "The workspace's member roster: its name, every member's name (the \
                 value a member tool's `repo` takes) and the default member. Read from the \
                 manifest alone — it starts no member engine, so it is the cheap first move \
                 for a question about which members exist."
                .to_string(),
            parameters: no_parameters(),
        }
    }

    async fn call(&self, _args: NoArgs) -> Result<Self::Output, ToolCallError> {
        run_federated(self.xs.clone(), |registry, _bridge| {
            let answer = query::workspace_roster(registry);
            XserviceAnswer {
                reading: read_roster(&answer),
                answer,
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    //! The readings over hand-built read-models — independent of any fixture
    //! workspace, so each verdict word is pinned on its own.

    use super::*;

    fn roster(default: Option<&str>, members: &[&str]) -> WorkspaceRoster {
        WorkspaceRoster {
            workspace: "shop".to_string(),
            default: default.map(str::to_string),
            members: members.iter().map(|m| m.to_string()).collect(),
        }
    }

    #[test]
    fn the_roster_reading_names_its_members_and_the_default() {
        let reading = read_roster(&roster(Some("api"), &["api", "web"]));
        assert_eq!(
            reading,
            "workspace_roster \"shop\" — 2 member(s): api; web; default member: api"
        );
        let reading = read_roster(&roster(None, &["a", "b", "c", "d", "e", "f", "g"]));
        assert_eq!(
            reading,
            "workspace_roster \"shop\" — 7 member(s): a; b; c; d; e; +2 more"
        );
    }

    #[test]
    fn an_undeclared_rule_family_never_reads_as_a_pass() {
        let reading = read_check(None);
        assert!(
            reading.contains("nothing was checked, which is not a pass"),
            "{reading}"
        );
        assert!(!reading.contains("no violations"), "{reading}");

        let clean = WorkspaceGovernance {
            workspace: "shop".to_string(),
            rules_checked: 1,
            bindings_checked: 3,
            unknown_member_refs: vec!["ghost".to_string()],
            violations: Vec::new(),
        };
        assert_eq!(
            read_check(Some(&clean)),
            "workspace_check \"shop\" — ADVISORY: 1 rule(s) over 3 cross-service binding(s), \
             no violations | rules name unknown members: ghost"
        );
    }

    #[test]
    fn the_reading_is_lifted_only_from_a_workspace_tool() {
        let output =
            r#"{"reading":"workspace_roster \"shop\" — 1 member(s): api","workspace":"shop"}"#;
        assert_eq!(
            workspace_reading("workspace_roster", output).as_deref(),
            Some("workspace_roster \"shop\" — 1 member(s): api")
        );
        // The near misses: an xservice tool, and a name one character off.
        assert_eq!(workspace_reading("xservice_search", output), None);
        assert_eq!(workspace_reading("workspace_roste", output), None);
        assert_eq!(workspace_reading("workspace_roster", "not json"), None);
    }
}
