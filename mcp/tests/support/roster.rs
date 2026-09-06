//! The shipped MCP tool roster, by name — the mcp crate's single source of
//! truth for "which tools register", shared by every guard that asserts on it.
//!
//! # Why a list and not a number
//!
//! Each guard in this crate used to carry its own hard-coded roster **count**.
//! Sprint 64 iteration 4 showed the cost: two dev sessions each appended one
//! tool at the same registration points and each wrote the new count as `29`.
//! Git auto-merged the two identical edits **without a conflict**, so eleven
//! assertions across the workspace claimed 29 while the shipped set was 30, and
//! a human had to spot it.
//!
//! Appending a *line* has no such failure mode: two sessions each adding one
//! entry below merge into two entries, and every count derives from
//! `…TOOLS.len()`. Registering a tool therefore means adding its name here —
//! once — and the guards follow (S-361, [FR-CL-06]).
//!
//! The complementary CLI-side check lives in `cli/src/main.rs::surface_parity`,
//! which reconciles this roster against the shipped clap definition so a tool
//! cannot land on one surface only.
//!
//! [FR-CL-06]: ../../../docs/specs/requirements/FR-CL-06.md

// Each including test binary uses a different subset; the module is the shared
// declaration, not a per-binary one.
#![allow(dead_code)]

/// The navigation tools wired to `Engine` methods (FR-NV-01..07;
/// `impact_intersection` added by S-358/CR-114, FR-NV-11; `precedent` by
/// S-359/CR-114, FR-NV-12; `branch_overlap` by S-360/CR-114, FR-NV-13).
pub const NAV_TOOLS: &[&str] = &[
    "search",
    "context",
    "explore",
    "node",
    "callers",
    "callees",
    "impact",
    "impact_intersection",
    "precedent",
    "branch_overlap",
    "status",
];

/// The quality tools wired to the governance engine (S-020, FR-MC-01;
/// `doctor` added by S-204/CR-052, `verify` by S-205/CR-052, FR-GV-18/FR-GV-19;
/// the static test-gap tool removed by S-289/CR-079).
pub const QUALITY_TOOLS: &[&str] = &[
    "scan",
    "health",
    "doctor",
    "verify",
    "session_start",
    "session_end",
    "rescan",
    "check_rules",
    "evolution",
    "dsm",
];

/// The temporal tool wired to the history engine (S-048, CR-006, FR-GH-06).
pub const TEMPORAL_TOOLS: &[&str] = &["hotspots"];

/// The coverage tools wired to the evidence store (S-051, CR-007, FR-CV-06/07;
/// `coverage_refresh` added by S-140/CR-036, FR-CV-10).
pub const COVERAGE_TOOLS: &[&str] =
    &["coverage_ingest", "coverage_status", "coverage_refresh"];

/// The wiki twins wired to the wiki store (S-053, CR-008, FR-WK-09;
/// `wiki_materialize` added by S-263/CR-062, FR-WK-20). `wiki delete`/`wiki
/// skill` are CLI-only — destructive/install ops off the agent surface.
pub const WIKI_TOOLS: &[&str] = &[
    "wiki_write",
    "wiki_read",
    "wiki_search",
    "wiki_status",
    "wiki_materialize",
];

/// The cross-service tools the **federated** backing adds on top of the
/// single-root roster (S-248/CR-061 FR-WS-05; `workspace_reachability` by
/// S-257, FR-WS-12; `workspace_check` by S-258, FR-WS-13). Alphabetical —
/// `list_all` sorts by name, so this doubles as the expected added-set order.
pub const XSERVICE_TOOLS: &[&str] = &[
    "workspace_check",
    "workspace_reachability",
    "workspace_status",
    "xservice_callers",
    "xservice_impact",
    "xservice_route_providers",
    "xservice_search",
];

/// How many tools the single-root backing registers — derived, never written.
pub const SINGLE_ROOT: usize = NAV_TOOLS.len()
    + QUALITY_TOOLS.len()
    + TEMPORAL_TOOLS.len()
    + COVERAGE_TOOLS.len()
    + WIKI_TOOLS.len();

/// How many tools the federated backing registers — the single-root roster
/// plus the cross-service family.
pub const FEDERATED: usize = SINGLE_ROOT + XSERVICE_TOOLS.len();
