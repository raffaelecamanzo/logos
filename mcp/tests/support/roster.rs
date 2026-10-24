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
//! The complementary CLI-side check lives in `cli/src/lib.rs::surface_parity`,
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
    // The ratchet's bless command, twin of `check --bless` (S-507, FR-GV-23).
    "bless_rules",
    // The uncapped per-symbol offender listing (S-676, FR-QM-19).
    "offenders",
    // The branch delta against the merge-base (S-677, FR-GV-24).
    "review",
    // The advisory "where to start" ranking (S-505, FR-QM-17).
    "improvements",
];

/// The temporal tool wired to the history engine (S-048, CR-006, FR-GH-06).
pub const TEMPORAL_TOOLS: &[&str] = &["hotspots"];

/// The coverage tools wired to the evidence store (S-051, CR-007, FR-CV-06/07;
/// `coverage_refresh` added by S-140/CR-036, FR-CV-10; `coverage_run` by
/// S-737/CR-234, FR-CV-14).
pub const COVERAGE_TOOLS: &[&str] =
    &["coverage_ingest", "coverage_status", "coverage_refresh", "coverage_run"];

/// The security tools wired to the evidence store and the scanner orchestrator
/// (S-667, CR-217, FR-SC-01/06/07/08).
pub const SECURITY_TOOLS: &[&str] =
    &["security_scan", "security_ingest", "security_status", "security_findings"];

/// The cross-service tools the **federated** backing adds on top of the
/// single-root roster (S-248/CR-061 FR-WS-05; `workspace_reachability` by
/// S-257, FR-WS-12; `workspace_check` by S-258, FR-WS-13; `xservice_build_deps`
/// by S-464, FR-WS-33; `xservice_type_refs` by S-474, FR-WS-35). Alphabetical —
/// `list_all` sorts by name, so this doubles as the expected added-set order.
pub const XSERVICE_TOOLS: &[&str] = &[
    "workspace_check",
    "workspace_reachability",
    "workspace_status",
    "xservice_build_deps",
    "xservice_callers",
    "xservice_impact",
    "xservice_route_providers",
    "xservice_search",
    "xservice_type_refs",
];

/// How many tools the single-root backing registers — derived, never written.
///
/// The core's own roster. A composed extension's tools join it through the
/// extension host (S-644), and are rostered with that extension; this
/// repository's binary composes none.
pub const SINGLE_ROOT: usize = NAV_TOOLS.len()
    + QUALITY_TOOLS.len()
    + TEMPORAL_TOOLS.len()
    + COVERAGE_TOOLS.len()
    + SECURITY_TOOLS.len();

/// How many tools the federated backing registers — the single-root roster
/// plus the cross-service family.
pub const FEDERATED: usize = SINGLE_ROOT + XSERVICE_TOOLS.len();

/// The single-root tools that take an optional `repo` selecting the workspace
/// member they answer for (S-710, [FR-WS-42], [CR-225]): every read-only one.
/// Each persists nothing a member's agent did not ask for ([CR-225] CRA-02,
/// confirmed per tool in S-710). Every other single-root tool writes or gates,
/// takes no `repo`, and acts on the default member.
///
/// [FR-WS-42]: ../../../docs/specs/requirements/FR-WS-42.md
/// [CR-225]: ../../../docs/requests/CR-225-per-member-single-root-tools.md
pub const MEMBER_SELECTABLE_TOOLS: &[&str] = &[
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
    "health",
    "hotspots",
    "dsm",
    "evolution",
    "coverage_status",
    "security_status",
    "security_findings",
    "offenders",
    "review",
    "improvements",
];
