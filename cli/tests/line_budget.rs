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
// + output/exit-code helpers), `dispatch.rs` (per-domain subcommand routing,
// extracted from `run` to keep each function under the max_cc/max_fn_lines gates),
// `workspace_init.rs` (the `logos init --workspace` enablement flow, S-244), and
// `xservice.rs` (the S-248 cross-service query routing). The latter two were
// added without being registered here (a guard blind spot); S-248 wires them in
// so the whole crate is measured, per the invariant above.
const CLI_MAIN: &str = include_str!("../src/main.rs");
const CLI_DISPATCH: &str = include_str!("../src/dispatch.rs");
const CLI_WORKSPACE_INIT: &str = include_str!("../src/workspace_init.rs");
const CLI_XSERVICE: &str = include_str!("../src/xservice.rs");

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
    file_lines(CLI_MAIN)
        + file_lines(CLI_DISPATCH)
        + file_lines(CLI_WORKSPACE_INIT)
        + file_lines(CLI_XSERVICE)
}

/// Budget: ≤ 825 production lines of Rust in the CLI adapter (NFR-MA-02).
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
///
/// S-248/CR-061 540→745 for the FR-WS-05 cross-service surface AND a guard
/// integrity fix. The two new top-level commands (`xservice
/// <route-providers|callers|impact|search>` and `workspace status`) add their
/// `Commands` variant declarations in `main.rs` + two one-line dispatch arms in
/// `dispatch.rs` (~+12), with the subcommand enums and the
/// discover→registry→`query::*` routing in the new `cli/src/xservice.rs`
/// delegation module (~108 lines, one-`query::*`-call routing, no business logic
/// — all logic is in `logos_core::federation::query`, verified by `logos
/// check`). The bulk of the raise, however, is honesty, not growth: `adapter_lines`
/// previously summed only `main.rs` + `dispatch.rs`, so `workspace_init.rs`
/// (~70, S-244) and now `xservice.rs` (~108) escaped the whole-crate invariant
/// this guard documents. S-248 registers both modules above, so the measured
/// total jumps from 549 to ~727 (a one-time correction of the blind spot, not
/// new surface), and the budget is set to 745 to fit with headroom. Future
/// logic-creep in ANY `cli/src/*.rs` now counts. Flagged for the Sprint 55 human
/// review. Do not raise the number without a story-level justification.
///
/// S-294/CR-084 745→760 for the `workspace reachability` payload filter: the
/// `Reachability` command variant gains `--repo`/`--all` `#[arg]` declarations and
/// its dispatch arm builds a `federation::ReachabilityScope` before delegating to
/// the same one `app_wide_reachability(..).bound(scope)` call (+6 measured, landing
/// at 751). The scoped, promotions-only projection lives entirely in
/// `logos_core::federation::reach` (verified by `logos check`); the adapter only
/// declares the flags and delegates. CR-084 §4.4 owns and blesses this raise.
///
/// S-312/CR-095 760→775 for the `quality-report` report-tier command: the
/// `QualityReport { hook_json }` variant declaration (+5) and its two-branch
/// dispatch arm (+6, landing at 763), each branch **one** `Engine` call —
/// `quality_readout` for the plain read-model, `quality_report_hook_payload` for
/// the agent-host session-start payload. The readout, its baseline-comparability
/// logic and the payload's JSON shape all live in `logos_core`
/// (`governance::quality_readout`, `wiki::session_start_payload`) precisely so the
/// hook script does not assemble JSON in shell; the adapter declares the flag and
/// picks the rendering. CR-095 §4.4 owns and blesses this raise. It is the last
/// raise this surface should need for the report tier — a third rendering belongs
/// behind a `--format` flag on the same arm, not a new command.
///
/// **S-321/CR-099 775→825** for the bounded warm supervisor (FR-WS-14, BR-44),
/// measured 763→819 (+56), 6 lines of headroom. The raise buys the *process*
/// half of the correction; the *logic* half deliberately went the other way.
/// What is here:
/// - `main.rs` (+9): the hidden `internal-warm` `Commands` variant — the
///   supervisor's own entry point — plus the one arm that skips
///   `observability::init`. It has to be a subcommand of this binary because the
///   warm must outlive `init --workspace` (a thread dies with the parent; a
///   detached child is reparented), and the warm already worked by re-invoking
///   `current_exe()`. Hidden from help, not a public contract (FR-CL-01). The
///   telemetry skip is what makes "the supervisor holds no `.logos` file"
///   literal rather than nearly true: it is the only command that lives for a
///   whole serialized warm, so holding a migrated `telemetry.db` connection for
///   minutes — at whatever root a detached child inherited — is not the same
///   cost it is for a millisecond-long read command.
/// - `dispatch.rs` (+4): one routing arm, and the injected warm at the
///   `Init --workspace` call site.
/// - `workspace_init.rs` (+43): `spawn_supervisor` (one detached child in its
///   own process group, whatever N is), `supervisor_argv` (the single `--`
///   terminated command line the whole delta rides — factored out so "one
///   supervisor, not N indexers" is assertable without spawning), `run_supervisor`
///   (the supervisor body), and `index_member` (spawn-and-*await* one `index`
///   child) — minus the 14-line `spawn_background_warm` and the 3-line
///   per-member fan-out they replace.
///
/// Every one of those is irreducibly surface: detaching a process, choosing its
/// process group, building an argv, and awaiting a child cannot live in a
/// deterministic core. The scheduling itself — the bound resolution and the
/// bounded queue, i.e. the business logic BR-44 constrains — is in
/// `logos_core::federation::warm`, where the ceiling is unit-tested against a
/// stubbed spawn (NFR-PE-08: launching 84 real indexers to test a bound would
/// oversubscribe the machine running the suite).
///
/// **Attribution.** S-321 owns and spends it, and the raise is now **recorded**
/// per the CR-084 §6 protocol ("the raise is recorded, not laundered") in all
/// three places it was owed: the CR-099 §4.1 NFR-MA-02 row, the §4.4 surface-
/// budget bullet naming 775→825 measured 819, and the `cli-surface.md` Notes.
/// Approved at the Sprint 61 human review. Do not raise this number again
/// without a story-level justification.
///
/// **Headroom, and how it was bought back.** The raise landed at 819 of 825 —
/// six lines. The Sprint 61 review's disposition bought nine back by deleting
/// `parse_kind` from `main.rs`, which duplicated `NodeKind::from_wire`: the
/// ontology now carries its own `FromStr` (`logos_core::model::kinds`), so
/// `--kind` validates through the vocabulary that defines it and clap needs no
/// adapter-side `value_parser`. That is the *correct* shape of a reduction
/// here — remove a duplication of core logic, not relocate surface. Note what
/// was considered and rejected: moving `supervisor_argv` into
/// `logos_core::federation::warm` would have given the core its first
/// production knowledge of this binary's own flag spelling (`internal-warm`,
/// `--concurrency`), inverting the seam `warm_queue`'s spawn closure exists to
/// hold. The adapter is genuinely thin at this point; the next author needing
/// room should expect to find a duplication, not a relocation.
///
/// **S-333 spent three of the nine first, ahead of S-319: 816 → 819.** The
/// `init --workspace` working-tree footprint notice (FR-WS-02, FR-IN-04)
/// needed exactly one adapter statement — `if let Some(notice) =
/// report.footprint.notice().filter(|_| !out.quiet) { eprintln!("{notice}"); }`
/// in `run` — everything else (the count, the committed/ignored split, the
/// prose) composed in `logos_core::federation::enable`/`init`. This is the
/// sprint's first writer of the three-writer sequence on this file (Iteration
/// 1, ahead of S-319 and S-331 below); it is recorded here, not laundered into
/// the pre-existing "819" figure above, which describes the state *after* this
/// spend, not before it. Recorded, not laundered (CR-084 §6).
///
/// **S-319 spent five of the remaining six: 819 → 824.** The `logos init`
/// parent-of-repos nudge (FR-IN-08) needed exactly two irreducible adapter
/// statements — print a pre-composed line, ask a y/n — plus the `let`-else that
/// declines every other root. Detection, the explanation prose and the offer
/// text are all composed in `logos_core::federation::enable::ParentOfRepos`;
/// the dispatch wiring cost nothing, being a modification of the existing
/// `if workspace {` condition. Recorded, not laundered (CR-084 §6).
///
/// Also considered and **rejected** while looking for room: adding an
/// `Engine::open`-flavoured `Output::open_query` chokepoint to fold the two
/// hand-rolled `stats`/`languages` arms into the `query`/`try_query` family.
/// It reads like the duplication the paragraph above predicts, and it is one —
/// but under this file's multi-line-signature convention the helper costs eight
/// lines to save six, so it makes the budget *worse*. Measured, not assumed. A
/// future author should not re-derive it and assume otherwise.
///
/// **S-331 spent the last one: 824 → 825.** The durable warm-outcome record
/// (FR-WS-17) needed exactly one adapter statement — `warm::record_outcomes(&summary)`
/// in `run_supervisor`, immediately after the queue drains. Everything that call
/// does is business logic and lives in `logos_core::federation::warm`: locating
/// the workspace root by walking up for the manifest, turning the summary's
/// absolute roots into the member names read-models join on, merging with the
/// record already on disk, and the atomic sibling-temp-plus-rename write. The
/// schema and the degrading read are `federation::warm_state`'s, and the
/// consumer side never touches the adapter at all — `query::workspace_status`
/// reads the record itself. Recorded, not laundered (CR-084 §6).
///
/// Also considered and **rejected** while looking for room: folding the
/// supervisor's four-line stderr degraded-member loop into the same core call.
/// It would have bought three lines, but that loop is the *foreground*
/// diagnostic channel — a human running `internal-warm` directly — and it is
/// asserted by `init_workspace.rs`'s failing-member test; moving it would give
/// the core a `eprintln!` on a path that has no terminal in production. The
/// budget is not worth buying with a worse seam.
///
/// **S-352/CR-112 825→841** for the `check` absent-contract exit state
/// (FR-GV-22): the `--allow-no-rules` opt-out flag declaration (+2, one
/// `#[arg(long)]` bool) and the `Output::report_check` chokepoint (+13) that
/// replaces `check`'s use of the shared `report_gate` — its verdict is a
/// tri-state `Option<bool>`, not the plain bool `report_gate`'s callers
/// supply, and its human rendering must name the absent-contract condition
/// instead of the pretty-printed report. Both are irreducible surface: the
/// exit-code projection itself lives in `logos_core` as
/// `RulesReport::exit_code` (verified by `logos check`), so nothing here is a
/// duplication waiting to be deleted — the adapter only picks the flag and
/// the rendering. Recorded, not laundered (CR-084 §6).
///
/// **The budget was exactly full at 841.**
///
/// **S-358/CR-114 841→855** for the `impact-intersection` command ([FR-NV-11]):
/// measured 841→851 (+10), 4 lines of headroom. The whole delta is `main.rs`'s
/// `ImpactIntersection` variant declaration (+7: the `#[command(name = ...)]`
/// rename, the repeatable `--item` `#[arg]` and the `--depth` `#[arg]`) plus a
/// 3-line dispatch arm delegating to ONE `Engine::impact_intersection` call.
///
/// The S-321 note above told the next author to expect a duplication of core
/// logic to delete. There is none: every remaining function in `cli/src` is a
/// clap declaration, one of the four `Output` chokepoints, or surface I/O (the
/// TTY prompt, `--body-file`, the detached supervisor argv). That was checked
/// before raising, not asserted after.
///
/// What the raise bought back instead is the reduction this guard actually
/// wants. The `<id>=<symbol>[,<symbol>...]` work-item spelling is parsed in
/// `logos_core::models::navigation::WorkItem::from_specs` — NOT here — so the
/// adapter hands the raw `Vec<String>` straight through, and the CLI, the MCP
/// tool and the `/api/v1` route cannot drift in what they accept. A clap
/// `value_parser` would have cost more adapter lines *and* put the parse in
/// three surfaces. Recorded, not laundered (CR-084 §6).
#[test]
fn cli_surface_line_budget() {
    let lines = adapter_lines();
    assert!(
        lines <= 855,
        "cli adapter exceeds the 855 production-LOC budget (NFR-MA-02): \
         found {lines} lines across cli/src/*.rs — move logic to logos-core"
    );
}
