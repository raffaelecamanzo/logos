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
//!   priming graph-first usage, preserving user content outside the markers
//!   ([FR-IN-02]);
//! - the optional git-hook installer via `core.hooksPath` ([FR-IN-03],
//!   [FR-SY-05]);
//! - the embedded wiki-generation skill materialized into the canonical
//!   `.agents/skills/logos-wiki/` + `.claude/skills/logos-wiki` layout,
//!   skip-if-present ([FR-IN-02] as modified by CR-008, [FR-WK-08]).
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
//! [FR-SY-05]: ../../../docs/specs/requirements/FR-SY-05.md
//! [MCP Host]: ../../../docs/specs/architecture/integrations/mcp-host.md

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::models::pipeline::{InitAction, InitStep};

/// Which optional `init` steps to run (S-023, FR-IN-02/03).
///
/// The default (all `false`) is the plain `logos init` contract: policy
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
    /// Materialize the embedded wiki-generation skill into the canonical layout
    /// (`.agents/skills/logos-wiki/` + `.claude/skills/logos-wiki`),
    /// skip-if-present (FR-IN-02 as modified by CR-008, FR-WK-08).
    pub materialize_skill: bool,
    /// Install the Claude Code augmentation hook (FR-WK-14, ADR-33): the
    /// marker-tagged PostToolUse hook script + a non-clobbering merge into
    /// `.claude/settings.json`, default-on under `-i` alongside the skill.
    pub materialize_hook: bool,
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
# `exclude` prunes the planning/security/notes prose paths (CR-029/FR-CF-05);
# set `exclude = []` to re-admit them, or list your own globs to replace it.
# include = ["**"]
# exclude = ["docs/planning/**", "docs/security/**", "notes/**"]

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
# The chat API key (FR-CF-06, NFR-SE-07) — a secret, never committed.
secrets.toml
# logos:managed:end
";

/// Header written above the managed block on first generation (outside the
/// markers, so a user may edit or remove it freely).
const GITIGNORE_HEADER: &str = "\
# Generated by `logos init` (FR-IN-04): derived/machine-specific state is
# ignored; the checked-in policy (config.toml, rules.toml) travels.
";

/// The managed `CLAUDE.md` block (FR-IN-02): the graph-first usage steer —
/// the project-memory twin of the MCP `server-instructions`.
const CLAUDE_MD_BLOCK: &str = "\
<!-- logos:managed:begin -->
## Logos — structural code intelligence

This project is indexed by Logos. Use the `logos:*` MCP graph tools to navigate by
structure, and reach for them by the *shape* of the question:

- **Relational / cross-file** (\"who calls this?\", \"what breaks if I change it?\",
  \"where is X used?\", dead code, blast radius) — start with `logos:context` (one
  call replaces several speculative reads), then `logos:node` / `logos:callers` /
  `logos:callees` / `logos:impact`. The graph beats grep here.
- **Localized lookups** (a string, a value, a formula inside a file you can already
  name) — a direct read or grep is fine, sometimes faster. Don't force the graph on
  a question grep already answers.
- **Disambiguate by symbol** — prefer a unique name as the entry point. `logos:node`
  on a common bare name (`new`, `map`, `severity`) resolves to one arbitrary match;
  pivot from a unique caller or qualify the path instead.

Wrap editing sessions in the quality gate: `logos:session_start` before edits,
`logos:session_end` after — on a failing gate, stop and fix the regression before
piling on more changes. Run `logos:check_rules` before declaring any task done.

The full quality loop has four moves — **freshen** (index/sync so the graph matches
the code), **enforce** (`logos check` blocks regressions; the `pre-push` gate runs
it), **report** (`logos scan` surfaces the 0–10000 signal; the SessionEnd hook prints
it), and **bless** (`logos gate --save` records a new baseline, at release only). The
copy-pasteable CI recipe is `docs/howto/ci-integration.md`.

Every tool has a CLI twin (`logos context`, `logos search`, …) with `--json` output.

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
pub(crate) fn run(root: &Path, options: &InitOptions) -> Result<Vec<InitStep>> {
    let mut steps = vec![
        write_if_absent(root, ".logos/config.toml", CONFIG_TEMPLATE)?,
        write_if_absent(root, ".logos/rules.toml", RULES_TEMPLATE)?,
        upsert_block_file(
            root,
            ".logos/.gitignore",
            GITIGNORE_HEADER,
            GI_BEGIN,
            GI_END,
            GITIGNORE_BLOCK,
        )?,
    ];
    if options.inject_mcp {
        steps.push(inject_mcp(root)?);
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
    if options.materialize_skill {
        steps.push(materialize_skill(root)?);
    }
    if options.materialize_hook {
        steps.push(materialize_hook(root)?);
        steps.push(materialize_quality_report_hook(root)?);
    }
    Ok(steps)
}

/// Install the Claude Code augmentation hook (FR-WK-14, [ADR-33]), default-on
/// under `-i` alongside the skill. Delegates to the [`crate::wiki`] engine — the
/// sole owner of the hook artifacts — and maps the
/// [`crate::wiki::HookEmitSummary`] onto an [`InitStep`]. Non-clobbering: an
/// already-present managed entry is `Unchanged`; a foreign `.claude/settings.json`
/// is `Skipped` with the reason, never overwritten.
///
/// [ADR-33]: ../../../docs/specs/architecture/decisions/ADR-33.md
fn materialize_hook(root: &Path) -> Result<InitStep> {
    use crate::wiki::EmitAction;
    // Merging into a pre-existing settings file is an Updated, not a Created —
    // the same convention as the `.mcp.json` injection above.
    let settings_existed = root.join(crate::wiki::SETTINGS_REL).exists();
    // `init -i` never clobbers: unforced, so an existing managed entry skips.
    let summary = crate::wiki::materialize_hook(root, false)?;
    let (action, detail) = match (summary.action, &summary.notice) {
        (EmitAction::Created, _) => (
            if settings_existed {
                InitAction::Updated
            } else {
                InitAction::Created
            },
            format!("PostToolUse augmentation hook → {}", summary.script),
        ),
        (EmitAction::Forced, _) => (InitAction::Updated, "augmentation hook re-emitted".to_string()),
        (EmitAction::Skipped, Some(reason)) => (InitAction::Skipped, reason.clone()),
        (EmitAction::Skipped, None) => (
            InitAction::Unchanged,
            "already present — never overwritten; `logos wiki hook --emit --force` refreshes"
                .to_string(),
        ),
    };
    Ok(step(&summary.settings, action, detail))
}

/// Materialize the Claude Code SessionEnd quality-report hook ([FR-IN-07],
/// [FR-GV-05], [FR-GV-09], [ADR-49], [CR-055]), default-on under `-i` alongside
/// the wiki hooks. Delegates to the [`crate::wiki`] engine — the sole owner of
/// the hook artifacts — and maps the [`crate::wiki::HookEmitSummary`] onto an
/// [`InitStep`] targeting the **shared** `.claude/settings.json` (the same file
/// the augmentation hook wires its PostToolUse entry into; the merge touches only
/// `hooks.SessionEnd`, so the two coexist). Non-clobbering: an already-present
/// managed entry is `Unchanged`; a foreign settings file is `Skipped` with the
/// reason, never overwritten.
///
/// [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
/// [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
/// [FR-GV-09]: ../../../docs/specs/requirements/FR-GV-09.md
/// [ADR-49]: ../../../docs/specs/architecture/decisions/ADR-49.md
fn materialize_quality_report_hook(root: &Path) -> Result<InitStep> {
    use crate::wiki::EmitAction;
    // Merging into a pre-existing settings file is an Updated, not a Created —
    // the same convention as the augmentation hook above. By install order the
    // augmentation hook has already created settings.json, so this is normally an
    // Updated (it adds the SessionEnd array beside PostToolUse).
    let settings_existed = root.join(crate::wiki::SETTINGS_REL).exists();
    // `init -i` never clobbers: unforced, so an existing managed entry skips.
    let summary = crate::wiki::materialize_quality_report_hook(root, false)?;
    let (action, detail) = match (summary.action, &summary.notice) {
        (EmitAction::Created, _) => (
            if settings_existed {
                InitAction::Updated
            } else {
                InitAction::Created
            },
            format!("SessionEnd quality-report hook → {}", summary.script),
        ),
        (EmitAction::Forced, _) => {
            (InitAction::Updated, "quality-report hook re-emitted".to_string())
        }
        (EmitAction::Skipped, Some(reason)) => (InitAction::Skipped, reason.clone()),
        (EmitAction::Skipped, None) => (
            InitAction::Unchanged,
            "already present — never overwritten; `logos wiki hook --emit --force` refreshes"
                .to_string(),
        ),
    };
    Ok(step(&summary.settings, action, detail))
}

/// Materialize the embedded wiki-generation skill into the canonical layout,
/// skip-if-present (FR-IN-02 as modified by CR-008, [FR-WK-08]). Delegates to
/// the [`crate::wiki`] engine — the sole owner of the skill asset — and maps the
/// [`crate::wiki::EmitSummary`] onto an [`InitStep`].
///
/// [FR-WK-08]: ../../../docs/specs/requirements/FR-WK-08.md
fn materialize_skill(root: &Path) -> Result<InitStep> {
    use crate::wiki::{EmitAction, LinkKind};
    // `init -i` never clobbers local edits: unforced, so an existing skill skips.
    let summary = crate::wiki::materialize_skill(root, false)?;
    let action = match summary.action {
        EmitAction::Created => InitAction::Created,
        EmitAction::Forced => InitAction::Updated,
        EmitAction::Skipped => InitAction::Unchanged,
    };
    let detail = match (summary.action, summary.link_kind, &summary.notice) {
        (EmitAction::Skipped, _, _) => {
            "already present — never overwritten; `logos wiki skill --emit --force` refreshes"
                .to_string()
        }
        (_, _, Some(notice)) => notice.clone(),
        (_, Some(LinkKind::Symlink), None) => format!("{} → {}", summary.link, summary.skill_dir),
        _ => String::new(),
    };
    Ok(step(&summary.skill_dir, action, detail))
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
    let Some(begin_at) = find_marker(existing, begin) else {
        if find_marker(existing, end).is_some() {
            return Upsert::Malformed;
        }
        let mut out = existing.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(block);
        return Upsert::Write(out);
    };
    let Some(end_at) = find_marker(&existing[begin_at..], end).map(|i| begin_at + i) else {
        return Upsert::Malformed;
    };
    // The span runs to the end of the end-marker's line (incl. its newline).
    let span_end = existing[end_at..]
        .find('\n')
        .map_or(existing.len(), |i| end_at + i + 1);
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
/// host launches `logos serve --mcp` rooted at the project.
fn mcp_server_entry() -> Value {
    json!({ "command": "logos", "args": ["serve", "--mcp"] })
}

/// Inject the `logos` block into the project `.mcp.json` (the file-based
/// host-config flavour of the [MCP Host] integration): create the file if
/// absent, add the key if missing, and *skip* if a `logos` entry already
/// exists or the file cannot be parsed safely (FR-IN-02 non-clobber).
///
/// [MCP Host]: ../../../docs/specs/architecture/integrations/mcp-host.md
fn inject_mcp(root: &Path) -> Result<InitStep> {
    const TARGET: &str = ".mcp.json";
    let path = root.join(TARGET);
    let Some(text) = read_optional(&path)? else {
        let value = json!({ "mcpServers": { "logos": mcp_server_entry() } });
        write_json(&path, &value)?;
        return Ok(step(TARGET, InitAction::Created, ""));
    };
    let Ok(mut config) = serde_json::from_str::<Value>(&text) else {
        return Ok(step(
            TARGET,
            InitAction::Skipped,
            "existing .mcp.json is not valid JSON — left untouched; \
             add the `logos` server entry manually",
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
    if servers.contains_key("logos") {
        return Ok(step(
            TARGET,
            InitAction::Unchanged,
            "a `logos` server entry is already present — left untouched",
        ));
    }
    servers.insert("logos".to_string(), mcp_server_entry());
    write_json(&path, &config)?;
    Ok(step(
        TARGET,
        InitAction::Updated,
        "`logos` server entry added",
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
/// the user's hook setup is never clobbered (DL-07).
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
        // Write only when absent or when the file carries our marker but not
        // the richer targeted body from `Engine::install_hooks`. A file that
        // already carries `MANAGED_MARKER` with a different (richer) body is
        // left as-is — `init` never downgrades an existing managed installation.
        let current_managed = existing
            .as_deref()
            .is_some_and(|b| b.contains(crate::hooks::MANAGED_MARKER));
        if !current_managed {
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
    Ok(match (wrote, any_existed) {
        (false, _) => step(TARGET, InitAction::Unchanged, ""),
        (true, false) => step(TARGET, InitAction::Created, "core.hooksPath → .logos/hooks"),
        (true, true) => step(TARGET, InitAction::Updated, "managed hooks regenerated"),
    })
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
    /// semantics), so the generated `.gitignore` block round-trips.
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
    }
}
