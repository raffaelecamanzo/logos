//! Project setup — the full `logos init` experience (S-023, FR-IN-01..04).
//!
//! Extends the Sprint 4 minimal bootstrap ([`crate::Engine::init`]: `.logos/`
//! + store open/migrate) with everything DL-07 mandates to be **idempotent,
//! non-clobbering, and managed-block based**:
//!
//! - starter policy templates (`config.toml`, `rules.toml`) written only if
//!   absent — re-running `init` never overwrites an edited policy file
//!   ([FR-IN-01]);
//! - the generated `.logos/.gitignore` whose managed block ignores the
//!   derived/machine-specific state (`logos.db*`, `telemetry.db*`, …) while
//!   the checked-in policy travels ([FR-IN-04], SRS §12 layout);
//! - the `logos` MCP server block injected into the project's `.mcp.json`
//!   host config, skipping if already present ([FR-IN-02], [MCP Host]);
//! - a delimited, re-generatable managed block in the project `CLAUDE.md`
//!   priming graph-first usage — scoping before decomposition first, navigation
//!   while editing second ([FR-IN-09]) — preserving user content outside the
//!   markers ([FR-IN-02]);
//! - the optional git-hook installer via `core.hooksPath` ([FR-IN-03],
//!   [FR-SY-05]);
//! - no skill of its own: a skill an extension ships is materialized by the
//!   step that extension contributes as an [`InitContribution`] (S-644), which
//!   reads [`InitOptions::materialize_skill`] ([FR-IN-02] as modified by
//!   CR-008);
//! - with the Claude Code plugin enabled ([`ClaudePlugin`]), the plugin
//!   enrolment in place of the `.mcp.json` entry and the readout hook, and the
//!   removal of those legacy files ([FR-IN-12], [`plugin`], [`retire`]);
//! - `init --uninstall`, which takes all of the above back out and leaves
//!   `.logos/` ([`retire::uninstall`]).
//!
//! Failure posture (DL-06): a host-integration target we cannot *safely*
//! touch (malformed `.mcp.json`, a foreign `core.hooksPath`, a non-managed
//! hook file, not a git repo) is reported as a `Skipped` step with the
//! reason — never clobbered, never a hard error. I/O failures writing our
//! own artifacts fail loud.
//!
//! [FR-IN-01]: ../../../docs/specs/requirements/FR-IN-01.md
//! [FR-IN-02]: ../../../docs/specs/requirements/FR-IN-02.md
//! [FR-IN-03]: ../../../docs/specs/requirements/FR-IN-03.md
//! [FR-IN-04]: ../../../docs/specs/requirements/FR-IN-04.md
//! [FR-IN-09]: ../../../docs/specs/requirements/FR-IN-09.md
//! [FR-IN-12]: ../../../docs/specs/requirements/FR-IN-12.md
//! [FR-SY-05]: ../../../docs/specs/requirements/FR-SY-05.md
//! [FR-WK-08]: ../../../docs/specs/requirements/FR-WK-08.md
//! [MCP Host]: ../../../docs/specs/architecture/integrations/mcp-host.md

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::models::pipeline::{InitAction, InitStep};

pub(crate) mod plugin;
mod retire;

pub use plugin::ClaudePlugin;
// Re-exported rather than reached as `init::retire::uninstall`, so the engine's
// call folds into its existing `init/mod.rs` edge: the engine sits at its
// `max_fan_out` budget, which counts module files.
pub(crate) use retire::uninstall;

/// Which optional `init` steps to run (S-023, FR-IN-02/03).
///
/// The default (every flag `false`) is the plain `logos init` contract: policy
/// templates + managed `.gitignore` + store bootstrap only ([FR-IN-01]).
/// The CLI's `-i` / `--hooks` flags enable the rest.
///
/// [FR-IN-01]: ../../../docs/specs/requirements/FR-IN-01.md
#[derive(Debug, Clone, Copy, Default)]
pub struct InitOptions {
    /// Inject the `logos` MCP server block into `.mcp.json` (FR-IN-02).
    pub inject_mcp: bool,
    /// Write/refresh the managed `CLAUDE.md` block (FR-IN-02).
    pub write_claude_md: bool,
    /// Install git hooks via `core.hooksPath` (FR-IN-03).
    pub install_hooks: bool,
    /// Materialize the agent skills the composed extensions ship,
    /// skip-if-present (FR-IN-02 as modified by CR-008). The core owns the
    /// flag, not the step: a step an extension contributes reads it (S-644),
    /// and the core materializes no skill of its own.
    pub materialize_skill: bool,
    /// Install the Claude Code **session-start** quality-report hook (FR-IN-07,
    /// ADR-49, CR-095): a marker-tagged hook script + a non-clobbering merge into
    /// `.claude/settings.json`, default-on under `-i` alongside the skill. Also
    /// sweeps the retired SessionEnd entry and its orphaned script (CR-095), so
    /// an upgrade stops the `Hook cancelled` error with no hand-editing.
    /// (The PostToolUse wiki-augmentation hook this once also installed
    /// alongside was retired — CR-070.)
    pub install_quality_report_hook: bool,
    /// This `init` is running as one member of `logos init --workspace`
    /// ([FR-WS-02], CR-119) — changes only the `message`
    /// [`Engine::init_with`](crate::Engine::init_with) returns, never what it
    /// does. The single-repo message recommending `logos index` is wrong twice
    /// over in this context: the [FR-WS-14] warm supervisor `init --workspace`
    /// just spawned is already indexing this member, and `logos index` run at
    /// the workspace root builds only the root, not this member. Set only by
    /// [`crate::federation::enable::enable`]; every other caller takes the
    /// default `false`.
    ///
    /// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
    /// [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
    pub workspace_member: bool,
    /// Whether the Claude Code plugin is enabled ([FR-IN-12]). When it is,
    /// [`inject_mcp`](Self::inject_mcp) enrols the plugin in
    /// `.claude/settings.json` and removes the legacy `logos` `.mcp.json`
    /// entry instead of writing one,
    /// [`install_quality_report_hook`](Self::install_quality_report_hook)
    /// removes the legacy readout hook instead of installing it, and a copy of a
    /// skill the core itself once emitted ([`retire::legacy_core_skill`]) is
    /// removed unless [`materialize_skill`](Self::materialize_skill) is asked for
    /// or an extension owns the name (the `owned_skills` the host passes to
    /// `run`, from every registered extension, active or not). The default reads
    /// the Claude Code settings ([`ClaudePlugin::Detect`]).
    ///
    /// [FR-IN-12]: ../../../docs/specs/requirements/FR-IN-12.md
    pub claude_plugin: ClaudePlugin,
}

/// An `init` step contributed from outside the core — by an extension, through
/// the extension host ([FR-DS-04], [ADR-74]).
///
/// The core runs contributed steps after its own, in the order given, against
/// the same resolved root and with the same [`InitOptions`]: a step decides from
/// them whether it applies (an `-i`-only step returns no rows on a plain
/// `init`). It follows the core steps' failure posture — a target it cannot
/// safely touch is a `Skipped` row with the reason, and only an I/O failure on
/// an artifact it owns is an error.
///
/// Declared here rather than in the host's crate because `init` is the core's,
/// and the core names no extension ([BR-63]): the host implements this
/// contract; the core never learns who did.
///
/// [FR-DS-04]: ../../../docs/specs/requirements/FR-DS-04.md
/// [ADR-74]: ../../../docs/specs/architecture/decisions/ADR-74.md
/// [BR-63]: ../../../docs/specs/software-spec.md#320-distribution
pub trait InitContribution: Send + Sync {
    /// Run the step for the resolved project `root`, returning the rows it
    /// reports — none when it does not apply to `options`.
    ///
    /// # Errors
    /// An I/O failure writing an artifact the step owns.
    fn run(&self, root: &Path, options: &InitOptions) -> Result<Vec<InitStep>>;
}

// ── Generated content ──────────────────────────────────────────────────────

/// Starter `config.toml`: every key commented out, so the file deserialises
/// to exactly [`crate::config::Config::default`] (FR-CF-01 — the commented
/// values ARE the defaults; the template never drifts silently because the
/// init e2e test pins template == defaults).
const CONFIG_TEMPLATE: &str = r#"# Logos project configuration (.logos/config.toml) — checked-in policy that
# travels into every worktree (NFR-DM-04). Every key is optional; the
# commented values below ARE the built-in defaults.
# Reference: docs/howto/configuration.md

# Code-language admission allowlist (grammar names from `logos languages`).
# Omit it (or leave it empty) to index every compiled-in code language — the
# default; set it to restrict indexing to a subset (a narrowing purges the rest).
# languages = ["rust", "python"]

# Include/exclude globs, matched against root-relative paths. The default
# `exclude` prunes the planning/security/notes prose paths (CR-029/FR-CF-05)
# and minified JavaScript, `**/*.min.js` at any depth (CR-154); set
# `exclude = []` to re-admit them, or list your own globs to replace it (a list
# that omits `**/*.min.js` re-admits minified files). Add a directory of
# non-minified vendored scripts here to prune it too.
# include = ["**"]
# exclude = ["docs/planning/**", "docs/security/**", "notes/**", "**/*.min.js"]

# Files larger than this many bytes are skipped with a notice.
# max_file_size = 2097152

# Framework hints biasing route/component extraction.
# framework_hints = []

# [semantics]
# Directory names pruned anywhere in the tree during discovery — build outputs,
# agent/tooling dirs, per-language caches, and scratch dirs (CR-029/FR-CF-05,
# CR-054). Listing your own replaces this set wholesale.
# ignored_dirs = ["target", "node_modules", "dist", "build", "vendor", ".git", ".logos", ".agents", ".claude", "__pycache__", ".venv", "venv", ".tox", ".mypy_cache", ".pytest_cache", "bin", "obj", ".gradle", "out", "Pods", ".next", ".svelte-kit", "coverage", "cmake-build-debug", "cmake-build-release", ".worktrees", ".playwright-mcp"]
# Dead-code reachability roots (node names) on top of exports and routes.
# entry_points = ["main"]

# [resolution]
# Reference-binder aggressiveness: "strict" | "balanced" | "aggressive".
# policy = "balanced"

# [watcher]
# Debounce window (ms) for the `serve --mcp` filesystem watcher.
# debounce_ms = 300

# [documentation]
# Markdown documentation indexing (CR-003). On by default; docs ride the same
# discover/extract/sync pipeline as code. `include`/`exclude` are anchored
# globs, so `*.md` is top-level only and `README*` is the root README.
# enabled = true
# include = ["docs/**/*.md", "*.md", "README*"]
# exclude = []
"#;

/// Starter `rules.toml`: an empty (everything-optional) architecture contract
/// with commented examples — parses to [`crate::config::Rules::default`].
const RULES_TEMPLATE: &str = r#"# Logos architecture contract (.logos/rules.toml) — checked-in policy
# enforced by `logos check` and the quality gate (FR-GV-01..05). Everything
# is optional: an omitted constraint is simply not enforced.
# Reference: docs/howto/configuration.md

# [constraints]
# max_cycles     = 0    # maximum allowed dependency cycles
# max_cc         = 15   # maximum cyclomatic complexity per function
# max_fn_lines   = 100  # maximum lines per function
# no_god_files   = 50   # maximum symbols per file
# max_fan_in     = 30   # max inbound dependency edges for any one symbol (FR-GV-11)
# max_fan_out    = 30   # max outbound dependency edges for any one symbol (FR-GV-11)
# max_dead       = 0    # max project-wide dead functions, absolute (FR-GV-11)
#   # — or, delta-from-blessed-baseline (CR-043/ADR-39): fail only when the dead
#   # count rises above the blessed steady-state. Re-bless `baseline` after the
#   # dead-code count legitimately changes (the gate-baseline re-bless discipline).
# max_dead       = { baseline = 0, delta = 0 }
# max_duplicates = 0    # max project-wide duplicate functions (FR-GV-11)

# Ordered layers: a higher `order` may not depend on a lower one. Files not
# matching any layer are exempt from layer ordering (DL-05).
# [[layers]]
# name  = "domain"
# paths = ["src/domain/**"]
# order = 0
#
# [[layers]]
# name  = "infrastructure"
# paths = ["src/infra/**"]
# order = 1

# Forbidden dependencies between named layers, reported with the reason.
# [[boundaries]]
# from   = "domain"
# to     = "infrastructure"
# reason = "the domain stays persistence-free"

# Glob-level import bans (FR-GV-12): any import/reference edge from a `from`-glob
# file into a `to`-glob file is a violation. Globs match file paths, not layer
# names. v1 covers resolved intra-workspace edges.
# [[forbidden_imports]]
# from   = "src/web/**"
# to     = "src/db/**"
# reason = "the web layer must not import the db directly"

# Git-history analytics tuning (CR-006, FR-GH-03..05). These keys tune the
# NON-GATED temporal tier only (`logos hotspots`) — they never affect `check`
# or the quality gate (BR-26). All optional; shown with their defaults.
# [history]
# window_months              = 12   # HEAD-anchored window length, in calendar months
# co_change_max_commit_files = 50   # commits touching more files are skipped for co-change pairing only
# defect_patterns            = ["(?i)\\bfix(es|ed)?\\b", "(?i)\\bbug\\b", "(?i)\\bhotfix\\b"]

# Coverage-ingestion tuning (CR-007, FR-CV-03/09). Tunes the NON-GATED advisory
# evidence tier only (`logos coverage`) — it never affects `check` or the quality
# gate (BR-28). `path_strip_prefixes` are stripped from coverage-report paths
# before they map to indexed files, so absolute build-dir paths bind.
# [coverage]
# path_strip_prefixes = ["/home/runner/work/myproject/", "/build/"]
"#;

/// Managed-block markers. Hash-comment flavour for `.gitignore` and hook
/// scripts; HTML-comment flavour for `CLAUDE.md` (invisible when rendered).
const GI_BEGIN: &str = "# logos:managed:begin";
const GI_END: &str = "# logos:managed:end";
const MD_BEGIN: &str = "<!-- logos:managed:begin -->";
const MD_END: &str = "<!-- logos:managed:end -->";

/// The `.logos/.gitignore` managed block (FR-IN-04, SRS §12): derived and
/// machine-specific state is ignored; the checked-in policy (`config.toml`,
/// `rules.toml`, this `.gitignore`) stays tracked. `logos.db*` covers the
/// `-wal`/`-shm` sidecars (the task's `.logos/*.db` / `.logos/*.db-*` shape,
/// relative to `.logos/`).
///
/// `secrets.toml` (S-169, [FR-CF-06], [NFR-SE-07]) is the **one non-derived file
/// that is still gitignored**: it holds the chat API key — the first secret
/// Logos stores — so unlike the checked-in policy it must never be committed nor
/// travel into worktrees.
///
/// [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
/// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
const GITIGNORE_BLOCK: &str = "\
# logos:managed:begin — regenerated by `logos init`; edit outside this block
logos.db*
telemetry.db*
history.db*
wiki.db*
chat.db*
baseline.json
history.jsonl
logs/
# Security scan output: SARIF reports (which can quote a secret) and CodeQL
# databases (S-665, ADR-80).
security/
# The chat API key (FR-CF-06, NFR-SE-07) — a secret, never committed.
secrets.toml
# The branch review's merge-base before-states, cached by commit (FR-GV-24).
review-cache/
# logos:managed:end
";

/// Header written above the managed block on first generation (outside the
/// markers, so a user may edit or remove it freely).
const GITIGNORE_HEADER: &str = "\
# Generated by `logos init` (FR-IN-04): derived/machine-specific state is
# ignored; the checked-in policy (config.toml, rules.toml) travels.
";

/// Header written above the **workspace-root** managed block on first
/// generation ([FR-WS-02], [CR-104]) — the root flavour of
/// [`GITIGNORE_HEADER`], and outside the markers for the same reason.
///
/// Worded for where it lands: a reader who finds this at a federated parent is
/// not standing in a repository Logos initialised, so it names the command that
/// maintains it rather than pointing at a `.logos/` that is not there.
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
/// [CR-104]: ../../../docs/requests/CR-104-managed-workspace-root-ignore-for-the-warm-sidecar.md
const WORKSPACE_GITIGNORE_HEADER: &str = "\
# Generated by `logos init --workspace` (FR-WS-02): the background warm's
# per-member outcome record and the workspace chat's conversations (FR-WS-34)
# are host-local state, and the workspace chat credential (FR-WS-30) is a
# secret — none of them travels.
";

/// The `.logos/` targets [`run`] writes that [`GITIGNORE_BLOCK`] deliberately
/// does **not** ignore: the checked-in policy that travels with the repository
/// ([FR-IN-04]). Root-relative, spelled exactly as the [`InitStep::target`] each
/// one reports, so a caller may match the two without normalising.
///
/// A guarded duplicate, not a silent one: `committed_policy_is_what_run_writes_
/// and_the_block_leaves_tracked` below asserts every entry is a real step target
/// of a real `run` *and* that none of them is ignored, so drifting this list or
/// the block apart fails the suite rather than the operator.
///
/// [FR-IN-04]: ../../../docs/specs/requirements/FR-IN-04.md
const COMMITTED_POLICY: [&str; 3] = [
    ".logos/config.toml",
    ".logos/rules.toml",
    ".logos/.gitignore",
];

/// [FR-IN-04]'s committed half: what an operator is meant to commit inside a
/// freshly created `.logos/`.
///
/// Exposed as data so a report can *state* the layout instead of asking the
/// reader to look it up — see
/// [`federation::enable::WorkingTreeFootprint`](crate::federation::enable::WorkingTreeFootprint),
/// whose whole reason to exist is that enabling N members left N dirty
/// repositories and said nothing about them.
///
/// [FR-IN-04]: ../../../docs/specs/requirements/FR-IN-04.md
#[must_use]
pub(crate) fn committed_policy() -> Vec<&'static str> {
    COMMITTED_POLICY.to_vec()
}

/// [FR-IN-04]'s ignored half: everything the generated `.logos/.gitignore`
/// keeps out of git, read out of the very block `init` writes rather than
/// restated.
///
/// Mostly derived/machine-specific state, but **not** only that: `secrets.toml`
/// is ignored because it is a secret ([FR-CF-06], [NFR-SE-07]), not because it
/// is regenerable. So the name here — and the wording a caller renders — is
/// "ignored", not "derived": a report telling an operator to leave the *derived*
/// state to git-ignore, while listing a secret among it, would contradict the
/// distinction [`GITIGNORE_BLOCK`] draws one item away.
///
/// Parsed, not duplicated — the block is the single source of truth, so an
/// entry added to it (as `wiki.db*` and `chat.db*` were) is explained to the
/// operator on the next run with no second edit here.
///
/// [FR-IN-04]: ../../../docs/specs/requirements/FR-IN-04.md
/// [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
/// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
#[must_use]
pub(crate) fn ignored_state() -> Vec<&'static str> {
    GITIGNORE_BLOCK
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// The managed `CLAUDE.md` block (FR-IN-02, FR-IN-09): the graph-first usage
/// steer — the project-memory twin of the MCP `server-instructions`.
///
/// Ordered **scoping first, navigation second** since S-362/CR-114. The two
/// texts are deliberately parallel: an agent that reads only one of them should
/// come away with the same ordering, so `mcp/src/instructions.md` moves with
/// this constant (`cli/src/lib.rs`'s `surface_parity` module asserts each of
/// the three satisfies [FR-IN-09]).
///
/// The ordering is a hypothesis, not a finding, and the block says so — the
/// evidence is one project's telemetry, and [FR-OB-11]'s tool class is what
/// re-measures it ([NFR-CC-04]: an explicit caveat over a confident claim).
///
/// Its relational claims are scoped by language ([CR-143] §3.8): the
/// "Where the relational claims hold" paragraph is one statement all three
/// texts carry verbatim, pointing at the resolution denominator rather than
/// naming languages, and `surface_parity`'s `RELATIONAL_SCOPE` is the wording
/// each is checked against — edit it there and in all three, or none.
///
/// [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [CR-143]: ../../../docs/requests/CR-143-a-relational-answer-states-its-resolution-denominator.md
const CLAUDE_MD_BLOCK: &str = "\
<!-- logos:managed:begin -->
## Logos — structural code intelligence

This project is indexed by Logos. The graph's primary use is **scoping work before
you decompose it**. Navigating while you code is the secondary mode.

### Primary — before you decompose the work

Each tool's own description says what it returns; these are the moments to ask.

- **What does this task touch?** — `logos:context` with the task in one sentence.
  The first move of a task, not a mid-edit lookup.
- **Can these two work items run in parallel?** — `logos:impact_intersection`.
  Split colliding items across iterations instead of colliding at merge.
- **Has this already been done here?** — `logos:precedent` on what you are about
  to write. Follow the sibling rather than inventing a second spelling.
- **How large is the change really?** — `logos:impact` for one symbol;
  `logos affected <file>…` for a changed set, the pre-review reach check.
- **Which branches collide, and did the merge carry everything?** —
  `logos:branch_overlap`. A clean merge is not a complete merge.

### Secondary — while you are editing

- **Relational / cross-file** (\"who calls this?\", \"where is X used?\", dead code)
  — `logos:node` / `logos:callers` / `logos:callees`, and `logos:search` /
  `logos:explore` to find and group, on the languages the graph resolves (below).
- **Localized lookups** (a string, a value, a formula inside a file you can already
  name) — a direct read or grep is fine, sometimes faster. Don't force the graph on
  a question grep already answers.
- **Disambiguate by symbol** — prefer a unique name as the entry point. `logos:node`
  on a common bare name (`new`, `map`, `severity`) resolves to one arbitrary match;
  pivot from a unique caller or qualify the path instead.

**Where the relational claims hold.** Relational answers hold only on the languages
the graph resolves. The answers that traverse resolved edges — callers, callees,
impact, affected, precedent, impact intersection and branch overlap — say which:
each carries a `resolution_denominator` whose `languages` rows give each anchor
language's `calls` resolution. A node answer's edge list carries none, so read it
against the project's rows named below. Where a row has a `cross_file_edges` figure,
the graph beats grep for that language, as completely as its `bound` share of
`references` — a partial ratio is a partial answer. Where its `cross_file_absence`
reads `same-file-only` or `no-resolved-edges`, the graph binds no call across a file
boundary there: an empty or short answer is the index's reach, not evidence of
absence, and grep is the better tool. Callers can live in another language than
their target, so read the whole project's rows as well — `resolution_by_language` in
the status readout. For this project that is `logos status --json`.

This ordering is a **hypothesis under measurement, not a settled finding** — it comes
from one project's telemetry. `logos stats --json` breaks calls down by tool class
and origin; read the figure for THIS project there.

Wrap editing sessions in the quality gate: `logos:session_start` before edits,
`logos:session_end` after — on a failing gate, stop and fix the regression before
piling on more changes. Run `logos:check_rules` before declaring any task done.
Without an MCP host the same gate is `logos session-start` / `logos session-end`
(a failing gate exits 1) and `logos check`.

The full quality loop has four moves — **freshen** (index/sync so the graph matches
the code), **enforce** (`logos check` blocks regressions; the `pre-push` gate runs
it), **report** (`logos scan` surfaces the 0–10000 signal; the session-start hook
prints it), and **bless** (`logos gate --save` records a new baseline, at release only). The
copy-pasteable CI recipe is `docs/howto/ci-integration.md`.

Every tool has a CLI twin (`logos impact-intersection`, `logos precedent`,
`logos branch-overlap`, …) with `--json` output, with one deliberate exception:
`rescan` is MCP-only, because it replays the parameters of the last scan **in the
same server process** and a one-shot CLI invocation has no earlier scan to replay —
run `logos scan` instead.

This block is managed by `logos init -i`: edits inside the markers are regenerated on
re-run; content outside the markers is never touched.
<!-- logos:managed:end -->
";

/// Root-relative hooks directory wired into `core.hooksPath`.
/// Sourced from [`crate::hooks`] to avoid a duplicate constant that could drift.
use crate::hooks::HOOKS_RELDIR;

// ── Orchestration ──────────────────────────────────────────────────────────

/// Run every requested init step against `root`, in a fixed order, returning
/// one [`InitStep`] per target.
///
/// # Errors
/// Only on I/O failures writing Logos-owned artifacts; judgment-call refusals
/// surface as `Skipped` steps instead (DL-06, DL-07).
pub(crate) fn run(
    root: &Path,
    options: &InitOptions,
    owned_skills: &[String],
) -> Result<Vec<InitStep>> {
    let mut steps = vec![
        write_if_absent(root, ".logos/config.toml", CONFIG_TEMPLATE)?,
        append_project_id_if_absent(root)?,
        write_if_absent(root, ".logos/rules.toml", RULES_TEMPLATE)?,
        logos_dir_gitignore(root)?,
    ];
    // FR-IN-12: read only when a step it changes was asked for, so a plain
    // `init` never looks at the Claude Code settings.
    let plugin = (options.inject_mcp || options.install_quality_report_hook)
        && options.claude_plugin.is_enabled(root);
    if options.inject_mcp && plugin {
        steps.push(plugin::enrol(root)?);
    }
    // The legacy registrations are committed, so they go only once the
    // committed settings enable the plugin: otherwise a teammate without it
    // would be left with neither, and their next `init -i` would put them back.
    let retire = plugin && plugin::committed(root);
    if options.inject_mcp {
        steps.push(match (plugin, retire) {
            (false, _) => inject_mcp(root)?,
            (true, true) => retire::mcp_entry(root)?,
            (true, false) => plugin::legacy_kept(".mcp.json"),
        });
    }
    if options.write_claude_md {
        steps.push(upsert_block_file(
            root,
            "CLAUDE.md",
            "",
            MD_BEGIN,
            MD_END,
            CLAUDE_MD_BLOCK,
        )?);
    }
    if options.install_hooks {
        steps.push(install_hooks(root)?);
    }
    if options.install_quality_report_hook {
        steps.push(match (plugin, retire) {
            (false, _) => materialize_quality_report_hook(root)?,
            (true, true) => retire::readout_hook(root)?,
            (true, false) => plugin::legacy_kept(crate::hooks::QUALITY_REPORT_HOOK_SCRIPT_REL),
        });
    }
    // A composed extension that materializes skills owns them (FR-WK-08 as
    // modified by CR-218), so only a run not asked to materialize one removes
    // the copy an earlier core `init -i` wrote — and only a skill the core
    // itself emitted, never one an extension owns (HF-10).
    if retire && !options.materialize_skill {
        steps.extend(retire::emitted_skills(root, |name| {
            retire::legacy_core_skill(name) && !owned_skills.iter().any(|o| o == name)
        })?);
    }
    Ok(steps)
}

/// Materialize the Claude Code session-start quality-report hook ([FR-IN-07],
/// [FR-GV-02], [FR-GV-05], [ADR-49], [CR-055], [CR-095]), default-on under `-i`
/// alongside the embedded skill. Delegates to the [`crate::hooks`] materializer —
/// the sole owner of the hook artifacts — and maps the
/// [`crate::hooks::HookEmitSummary`] onto an [`InitStep`] targeting
/// `.claude/settings.json`. Non-clobbering: an already-present managed entry is
/// `Unchanged`; a foreign settings file is `Skipped` with the reason, never
/// overwritten.
///
/// A sweep of the retired SessionEnd hook ([CR-095]) is reported in the step
/// detail rather than happening silently — it is the one case where `init`
/// *removes* a line it previously wrote into `.claude/settings.json`, so it
/// should be visible in the output.
///
/// [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
/// [ADR-49]: ../../../docs/specs/architecture/decisions/ADR-49.md
/// [CR-095]: ../../../docs/requests/CR-095-session-start-quality-readout.md
fn materialize_quality_report_hook(root: &Path) -> Result<InitStep> {
    use crate::hooks::EmitAction;
    // Merging into a pre-existing settings file is an Updated, not a Created —
    // the same convention as the `.mcp.json` injection above.
    let settings_existed = root.join(crate::hooks::SETTINGS_REL).exists();
    // `init -i` never clobbers: unforced, so an existing managed entry skips.
    let summary = crate::hooks::materialize_quality_report_hook(root, false)?;
    let (action, detail) = match (summary.action, &summary.notice) {
        (EmitAction::Created, _) => (
            if settings_existed {
                InitAction::Updated
            } else {
                InitAction::Created
            },
            format!("session-start quality-report hook → {}", summary.script),
        ),
        (EmitAction::Forced, _) => {
            (InitAction::Updated, "quality-report hook re-emitted".to_string())
        }
        // `init -i` never forces, so this is the variant a re-run on an older
        // install actually reports: the entry was there but its shape had
        // drifted, and Logos corrected it ([CR-095]). Naming that distinctly
        // matters here — "re-emitted" would read as "your edits were replaced".
        (EmitAction::Reconciled, _) => (
            InitAction::Updated,
            "quality-report hook entry reconciled to the current shape".to_string(),
        ),
        (EmitAction::Skipped, Some(reason)) => (InitAction::Skipped, reason.clone()),
        (EmitAction::Skipped, None) => (
            InitAction::Unchanged,
            "already present — never overwritten; `logos hook --emit --force` refreshes"
                .to_string(),
        ),
    };
    // A sweep can accompany any outcome above (including `Unchanged`, when only
    // an orphaned script was left on disk), so it is appended, not substituted.
    let detail = if summary.retired_removed.is_empty() {
        detail
    } else {
        format!(
            "{detail}; removed the retired SessionEnd quality-report hook ({})",
            summary.retired_removed.join(", ")
        )
    };
    Ok(step(&summary.settings, action, detail))
}

fn step(target: &str, action: InitAction, detail: impl Into<String>) -> InitStep {
    InitStep {
        target: target.to_string(),
        action,
        detail: detail.into(),
    }
}

/// FR-IN-01: a policy template is written once and never again — an existing
/// file is the user's, whatever its content.
fn write_if_absent(root: &Path, rel: &str, content: &str) -> Result<InitStep> {
    let path = root.join(rel);
    if path.exists() {
        return Ok(step(
            rel,
            InitAction::Unchanged,
            "already present — never overwritten",
        ));
    }
    fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    Ok(step(rel, InitAction::Created, ""))
}

/// The [`InitStep::target`] of the `[project] id` step — a key inside
/// `config.toml`, reported apart from the file's own row.
pub(crate) const PROJECT_ID_TARGET: &str = ".logos/config.toml [project] id";

/// [FR-CF-10]: append `[project] id` to `config.toml` when the key is absent,
/// set to the derived coordinate ([`crate::config::derive_project_id`]). A
/// present `id` is the user's and is never touched — not even reformatted — so
/// a re-run leaves the file byte-identical. Everything already in the file
/// stays byte-identical too: the key is appended after it, or inserted on the
/// line after an existing, key-less `[project]` header.
///
/// A file that cannot be read as text or is not valid TOML, or holds `project`
/// in a shape the step cannot extend safely (an inline table, a non-table
/// value), is `Skipped` with the reason (DL-06) — never guessed at.
///
/// When `config.toml` is git-ignored the written id cannot reach another
/// checkout, and the row says so: elsewhere the derived default applies.
///
/// [FR-CF-10]: ../../../docs/specs/requirements/FR-CF-10.md
fn append_project_id_if_absent(root: &Path) -> Result<InitStep> {
    let path = root.join(".logos/config.toml");
    // The file is the user's: one that cannot be read as text (TOML is UTF-8)
    // is reported, never failed on, so every later step still runs (DL-06).
    let existing = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            return Ok(step(
                PROJECT_ID_TARGET,
                InitAction::Skipped,
                format!("config.toml could not be read, so `[project] id` was not written: {err}"),
            ))
        }
    };
    let derived = crate::config::derive_project_id(root);
    let travel = project_id_travel_notice(root, &derived);
    let table: toml::Table = match toml::from_str(&existing) {
        Ok(table) => table,
        Err(err) => {
            return Ok(step(
                PROJECT_ID_TARGET,
                InitAction::Skipped,
                format!("config.toml is not valid TOML, so `[project] id` was not written: {err}"),
            ))
        }
    };
    let updated = match table.get("project") {
        None => Some(format!(
            "{existing}{}{}",
            if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" },
            project_block(&derived)
        )),
        Some(toml::Value::Table(project)) if project.contains_key("id") => {
            let valid = project
                .get("id")
                .and_then(toml::Value::as_str)
                .is_some_and(crate::config::is_valid_coordinate);
            return Ok(if valid {
                step(
                    PROJECT_ID_TARGET,
                    InitAction::Unchanged,
                    format!("already present — never overwritten{travel}"),
                )
            } else {
                step(
                    PROJECT_ID_TARGET,
                    InitAction::Skipped,
                    "the present `[project] id` is not a valid coordinate (letters, digits \
                     and `._/@:-`, no spaces); it was left as is — fix it by hand",
                )
            });
        }
        Some(toml::Value::Table(_)) => insert_after_project_header(&existing, &derived),
        Some(_) => None,
    };
    // Write only what re-reads as exactly the derived id, so no input shape this
    // step mis-edits can ever reach the disk.
    let verified = updated.filter(|text| {
        toml::from_str::<toml::Table>(text).ok().is_some_and(|t| {
            t.get("project").and_then(|p| p.get("id")).and_then(toml::Value::as_str)
                == Some(derived.as_str())
        })
    });
    let Some(updated) = verified else {
        return Ok(step(
            PROJECT_ID_TARGET,
            InitAction::Skipped,
            format!(
                "`project` in config.toml is not a `[project]` table this step can extend; \
                 add `id = \"{derived}\"` to it by hand"
            ),
        ));
    };
    crate::config::atomic::publish(&path, updated.as_bytes(), None)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(step(
        PROJECT_ID_TARGET,
        InitAction::Updated,
        format!("id = \"{derived}\"{travel}"),
    ))
}

/// The appended `[project]` table, with the comment that says what it is.
fn project_block(id: &str) -> String {
    format!(
        "\n# Repository identity coordinate (FR-CF-10). Written once by `logos init` from\n\
         # the `origin` remote (else the main worktree's directory name) and never\n\
         # overwritten; edit it to pin a different coordinate.\n\
         [project]\nid = \"{id}\"\n"
    )
}

/// `existing` with `id = "<id>"` on the line after its `[project]` header, or
/// `None` when no such header line exists (the table is spelled another way).
fn insert_after_project_header(existing: &str, id: &str) -> Option<String> {
    let mut offset = 0;
    for line in existing.split_inclusive('\n') {
        let header = line.trim_start();
        let is_project = header.strip_prefix("[project]").is_some_and(|rest| {
            let rest = rest.trim();
            rest.is_empty() || rest.starts_with('#')
        });
        offset += line.len();
        if is_project {
            let newline = if line.ends_with('\n') { "" } else { "\n" };
            return Some(format!(
                "{}{newline}id = \"{id}\"\n{}",
                &existing[..offset],
                &existing[offset..]
            ));
        }
    }
    None
}

/// The clause an init row carries when `.logos/config.toml` is git-ignored: the
/// id stays on this machine, and other checkouts derive the default. Empty when
/// the file is tracked or committable, or `root` is not in a git repository.
fn project_id_travel_notice(root: &Path, derived: &str) -> String {
    let ignored = git(root, &["check-ignore", "-q", ".logos/config.toml"])
        .is_some_and(|out| out.status.success());
    if ignored {
        format!(
            "; .logos/config.toml is git-ignored, so this id will not travel — other \
             checkouts and CI use the derived default (\"{derived}\")"
        )
    } else {
        String::new()
    }
}

// ── Managed blocks (DL-07) ─────────────────────────────────────────────────

/// Outcome of a marker-delimited upsert against existing file content.
#[derive(Debug, PartialEq, Eq)]
enum Upsert {
    /// The managed block is already byte-identical.
    Unchanged,
    /// The block was regenerated in place (or appended); user content
    /// outside the markers is preserved verbatim.
    Write(String),
    /// A begin marker without its end marker: refuse to guess (DL-07).
    Malformed,
}

/// Regenerate exactly the `begin..end` marker span inside `existing`,
/// appending the block (blank-line separated) when no markers are present.
///
/// `block` carries its own markers and trailing newline. Matching is by
/// marker *prefix* at line start, so a begin line may carry a trailing
/// human note.
fn upsert_managed_block(existing: &str, begin: &str, end: &str, block: &str) -> Upsert {
    let (begin_at, span_end) = match managed_span(existing, begin, end) {
        ManagedSpan::Malformed => return Upsert::Malformed,
        ManagedSpan::At { begin_at, span_end } => (begin_at, span_end),
        ManagedSpan::Absent => {
            let mut out = existing.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(block);
            return Upsert::Write(out);
        }
    };
    let current = &existing[begin_at..span_end];
    if current == block {
        return Upsert::Unchanged;
    }
    Upsert::Write(format!(
        "{}{}{}",
        &existing[..begin_at],
        block,
        &existing[span_end..]
    ))
}

/// Where the `begin..end` managed block sits in `text` — the one reading both
/// the writer ([`upsert_managed_block`]) and the remover
/// ([`retire::claude_md_block`]) use, so they never disagree about which bytes
/// are Logos's.
#[derive(Debug, PartialEq, Eq)]
enum ManagedSpan {
    /// No markers at all.
    Absent,
    /// The block's byte range: from the begin marker to the end of the end
    /// marker's line, its newline included.
    At { begin_at: usize, span_end: usize },
    /// A begin marker without its end marker, or the reverse: refuse to guess
    /// (DL-07).
    Malformed,
}

fn managed_span(text: &str, begin: &str, end: &str) -> ManagedSpan {
    let Some(begin_at) = find_marker(text, begin) else {
        return if find_marker(text, end).is_some() {
            ManagedSpan::Malformed
        } else {
            ManagedSpan::Absent
        };
    };
    let Some(end_at) = find_marker(&text[begin_at..], end).map(|i| begin_at + i) else {
        return ManagedSpan::Malformed;
    };
    let span_end = text[end_at..].find('\n').map_or(text.len(), |i| end_at + i + 1);
    ManagedSpan::At { begin_at, span_end }
}

/// Find a marker as a line *prefix* (start of content or right after `\n`).
fn find_marker(text: &str, marker: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = text[from..].find(marker).map(|i| from + i) {
        if i == 0 || text.as_bytes()[i - 1] == b'\n' {
            return Some(i);
        }
        from = i + marker.len();
    }
    None
}

/// Create-or-refresh a file whose Logos-owned content lives in one managed
/// block (`.logos/.gitignore`, `CLAUDE.md`): fresh file = `header` + block;
/// existing file = upsert the block, preserving everything else.
fn upsert_block_file(
    root: &Path,
    rel: &str,
    header: &str,
    begin: &str,
    end: &str,
    block: &str,
) -> Result<InitStep> {
    let path = root.join(rel);
    let Some(existing) = read_optional(&path)? else {
        fs::write(&path, format!("{header}{block}"))
            .with_context(|| format!("writing {}", path.display()))?;
        return Ok(step(rel, InitAction::Created, ""));
    };
    match upsert_managed_block(&existing, begin, end, block) {
        Upsert::Unchanged => Ok(step(rel, InitAction::Unchanged, "")),
        Upsert::Write(content) => {
            fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
            Ok(step(rel, InitAction::Updated, "managed block regenerated"))
        }
        Upsert::Malformed => Ok(step(
            rel,
            InitAction::Skipped,
            "managed-block markers are unbalanced — left untouched",
        )),
    }
}

/// Maintain `<root>/.logos/.gitignore` — the [`GITIGNORE_BLOCK`] that keeps every
/// derived artefact and the credential store out of version control ([FR-IN-04],
/// [NFR-SE-07]). [`run`] writes it for a member; the workspace-tier config
/// writers write it for the workspace root's own `.logos/` ([FR-WS-30]), because
/// that directory is created by the first workspace save rather than by `init`,
/// and the ignore must not depend on whether — or with which build — `logos init
/// --workspace` last ran. The `.logos/` directory must already exist.
///
/// Self-contained on purpose: git honours a nested `.gitignore` whichever
/// repository encloses it, so it covers a root that is its own repository, one
/// nested inside an enclosing repository, and a root with no repository at all.
///
/// [FR-IN-04]: ../../../docs/specs/requirements/FR-IN-04.md
/// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
/// [FR-WS-30]: ../../../docs/specs/requirements/FR-WS-30.md
pub(crate) fn logos_dir_gitignore(root: &Path) -> Result<InitStep> {
    upsert_block_file(
        root,
        ".logos/.gitignore",
        GITIGNORE_HEADER,
        GI_BEGIN,
        GI_END,
        GITIGNORE_BLOCK,
    )
}

/// Maintain the **workspace-root** `.gitignore` managed block ignoring each of
/// `patterns` ([FR-WS-02], [FR-WS-17], [FR-WS-30], [CR-104]).
///
/// The same non-clobbering writer [`run`] uses for `.logos/.gitignore`, reached
/// through the same markers — not a second implementation. The relationship is
/// [`inject_mcp_entry`]'s exactly: one writer owns create-or-append, delimiter
/// matching and idempotency, and the *content* is supplied by whichever caller
/// knows it. Here that caller is
/// [`crate::federation::enable::enable`], which passes
/// [`OUTCOME_FILENAME`](crate::federation::warm_state::OUTCOME_FILENAME), the
/// credential store's own relative path and the workspace chat store's pattern,
/// so each ignored name has exactly one spelling in the tree ([NFR-MA-02]).
///
/// A workspace root is a **shared, human-authored** directory in a way a
/// generated `.logos/` is not — `logos.workspace.toml` lives there and is meant
/// to be committed — so the clobber risk is higher and the block discipline
/// matters more, not less: everything outside the markers is preserved
/// byte-for-byte and a settled re-run reports `Unchanged`.
///
/// # Errors
/// On I/O failure **reading or writing** the file — two of the three reachable
/// failures are reads, not writes: an existing `.gitignore` that is a directory,
/// and one that is not valid UTF-8, both fail in [`read_optional`] before
/// anything is written. An existing file whose markers are unbalanced is not a
/// failure at all: it is `Skipped` with the reason rather than guessed at
/// (DL-07).
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
/// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
/// [FR-WS-30]: ../../../docs/specs/requirements/FR-WS-30.md
/// [NFR-MA-02]: ../../../docs/specs/requirements/NFR-MA-02.md
/// [CR-104]: ../../../docs/requests/CR-104-managed-workspace-root-ignore-for-the-warm-sidecar.md
pub(crate) fn workspace_root_gitignore(root: &Path, patterns: &[&str]) -> Result<InitStep> {
    // Root-**anchored** (`/`), which matters here in a way it does not inside a
    // generated `.logos/`. An unanchored basename matches at every depth below
    // the file, and a workspace root's depth is the whole estate: a member
    // admitted by `.logos/logos.db` alone carries no `.git` of its own
    // (`federation::discover_candidates`), so it shares this repository and an
    // unanchored entry would reach inside it and hide a same-named file there.
    // A git-root member is shielded by its own repository boundary; that member
    // is not. Each file this ignores exists at exactly one place — the sidecar
    // beside the manifest, the credential in the root's own `.logos/` — so each
    // pattern says exactly that.
    let entries: String = patterns.iter().map(|pattern| format!("/{pattern}\n")).collect();
    let block = format!(
        "{GI_BEGIN} — regenerated by `logos init --workspace`; edit outside this block\n\
         {entries}\
         {GI_END}\n"
    );
    upsert_block_file(
        root,
        ".gitignore",
        WORKSPACE_GITIGNORE_HEADER,
        GI_BEGIN,
        GI_END,
        &block,
    )
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(path)
        .map(Some)
        .with_context(|| format!("reading {}", path.display()))
}

// ── MCP host config injection (FR-IN-02) ───────────────────────────────────

/// The `logos` server entry per the locked operating model (SRS §10.1): the
/// host launches `logos serve --mcp` rooted at the project. Shared with the
/// workspace-wide entry `logos init --workspace` injects at the parent
/// ([`crate::federation::enable`], FR-WS-02) — same command, different
/// `.mcp.json` key.
pub(crate) fn mcp_server_entry() -> Value {
    json!({ "command": "logos", "args": ["serve", "--mcp"] })
}

/// Inject the `logos` block into the project `.mcp.json` (the file-based
/// host-config flavour of the [MCP Host] integration): create the file if
/// absent, add the key if missing, and *skip* if a `logos` entry already
/// exists or the file cannot be parsed safely (FR-IN-02 non-clobber).
///
/// [MCP Host]: ../../../docs/specs/architecture/integrations/mcp-host.md
fn inject_mcp(root: &Path) -> Result<InitStep> {
    inject_mcp_entry(root, "logos", mcp_server_entry())
}

/// Shared `.mcp.json` server-entry injector (FR-IN-02, FR-WS-02): create the
/// file if absent, add `entry` under `server_key` if missing, and *skip* if
/// that key is already present or the file cannot be parsed safely
/// (non-clobber). [`inject_mcp`] is the per-repo `"logos"` caller;
/// [`crate::federation::enable::enable`] is the workspace-wide
/// `"logos-workspace"` caller.
///
/// [MCP Host]: ../../../docs/specs/architecture/integrations/mcp-host.md
pub(crate) fn inject_mcp_entry(root: &Path, server_key: &str, entry: Value) -> Result<InitStep> {
    const TARGET: &str = ".mcp.json";
    let path = root.join(TARGET);
    let Some(text) = read_optional(&path)? else {
        let value = json!({ "mcpServers": { server_key: entry } });
        write_json(&path, &value)?;
        return Ok(step(TARGET, InitAction::Created, ""));
    };
    let Ok(mut config) = serde_json::from_str::<Value>(&text) else {
        return Ok(step(
            TARGET,
            InitAction::Skipped,
            format!(
                "existing .mcp.json is not valid JSON — left untouched; \
                 add the `{server_key}` server entry manually"
            ),
        ));
    };
    let Some(servers) = config
        .as_object_mut()
        .map(|o| o.entry("mcpServers").or_insert_with(|| json!({})))
        .and_then(Value::as_object_mut)
    else {
        return Ok(step(
            TARGET,
            InitAction::Skipped,
            "existing .mcp.json does not hold an `mcpServers` object — left untouched",
        ));
    };
    if servers.contains_key(server_key) {
        return Ok(step(
            TARGET,
            InitAction::Unchanged,
            format!("a `{server_key}` server entry is already present — left untouched"),
        ));
    }
    servers.insert(server_key.to_string(), entry);
    write_json(&path, &config)?;
    Ok(step(
        TARGET,
        InitAction::Updated,
        format!("`{server_key}` server entry added"),
    ))
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value).context("serialising .mcp.json")?;
    text.push('\n');
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

// ── Git hooks (FR-IN-03, FR-SY-05) ─────────────────────────────────────────

fn git(root: &Path, args: &[&str]) -> Option<Output> {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()
}

fn stdout_line(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Install the sync hooks under `.logos/hooks/` and point `core.hooksPath`
/// at them. Refuses (Skipped) when: not a git repository, `core.hooksPath`
/// already points elsewhere, or an existing hook file is not Logos-managed —
/// the user's hook setup is never clobbered (DL-07). Logos-managed scripts are
/// rewritten to the current canonical body when they differ.
///
/// Hook scripts and the managed-marker constant are sourced from
/// [`crate::hooks`] so both code paths (the `init -i` wizard and the
/// `Engine::install_hooks` API) recognise each other's installations.
fn install_hooks(root: &Path) -> Result<InitStep> {
    const TARGET: &str = HOOKS_RELDIR;
    let in_repo = git(root, &["rev-parse", "--git-dir"]).is_some_and(|o| o.status.success());
    if !in_repo {
        return Ok(step(
            TARGET,
            InitAction::Skipped,
            "not a git repository (or `git` not on PATH) — hooks not installed",
        ));
    }
    let hooks_path = git(root, &["config", "core.hooksPath"])
        .filter(|o| o.status.success())
        .map(|o| stdout_line(&o));
    if let Some(existing) = hooks_path.as_deref() {
        if existing != HOOKS_RELDIR {
            return Ok(step(
                TARGET,
                InitAction::Skipped,
                format!("core.hooksPath already set to `{existing}` — left untouched"),
            ));
        }
    }

    let dir = root.join(HOOKS_RELDIR);
    // Whole-install veto: any non-managed hook file blocks the entire install
    // (DL-07). Uses the canonical marker from `crate::hooks` so a file written
    // by `Engine::install_hooks` is also recognised as managed.
    for hook in crate::hooks::all_hook_names() {
        if let Some(body) = read_optional(&dir.join(hook))? {
            if !body.contains(crate::hooks::MANAGED_MARKER) {
                return Ok(step(
                    TARGET,
                    InitAction::Skipped,
                    format!("existing non-managed hook `{HOOKS_RELDIR}/{hook}` — left untouched"),
                ));
            }
        }
    }

    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut wrote = false;
    let mut any_existed = false;
    for (hook, body) in crate::hooks::managed_scripts() {
        let path = dir.join(hook);
        let existing = read_optional(&path)?;
        any_existed |= existing.is_some();
        // Write when absent, or when a managed script (the whole-install veto
        // above already refused any foreign one) differs from the canonical
        // body: a managed script is regenerated to what this version of Logos
        // generates, exactly as `Engine::install_hooks` rewrites it, so a hook
        // written by an older release picks up a fixed body on the next `init`
        // instead of running the old one forever.
        if existing.as_deref() != Some(body.as_str()) {
            fs::write(&path, &body).with_context(|| format!("writing {}", path.display()))?;
            wrote = true;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o755);
            fs::set_permissions(&path, perms)
                .with_context(|| format!("marking {} executable", path.display()))?;
        }
    }
    if hooks_path.is_none() {
        let set = git(root, &["config", "core.hooksPath", HOOKS_RELDIR]);
        if !set.is_some_and(|o| o.status.success()) {
            // Hook files WERE written above, so this is a partial write, not
            // a refusal: `Skipped` is reserved for zero-write outcomes (its
            // documented "deliberately not touched" contract) — report
            // `Updated` and name the manual remedy.
            return Ok(step(
                TARGET,
                InitAction::Updated,
                "hooks written but core.hooksPath not set — \
                 run `git config core.hooksPath .logos/hooks` manually",
            ));
        }
        wrote = true;
    }

    // CR-106: `core.hooksPath` stays the relative `.logos/hooks`, which git
    // resolves against whichever working tree the command runs in — so every
    // OTHER working tree of this repository needs its own hooks directory or
    // the installation this step just reported fires nothing there. Seeding
    // runs on the unchanged path too: an existing installation predating this
    // behaviour has worktrees that were never seeded.
    let seeded = crate::hooks::seed_all_worktrees(root).len();
    let worktrees = match crate::hooks::linked_worktree_install_notice(root) {
        // Installed from inside a linked worktree: it covers this tree alone,
        // and saying so is not optional. `init --hooks` is the command users
        // actually run, so a silent partial installation here is exactly the
        // "every signal reads as success" failure CR-106 exists to close.
        Some(notice) => format!("; {notice}"),
        None => match seeded {
            0 => String::new(),
            1 => "; 1 linked worktree seeded".to_string(),
            n => format!("; {n} linked worktrees seeded"),
        },
    };

    Ok(match (wrote, any_existed) {
        (false, _) => step(TARGET, InitAction::Unchanged, worktrees.trim_start_matches("; ")),
        (true, false) => step(
            TARGET,
            InitAction::Created,
            format!("core.hooksPath → .logos/hooks{worktrees}"),
        ),
        (true, true) => step(
            TARGET,
            InitAction::Updated,
            format!("managed hooks regenerated{worktrees}"),
        ),
    })
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    const BLOCK: &str = "# logos:managed:begin\nbody\n# logos:managed:end\n";

    /// No markers: the block is appended, blank-line separated, and the
    /// existing content survives verbatim.
    #[test]
    fn upsert_appends_when_markers_are_absent() {
        let Upsert::Write(out) = upsert_managed_block("user\n", GI_BEGIN, GI_END, BLOCK) else {
            panic!("expected a write");
        };
        assert_eq!(out, format!("user\n\n{BLOCK}"));
        // …and onto an empty file the block stands alone.
        assert_eq!(
            upsert_managed_block("", GI_BEGIN, GI_END, BLOCK),
            Upsert::Write(BLOCK.to_string())
        );
    }

    /// Appending to a non-empty file WITHOUT a trailing newline first
    /// normalises the boundary (one `\n`), then blank-line-separates the
    /// block — the managed block never glues onto the user's last line.
    #[test]
    fn upsert_append_normalises_a_missing_trailing_newline() {
        let Upsert::Write(out) = upsert_managed_block("user", GI_BEGIN, GI_END, BLOCK) else {
            panic!("expected a write");
        };
        assert_eq!(out, format!("user\n\n{BLOCK}"));
    }

    /// Existing markers: only the span is regenerated; user content on both
    /// sides is preserved.
    #[test]
    fn upsert_replaces_only_the_marker_span() {
        let existing = "before\n# logos:managed:begin\nstale\n# logos:managed:end\nafter\n";
        let Upsert::Write(out) = upsert_managed_block(existing, GI_BEGIN, GI_END, BLOCK) else {
            panic!("expected a write");
        };
        assert_eq!(out, format!("before\n{BLOCK}after\n"));
    }

    /// A byte-identical block is recognised — no write, no churn.
    #[test]
    fn upsert_detects_an_identical_block() {
        let existing = format!("before\n{BLOCK}after\n");
        assert_eq!(
            upsert_managed_block(&existing, GI_BEGIN, GI_END, BLOCK),
            Upsert::Unchanged
        );
    }

    /// Unbalanced markers: refuse to guess (DL-07 non-clobber).
    #[test]
    fn upsert_refuses_unbalanced_markers() {
        assert_eq!(
            upsert_managed_block("# logos:managed:begin\nno end\n", GI_BEGIN, GI_END, BLOCK),
            Upsert::Malformed
        );
        assert_eq!(
            upsert_managed_block("# logos:managed:end\nno begin\n", GI_BEGIN, GI_END, BLOCK),
            Upsert::Malformed
        );
    }

    /// Markers match only at line starts — an indented or quoted mention is
    /// not a marker (the fuzzy-match trap).
    #[test]
    fn markers_match_only_at_line_start() {
        assert_eq!(find_marker("  # logos:managed:begin\n", GI_BEGIN), None);
        assert_eq!(find_marker("x # logos:managed:begin\n", GI_BEGIN), None);
        assert_eq!(find_marker("# logos:managed:begin\n", GI_BEGIN), Some(0));
        assert_eq!(find_marker("a\n# logos:managed:begin\n", GI_BEGIN), Some(2));
    }

    /// A begin marker carrying a trailing human note still matches (prefix
    /// semantics), so every generated block round-trips.
    ///
    /// One assertion per block Logos generates — the workspace-root one
    /// ([`workspace_root_gitignore`]) included, since it carries its own
    /// trailing note and is the block written into a file the *operator* owns,
    /// where a failure to round-trip would mean a duplicated block on every
    /// `init --workspace` re-run.
    #[test]
    fn generated_blocks_round_trip_through_their_own_markers() {
        assert_eq!(
            upsert_managed_block(GITIGNORE_BLOCK, GI_BEGIN, GI_END, GITIGNORE_BLOCK),
            Upsert::Unchanged
        );
        assert_eq!(
            upsert_managed_block(CLAUDE_MD_BLOCK, MD_BEGIN, MD_END, CLAUDE_MD_BLOCK),
            Upsert::Unchanged
        );

        // The workspace-root block is built at call time rather than being a
        // constant, so it round-trips only through a real second call: reading
        // the block back out of the file it just wrote would assert nothing.
        let tmp = TempDir::new().unwrap();
        let gitignore = tmp.path().join(".gitignore");
        let first = workspace_root_gitignore(tmp.path(), &[".logos.workspace.warm.json", ".logos/secrets.toml"]).unwrap();
        assert_eq!(first.action, InitAction::Created);
        let written = fs::read_to_string(&gitignore).unwrap();

        // The header and the trailing newline are part of what is written, so
        // they are asserted rather than left to a reader's assumption: emptying
        // the header constant, or dropping the final newline, broke no test
        // before this. The newline matters beyond tidiness — `upsert_managed_block`
        // falls back to `existing.len()` when the end marker has no newline, so
        // the block still round-trips and the omission is otherwise silent.
        let begin_at = find_marker(&written, GI_BEGIN).expect("a begin marker");
        assert!(
            begin_at > 0,
            "the header is written ABOVE the block, outside the markers, so a \
             user may edit or remove it freely: {written:?}"
        );
        assert_eq!(
            &written[..begin_at],
            WORKSPACE_GITIGNORE_HEADER,
            "everything above the marker is exactly the header"
        );
        assert!(
            written.ends_with(&format!("{GI_END}\n")),
            "the file ends with the end marker and a newline: {written:?}"
        );

        let second = workspace_root_gitignore(tmp.path(), &[".logos.workspace.warm.json", ".logos/secrets.toml"]).unwrap();
        assert_eq!(
            second.action,
            InitAction::Unchanged,
            "the trailing human note must not defeat marker matching"
        );
        assert_eq!(fs::read_to_string(&gitignore).unwrap(), written, "no rewrite");
        assert_eq!(
            written.matches(GI_BEGIN).count(),
            1,
            "a re-run appends no second block: {written}"
        );
    }

    // ── FR-IN-04 as data: what travels vs what is ignored ─────────────────

    /// [`ignored_state`] reads the managed block rather than restating it, so
    /// the derived state it names is exactly what the written file ignores.
    #[test]
    fn ignored_state_is_read_out_of_the_block_that_is_written() {
        let ignored = ignored_state();
        assert!(ignored.contains(&"logos.db*"), "{ignored:?}");
        assert!(ignored.contains(&"secrets.toml"), "the one non-derived exception: {ignored:?}");
        assert!(
            ignored.iter().all(|p| !p.starts_with('#') && !p.is_empty()),
            "comments and the markers are not patterns: {ignored:?}"
        );
        // Every pattern is a line of the block, and every non-comment line of
        // the block is a pattern — neither list may grow past the other.
        let lines = GITIGNORE_BLOCK.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).count();
        assert_eq!(ignored.len(), lines);
    }

    /// [`COMMITTED_POLICY`] duplicates what [`run`] writes, so this asserts the
    /// duplication is true on both sides: every entry is a real step target of a
    /// real `run`, and none of them is ignored by the block written beside it.
    /// Drifting either apart fails here, not in an operator's working tree.
    #[test]
    fn committed_policy_is_what_run_writes_and_the_block_leaves_tracked() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".logos")).unwrap();
        let steps = run(tmp.path(), &InitOptions::default(), &[]).expect("the default init runs");

        // The `[project] id` row (S-702) names a key inside `config.toml`, not a
        // file of its own, so it is not a policy-file target.
        let written: Vec<&str> = steps
            .iter()
            .map(|s| s.target.as_str())
            .filter(|target| *target != PROJECT_ID_TARGET)
            .collect();
        assert_eq!(written, COMMITTED_POLICY, "the policy list IS the default step set");
        assert!(
            steps.iter().any(|s| s.target == PROJECT_ID_TARGET),
            "the default step set carries the `[project] id` row"
        );

        // Matched as the globs they are — `logos.db*` is a pattern, not a name,
        // so a future `*.toml` would have to fail here rather than slip past a
        // string comparison and quietly stop the policy travelling.
        let patterns: Vec<String> = ignored_state().into_iter().map(String::from).collect();
        let ignored = crate::config::globs::compile(&patterns).expect("the block's globs compile");
        // …and the matcher is live, so the loop below cannot pass vacuously.
        assert!(ignored.is_match("logos.db-wal"), "the derived store IS matched");
        for target in committed_policy() {
            let name = target.strip_prefix(".logos/").expect("root-relative under .logos/");
            assert!(
                !ignored.is_match(name),
                "{target} is meant to travel, but the block ignores {name}"
            );
        }
    }

    /// The ratchet's blessed set ([FR-GV-23], [ADR-73]) is checked in: CI reads
    /// it and blessing new debt must show up in review, so the generated block
    /// must never ignore it ([FR-IN-04]). `init` does not write it — the bless
    /// command does — so it is pinned here rather than in `COMMITTED_POLICY`.
    ///
    /// [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
    /// [ADR-73]: ../../../docs/specs/architecture/decisions/ADR-73.md
    /// [FR-IN-04]: ../../../docs/specs/requirements/FR-IN-04.md
    #[test]
    fn the_block_leaves_the_ratchet_blessed_set_tracked() {
        let patterns: Vec<String> = ignored_state().into_iter().map(String::from).collect();
        let ignored = crate::config::globs::compile(&patterns).expect("the block's globs compile");
        // The near miss is live: the machine-local gate baseline one prefix
        // away IS ignored, so the assertion below cannot pass vacuously.
        assert!(ignored.is_match("baseline.json"), "the gate baseline is ignored");
        assert!(
            !ignored.is_match(crate::governance::ratchet::BLESSED_FILE),
            "the block must not ignore {}",
            crate::governance::ratchet::BLESSED_FILE_REL
        );
    }

    /// The security ratchet's checked-in baseline ([FR-SC-25]) travels for the
    /// same reason: a CI clone reads it, and blessing a finding must show up in
    /// review. `security bless` writes it, never `init`.
    ///
    /// [FR-SC-25]: ../../../docs/specs/requirements/FR-SC-25.md
    #[test]
    fn the_block_leaves_the_checked_in_security_baseline_tracked() {
        let patterns: Vec<String> = ignored_state().into_iter().map(String::from).collect();
        let ignored = crate::config::globs::compile(&patterns).expect("the block's globs compile");
        assert!(ignored.is_match("baseline.json"), "the near miss — the gate baseline — is ignored");
        let file = crate::governance::security::CHECKED_IN_FILE_REL;
        let name = file.strip_prefix(".logos/").expect("under .logos/");
        assert!(!ignored.is_match(name), "the block must not ignore {file}");
    }
}
