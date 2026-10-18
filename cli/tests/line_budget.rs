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
//! `logos_core::wiki`, since S-644 the wiki extension's crate) — all one-Engine-call
//! delegations, no logic in the adapter. Combined Iteration-2 budget 395→500.
//!
//! Uses `include_str!` so the check is hermetic (no filesystem I/O at runtime)
//! and the count is stable across machines.

// NOTE: this guard sums adapter production lines across ALL `cli/src/*.rs` files
// so the thin-surface invariant (NFR-MA-02) covers the whole crate, not just the
// entry point. When a new `cli/src/*.rs` module is added, `include_str!` it here
// and add it to `adapter_lines` below. Current modules: `lib.rs` (parse + setup
// + extension composition + output/exit-code helpers — `main.rs` until S-639
// split the CLI into a library), `main.rs` (the composition root) and
// `composition.rs` (the host it composes, which the surface snapshots also read), `dispatch.rs` (per-domain subcommand routing,
// extracted from `run` to keep each function under the max_cc/max_fn_lines gates),
// `workspace_init.rs` (the `logos init --workspace` enablement flow, S-244), and
// `xservice.rs` (the S-248 cross-service query routing). The latter two were
// added without being registered here (a guard blind spot); S-248 wires them in
// so the whole crate is measured, per the invariant above.
const CLI_LIB: &str = include_str!("../src/lib.rs");
const CLI_MAIN: &str = include_str!("../src/main.rs");
const CLI_COMPOSITION: &str = include_str!("../src/composition.rs");
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
    file_lines(CLI_LIB)
        + file_lines(CLI_MAIN)
        + file_lines(CLI_COMPOSITION)
        + file_lines(CLI_DISPATCH)
        + file_lines(CLI_WORKSPACE_INIT)
        + file_lines(CLI_XSERVICE)
}

/// Budget: ≤ 900 production lines of Rust in the CLI adapter (NFR-MA-02).
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
/// logic to delete. The one this file already records — the `stats`/`languages`
/// `open_query` fold, in the S-319 paragraph above — was re-examined and is
/// still net-negative here: the helper costs eight lines to save six under this
/// file's multi-line-signature convention. Beyond it there is nothing: every
/// remaining function in `cli/src` is a clap declaration, one of the four
/// `Output` chokepoints, or surface I/O (the TTY prompt, `--body-file`, the
/// detached supervisor argv). Checked before raising, not asserted after.
///
/// What the raise bought back instead is the reduction this guard actually
/// wants. The `<id>=<symbol>[,<symbol>...]` work-item spelling is parsed in
/// `logos_core::models::navigation::WorkItem::from_specs` — NOT here — so the
/// adapter hands the raw `Vec<String>` straight through, and the CLI, the MCP
/// tool and the `/api/v1` route cannot drift in what they accept. A clap
/// `value_parser` would have cost more adapter lines *and* put the parse in
/// three surfaces. Recorded, not laundered (CR-084 §6).
///
/// **S-359/CR-114 855→865** for the `precedent` command ([FR-NV-12]): measured
/// 851→857 (+6), 8 lines of headroom. The whole delta is `main.rs`'s
/// `Precedent` variant (+5: the four-line doc comment the `--help` renders, the
/// `target` positional and the `--limit` `#[arg]`) plus a 1-line dispatch arm
/// delegating to ONE `Engine::precedent` call.
///
/// Nothing to trim, and this was checked rather than assumed: the duplication
/// the S-358 paragraph above re-examined (the `stats`/`languages` `open_query`
/// fold) is still net-negative, and no new candidate appeared — the command
/// carries no parsing of its own. The symbol-then-file resolution rule that
/// decides what a `precedent` target *is* lives in
/// `logos_core::navigate::resolve_precedent_target`, shared verbatim with the
/// MCP tool and the `/api/v1` route, so a target string means the same thing on
/// every surface (ADR-01). Recorded, not laundered (CR-084 §6).
/// **S-360/CR-114 855→868** for the `branch-overlap` command ([FR-NV-13]):
/// measured 851→863 (+12), 5 lines of headroom. The whole delta is `main.rs`'s
/// `BranchOverlap` variant declaration (+9: the `#[command(name = ...)]` rename
/// and the repeatable `--ref` plus `--base` and `--merge` `#[arg]`s) and a
/// 3-line dispatch arm delegating to ONE `Engine::branch_overlap` call.
///
/// The duplication hunt this file keeps asking of each author was run again and
/// is still negative: the `stats`/`languages` `open_query` fold recorded in the
/// S-319 paragraph above remains the only candidate, and S-358 re-examined it
/// and left it. What the raise buys instead is that ref resolution, the git
/// diff parse and the span attribution all stayed in
/// `logos_core::navigate::branch` — the CLI hands over the `Vec<String>` it was
/// given and interprets nothing. CR-084 §6 — recorded, not laundered.
///
/// [FR-NV-13]: ../../docs/specs/requirements/FR-NV-13.md
///
/// **S-361/CR-114 878→890** for the session-gate CLI twins ([FR-CL-06]):
/// measured 869→880 (+11), 10 lines of headroom. Itemised: `main.rs` +8 — the
/// `Health` variant with its `--no-reconcile` flag (+4) and the `SessionStart`
/// / `SessionEnd` variants, each a `#[command(name = "session-start", …)]`
/// rename plus a fieldless variant (+2 each) — and `dispatch.rs` +3, one arm
/// per command, each ONE `Engine::health` / `Engine::session_start` /
/// `Engine::session_end` call through the existing `try_query` / `report_gate`
/// chokepoints. Nothing computed in the adapter: the baseline upsert and the
/// epsilon comparison are `logos_core::governance`, exactly as they already
/// were for the MCP twins these commands now pair with.
///
/// The duplication hunt this file asks of each author was run again and is
/// still negative. The one recorded candidate — folding the `stats`/`languages`
/// `Engine::open(root)` + `print` pair into an `Output::open_query`
/// chokepoint — was re-examined: the helper costs more lines than the two arms
/// it would shorten, so it stays rejected for the same reason S-319 and S-358
/// left it. CR-084 §6 — recorded, not laundered.
///
/// [FR-CL-06]: ../../docs/specs/requirements/FR-CL-06.md
///
/// **S-334/CR-103 spends nothing: 886 → 886.** The declined-to-empty fallback
/// ([FR-IN-08]) needed the nudge's *provenance* to reach `run`'s empty-member
/// branch, and it rides `Option`s that already existed: `nudge`'s `bool` return
/// becomes `Option<bool>`, and the one-arm `eprintln!` becomes a two-arm
/// `match` of the same length. CR-103 §7 asked for line-neutral or negative;
/// it is neutral, so there is no raise to record under CR-084 §6.
///
/// The duplication hunt, re-run rather than assumed, now has **two** candidates
/// and rejects both on measurement. The standing `stats`/`languages`
/// `open_query` fold is still net-negative. The new one is this change's own:
/// `Engine::init_with(root, &init_options(i, h))` appears in both `dispatch.rs`
/// and `workspace_init.rs`. A shared `plain_init` helper costs +3 and removes
/// 0 — both call sites stay one line — landing at 889 of 890 to de-duplicate a
/// single expression. Rejected: the duplication is an expression, not a
/// mechanism.
///
/// [FR-IN-08]: ../../docs/specs/requirements/FR-IN-08.md
///
/// **S-401/[CR-125] spends 2 and raises nothing: 886 → 888**, 2 lines of
/// headroom left under 890. `xservice callers` and `xservice impact` each gain
/// one line — a `let (edges, residue) = query::reachability_inputs(…)` binding
/// where they previously bound the edges alone. The residue itself is assembled
/// in `logos_core::federation::residue` and merely serialised here, so this is
/// delegation, not logic.
///
/// **It first spent 4, landing at 890 of 890 — the whole headroom, silently.**
/// The story's review caught it: the arms were calling `query::edges` and a
/// separate residue accessor on two lines. Folding them into the one
/// `reachability_inputs` entry point was required anyway (two calls read the
/// member sync-stamps twice and let the answer and its residue describe two
/// different generations), and it hands back half the spend. Recorded because a
/// budget silently consumed to zero is discovered by the next author as a gate
/// failure with no context — CR-084 §6, recorded, not laundered.
///
/// [CR-125]: ../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
///
/// **S-464/[CR-148] 890→900** for `xservice build-deps` ([FR-WS-33]): measured
/// 888→897 (+9), 3 lines of headroom.
///
/// | Region | Δ |
/// |---|---|
/// | the `BuildDeps { repo }` variant (`#[command]`, the field and its `#[arg]`) | **+5** |
/// | its dispatch arm (one `relation`, one `xservice_build_deps` print) | **+4** |
/// | | **+9** |
///
/// Delegation only: the arm serialises ONE `federation::xservice_build_deps`
/// over the relation a fresh `BuildDependencies` joins. The join, the per-member
/// projection, the `--repo` scope note and the cross-context hint live in
/// `logos_core::federation::build_deps`, shared with the MCP and web twins. The
/// variant's doc comment costs nothing here (comments are not counted). The
/// same shape as every other `xservice` subcommand, so there is nothing to fold.
///
/// [CR-148]: ../../docs/requests/CR-148-build-manifests-yield-a-build-dependency-relation.md
/// [FR-WS-33]: ../../docs/specs/requirements/FR-WS-33.md
///
/// **S-474/[CR-152] 900→906** for `xservice type-refs` and the type-reference
/// stitch in `callers`/`impact` ([FR-WS-35]): measured 899→903 (+4), 3 lines of
/// headroom.
///
/// | Region | Δ |
/// |---|---|
/// | the `TypeRefs { repo }` variant (`#[command]`, the field and its `#[arg]`) | **+5** |
/// | its dispatch arm (one `xservice_type_refs` print) | **+3** |
/// | `type_references`, the one-shot overlay (one `TypeReferences::index` over a fresh `BuildDependencies`) | **+3** |
/// | `use std::sync::Arc` for that helper's return type | **+1** |
/// | the `callers`/`impact` arms: each chains one `.with_type_references(…)`, and the call folds from 8 lines to 4 | **−8** |
/// | | **+4** |
///
/// Delegation only: the listing, the scope note, the stitch's match, the
/// importer's file closure and the per-member error all live in
/// `logos_core::federation::query`, shared with the MCP twin. The helper is
/// shared by the three arms rather than repeated in each, which is what keeps
/// the reflowed arms one line apiece.
///
/// [CR-152]: ../../docs/requests/CR-152-cross-member-type-references-overlay.md
/// [FR-WS-35]: ../../docs/specs/requirements/FR-WS-35.md
///
/// **S-513/[CR-168] 906→908** for the `index`/`sync` exit-code split
/// ([FR-EH-05], [FR-CL-03]): measured 903→907 (+4), 1 line of headroom.
///
/// | Region | Δ |
/// |---|---|
/// | the `index` arm binds its result to project `failed` onto the exit code (`violation_code`, the existing helper) | **+1** |
/// | the `sync` arm becomes a block: `load_config_from_root(root)?` before the existing `report_gate` chokepoint | **+3** |
/// | | **+4** |
///
/// Delegation only: whether a run persisted nothing is decided in
/// `logos_core::pipeline` and carried as `failed`; the adapter projects it.
/// The `sync` config guard mirrors the one `index` already had — without it an
/// invalid `config.toml` reaches `Engine::sync`, which now reports a failed run
/// (exit 1) where the contract says usage fault (exit 2). The duplication hunt
/// was run again: folding the guard into `report_gate` would put a config read
/// on `check`/`gate`/`doctor`/`verify`, which load their own; rejected.
/// Recorded, not laundered (CR-084 §6).
///
/// [CR-168]: ../../docs/requests/CR-168-an-index-never-silently-empties.md
/// [FR-EH-05]: ../../docs/specs/requirements/FR-EH-05.md
/// [FR-CL-03]: ../../docs/specs/requirements/FR-CL-03.md
///
/// **S-639/CR-213 908→1007** for the extension host's composition root
/// ([FR-DS-04], [ADR-74]): measured 907→1004 (+97), 3 lines of headroom. The CLI
/// became a library (`lib.rs`) with a thin `main.rs`, and this guard read only
/// `main.rs` — after the move it would have measured a 3-line file and passed
/// on nothing. `lib.rs` is registered above.
///
/// | Region | Δ |
/// |---|---|
/// | `fn main` removed from the library (its fdlimit raise, parse and panic boundary moved into `run_from`) | **−13** |
/// | `run(host)`: the composition root's one call, flushing before `process::exit` | **+7** |
/// | `run_from(host, args)`: parse, register the extensions' telemetry tools, the unchanged panic boundary | **+30** |
/// | `command(host)`: the composed clap command | **+3** |
/// | `Composed`/`Invocation`: compose extension subcommands, route a parse | **+39** |
/// | `clash`: asked of the built command — names, aliases, clap's own `help`, the global flags — so an extension can neither shadow a command nor panic clap (the story's review) | **+14** |
/// | `run_extension`: an extension subcommand over the core's root and output flags | **+11** |
/// | `Output::print` delegates to the one printing rule extensions share | **−5** |
/// | imports (`OsString`, the clap traits, `logos_ext`) | **+3** |
/// | `main.rs`: the public composition root | **+4** |
/// | `composition.rs`: the public host, in the one file `main` and the surface snapshots both compile (the story's review) | **+3** |
/// | `dispatch.rs`: the `ExtensionHost` import (the signature stays one line) | **+1** |
/// | | **+97** |
///
/// Composition, not logic: an extension's subcommand is clap wiring the core
/// cannot own (it names no CLI type), and its handler — the only part that
/// computes anything — is the extension's. The printing rule moved the other
/// way, into `logos_ext::cli`, so the core subcommands and every extension's
/// print through one function rather than a twin.
///
/// [FR-DS-04]: ../../docs/specs/requirements/FR-DS-04.md
/// [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
///
/// **S-638/[FR-IN-07] 1007→1012** for the session-start hook becoming a core
/// command (`logos hook --emit [--force]`, [ADR-49]): measured 1004→1010 (+6), 2
/// lines of headroom. The two spellings — the public `hook` and the hidden
/// `wiki hook` alias — share one argument struct and one dispatch function, so
/// the alias costs less than the old stand-alone variant did.
///
/// | Region | Δ |
/// |---|---|
/// | `Commands::Hook(HookArgs)` | **+1** |
/// | `WikiCommands::Hook` shares the args and gains `hide = true` (6 lines → 2) | **−4** |
/// | `HookArgs`: the one `--emit`/`--force` definition | **+7** |
/// | `dispatch.rs`: the `Commands::Hook` arm | **+1** |
/// | `dispatch.rs`: the `wiki hook` arm (5 lines → 1) | **−4** |
/// | `dispatch.rs`: `hook_emit`, the one body both spellings call | **+5** |
/// | | **+6** |
///
/// Routing, not logic: `hook_emit` is one `Engine::quality_report_hook_emit`
/// call and one `print`; the materializer lives in `logos_core::hooks`.
///
/// [FR-IN-07]: ../../docs/specs/requirements/FR-IN-07.md
/// [ADR-49]: ../../docs/specs/architecture/decisions/ADR-49.md
///
/// **S-643/[FR-DS-04] 1012→1015** for the public composition registering its
/// first extension, `chat`, under `agents` ([S-643]): measured 1010→1013 (+3), 2
/// lines of headroom, as before.
///
/// | Region | Δ |
/// |---|---|
/// | `composition.rs`: `host()` binds the host, registers the `chat` extension under `#[cfg(feature = "agents")]`, returns it (1 line → 4) | **+3** |
/// | | **+3** |
///
/// Composition, not logic: the registration is the one line the composition root
/// exists for; everything chat does is the extension crate's, outside this budget.
///
/// [S-643]: ../../docs/planning/journal.md#s-643-chat-is-a-registered-extension
///
/// **S-644 T1 1012→1016** for the wiki leaving `logos-core` for a crate of its
/// own ([ADR-74]): measured 1010→1014 (+4), 2 lines of headroom.
///
/// | Region | Δ |
/// |---|---|
/// | `dispatch.rs`: the wiki crate's `WikiEngine` trait import — the `wiki_*` calls are its trait methods now | **+1** |
/// | `dispatch.rs`: `init_steps`, the wiki's `-i` skill step (no longer a core step) ahead of the host's | **+3** |
/// | `workspace_init.rs`: the fallback `init` passes `init_steps(host)` instead of `host.init_steps()` | **0** |
/// | | **+4** |
///
/// Composition, not logic: the step itself is the wiki crate's `SkillInit`, and one
/// list feeds both `init` paths so they cannot disagree. Interim: T2 registers
/// the wiki as an extension, its step arrives through `host.init_steps()`, and
/// all four lines go.
///
/// [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
///
/// **Sprint 97 merge, 1015/1016 → 1019**: [S-643] (+3) and S-644 T1 (+4)
/// raised the same 1012 budget on parallel branches, each over its own tree.
/// Merged, the two deltas add: measured 1010→1017 (+7), 2 lines of headroom.
///
/// **S-644 T2 1019→938** for the wiki becoming the `wiki` extension ([ADR-74]):
/// `logos wiki` is the extension's subcommand, its `-i` skill step arrives through
/// `host.init_steps()`, and the four interim T1 lines go. Measured 1017→936
/// (−81); the budget is lowered to the measurement plus the usual 2 lines, so the
/// guard keeps its grip rather than carrying 81 lines of slack.
///
/// | Region | Δ |
/// |---|---|
/// | `lib.rs`: the `Commands::Wiki` variant | **−4** |
/// | `lib.rs`: the `WikiCommands` enum — moved verbatim to the wiki crate's `cli.rs` | **−38** |
/// | `lib.rs`: `read_wiki_body` — moved with it | **−15** |
/// | `lib.rs`: `engine()` delegates its index guard to `logos_ext::cli::indexed_engine`, the one body extension subcommands share | **−5** |
/// | `lib.rs`: `EXTENSION_SLOT_AFTER` + `place_extensions` — extension subcommands listed after `coverage` in `--help`, where `wiki` stood | **+16** |
/// | `dispatch.rs`: the `wiki` dispatch function, its arm and the wiki crate's import | **−38** |
/// | `dispatch.rs`: T1's interim `init_steps` (the four lines go) | **−3** |
/// | `composition.rs`: registers the `wiki` extension under `wiki`, with the in-app generator under `agents` | **+6** |
/// | | **−81** |
///
/// Composition, not logic: the wiki's commands and their handlers are the
/// extension's, outside this budget; what is left here is the one registration
/// and the one help-placement rule every extension's subcommand shares.
///
/// **S-651 938→936** for chat and the wiki leaving this repository ([FR-DS-05]):
/// the composition registers nothing, and `init -i` asks about skills only when
/// a composed extension contributes an init step. Measured 937→934 (−3); the
/// budget is lowered to the measurement plus the usual 2 lines.
///
/// | Region | Δ |
/// |---|---|
/// | `composition.rs`: `host()` registers nothing — the `chat` and `wiki` registrations and their `cfg` arms go | **−9** |
/// | `lib.rs`: `init_options` takes the composed `steps` and its prompt, and asks the skill question only with a step (1 line → 3) | **+2** |
/// | `dispatch.rs`: the `init` arm binds the host's steps once, for the question and the run | **+1** |
/// | `workspace_init.rs`: the fallback `init` arm does the same, as a block | **+3** |
/// | | **−3** |
///
/// [FR-DS-05]: ../../docs/specs/requirements/FR-DS-05.md
///
/// **S-711 936→996** for `--repo` on the CLI twins of the read-only MCP tools
/// ([FR-WS-42], [CR-225]): measured 933→994 (+61: `lib.rs` +57, `dispatch.rs` +4),
/// 2 lines of headroom.
///
/// | Region | Δ |
/// |---|---|
/// | `lib.rs`: the `federation` import (1), `MemberArg`, the one `--repo` definition (5), and its `#[command(flatten)] member` field on the 17 commands that take it (15 added lines: `status` and `coverage status` become struct variants in place) | **+21** |
/// | `lib.rs`: `Output::query_for` (10) / `try_query_for` (10) / `report_gate_for` (13), the selecting twins of the three chokepoints, and `member_engine` (3) | **+36** |
/// | `dispatch.rs`: each twin's arm binds `member` and calls the `_for` chokepoint (one arm gains lines for the longer pattern) | **+4** |
/// | | **+61** |
///
/// Routing, not logic: the selection and its errors are `Backing::one_shot` and
/// `Backing::engine_for` in `logos_core::federation` — the same function the MCP
/// `repo` parameter runs — and the exit code is `logos_core::error::exit_code`'s.
/// What is left here is the flag, and the choice of chokepoint per command.
///
/// [FR-WS-42]: ../../docs/specs/requirements/FR-WS-42.md
/// [CR-225]: ../../docs/requests/CR-225-per-member-single-root-tools.md
///
/// **S-667 996→1039** for the `security` group, the CLI twins of the four
/// security tools ([FR-SC-06], [FR-SC-08]): measured 996→1037 (+41: `lib.rs`
/// +30, `dispatch.rs` +11), 2 lines of headroom.
///
/// | Region | Δ |
/// |---|---|
/// | `lib.rs`: the `Security` variant of `Commands` | **+4** |
/// | `lib.rs`: `SecurityCommands` — `scan --consent` (4), `ingest <REPORT>` (3), `status` with `--repo` (1), `findings` with its six filter flags and `--repo` (15), the derive, header and close (3) | **+26** |
/// | `dispatch.rs`: the `security` arm — three one-line delegations, and `findings` turning its six tokens into the core's `FindingsFilter` before its one delegation | **+11** |
/// | | **+41** |
///
/// Routing, not logic: the filter tokens are parsed by
/// `logos_core::security::FindingsFilter::parse`, the function the MCP tool
/// calls, and the consent check, the scan and the payloads are the core's.
///
/// [FR-SC-06]: ../../docs/specs/requirements/FR-SC-06.md
/// [FR-SC-08]: ../../docs/specs/requirements/FR-SC-08.md
///
/// **S-507 1039→1047** for `check --bless`, the CLI twin of the `bless_rules`
/// tool ([FR-GV-23]): measured 1037→1045 (+8: `lib.rs` +2, `dispatch.rs` +6),
/// 2 lines of headroom.
///
/// | Region | Δ |
/// |---|---|
/// | `lib.rs`: the `--bless` flag on `Check` (`#[arg(long)]` and the field) | **+2** |
/// | `dispatch.rs`: the `check` arm binds `bless` and picks `bless_rules` or `check_rules` inside the one `report_check` closure | **+6** |
/// | | **+8** |
///
/// Routing, not logic: the bless write, the classification and the report are
/// `logos_core::governance`'s, reached through `Engine::bless_rules`.
///
/// [FR-GV-23]: ../../docs/specs/requirements/FR-GV-23.md
///
/// **S-676 1047→1058** for `offenders`, the CLI twin of the `offenders` tool
/// ([FR-QM-19]): measured 1045→1056 (+11: `lib.rs` +7, `dispatch.rs` +4),
/// 2 lines of headroom.
///
/// | Region | Δ |
/// |---|---|
/// | `lib.rs`: the `Offenders` variant of `Commands` — `--path` (2), `--dimension` (2), `--repo` (1), header and close (2) | **+7** |
/// | `dispatch.rs`: the `offenders` arm turns its two tokens into the core's `OffendersFilter` before its one delegation | **+4** |
/// | | **+11** |
///
/// Routing, not logic: the glob, the dimension vocabulary and the listing are
/// `logos_core::governance::offenders`', reached through `Engine::offenders` —
/// the function the MCP tool calls.
///
/// [FR-QM-19]: ../../docs/specs/requirements/FR-QM-19.md
///
/// **S-744 1047→1075** for `security install|uninstall` and `scan --install`
/// ([FR-SC-22]), the CLI-only install lifecycle: measured 1045→1073 (+28:
/// `lib.rs` +20, `dispatch.rs` +8), 2 lines of headroom.
///
/// | Region | Δ |
/// |---|---|
/// | `lib.rs`: the `--install` flag on `Scan` | **+2** |
/// | `lib.rs`: `Install` — the variant, its five arguments (scanner names, `--all`, `--update`, `--from <DIR>`, `--consent`) and its close | **+12** |
/// | `lib.rs`: `Uninstall` — the variant, the scanner names and `--all`, and its close | **+6** |
/// | `dispatch.rs`: two imports, `scan` split into its plain and `--install` arms, the `install` arm building the core's `InstallRequest`, and the one-line `uninstall` arm | **+8** |
/// | | **+28** |
///
/// Routing, not logic: the plan, the consent check and its refusal, the
/// installs and the payloads are `logos_core::tools::surface`'s, reached
/// through `Engine::security_install`, `security_uninstall` and
/// `security_scan_install`; `--all` versus the named scanners is
/// `Selection::named_or_all`. The two `CLI_ONLY` entries sit under
/// `#[cfg(test)]` and are not counted.
///
/// [FR-SC-22]: ../../docs/specs/requirements/FR-SC-22.md
///
/// **Sprint 106 Iteration 2 merge 1058/1075→1086:** S-676 (+11) and S-744 (+28)
/// branched from the same 1047 budget; the merged budget is 1047 + 11 + 28 = 1086,
/// measured 1045 + 11 + 28 = 1084, keeping the 2 lines of headroom.
///
/// **S-679 1086→1091** for `init --uninstall` ([FR-IN-12]), measured 1086→1091:
///
/// | Region | Δ |
/// |---|---|
/// | `lib.rs`: the `--uninstall` flag on `Init` (attribute + field) | **+2** |
/// | `lib.rs`: `init_options` names the new `claude_plugin` field | **+1** |
/// | `dispatch.rs`: the one-line `uninstall` arm, and `uninstall: _` in the existing `Init` pattern | **+2** |
/// | | **+5** |
///
/// Routing, not logic: plugin detection, the enrolment, the legacy removals and
/// the uninstall are `logos_core::init`'s, reached through `Engine::init_with_contributions`
/// and `Engine::init_uninstall`.
///
/// [FR-IN-12]: ../../docs/specs/requirements/FR-IN-12.md
///
/// **S-505 1086→1090** for `improvements`, the CLI twin of the `improvements`
/// tool ([FR-QM-17]): measured 1086→1090 (+4), no headroom (the 1086 budget was
/// already met exactly at the branch point). A sibling session in the same
/// iteration adds its own delta to the 1086 this one branched from; the merged
/// budget is the sum.
///
/// | Region | Δ |
/// |---|---|
/// | `lib.rs`: the `Improvements` variant of `Commands` — `--repo` (1), header and close (2) | **+3** |
/// | `dispatch.rs`: the one-delegation `improvements` arm | **+1** |
/// | | **+4** |
///
/// The surface-parity roster entry sits under `#[cfg(test)]` and is not counted.
///
/// Routing, not logic: the list is `Engine::improvements`' — the function the
/// MCP tool calls — and `GET /api/v1/health` carries the same value.
///
/// [FR-QM-17]: ../../docs/specs/requirements/FR-QM-17.md
///
/// Sprint 106 Iteration 3 merge: S-679 (+5) and S-505 (+4) each branched from
/// the same 1086 budget; the merged budget is 1086 + 5 + 4 = 1095.
///
/// **S-677 1095→1105** for `review`, the CLI twin of the `review` tool
/// ([FR-GV-24]): measured 1095→1105 (+10), no headroom. Sibling sessions in
/// Iteration 4 add their own deltas to the 1095 this one branched from; the
/// merged budget is the sum.
///
/// | Region | Δ |
/// |---|---|
/// | `lib.rs`: the `Review` variant of `Commands` — `--base` (2), `--repo` (1), header and close (2) | **+5** |
/// | `dispatch.rs`: the `review` arm — one `Engine::review` call, the verdict on `measured`, and the core's own stderr note on the cache | **+5** |
/// | | **+10** |
///
/// The surface-parity roster entry sits under `#[cfg(test)]` and is not counted.
/// The note's wording is `BeforeStateCache::note`'s, in the core.
///
/// [FR-GV-24]: ../../docs/specs/requirements/FR-GV-24.md
///
/// **S-671 1095→1098** for `hotspots --with-findings` ([FR-SC-14]), measured
/// 1095→1098 (+3). The coordinator sums this delta with the other Iteration 4
/// sessions' at merge.
///
/// | Region | Δ |
/// |---|---|
/// | `lib.rs`: the `--with-findings` flag on `Hotspots` (attribute + field) | **+2** |
/// | `dispatch.rs`: `with_findings` named in the `Hotspots` pattern | **+1** |
/// | | **+3** |
///
/// Routing, not logic: the column and the filter are `logos_core::history`'s,
/// reached through `Engine::hotspots_with_findings`, the call the MCP tool and
/// `GET /api/v1/files` also make.
///
/// [FR-SC-14]: ../../docs/specs/requirements/FR-SC-14.md
///
/// Sprint 106 Iteration 4 merge: S-677 (+10) and S-671 (+3) each branched from
/// the same 1095 budget; S-673 and S-674 changed no `cli/src` file. The merged
/// budget is 1095 + 10 + 3 = 1108.
///
/// **S-685 1108→1115** for `security scan --scanner --path` ([FR-SC-17]),
/// measured 1108→1115 (+7), no headroom. Sibling sessions in Iteration 5 add
/// their own deltas to the 1108 this one branched from; the merged budget is
/// the sum.
///
/// | Region | Δ |
/// |---|---|
/// | `lib.rs`: `--scanner` and `--path` on `Scan` (attribute + field each) | **+4** |
/// | `dispatch.rs`: the `Scan` arm builds a `ScanRequest` and calls `Engine::security_scan_selected` | **+3** |
/// | | **+7** |
///
/// Routing, not logic: path confinement, the plan per scanner and the partial
/// ingest are `logos_core::security`'s, the same call the MCP tool makes.
///
/// [FR-SC-17]: ../../docs/specs/requirements/FR-SC-17.md
///
/// **S-746 1115→1118** for `security install --accept-codeql-licence`
/// ([FR-SC-23]), measured 1115→1118 (+3), no headroom. Sibling sessions in
/// Iteration 6 add their own deltas to the 1115 this one branched from; the
/// merged budget is the sum.
///
/// | Region | Δ |
/// |---|---|
/// | `lib.rs`: `--accept-codeql-licence` on `Install` (attribute + field) | **+2** |
/// | `dispatch.rs`: the `Install` arm turns the flag into the request's accepted licences | **+1** |
/// | | **+3** |
///
/// Routing, not logic: the opt-in set, the licence check and its record are
/// `logos_core::tools::surface`'s.
///
/// [FR-SC-23]: ../../docs/specs/requirements/FR-SC-23.md
#[test]
fn cli_surface_line_budget() {
    let lines = adapter_lines();
    assert!(
        lines <= 1118,
        "cli adapter exceeds the 1118 production-LOC budget (NFR-MA-02): \
         found {lines} lines across cli/src/*.rs — move logic to logos-core"
    );
}
