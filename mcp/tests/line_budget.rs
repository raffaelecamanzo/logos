//! Surface line-count budget guard for the MCP adapter (NFR-MA-02, ADR-01).
//!
//! Extended in S-017 (per the original note) to sum non-blank Rust lines
//! across ALL of `mcp/src/` — lib root, server module, and the test-harness
//! bin — so the thin-surface invariant covers the whole crate.
//!
//! The `server-instructions` text (src/instructions.md) is data, not code:
//! it is excluded by construction (only `.rs` files are counted).

use std::path::Path;

/// Recursively count non-blank lines of every `.rs` file under `dir`.
fn non_blank_rust_lines(dir: &Path) -> usize {
    let mut total = 0;
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            total += non_blank_rust_lines(&path);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let source = std::fs::read_to_string(&path).expect("read source file");
            total += source.lines().filter(|l| !l.trim().is_empty()).count();
        }
    }
    total
}

/// Budget: ≤ 880 non-blank lines of Rust across the whole MCP adapter
/// (NFR-MA-02 thick-core/thin-surface invariant).
///
/// Derivation (combined S-020, S-022, S-048, S-051, S-053 re-base): 28 `#[tool]`
/// registrations (8 navigation, 11 quality, 1 temporal `hotspots`, 3 coverage per
/// CR-007/CR-036, 5 wiki per CR-008/CR-062), each a mechanical attribute, signature,
/// and ONE Engine call — cost ~15 non-blank lines each ≈420; the typed parameter
/// structs (6 navigation, 4 quality, 1 temporal, 1 coverage, 3 wiki) ≈135; the
/// fixed scaffolding (serve entrypoint, the shared ADR-03 bridge with its ADR-14
/// error mapping, two `invalid_params` token parsers, ServerHandler info, bin)
/// ≈165; hosting the core-owned watcher handle for the serve loop's lifetime
/// (S-022, zero policy per ADR-03) ≈10. Any growth beyond this means logic is
/// leaking into the surface — move it to logos-core instead of raising the budget.
/// (S-053 raised 575→660 for the four wiki twins; S-140/CR-036 raised 660→675 for
/// the `coverage_refresh` twin, a single `Engine::coverage_refresh` delegation;
/// S-204/S-205/CR-052 raised 675→680 for the `doctor` and `verify` quality twins,
/// each a single `Engine::doctor`/`Engine::verify` delegation; S-263/CR-062 raised
/// 680→690 for the `wiki_materialize` twin, a single `Engine::wiki_materialize`
/// delegation — logic lives in logos-core governance/wiki, not the surface.
/// S-284/CR-076 raised 690→693 for a genuine surface addition; that +3 was
/// reviewed at the Sprint 52 human review and blessed as a deliberate governance
/// decision — an honest, delegation-only addition, not leaked logic to trim.)
///
/// S-248/CR-061 raised 693→880 for the FR-WS-05 cross-service surface (the
/// `Backing::Single | Federated` seam + the `xservice_*` tool family). The
/// `xservice_tool_router` adds 5 tools (`route-providers`, `callers`, `impact`,
/// `search`, `workspace_status`) plus their 4 typed param structs (~107 lines),
/// each a one-`query::*` delegation over the member registry — all logic lives
/// in `logos_core::federation::query`, directly comparable to the S-053 wiki
/// family's five-twin +85. The `Backing` seam itself (~40 lines) is the
/// `backing`/`bridge` fields, the `new` (single) / `federated` constructors,
/// `default_engine` (default-member resolution), the `run_xservice` registry
/// delegator, and the `run_blocking` factoring that keeps the ADR-14 error
/// mapping in one place for both the per-engine and registry tools — pure
/// adapter plumbing (ADR-03), no logic; plus `list_tools`, the engine-free
/// roster introspection the byte-identity test reads. The single-root
/// `single_tool_router` roster stays byte-for-byte unchanged (asserted by
/// `tests/xservice_roster.rs`), so this is additive surface, not leaked logic.
/// Flagged for the Sprint 55 human review as a deliberate tool-family addition,
/// per the S-284 precedent above.
///
/// Sprint 57/CR-061 raises 880→900 for the two workspace twins added by the
/// sprint's parallel sessions. Measured against the Sprint 56 base of 857, the
/// whole sprint costs +38, landing the adapter at 895.
///
/// The two sessions ran in parallel and each measured only its own branch, so
/// neither saw the other's growth: S-257 added `workspace_reachability` (+9,
/// which fit inside the then-880 budget and correctly did not raise it), and
/// S-258 added `workspace_check` + `run_xservice_result` (+31) and raised the
/// budget to 890 — enough for its own branch (888) but not for the merge of
/// both (895). The sprint review re-derived the figure over merged HEAD; the
/// 900 below is that measurement plus the usual small headroom, not a number
/// bumped until the test went green.
///
/// All three additions are delegation-only, so there is no logic to trim:
///
/// `workspace_reachability` and `workspace_check` are each an attribute, a
/// signature, and ONE `logos_core::federation` call. The union-view walk and the
/// whole workspace rule family (manifest schema, glob compilation, the boundary
/// and no-cross-service-callers evaluation, the honest-empty `Option`) live in
/// `logos-core`; the surface decides nothing.
///
/// `run_xservice_result` is the fallible twin of the registry delegator,
/// mirroring the `run`/`run_result` pair that already exists directly above it.
/// It is the ADR-14 error-mapping seam, not policy: `workspace_check` compiles
/// user-authored globs, and a malformed rule must surface as a structured MCP
/// error rather than silently matching nothing (a rule that quietly never fires
/// would report a false all-clear).
///
/// The same "honest, delegation-only addition" the S-284 precedent blesses.
/// Flagged for the Sprint 57 human review as a deliberate governance decision.
#[test]
fn mcp_surface_line_budget() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let non_blank = non_blank_rust_lines(&src);

    assert!(
        non_blank <= 900,
        "mcp adapter exceeds the 900 non-blank LOC budget (NFR-MA-02): \
         found {non_blank} lines — move logic to logos-core"
    );
}
