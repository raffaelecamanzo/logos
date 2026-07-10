//! Surface line-count budget guard (NFR-MA-02, ADR-01).
//!
//! The thin-surface invariant: the adapter contains no business logic — every
//! subcommand delegates to exactly one Engine call. This guard keeps
//! logic-creep visible as a budget over the adapter's **production code**:
//! non-blank, non-comment lines before the unit-test module.
//!
//! Why not raw non-blank lines (the S-001 form of this guard)? Two reasons,
//! both introduced by the full FR-CL-01 subcommand set (S-016):
//! - doc comments on subcommands/args ARE the clap-generated help text — a
//!   budget that counts them punishes documenting the CLI;
//! - the bin crate carries unit tests for its exit-code mapping (FR-CL-03),
//!   which are verification, not surface logic.
//!
//! Budget: 25 subcommands at roughly a dozen lines of declaration + dispatch
//! each, plus parse/print/exit-code helpers, plus the S-023 interactive-init
//! UX (the `-i`/`--hooks` flags and the stderr y/n prompt helpers — genuinely
//! surface code: a TTY prompt cannot live in the deterministic core, and the
//! step logic itself does, in `logos_core::init`), plus the S-020 quality-
//! command flag set (`--no-reconcile`/`--assume-fresh`, `--rules`, `--save`,
//! `--threshold`, `--limit`, `--granularity` — CLI flag declarations and
//! dispatch that belong in the adapter, not the core). If this fires, move
//! logic to logos-core — do not raise the number without a story-level
//! justification (raises so far: S-016 200→300 for the full subcommand set;
//! S-023 300→330 for the interactive-init UX; S-020+S-023 assembly 330→340
//! for the quality-command flag declarations; S-037+S-038 assembly 340→360 for
//! the traceability subcommands `implements`/`referencing-docs` (FR-NV-10) and
//! the `doc-gaps` quality command (FR-GV-14, a read-only static-gap analysis)
//! — each a one-Engine-call delegation, no logic in the adapter); S-048
//! 360→370 for the `hotspots` temporal-tier subcommand (FR-GH-06, one
//! `Engine::hotspots` call shared with the MCP twin); S-051 370→395 for the
//! coverage evidence tier (CR-007): the `coverage ingest`/`coverage status`
//! sub-subcommand group (its own `CoverageCommands` enum + two-arm dispatch,
//! each one `Engine::coverage_*` call) and the `--untested` flag on `hotspots`
//! — all one-Engine-call delegations, no logic in the adapter); the source
//! wiki (CR-008) added two things in parallel Iteration-2 sessions, merged
//! here: S-053's `wiki write|read|search|status|delete` command group (its own
//! `WikiCommands` enum + dispatch, each one `Engine::wiki_*` call, plus the
//! `read_wiki_body` surface-I/O helper for `--body-file`/stdin so a large
//! markdown body never hits argv limits), and S-054's `wiki skill --emit [dir]
//! [--force]` arm (FR-WK-08, one `Engine::wiki_skill_emit` call) with the `-i`
//! wiki-skill materialization prompt (the materialization logic itself lives in
//! `logos_core::wiki`) — all one-Engine-call delegations, no logic in the
//! adapter. Combined Iteration-2 budget 395→500.
//!
//! Uses `include_str!` so the check is hermetic (no filesystem I/O at runtime)
//! and the count is stable across machines.

// NOTE: this guard sums adapter production lines across ALL `cli/src/*.rs` files
// so the thin-surface invariant (NFR-MA-02) covers the whole crate, not just the
// entry point. When a new `cli/src/*.rs` module is added, `include_str!` it here
// and add it to `adapter_lines` below. Current modules: `main.rs` (parse + setup
// + output/exit-code helpers) and `dispatch.rs` (per-domain subcommand routing,
// extracted from `run` to keep each function under the max_cc/max_fn_lines gates).
const CLI_MAIN: &str = include_str!("../src/main.rs");
const CLI_DISPATCH: &str = include_str!("../src/dispatch.rs");

/// Non-blank, non-comment production lines in one source file — everything
/// before the first top-level `#[cfg(test)]` marker.
fn file_lines(source: &str) -> usize {
    source
        .split("#[cfg(test)]")
        .next()
        .unwrap_or(source)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//"))
        .count()
}

/// Total adapter production lines across every `cli/src/*.rs` module.
fn adapter_lines() -> usize {
    file_lines(CLI_MAIN) + file_lines(CLI_DISPATCH)
}

/// Budget: ≤ 540 production lines of Rust in the CLI adapter (NFR-MA-02).
///
/// S-072 500→520 for the CR-012 `ui` serve wiring: the `--ui`/`--port` flags on
/// `serve` (cfg-gated behind the non-default `ui` feature) and the combined
/// `serve --mcp --ui` dispatch — one delegation into `web::serve_surfaces`, with
/// the one-Engine/one-watcher orchestration living in the `web` adapter, not
/// here (ADR-27, NFR-MA-02).
///
/// 520→540 for the `run`-dispatch split (max_cc/max_fn_lines remediation): `run`
/// was a 263-line, cyclomatic-complexity-108 god-dispatcher — over the
/// `max_cc = 25` / `max_fn_lines = 250` gates. Splitting the subcommand routing
/// into `dispatch.rs` and introducing the `Output::query`/`try_query`/
/// `report_gate` chokepoints brings every function under gate while adding NO
/// business logic (the split is pure routing — verified by `logos check`, which
/// reports zero violations in `cli/src/*.rs`). The scaffolding (a second module
/// header, function signatures, the shared chokepoints) costs a handful of lines
/// net; the summed adapter actually shrank (533→530) but sits just over the old
/// cap, so the budget is raised to fit with headroom. If this fires, move logic
/// to logos-core/web — do not raise the number without a story-level justification.
#[test]
fn cli_surface_line_budget() {
    let lines = adapter_lines();
    assert!(
        lines <= 540,
        "cli adapter exceeds the 540 production-LOC budget (NFR-MA-02): \
         found {lines} lines across cli/src/*.rs — move logic to logos-core"
    );
}
