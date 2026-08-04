//! The Claude Code session-start quality-report hook and its materialization
//! ([FR-IN-07], [ADR-49], [CR-055], [CR-095]).
//!
//! A marker-tagged hook script (`.claude/hooks/logos-quality-open.sh`) is
//! materialized alongside a **non-clobbering merge** of a `SessionStart` entry
//! into the project's `.claude/settings.json`. The merge is idempotent: an
//! existing managed entry is left untouched (recognized by our unique command
//! path); `force` re-emits it; and a foreign or unparseable
//! `.claude/settings.json` is never overwritten. Like the embedded skill
//! ([`crate::wiki::skill`]) this is pure local filesystem I/O — **no network, no
//! LLM call** ([NFR-SE-01]).
//!
//! # Why session start and not session end ([CR-095])
//!
//! The readout originally rode a **SessionEnd** hook. It never worked, for two
//! reasons that live in the agent host's contract rather than in Logos —
//! recorded here with the version observed (**Claude Code 2.1.220**) because
//! both are undocumented internals a future release may change:
//!
//! 1. **A SessionEnd hook's exit-0 output is discarded.** The host's own event
//!    description for SessionEnd is "Exit code 0 - command completes
//!    successfully / Other exit codes - show stderr to user only" — there is no
//!    exit-0 output clause, unlike SessionStart's "Exit code 0 - stdout shown to
//!    Claude". Internally a command hook's captured output is its stdout on
//!    success and its stderr on failure, and only the failure case is printed;
//!    hook stdio is piped (never inherited) and hooks spawn detached, so there
//!    is no controlling terminal and no `/dev/tty` fallback either. A
//!    report-only hook that writes to stderr and exits 0 is therefore silent by
//!    construction.
//! 2. **SessionEnd hooks are bounded at 1500 ms**, against 600 s for every other
//!    event. A readout costing seconds is cancelled on every firing — including
//!    on `/clear`, which fires SessionEnd with `reason: "clear"` — and surfaces
//!    as `SessionEnd hook [...] failed: Hook cancelled`.
//!
//! Session start carries the readout through two rendered channels: a
//! `systemMessage` field (user-visible) and `hookSpecificOutput.additionalContext`
//! with `hookEventName` exactly `SessionStart` (agent-visible; the host's parser
//! rejects a mismatch). Every emit also **sweeps** the retired SessionEnd entry
//! and its orphaned script, so upgrading stops the error with no hand-editing.
//!
//! This module once also materialized a **PostToolUse wiki-augmentation hook**
//! ([FR-WK-14], [ADR-33]) that ran `wiki generate` after every index/sync and
//! surfaced the queue to the connected agent. It was retired by [CR-070]: the
//! perpetually-non-empty queue made every firing unactioned context noise, and
//! its "writes it" complement had already been retired by [CR-047] in favour of
//! the `ui`-gated in-process generator ([FR-WK-18]). The generic hook-spec +
//! settings-merge engine below (`HookSpec`, `merge_settings`,
//! `materialize_spec`) is shared machinery that survives both retirements, now
//! exercised solely through the quality-report spec.
//!
//! [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
//! [FR-WK-14]: ../../../docs/specs/requirements/FR-WK-14.md
//! [FR-WK-18]: ../../../docs/specs/requirements/FR-WK-18.md
//! [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
//! [ADR-33]: ../../../docs/specs/architecture/decisions/ADR-33.md
//! [ADR-49]: ../../../docs/specs/architecture/decisions/ADR-49.md
//! [CR-047]: ../../../docs/requests/CR-047-internal-wiki-generation-on-agent-substrate.md
//! [CR-055]: ../../../docs/requests/CR-055-standalone-quality-integration.md
//! [CR-070]: ../../../docs/requests/CR-070-retire-wiki-augment-hook.md
//! [CR-095]: ../../../docs/requests/CR-095-session-start-quality-readout.md

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::skill::EmitAction;

/// The Claude Code settings file the SessionEnd entry merges into, repo-relative.
pub const SETTINGS_REL: &str = ".claude/settings.json";

// ── The session-start quality-report hook ([FR-IN-07], [FR-GV-02], [FR-GV-05], [ADR-49], [CR-055], [CR-095]) ──

/// The quality-report hook script, repo-relative ([FR-IN-07]).
pub const QUALITY_REPORT_HOOK_SCRIPT_REL: &str = ".claude/hooks/logos-quality-open.sh";

/// The quality-report hook command wired into the **shared** `.claude/settings.json`
/// ([FR-IN-07] — a project-wide readout). Uses the same `${CLAUDE_PROJECT_DIR}`
/// placeholder convention as the other hooks.
const QUALITY_REPORT_HOOK_COMMAND: &str = "${CLAUDE_PROJECT_DIR}/.claude/hooks/logos-quality-open.sh";

/// The quality-report hook's idempotency / ownership marker: its unique script
/// basename, found in the command of an entry we own.
const QUALITY_REPORT_HOOK_MARKER: &str = "logos-quality-open.sh";

/// Which session starts carry the readout ([FR-IN-07], [CR-095]). The host
/// matches this against the event's `source` as an **exact-match alternation
/// list** (split on `|` or `,`), not a regex.
///
/// `clear` is included deliberately: a `/clear` ends one chunk of work and opens
/// the next, so it is a natural readout boundary. `compact` and `fork` are
/// excluded — auto-compaction fires mid-task and can repeat within one chunk,
/// which would re-print the readout and re-pay its cost while work is in
/// progress, the noise pattern that retired [FR-WK-14] in [CR-070].
const QUALITY_REPORT_HOOK_MATCHER: &str = "startup|resume|clear";

/// The declared per-hook timeout, in seconds ([FR-IN-07], [CR-095]). Bounds a
/// wedged readout by a value **Logos** controls rather than inheriting a host
/// default: those are undocumented and range from 1500 ms (SessionEnd) to 600 s
/// (everything else), and inheriting one is what broke the retired hook.
const QUALITY_REPORT_HOOK_TIMEOUT_SECS: u64 = 30;

// ── The retired SessionEnd quality-report hook ([CR-095]) ────────────────────

/// A managed hook entry retired by a prior version of Logos, swept from
/// `.claude/settings.json` on every emit so an upgrade is self-healing
/// ([CR-095]).
///
/// Unlike the [CR-070] augment-hook retirement — whose stale entry kept doing
/// its noisy-but-working thing, so removing it by hand was tolerable — a stale
/// entry here keeps **emitting the defect** the retirement exists to remove, on
/// every session exit and every `/clear`. Hence an active sweep.
struct RetiredHook {
    /// The settings event the retired entry was registered under.
    event: &'static str,
    /// The retired entry's ownership marker (its unique script basename).
    marker: &'static str,
    /// The retired script artifact, repo-relative — deleted when found.
    script_rel: &'static str,
}

/// The retired SessionEnd quality-report hook ([CR-095]).
const RETIRED_HOOKS: &[RetiredHook] = &[RetiredHook {
    event: "SessionEnd",
    marker: "logos-quality-report.sh",
    script_rel: ".claude/hooks/logos-quality-report.sh",
}];

/// The marker-tagged session-start quality-report hook script ([FR-IN-07],
/// [FR-GV-02], [FR-GV-05], [ADR-49], [CR-095]). POSIX `sh`, **report-only** by
/// construction: it ALWAYS exits 0 (never blocks a session) and emits the
/// current quality signal, the blessed baseline signal and their delta, plus any
/// architecture-rule violations.
///
/// It makes **no** network or LLM call ([NFR-SE-01]) — it only shells out to two
/// pure-read quality commands: `logos gate --no-reconcile` (the signal **and**
/// the blessed `baseline_signal` in one pass, [FR-GV-05] — which is why no
/// reconciling `scan` is needed, [FR-GV-09]) and `logos check --no-reconcile`
/// (rule violations, [FR-GV-02]). Neither exit code is propagated — this is the
/// non-blocking report tier, distinct from the enforcing `pre-push` gate — and,
/// critically, neither is read as a *failure signal* either: `gate` exits 1 on a
/// regression and `check` exits 1 on an error violation, both by design
/// ([FR-GV-03]), so a run is judged by whether its output carries the expected
/// field. `LOGOS_QUALITY_REPORT_DISABLE` disables it without uninstalling.
///
/// The readout is emitted as one JSON object on **stdout**: `systemMessage` for
/// the user, `hookSpecificOutput.additionalContext` for the agent, with
/// `hookEventName` exactly `SessionStart` ([CR-095]).
///
/// [FR-GV-03]: ../../../docs/specs/requirements/FR-GV-03.md
const QUALITY_REPORT_HOOK_SCRIPT: &str = r#"#!/bin/sh
# logos:quality-report:managed — Claude Code SessionStart quality-report hook (FR-IN-07, FR-GV-02, FR-GV-05, ADR-49, CR-095).
#
# On session start — a fresh start, a resume, or the session opened by /clear —
# this emits a NON-BLOCKING quality readout: the current quality signal, the
# blessed baseline signal and their delta (logos gate), and any
# architecture-rule violations (logos check). It is REPORT-ONLY by construction:
# it ALWAYS exits 0, so it can never block a session (this is the report tier,
# not the enforcing pre-push gate). Logos makes no LLM or network call here
# (NFR-SE-01): gate and check are pure local reads over the graph.
#
# Output contract (CR-095): the readout goes to STDOUT as ONE JSON object. The
# host renders `systemMessage` to the user and hands
# `hookSpecificOutput.additionalContext` to the agent; `hookEventName` must be
# exactly SessionStart or the host rejects the whole payload. This is why the
# readout does NOT ride SessionEnd: that event's exit-0 output is discarded
# entirely, and its hooks are cancelled at a hardcoded 1500 ms.
#
# Exit codes are NOT failure signals here: `gate` exits 1 on a regression and
# `check` exits 1 on an error violation, both BY DESIGN (FR-GV-03). So each run
# is judged by whether its OUTPUT carries the expected field, never by its
# status — reading a regression as a failure would report a healthy graph as
# missing, which is the inverse of the honesty this readout exists for.
#
# Honest degradation: another logos process (e.g. a running MCP server) can hold
# the graph-DB write lock, so a read may fail with "database is locked". This
# hook CAPTURES that instead of swallowing it, and reports "graph busy —
# skipped" rather than mis-rendering a healthy, indexed project as un-indexed
# with a zeroed readout.
#
#   off-switch: export LOGOS_QUALITY_REPORT_DISABLE=1
#
# Regenerate with `logos wiki hook --emit --force` (or re-run `logos init -i`).

# Off-switch: disable the report without uninstalling the hook.
[ "${LOGOS_QUALITY_REPORT_DISABLE:-0}" = "1" ] && exit 0

# Best-effort: a missing binary is nothing to report.
command -v logos >/dev/null 2>&1 || exit 0

PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$(pwd)}"
cd "$PROJECT_DIR" 2>/dev/null || exit 0

# Escape a string for a JSON string literal. Backslash before quote (order
# matters), tabs folded to spaces, then real newlines folded to a two-character
# \n by awk. Deliberately not sed for the newline step: BSD sed interprets
# neither \001 nor \t in a pattern, so a sed-based fold silently no-ops on macOS.
json_escape() {
  tr '\t' ' ' \
    | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' \
    | awk 'NR > 1 { printf "%s", "\\n" } { printf "%s", $0 }'
}

# Emit the single JSON object: $1 = one-line user-visible summary, $2 = the full
# readout handed to the agent as session context.
emit() {
  printf '{"systemMessage":"%s","hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"%s"}}\n' \
    "$(printf '%s' "$1" | json_escape)" \
    "$(printf '%s' "$2" | json_escape)"
}

# --- signal + blessed baseline: one `gate` pass (FR-GV-05) ------------------
# `gate --no-reconcile` yields BOTH `signal` and `baseline_signal`, so the
# report needs no reconciling `scan` pass at all. stdout+stderr share one
# capture so an error message is classified rather than swallowed.
gate_out=$(logos gate --no-reconcile --json 2>&1)
signal=$(printf '%s' "$gate_out" | grep -oE '"signal":[0-9]+' | head -1 | grep -oE '[0-9]+')
if [ -z "$signal" ]; then
  # No signal in the output — a real read failure, NOT a regression (a regressed
  # gate still reports its signal and merely exits 1). Distinguish a TRANSIENT
  # lock, another logos process holding the DB, from a genuinely absent or
  # uninitialized graph, so the readout never lies.
  if printf '%s' "$gate_out" | grep -qi 'database is locked'; then
    reason='logos quality report: graph busy (locked by another logos process) — skipped.'
  else
    reason='logos quality report: graph unavailable (run logos index first) — skipped.'
  fi
  emit "$reason" "$reason"
  exit 0
fi
baseline=$(printf '%s' "$gate_out" | grep -oE '"baseline_signal":[0-9]+' | head -1 | grep -oE '[0-9]+')
freshness=$(printf '%s' "$gate_out" \
  | grep -oE '"freshness":"([^"\\]|\\.)*"' \
  | head -1 \
  | sed -e 's/^"freshness":"//' -e 's/"$//' -e 's/\\"/"/g')

# --- rule violations: `check` (FR-GV-02) -----------------------------------
# Only trust a count when the violations array is present in the output: `check`
# also exits non-zero with empty output when it cannot read the graph, and a
# blind grep would then mis-report that as a truthful "0 violations".
check_json=$(logos check --no-reconcile --json 2>/dev/null)
if printf '%s' "$check_json" | grep -q '"violations"'; then
  violations=$(printf '%s' "$check_json" | grep -oE '"severity":"[a-z]+"' | grep -c '.')
else
  violations=""
fi

# --- build the readout -----------------------------------------------------
readout="logos quality report (session start)"
readout="$readout
  signal:   $signal"
if [ -n "$baseline" ]; then
  delta=$((signal - baseline))
  readout="$readout
  baseline: $baseline
  delta:    $delta"
  summary="logos quality report: signal $signal · baseline $baseline · delta $delta"
else
  readout="$readout
  baseline: n/a (none saved — bless one with 'logos gate --save')"
  summary="logos quality report: signal $signal · no baseline saved"
fi
readout="$readout
  rule violations: ${violations:-n/a (check unavailable)}"
if [ -n "$violations" ]; then
  summary="$summary · $violations violation(s)"
else
  summary="$summary · violations n/a"
fi
if [ -n "$freshness" ]; then
  readout="$readout
  freshness: $freshness"
fi

# List the violation messages (report-only detail), capped for brevity. Match
# the full message value allowing escaped chars so a message containing a quote
# is not truncated at the first quote, then unescape for display — json_escape
# re-escapes the whole readout on the way out.
if [ "${violations:-0}" -gt 0 ] 2>/dev/null; then
  messages=$(printf '%s' "$check_json" \
    | grep -oE '"message":"([^"\\]|\\.)*"' \
    | sed -e 's/^"message":"//' -e 's/"$//' -e 's/\\"/"/g' \
    | head -20 \
    | sed -e 's/^/    - /')
  if [ -n "$messages" ]; then
    readout="$readout
$messages"
  fi
fi

emit "$summary" "$readout"

# ALWAYS exit 0: this hook reports, it never blocks a session (FR-GV-05).
exit 0
"#;

/// One materializable Claude Code hook: its script artifact plus the settings
/// merge target. Generalizes the marker-tagged / idempotent / non-clobbering
/// settings-merge machinery so it is written exactly once; today only the
/// [FR-IN-07] session-start quality-report hook is materialized through it (the
/// [FR-WK-14] PostToolUse wiki-augmentation hook this once also drove was
/// retired by [CR-070], and the SessionEnd quality-report hook by [CR-095]).
struct HookSpec {
    /// The hook script path, repo-relative.
    script_rel: &'static str,
    /// The settings file the entry merges into, repo-relative.
    settings_rel: &'static str,
    /// The Claude Code hook event the entry registers under (e.g. `SessionStart`).
    event: &'static str,
    /// The event-field matcher, or `None` for a matcher-less entry that fires on
    /// every occurrence. Compared by the host as an exact-match alternation list.
    matcher: Option<&'static str>,
    /// The declared per-hook timeout in seconds — never inherit a host default
    /// ([CR-095]).
    timeout_secs: u64,
    /// The wired command (uses the `${CLAUDE_PROJECT_DIR}` placeholder).
    command: &'static str,
    /// The idempotency / ownership marker: our unique script basename, found in
    /// the command of an entry we own.
    marker: &'static str,
    /// The marker-tagged script body.
    script: &'static str,
    /// Managed entries from prior versions, swept on every emit ([CR-095]).
    retired: &'static [RetiredHook],
}

/// The [FR-IN-07] session-start quality-report hook spec. Registers under
/// `SessionStart` in the shared `.claude/settings.json` ([FR-IN-07]); the merge
/// touches only the `hooks.SessionStart` array plus the retired events it
/// sweeps, leaving every other key and event (and any foreign entry) verbatim.
const QUALITY_REPORT_SPEC: HookSpec = HookSpec {
    script_rel: QUALITY_REPORT_HOOK_SCRIPT_REL,
    settings_rel: SETTINGS_REL,
    event: "SessionStart",
    matcher: Some(QUALITY_REPORT_HOOK_MATCHER),
    timeout_secs: QUALITY_REPORT_HOOK_TIMEOUT_SECS,
    command: QUALITY_REPORT_HOOK_COMMAND,
    marker: QUALITY_REPORT_HOOK_MARKER,
    script: QUALITY_REPORT_HOOK_SCRIPT,
    retired: RETIRED_HOOKS,
};

/// The outcome of materializing a Claude Code hook — currently only the
/// [FR-IN-07] session-start quality-report hook — a `Serialize` read-model the
/// CLI renders and `init` folds into its step list.
///
/// `action` reuses [`EmitAction`] for a uniform CLI JSON shape with the skill
/// (`"action":"created"|"forced"|"skipped"`). A [`EmitAction::Skipped`] is
/// disambiguated by `notice`: `None` means "already present" (idempotent
/// re-run); `Some(reason)` means a foreign/unsafe `.claude/settings.json` was
/// left untouched.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HookEmitSummary {
    /// The hook script path, repo-relative.
    pub script: String,
    /// The settings file the entry merges into, repo-relative.
    pub settings: String,
    /// What happened.
    pub action: EmitAction,
    /// A one-line reason when a foreign/unsafe settings file was skipped; else
    /// `None`.
    pub notice: Option<String>,
    /// Retired artifacts removed by this emit, repo-relative ([CR-095]) — the
    /// stale entry's script path, reported so the sweep is visible rather than
    /// silent. Empty on a clean install and on every subsequent re-run.
    pub retired_removed: Vec<String>,
}

/// What the settings merge resolved to — a pure function of the existing file
/// content and `force`, isolated for unit testing.
#[derive(Debug, PartialEq, Eq)]
enum Merge {
    /// Our managed entry is already present, `force` was not given, and there
    /// was nothing retired left to sweep.
    AlreadyPresent,
    /// Write this serialized settings document; `forced` distinguishes a
    /// re-emit (entry was present) from a first install. `swept` names the
    /// retired specs whose entries this merge removed ([CR-095]).
    Write {
        json: String,
        forced: bool,
        swept: Vec<&'static str>,
    },
    /// A foreign/unparseable settings file — never overwritten ([FR-IN-07]).
    Foreign { reason: String },
}

/// Materialize the [FR-IN-07] session-start quality-report hook under `base`.
///
/// Writes `<base>/.claude/hooks/logos-quality-open.sh` and merges a
/// marker-tagged `SessionStart` entry into `<base>/.claude/settings.json`, and
/// **sweeps** the retired SessionEnd entry plus its orphaned script ([CR-095]).
/// **Idempotent and non-clobbering:** an existing managed entry (recognized by
/// its command path) is left untouched unless `force`; a foreign or unparseable
/// settings file is never overwritten. Installing the hook performs **no** LLM
/// call and opens **no** network connection ([NFR-SE-01]) — the hook only
/// shells out to the pure-read `gate`/`check` commands at session start, and
/// always exits 0 ([FR-GV-05] report tier).
///
/// # Errors
/// Returns an error only when a Logos-owned path cannot be created, written or
/// removed.
pub fn materialize_quality_report(base: &Path, force: bool) -> Result<HookEmitSummary> {
    materialize_spec(base, force, &QUALITY_REPORT_SPEC)
}

/// Materialize one hook (`spec`) under `base` — the engine behind the
/// [FR-IN-07] quality-report hook.
fn materialize_spec(base: &Path, force: bool, spec: &HookSpec) -> Result<HookEmitSummary> {
    let settings_path = base.join(spec.settings_rel);
    let existing = if settings_path.exists() {
        Some(
            fs::read_to_string(&settings_path)
                .with_context(|| format!("reading {}", settings_path.display()))?,
        )
    } else {
        None
    };

    let summary_base = |action, notice, retired_removed| HookEmitSummary {
        script: spec.script_rel.to_string(),
        settings: spec.settings_rel.to_string(),
        action,
        notice,
        retired_removed,
    };

    match merge_settings(existing.as_deref(), force, spec) {
        // A settings file we refuse to parse is also one we refuse to sweep:
        // leave every artifact exactly as it is ([FR-IN-07] never-overwrite).
        Merge::Foreign { reason } => Ok(summary_base(EmitAction::Skipped, Some(reason), Vec::new())),
        // Nothing to write, but an orphaned retired script can still be on disk
        // (a prior sweep that removed the entry, or a hand-edited settings file).
        Merge::AlreadyPresent => {
            let removed = sweep_retired_scripts(base, spec)?;
            Ok(summary_base(EmitAction::Skipped, None, removed))
        }
        Merge::Write { json, forced, swept } => {
            // Write the script first so the wired entry never points at a
            // missing file, then commit the settings merge.
            write_script(base, spec)?;
            if let Some(parent) = settings_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            fs::write(&settings_path, json)
                .with_context(|| format!("writing {}", settings_path.display()))?;
            let removed = sweep_retired_scripts(base, spec)?;
            tracing::info!(
                script = spec.script_rel,
                settings = spec.settings_rel,
                event = spec.event,
                forced,
                swept = swept.len(),
                retired_scripts_removed = removed.len(),
                "wiki hook materialized"
            );
            Ok(summary_base(
                if forced {
                    EmitAction::Forced
                } else {
                    EmitAction::Created
                },
                None,
                removed,
            ))
        }
    }
}

/// Delete the retired hooks' orphaned script artifacts, returning the
/// repo-relative paths actually removed ([CR-095]).
///
/// Bounded by [`HookSpec::retired`] — only paths Logos itself wrote in a prior
/// version are touched. An absent file is not an error: the sweep is idempotent,
/// so the common case (a project that never had the retired hook, or already
/// swept it) removes nothing and reports nothing.
fn sweep_retired_scripts(base: &Path, spec: &HookSpec) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    for retired in spec.retired {
        let path = base.join(retired.script_rel);
        if !path.exists() {
            continue;
        }
        fs::remove_file(&path)
            .with_context(|| format!("removing the retired hook script {}", path.display()))?;
        removed.push(retired.script_rel.to_string());
    }
    Ok(removed)
}

/// Write the marker-tagged hook script, marking it executable on Unix.
fn write_script(base: &Path, spec: &HookSpec) -> Result<()> {
    let path = base.join(spec.script_rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {} for the hook script", parent.display()))?;
    }
    fs::write(&path, spec.script).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("marking {} executable", path.display()))?;
    }
    Ok(())
}

/// The settings entry this hook installs. Matcher-less — SessionEnd (the only
/// event materialized today) has no tool to match on, so it fires on every
/// occurrence and the script self-gates.
fn hook_entry(spec: &HookSpec) -> Value {
    // The declared timeout is never omitted: inheriting an undocumented host
    // default is what broke the retired SessionEnd hook ([CR-095]).
    let hook = json!({
        "type": "command",
        "command": spec.command,
        "timeout": spec.timeout_secs,
    });
    match spec.matcher {
        Some(matcher) => json!({ "matcher": matcher, "hooks": [hook] }),
        None => json!({ "hooks": [hook] }),
    }
}

/// Does this hook entry belong to us? An entry is ours when any of its `hooks`
/// commands references our unique script path (`marker`).
fn is_ours(entry: &Value, marker: &str) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|c| c.contains(marker))
            })
        })
}

/// Resolve the settings merge purely (no I/O) so the idempotent/non-clobbering
/// contract is unit-testable. An absent file starts from `{}`; an unparseable
/// or structurally foreign file is refused ([FR-IN-07] never-overwrite). The
/// `spec.event` array (e.g. `SessionStart`) and the retired events swept out of
/// it are the only keys touched; every other key and a foreign entry survive
/// verbatim.
fn merge_settings(existing: Option<&str>, force: bool, spec: &HookSpec) -> Merge {
    let settings = spec.settings_rel;
    let mut config: Value = match existing {
        None => json!({}),
        Some(text) if text.trim().is_empty() => json!({}),
        Some(text) => match serde_json::from_str(text) {
            Ok(value) => value,
            Err(_) => {
                return Merge::Foreign {
                    reason: format!(
                        "existing {settings} is not valid JSON — left untouched; \
                         run `logos wiki hook --emit` after fixing it"
                    ),
                };
            }
        },
    };

    let Some(obj) = config.as_object_mut() else {
        return Merge::Foreign {
            reason: format!("existing {settings} is not a JSON object — left untouched"),
        };
    };
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return Merge::Foreign {
            reason: format!("existing {settings} `hooks` is not an object — left untouched"),
        };
    };
    // Sweep managed entries retired by a prior version ([CR-095]). Bounded by
    // the ownership marker, so a foreign entry sharing the retired event — and
    // an event array whose shape we do not recognize — survives untouched.
    let mut swept = Vec::new();
    for retired in spec.retired {
        let Some(event) = hooks_obj.get_mut(retired.event) else {
            continue;
        };
        let Some(arr) = event.as_array_mut() else {
            continue;
        };
        let before = arr.len();
        arr.retain(|entry| !is_ours(entry, retired.marker));
        if arr.len() == before {
            continue;
        }
        swept.push(retired.marker);
        // Drop an event key we just emptied rather than leaving `"SessionEnd": []`
        // behind — the retirement should be invisible, not archaeological.
        if arr.is_empty() {
            hooks_obj.remove(retired.event);
        }
    }

    let event = hooks_obj.entry(spec.event).or_insert_with(|| json!([]));
    let Some(arr) = event.as_array_mut() else {
        return Merge::Foreign {
            reason: format!(
                "existing {settings} `hooks.{}` is not an array — left untouched",
                spec.event
            ),
        };
    };

    let present = arr.iter().any(|e| is_ours(e, spec.marker));
    // Idempotent only when there is also nothing retired left to remove:
    // otherwise a project whose entry is already current would keep its stale
    // SessionEnd entry — and keep emitting the error the sweep exists to stop.
    if present && !force && swept.is_empty() {
        return Merge::AlreadyPresent;
    }
    // `force` re-emit: drop our prior entries before re-adding so a refresh
    // never accumulates duplicates. Foreign entries are preserved untouched.
    if present {
        arr.retain(|entry| !is_ours(entry, spec.marker));
    }
    arr.push(hook_entry(spec));

    Merge::Write {
        json: serialize(&config),
        forced: present,
        swept,
    }
}

/// Pretty-print the merged settings with a trailing newline — matches the
/// `.mcp.json` injection style ([`crate::init`]) and is byte-stable because
/// `serde_json` preserves key insertion order.
fn serialize(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).expect("settings serialise");
    text.push('\n');
    text
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// The retired SessionEnd entry as a prior Logos version wrote it, for the
    /// [CR-095] sweep tests.
    const RETIRED_ENTRY: &str =
        r#"{ "hooks": [ { "type": "command", "command": "${CLAUDE_PROJECT_DIR}/.claude/hooks/logos-quality-report.sh" } ] }"#;

    /// Unwrap a `Merge::Write`'s document, asserting the write happened.
    fn written(merge: Merge) -> Value {
        let Merge::Write { json, .. } = merge else {
            panic!("expected a write, got {merge:?}");
        };
        serde_json::from_str(&json).expect("valid JSON")
    }

    /// Our managed entries in `hooks.<event>`.
    fn ours(settings: &Value, event: &str) -> Vec<Value> {
        settings["hooks"][event]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|e| is_ours(e, QUALITY_REPORT_HOOK_MARKER))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    // ── Generic `merge_settings` machinery, verified via the [FR-IN-07]
    // quality-report spec ([CR-070]: the retired augment spec was previously
    // the vehicle for this coverage) ──────────────────────────────────────────

    /// A pre-existing foreign SessionStart entry survives our merge untouched —
    /// we append alongside it, never clobber it.
    #[test]
    fn merge_preserves_a_foreign_entry() {
        let existing = r#"{
            "hooks": {
                "SessionStart": [
                    { "hooks": [ { "type": "command", "command": "my-own.sh" } ] }
                ]
            },
            "permissions": { "allow": ["Bash"] }
        }"#;
        let Merge::Write { json, forced, swept } =
            merge_settings(Some(existing), false, &QUALITY_REPORT_SPEC)
        else {
            panic!("expected a write");
        };
        assert!(!forced, "a first install is not a forced re-emit");
        assert!(swept.is_empty(), "nothing retired to sweep here");
        let value: Value = serde_json::from_str(&json).unwrap();
        let start = value["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 2, "the foreign entry is preserved alongside ours");
        assert!(start.iter().any(|e| e["hooks"][0]["command"] == "my-own.sh"));
        assert!(start.iter().any(|e| is_ours(e, QUALITY_REPORT_HOOK_MARKER)));
        // Unrelated keys survive verbatim.
        assert_eq!(value["permissions"]["allow"][0], "Bash");
    }

    /// An unparseable settings file is foreign — never overwritten ([FR-IN-07]).
    #[test]
    fn unparseable_settings_is_foreign() {
        let Merge::Foreign { reason } =
            merge_settings(Some("{ not json"), false, &QUALITY_REPORT_SPEC)
        else {
            panic!("expected a foreign refusal");
        };
        assert!(reason.contains("not valid JSON"));
        // Even with `--force`, a file we cannot parse is never overwritten.
        assert!(matches!(
            merge_settings(Some("{ not json"), true, &QUALITY_REPORT_SPEC),
            Merge::Foreign { .. }
        ));
    }

    /// A settings file whose shape is wrong anywhere on the `hooks.SessionStart`
    /// path — including a valid-JSON-but-non-object root — is foreign and never
    /// overwritten.
    #[test]
    fn structurally_foreign_settings_is_refused() {
        let bad = r#"{ "hooks": { "SessionStart": "not-an-array" } }"#;
        assert!(matches!(
            merge_settings(Some(bad), false, &QUALITY_REPORT_SPEC),
            Merge::Foreign { .. }
        ));
        let bad_hooks = r#"{ "hooks": [] }"#;
        assert!(matches!(
            merge_settings(Some(bad_hooks), false, &QUALITY_REPORT_SPEC),
            Merge::Foreign { .. }
        ));
        // Valid JSON whose root is not an object (a string, an array) is foreign.
        assert!(matches!(
            merge_settings(Some(r#""just a string""#), false, &QUALITY_REPORT_SPEC),
            Merge::Foreign { .. }
        ));
        assert!(matches!(
            merge_settings(Some("[1, 2, 3]"), false, &QUALITY_REPORT_SPEC),
            Merge::Foreign { .. }
        ));
    }

    /// An absent or empty file starts from `{}` and installs cleanly.
    #[test]
    fn absent_or_empty_settings_installs() {
        let Merge::Write { forced, .. } = merge_settings(None, false, &QUALITY_REPORT_SPEC) else {
            panic!("expected a write for an absent file");
        };
        assert!(!forced);
        assert!(matches!(
            merge_settings(Some("   \n"), false, &QUALITY_REPORT_SPEC),
            Merge::Write { .. }
        ));
    }

    /// The materialized settings document is valid JSON ending in a newline; the
    /// wired command uses the `${CLAUDE_PROJECT_DIR}` placeholder; and the entry
    /// carries the source matcher plus an explicit timeout ([CR-095] — never
    /// inherit a host default).
    #[test]
    fn merged_document_is_well_formed() {
        let Merge::Write { json, .. } = merge_settings(None, false, &QUALITY_REPORT_SPEC) else {
            panic!("expected a write");
        };
        assert!(json.ends_with('\n'), "trailing newline like .mcp.json");
        let value: Value = serde_json::from_str(&json).expect("valid JSON");
        let entry = &value["hooks"]["SessionStart"][0];
        let cmd = entry["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, QUALITY_REPORT_HOOK_COMMAND);
        assert!(cmd.contains("${CLAUDE_PROJECT_DIR}"), "uses the placeholder");
        assert_eq!(
            entry["hooks"][0]["timeout"], QUALITY_REPORT_HOOK_TIMEOUT_SECS,
            "the entry declares its own timeout rather than inheriting a host default"
        );
        assert_eq!(
            entry["matcher"], QUALITY_REPORT_HOOK_MATCHER,
            "the source matcher is wired"
        );
    }

    /// The matcher names exactly the sources that should carry the readout: a
    /// fresh start, a resume, and the session `/clear` opens. `compact` and
    /// `fork` are excluded — auto-compaction fires mid-task and would re-print
    /// the readout inside one chunk of work ([CR-095]).
    #[test]
    fn matcher_covers_startup_resume_clear_but_not_compact() {
        let sources: Vec<&str> = QUALITY_REPORT_HOOK_MATCHER.split('|').collect();
        assert_eq!(sources, vec!["startup", "resume", "clear"]);
        assert!(
            !QUALITY_REPORT_HOOK_MATCHER.contains("compact"),
            "compact fires mid-task; the readout would interrupt work in progress"
        );
        assert!(!QUALITY_REPORT_HOOK_MATCHER.contains("fork"));
    }

    // ── [CR-095] retirement sweep ─────────────────────────────────────────────

    /// The retired SessionEnd entry is removed while a foreign entry sharing
    /// that same event survives — the sweep is bounded by the ownership marker,
    /// never by the event. An emptied `SessionEnd` key is dropped entirely.
    #[test]
    fn retired_session_end_entry_is_swept_and_foreign_survives() {
        let existing = format!(
            r#"{{
                "hooks": {{
                    "SessionEnd": [
                        {RETIRED_ENTRY},
                        {{ "hooks": [ {{ "type": "command", "command": "their-cleanup.sh" }} ] }}
                    ]
                }},
                "permissions": {{ "allow": ["Bash"] }}
            }}"#
        );
        let Merge::Write { json, swept, .. } =
            merge_settings(Some(&existing), false, &QUALITY_REPORT_SPEC)
        else {
            panic!("expected a write — a pending sweep is never a no-op");
        };
        assert_eq!(swept, vec!["logos-quality-report.sh"]);
        let value: Value = serde_json::from_str(&json).unwrap();
        let end = value["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(end.len(), 1, "only the foreign SessionEnd entry remains");
        assert_eq!(end[0]["hooks"][0]["command"], "their-cleanup.sh");
        assert_eq!(ours(&value, "SessionStart").len(), 1, "ours is installed");
        assert_eq!(value["permissions"]["allow"][0], "Bash", "unrelated keys survive");

        // With no foreign entry left behind, the emptied event key is removed
        // rather than left as `"SessionEnd": []`.
        let only_ours = format!(r#"{{ "hooks": {{ "SessionEnd": [{RETIRED_ENTRY}] }} }}"#);
        let value = written(merge_settings(Some(&only_ours), false, &QUALITY_REPORT_SPEC));
        assert!(
            value["hooks"].get("SessionEnd").is_none(),
            "an emptied retired event key is dropped: {value}"
        );
    }

    /// A settings file that is already current **except** for a stale retired
    /// entry still writes: treating it as idempotent would leave the entry in
    /// place, and with it the very error the retirement exists to stop.
    #[test]
    fn pending_sweep_defeats_idempotence() {
        // First install into a file carrying the retired entry.
        let existing = format!(r#"{{ "hooks": {{ "SessionEnd": [{RETIRED_ENTRY}] }} }}"#);
        let current = written(merge_settings(Some(&existing), false, &QUALITY_REPORT_SPEC));

        // Re-running over the now-clean document is idempotent.
        assert_eq!(
            merge_settings(Some(&serialize(&current)), false, &QUALITY_REPORT_SPEC),
            Merge::AlreadyPresent,
            "a clean, current document needs no write"
        );

        // But re-introduce the retired entry alongside our current one and the
        // merge must write again to sweep it.
        let mut regressed = current.clone();
        regressed["hooks"]["SessionEnd"] =
            serde_json::from_str(&format!("[{RETIRED_ENTRY}]")).unwrap();
        let Merge::Write { swept, .. } =
            merge_settings(Some(&serialize(&regressed)), false, &QUALITY_REPORT_SPEC)
        else {
            panic!("a pending sweep must write even when our entry is already present");
        };
        assert_eq!(swept, vec!["logos-quality-report.sh"]);
    }

    /// A retired event whose shape we do not recognize is left alone — the sweep
    /// refuses to guess, exactly as the merge refuses to overwrite a foreign file.
    #[test]
    fn sweep_leaves_a_structurally_foreign_retired_event_untouched() {
        let existing = r#"{ "hooks": { "SessionEnd": "not-an-array" } }"#;
        let value = written(merge_settings(Some(existing), false, &QUALITY_REPORT_SPEC));
        assert_eq!(
            value["hooks"]["SessionEnd"], "not-an-array",
            "an unrecognized retired event survives verbatim"
        );
        assert_eq!(ours(&value, "SessionStart").len(), 1);
    }

    /// End-to-end sweep over the filesystem: the retired script is deleted, the
    /// removal is reported in the summary, and a second run reports nothing —
    /// the sweep is idempotent, not a permanent "removed" claim.
    #[test]
    fn materialize_sweeps_the_retired_hook_artifacts() {
        let tmp = TempDir::new().unwrap();
        let claude = tmp.path().join(".claude/hooks");
        fs::create_dir_all(&claude).unwrap();
        let retired_script = tmp.path().join(".claude/hooks/logos-quality-report.sh");
        fs::write(&retired_script, "#!/bin/sh\nexit 0\n").unwrap();
        fs::write(
            tmp.path().join(SETTINGS_REL),
            format!(
                r#"{{
                    "hooks": {{
                        "SessionEnd": [
                            {RETIRED_ENTRY},
                            {{ "hooks": [ {{ "type": "command", "command": "their-cleanup.sh" }} ] }}
                        ]
                    }}
                }}"#
            ),
        )
        .unwrap();

        let summary = materialize_quality_report(tmp.path(), false).unwrap();
        assert_eq!(summary.action, EmitAction::Created);
        assert_eq!(
            summary.retired_removed,
            vec![".claude/hooks/logos-quality-report.sh".to_string()],
            "the sweep is reported, not silent"
        );
        assert!(!retired_script.exists(), "the orphaned script is deleted");
        assert!(
            tmp.path().join(QUALITY_REPORT_HOOK_SCRIPT_REL).exists(),
            "the replacement script is written"
        );
        let settings: Value =
            serde_json::from_str(&fs::read_to_string(tmp.path().join(SETTINGS_REL)).unwrap())
                .unwrap();
        let end = settings["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(end.len(), 1);
        assert_eq!(end[0]["hooks"][0]["command"], "their-cleanup.sh");

        // Second run: nothing left to sweep, and no phantom removal reported.
        let again = materialize_quality_report(tmp.path(), false).unwrap();
        assert_eq!(again.action, EmitAction::Skipped);
        assert!(
            again.retired_removed.is_empty(),
            "the sweep does not keep claiming a removal it already made"
        );
    }

    /// An orphaned retired script with no corresponding settings entry is still
    /// swept — the settings merge short-circuits as `AlreadyPresent`, so the
    /// file-level sweep cannot be gated on the merge writing.
    #[test]
    fn orphaned_retired_script_is_swept_without_a_settings_write() {
        let tmp = TempDir::new().unwrap();
        materialize_quality_report(tmp.path(), false).unwrap();
        let before = fs::read_to_string(tmp.path().join(SETTINGS_REL)).unwrap();

        let orphan = tmp.path().join(".claude/hooks/logos-quality-report.sh");
        fs::write(&orphan, "#!/bin/sh\nexit 0\n").unwrap();

        let summary = materialize_quality_report(tmp.path(), false).unwrap();
        assert_eq!(summary.action, EmitAction::Skipped, "settings are already current");
        assert_eq!(summary.retired_removed.len(), 1, "the orphan is still removed");
        assert!(!orphan.exists());
        assert_eq!(
            fs::read_to_string(tmp.path().join(SETTINGS_REL)).unwrap(),
            before,
            "sweeping a stray file does not rewrite the settings document"
        );
    }

    /// A foreign settings file is never swept either: if we refuse to parse it,
    /// we refuse to delete artifacts it may still reference ([FR-IN-07]).
    #[test]
    fn foreign_settings_blocks_the_sweep() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".claude/hooks")).unwrap();
        let retired_script = tmp.path().join(".claude/hooks/logos-quality-report.sh");
        fs::write(&retired_script, "#!/bin/sh\nexit 0\n").unwrap();
        fs::write(tmp.path().join(SETTINGS_REL), "{ not json").unwrap();

        let summary = materialize_quality_report(tmp.path(), false).unwrap();
        assert_eq!(summary.action, EmitAction::Skipped);
        assert!(summary.notice.is_some(), "the refusal is explained");
        assert!(summary.retired_removed.is_empty());
        assert!(
            retired_script.exists(),
            "a file we will not parse is a file we will not sweep"
        );
    }

    // ── [FR-IN-07] session-start quality-report hook ─────────────────────────

    /// A fresh project gets the quality-report script (executable on Unix) and a
    /// **shared** `settings.json` carrying exactly one marker-tagged
    /// `SessionStart` entry.
    #[test]
    fn materialize_quality_report_writes_script_and_merges_shared_settings() {
        let tmp = TempDir::new().unwrap();
        let summary = materialize_quality_report(tmp.path(), false).unwrap();
        assert_eq!(summary.action, EmitAction::Created);
        assert_eq!(summary.settings, SETTINGS_REL, "the shared settings.json");
        assert!(summary.retired_removed.is_empty(), "nothing to sweep on a fresh project");

        let script = tmp.path().join(QUALITY_REPORT_HOOK_SCRIPT_REL);
        assert_eq!(fs::read_to_string(&script).unwrap(), QUALITY_REPORT_HOOK_SCRIPT);
        assert!(
            QUALITY_REPORT_HOOK_SCRIPT.contains("logos:quality-report:managed"),
            "the script is marker-tagged"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&script).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "the script is executable");
        }
        // The retired script is never written alongside the replacement.
        assert!(!tmp.path().join(".claude/hooks/logos-quality-report.sh").exists());

        // The per-developer settings.local.json is untouched — this hook is shared.
        assert!(
            !tmp.path().join(".claude/settings.local.json").exists(),
            "the quality-report hook never writes the per-developer settings.local.json"
        );
        let settings: Value =
            serde_json::from_str(&fs::read_to_string(tmp.path().join(SETTINGS_REL)).unwrap())
                .unwrap();
        let start = settings["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 1, "exactly one SessionStart entry");
        assert!(is_ours(&start[0], QUALITY_REPORT_HOOK_MARKER));
        assert_eq!(start[0]["matcher"], QUALITY_REPORT_HOOK_MATCHER);
        let cmd = start[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, QUALITY_REPORT_HOOK_COMMAND);
        assert!(cmd.contains("${CLAUDE_PROJECT_DIR}"));
        // No SessionEnd key is created by an install on a clean project.
        assert!(settings["hooks"].get("SessionEnd").is_none());
    }

    /// The quality-report merge is idempotent (skip + byte-identical) and
    /// `--force` re-emits exactly one managed `SessionStart` entry, no duplicates.
    #[test]
    fn quality_report_is_idempotent_and_force_re_emits() {
        let tmp = TempDir::new().unwrap();
        materialize_quality_report(tmp.path(), false).unwrap();
        let before = fs::read_to_string(tmp.path().join(SETTINGS_REL)).unwrap();

        let again = materialize_quality_report(tmp.path(), false).unwrap();
        assert_eq!(again.action, EmitAction::Skipped);
        assert!(again.notice.is_none());
        assert_eq!(
            fs::read_to_string(tmp.path().join(SETTINGS_REL)).unwrap(),
            before,
            "an unforced re-emit is byte-identical"
        );

        let forced = materialize_quality_report(tmp.path(), true).unwrap();
        assert_eq!(forced.action, EmitAction::Forced);
        let settings: Value =
            serde_json::from_str(&fs::read_to_string(tmp.path().join(SETTINGS_REL)).unwrap())
                .unwrap();
        assert_eq!(
            ours(&settings, "SessionStart").len(),
            1,
            "force never duplicates the managed entry"
        );
    }

    /// The quality-report merge preserves a foreign SessionStart entry and an
    /// unrelated PostToolUse entry it shares the file with, and refuses an
    /// unparseable `settings.json` ([FR-IN-07] never-clobber).
    #[test]
    fn quality_report_merge_preserves_foreign_and_coexists_with_other_events() {
        // A settings.json that already carries an unrelated PostToolUse entry
        // (owned by some other tool) plus a foreign SessionStart entry and an
        // unrelated key.
        let existing = r#"{
            "hooks": {
                "PostToolUse": [
                    { "matcher": "Bash", "hooks": [ { "type": "command", "command": "some-other-tool.sh" } ] }
                ],
                "SessionStart": [
                    { "hooks": [ { "type": "command", "command": "their-startup.sh" } ] }
                ]
            },
            "permissions": { "allow": ["Bash"] }
        }"#;
        let Merge::Write { json, forced, .. } =
            merge_settings(Some(existing), false, &QUALITY_REPORT_SPEC)
        else {
            panic!("expected a write");
        };
        assert!(!forced);
        let value: Value = serde_json::from_str(&json).unwrap();
        let start = value["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 2, "the foreign SessionStart entry survives alongside ours");
        assert!(start
            .iter()
            .any(|e| e["hooks"][0]["command"] == "their-startup.sh"));
        assert!(start.iter().any(|e| is_ours(e, QUALITY_REPORT_HOOK_MARKER)));
        // The unrelated PostToolUse entry is untouched — only SessionStart moved.
        let post = value["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 1);
        assert_eq!(post[0]["hooks"][0]["command"], "some-other-tool.sh");
        assert_eq!(value["permissions"]["allow"][0], "Bash");

        let Merge::Foreign { reason } =
            merge_settings(Some("{ not json"), false, &QUALITY_REPORT_SPEC)
        else {
            panic!("expected a foreign refusal");
        };
        assert!(reason.contains("settings.json"));
        assert!(reason.contains("not valid JSON"));
    }

    /// The quality-report script is offline, report-only, and carries the
    /// documented off-switch. It reads via `gate`/`check` only, and always exits
    /// 0 — it never makes a network or LLM call ([NFR-SE-01]) and never blocks a
    /// session ([FR-GV-05]).
    #[test]
    fn quality_report_script_is_offline_report_only() {
        for net in ["curl", "wget", "nc ", "http://", "https://"] {
            assert!(
                !QUALITY_REPORT_HOOK_SCRIPT.contains(net),
                "the quality-report script invokes no network client ({net})"
            );
        }
        // No LLM/agent spawn — this is a pure readout.
        assert!(
            !QUALITY_REPORT_HOOK_SCRIPT.contains("claude "),
            "the quality-report hook spawns no agent — it only reports"
        );
        // The documented off-switch env var.
        assert!(
            QUALITY_REPORT_HOOK_SCRIPT.contains("LOGOS_QUALITY_REPORT_DISABLE"),
            "off-switch env var"
        );
        // Reads the signal + baseline via `gate` and violations via `check`
        // (FR-GV-02/05), both `--no-reconcile`.
        assert!(QUALITY_REPORT_HOOK_SCRIPT.contains("logos gate --no-reconcile --json"));
        assert!(QUALITY_REPORT_HOOK_SCRIPT.contains("logos check --no-reconcile --json"));
        assert!(QUALITY_REPORT_HOOK_SCRIPT.contains("baseline_signal"));
        assert!(QUALITY_REPORT_HOOK_SCRIPT.contains("signal - baseline"));
        // The reconciling `scan` pass is gone (CR-095): `gate` yields both the
        // signal and the baseline, so the report never pays for a reconcile.
        assert!(
            !QUALITY_REPORT_HOOK_SCRIPT.contains("logos scan"),
            "no reconciling scan pass in the report path"
        );
        // No backtick command-substitution in a double-quoted `${:-}` default —
        // it would run an unwanted `logos index` as a side effect (regression guard).
        assert!(
            !QUALITY_REPORT_HOOK_SCRIPT.contains("`logos index`"),
            "no command-substitution side effect in the signal fallback"
        );
        // The readout rides STDOUT as one JSON object with the exact event name
        // the host's parser demands (CR-095) — a mismatch discards the payload.
        assert!(QUALITY_REPORT_HOOK_SCRIPT.contains(r#""hookEventName":"SessionStart""#));
        assert!(QUALITY_REPORT_HOOK_SCRIPT.contains("systemMessage"));
        assert!(QUALITY_REPORT_HOOK_SCRIPT.contains("additionalContext"));
        // The old stderr readout is gone: on SessionStart, stderr is not the
        // channel and exit-0 stdout is.
        assert!(
            !QUALITY_REPORT_HOOK_SCRIPT.contains("} >&2"),
            "the readout is no longer written to stderr"
        );
        assert!(
            QUALITY_REPORT_HOOK_SCRIPT.trim_end().ends_with("exit 0"),
            "the script always exits 0 — never blocks a session"
        );
    }

    /// Install a fake `logos` on PATH whose `gate`/`check` output is scripted,
    /// returning the PATH to run the hook under. `exit_code` applies to both
    /// subcommands, so a caller can prove the hook does not read a non-zero exit
    /// as a failure.
    #[cfg(unix)]
    fn fake_logos(dir: &Path, gate: &str, check: &str, exit_code: i32) -> String {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("fakebin");
        fs::create_dir_all(&bin).unwrap();
        let logos = bin.join("logos");
        fs::write(
            &logos,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n  \
                 gate)  printf '%s\\n' '{gate}';;\n  \
                 check) printf '%s\\n' '{check}';;\n\
                 esac\nexit {exit_code}\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&logos, fs::Permissions::from_mode(0o755)).unwrap();
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default())
    }

    /// Run the materialized script under a fake `logos`, returning (stdout, stderr).
    #[cfg(unix)]
    fn run_hook(tmp: &Path, path: &str, off_switch: bool) -> (String, String) {
        use std::process::Command;
        let script = tmp.join(QUALITY_REPORT_HOOK_SCRIPT_REL);
        let mut cmd = Command::new("sh");
        cmd.arg(&script)
            .env("PATH", path)
            .env("CLAUDE_PROJECT_DIR", tmp);
        if off_switch {
            cmd.env("LOGOS_QUALITY_REPORT_DISABLE", "1");
        } else {
            cmd.env_remove("LOGOS_QUALITY_REPORT_DISABLE");
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "the hook always exits 0");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// End-to-end behavior: run the materialized script against a fake `logos`
    /// and assert the actual payload — valid JSON on **stdout**, the exact
    /// `hookEventName`, and a readout carrying the signal, baseline, delta and
    /// (escaped-quote-safe) violation messages. Exercises the real script rather
    /// than string-matching the constant.
    #[cfg(unix)]
    #[test]
    fn quality_report_script_emits_parseable_session_start_json() {
        let tmp = TempDir::new().unwrap();
        materialize_quality_report(tmp.path(), false).unwrap();

        // The check output carries a message with an escaped quote, so the run
        // also proves the unescape/re-escape round-trip keeps the payload valid.
        let path = fake_logos(
            tmp.path(),
            r#"{"passed":true,"signal":8234,"baseline_signal":8100,"freshness":"assumed-fresh (--no-reconcile)"}"#,
            r#"{"passed":false,"violations":[{"severity":"error","message":"bad \"x\" import"},{"severity":"error","message":"cc too high"}]}"#,
            0,
        );
        let (stdout, _) = run_hook(tmp.path(), &path, false);

        let payload: Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("stdout must be one valid JSON object ({e}): {stdout}"));
        assert_eq!(
            payload["hookSpecificOutput"]["hookEventName"], "SessionStart",
            "the host's parser rejects any other event name"
        );
        let context = payload["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("additionalContext is a string");
        let summary = payload["systemMessage"].as_str().expect("systemMessage is a string");

        assert!(context.contains("signal:   8234"), "current signal: {context}");
        assert!(context.contains("baseline: 8100"), "baseline signal: {context}");
        assert!(context.contains("delta:    134"), "signal-vs-baseline delta: {context}");
        assert!(context.contains("rule violations: 2"), "violation count: {context}");
        assert!(context.contains("assumed-fresh"), "freshness line: {context}");
        // The escaped-quote message survives the round-trip intact, and both
        // violations are listed.
        assert!(context.contains(r#"bad "x" import"#), "escaped-quote message: {context}");
        assert!(context.contains("cc too high"), "second violation listed: {context}");
        // The one-line summary is what the user sees.
        assert!(summary.contains("8234"), "summary names the signal: {summary}");
        assert!(summary.contains("8100"), "summary names the baseline: {summary}");
        assert!(summary.contains("2 violation"), "summary names the count: {summary}");
    }

    /// A non-zero exit from `gate`/`check` is **not** a failure signal: `gate`
    /// exits 1 on a regression and `check` on an error violation, both by design
    /// ([FR-GV-03]). Reading the status instead of the output would report a
    /// regressed-but-healthy graph as unavailable — the inverse of honest.
    #[cfg(unix)]
    #[test]
    fn regression_exit_code_is_not_read_as_a_failure() {
        let tmp = TempDir::new().unwrap();
        materialize_quality_report(tmp.path(), false).unwrap();
        let path = fake_logos(
            tmp.path(),
            r#"{"passed":false,"signal":7900,"baseline_signal":8100}"#,
            r#"{"passed":false,"violations":[{"severity":"error","message":"rule breach"}]}"#,
            1,
        );
        let (stdout, _) = run_hook(tmp.path(), &path, false);
        let payload: Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
        let context = payload["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.contains("signal:   7900"), "reports the regressed signal: {context}");
        assert!(context.contains("delta:    -200"), "reports a negative delta: {context}");
        assert!(context.contains("rule violations: 1"), "{context}");
        assert!(
            !context.contains("unavailable"),
            "a regression is not an unavailable graph: {context}"
        );
    }

    /// Honest degradation: a graph another process has locked is reported as
    /// busy, and an absent one as unavailable — never as a healthy zeroed
    /// readout. Both still emit parseable JSON and exit 0.
    #[cfg(unix)]
    #[test]
    fn locked_and_absent_graphs_degrade_honestly() {
        for (output, expected) in [
            ("Error: database is locked", "graph busy"),
            ("Error: no index found", "graph unavailable"),
        ] {
            let tmp = TempDir::new().unwrap();
            materialize_quality_report(tmp.path(), false).unwrap();
            let path = fake_logos(tmp.path(), output, output, 1);
            let (stdout, _) = run_hook(tmp.path(), &path, false);
            let payload: Value = serde_json::from_str(stdout.trim())
                .unwrap_or_else(|e| panic!("valid JSON even when degraded ({e}): {stdout}"));
            let summary = payload["systemMessage"].as_str().unwrap();
            assert!(summary.contains(expected), "expected {expected:?} in {summary:?}");
            assert!(
                !summary.contains("signal:   0"),
                "never a zeroed readout rendered as healthy: {summary}"
            );
        }
    }

    /// The off-switch silences the hook entirely — no stdout, no stderr — and it
    /// still exits 0.
    #[cfg(unix)]
    #[test]
    fn off_switch_silences_the_hook() {
        let tmp = TempDir::new().unwrap();
        materialize_quality_report(tmp.path(), false).unwrap();
        let path = fake_logos(
            tmp.path(),
            r#"{"signal":8234,"baseline_signal":8100}"#,
            r#"{"violations":[]}"#,
            0,
        );
        let (stdout, stderr) = run_hook(tmp.path(), &path, true);
        assert!(
            stdout.is_empty() && stderr.is_empty(),
            "the off-switch silences the hook entirely: stdout={stdout:?} stderr={stderr:?}"
        );
    }

    /// A missing baseline is reported as such rather than as a delta against
    /// zero, and a `check` that cannot read the graph yields "n/a", never a
    /// truthful-looking "0 violations".
    #[cfg(unix)]
    #[test]
    fn missing_baseline_and_unreadable_check_are_not_fabricated() {
        let tmp = TempDir::new().unwrap();
        materialize_quality_report(tmp.path(), false).unwrap();
        let path = fake_logos(tmp.path(), r#"{"signal":8234,"baseline_signal":null}"#, "", 0);
        let (stdout, _) = run_hook(tmp.path(), &path, false);
        let payload: Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
        let context = payload["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.contains("baseline: n/a"), "no baseline: {context}");
        assert!(!context.contains("delta:"), "no delta without a baseline: {context}");
        assert!(
            context.contains("rule violations: n/a"),
            "an unreadable check is n/a, not 0: {context}"
        );
    }
}
