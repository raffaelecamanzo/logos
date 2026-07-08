//! The Claude Code SessionEnd quality-report hook and its materialization
//! ([FR-IN-07], [ADR-49], [CR-055]).
//!
//! A marker-tagged hook script (`.claude/hooks/logos-quality-report.sh`) is
//! materialized alongside a **non-clobbering merge** of a SessionEnd entry into
//! the project's `.claude/settings.json`. The merge is idempotent: an existing
//! managed entry is left untouched (recognized by our unique command path);
//! `force` re-emits it; and a foreign or unparseable `.claude/settings.json` is
//! never overwritten. Like the embedded skill ([`crate::wiki::skill`]) this is
//! pure local filesystem I/O — **no network, no LLM call** ([NFR-SE-01]).
//!
//! This module once also materialized a **PostToolUse wiki-augmentation hook**
//! ([FR-WK-14], [ADR-33]) that ran `wiki generate` after every index/sync and
//! surfaced the queue to the connected agent. It was retired by [CR-070]: the
//! perpetually-non-empty queue made every firing unactioned context noise, and
//! its "writes it" complement had already been retired by [CR-047] in favour of
//! the `ui`-gated in-process generator ([FR-WK-18]). The generic hook-spec +
//! settings-merge engine below (`HookSpec`, `merge_settings`,
//! `materialize_spec`) is shared machinery that survives the retirement, now
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

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::skill::EmitAction;

/// The Claude Code settings file the SessionEnd entry merges into, repo-relative.
pub const SETTINGS_REL: &str = ".claude/settings.json";

// ── The SessionEnd quality-report hook ([FR-IN-07], [FR-GV-05], [FR-GV-09], [ADR-49], [CR-055]) ──

/// The quality-report hook script, repo-relative ([FR-IN-07]).
pub const QUALITY_REPORT_HOOK_SCRIPT_REL: &str = ".claude/hooks/logos-quality-report.sh";

/// The quality-report hook command wired into the **shared** `.claude/settings.json`
/// ([FR-IN-07] — a project-wide readout). Uses the same `${CLAUDE_PROJECT_DIR}`
/// placeholder convention as the other hooks.
const QUALITY_REPORT_HOOK_COMMAND: &str =
    "${CLAUDE_PROJECT_DIR}/.claude/hooks/logos-quality-report.sh";

/// The quality-report hook's idempotency / ownership marker: its unique script
/// basename, found in the command of an entry we own.
const QUALITY_REPORT_HOOK_MARKER: &str = "logos-quality-report.sh";

/// The marker-tagged SessionEnd quality-report hook script ([FR-IN-07],
/// [FR-GV-05], [FR-GV-09], [ADR-49]). POSIX `sh`, **report-only** by
/// construction: it ALWAYS exits 0 (never blocks session teardown) and prints
/// the current quality signal, the blessed baseline signal and their delta, and
/// any architecture-rule violations as session context.
///
/// It makes **no** network or LLM call ([NFR-SE-01]) — it only shells out to the
/// pure-read quality commands: `logos scan` (the current signal, [FR-GV-09]),
/// `logos gate` (the blessed `baseline_signal`, the only surface exposing it,
/// [FR-GV-05]), and `logos check` (rule violations, [FR-GV-02]). `check`/`gate`'s
/// non-zero exit on a regression is deliberately **not** propagated — this is the
/// non-blocking report tier, distinct from the enforcing `pre-push` gate.
/// `LOGOS_QUALITY_REPORT_DISABLE` disables it without uninstalling.
const QUALITY_REPORT_HOOK_SCRIPT: &str = r#"#!/bin/sh
# logos:quality-report:managed — Claude Code SessionEnd quality-report hook (FR-IN-07, FR-GV-05, FR-GV-09, ADR-49).
#
# On session end this prints a NON-BLOCKING quality readout: the current quality
# signal (logos scan), the blessed baseline signal and its delta (logos gate),
# and any architecture-rule violations (logos check). It is REPORT-ONLY by
# construction — it ALWAYS exits 0 and never propagates check/gate's non-zero
# exit, so it can never block session teardown (this is the report tier, not the
# enforcing pre-push gate). Logos makes no LLM or network call here (NFR-SE-01):
# check/scan/gate are pure local reads over the graph.
#
# Honest degradation: at session teardown another logos process (e.g. the
# still-alive MCP server) can briefly hold the graph-DB write lock, so `scan`
# may fail with "database is locked". This hook CAPTURES that error instead of
# swallowing it, and reports "graph busy — skipped" rather than mis-rendering a
# healthy, indexed project as un-indexed with a zeroed readout.
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

# Current signal: `scan` reconciles then scores (FR-GV-09), persisting the
# snapshot the baseline comparison below reads back. Capture stdout+stderr and
# the exit code in one run so a failure is classified, not swallowed: on success
# stdout is the JSON; on failure the error text shares the same capture.
scan_out=$(logos scan --json 2>&1)
if [ $? -ne 0 ]; then
  # The graph could not be scored. Distinguish a TRANSIENT lock (another logos
  # process holds the DB — the common teardown race with the MCP server) from a
  # genuinely absent/uninitialized graph, so the readout never lies.
  if printf '%s' "$scan_out" | grep -qi 'database is locked'; then
    printf 'logos quality report (session end): graph busy (locked by another logos process) — skipped.\n' >&2
  else
    printf 'logos quality report (session end): graph unavailable (run logos index first) — skipped.\n' >&2
  fi
  exit 0
fi
signal=$(printf '%s' "$scan_out" | grep -oE '"signal":[0-9]+' | head -1 | grep -oE '[0-9]+')

# Baseline signal: only `gate` exposes the blessed `baseline_signal` (FR-GV-05).
# Reuse scan's fresh reconcile (--no-reconcile) so this adds no extra graph pass.
gate_json=$(logos gate --no-reconcile --json 2>/dev/null)
baseline=$(printf '%s' "$gate_json" | grep -oE '"baseline_signal":[0-9]+' | head -1 | grep -oE '[0-9]+')

# Rule violations: `check` (FR-GV-02). Report-only — its non-zero exit on an
# error violation is deliberately NOT propagated (we always exit 0 below). Only
# trust a count when the violations array is present in the output: `check`
# also exits non-zero with empty output when it cannot read the graph, and a
# blind grep would then mis-report that as a truthful "0 violations".
check_json=$(logos check --no-reconcile --json 2>/dev/null)
if printf '%s' "$check_json" | grep -q '"violations"'; then
  violations=$(printf '%s' "$check_json" | grep -oE '"severity":"[a-z]+"' | grep -c '.')
else
  violations=""
fi

# --- render the readout (session context) ---------------------------------
# A SessionEnd hook cannot inject context back into the ending session and its
# stdout is discarded — only STDERR reaches the user's terminal. So the whole
# readout is written to stderr (>&2).
{
  printf 'logos quality report (session end):\n'
  printf '  signal:   %s\n' "${signal:-n/a}"
  if [ -n "$baseline" ]; then
    printf '  baseline: %s\n' "$baseline"
    [ -n "$signal" ] && printf '  delta:    %s\n' "$((signal - baseline))"
  else
    printf '  baseline: n/a (none saved — bless one with `logos gate --save`)\n'
  fi
  printf '  rule violations: %s\n' "${violations:-n/a (check unavailable)}"

  # List the violation messages (report-only detail), capped for brevity.
  if [ "${violations:-0}" -gt 0 ] 2>/dev/null; then
    # Match the full message value, allowing escaped chars (\" \\ ...) so a
    # message containing a quote is not truncated at the first `"`; then strip
    # the key/quotes and unescape \" for display.
    printf '%s' "$check_json" \
      | grep -oE '"message":"([^"\\]|\\.)*"' \
      | sed -e 's/^"message":"//' -e 's/"$//' -e 's/\\"/"/g' \
      | head -20 \
      | while IFS= read -r m; do printf '    - %s\n' "$m"; done
  fi
} >&2

# ALWAYS exit 0: this hook reports, it never blocks teardown (FR-GV-05).
exit 0
"#;

/// One materializable Claude Code hook: its script artifact plus the settings
/// merge target. Generalizes the marker-tagged / idempotent / non-clobbering
/// settings-merge machinery so it is written exactly once; today only the
/// [FR-IN-07] SessionEnd quality-report hook is materialized through it (the
/// [FR-WK-14] PostToolUse wiki-augmentation hook this once also drove was
/// retired by [CR-070]).
struct HookSpec {
    /// The hook script path, repo-relative.
    script_rel: &'static str,
    /// The settings file the entry merges into, repo-relative.
    settings_rel: &'static str,
    /// The Claude Code hook event the entry registers under (e.g. `SessionEnd`).
    event: &'static str,
    /// The wired command (uses the `${CLAUDE_PROJECT_DIR}` placeholder).
    command: &'static str,
    /// The idempotency / ownership marker: our unique script basename, found in
    /// the command of an entry we own.
    marker: &'static str,
    /// The marker-tagged script body.
    script: &'static str,
}

/// The [FR-IN-07] SessionEnd quality-report hook spec. Registers under
/// `SessionEnd` in the shared `.claude/settings.json` ([FR-IN-07]); the merge
/// touches only the `hooks.SessionEnd` array, leaving every other key and
/// event (and any foreign entry) verbatim.
const QUALITY_REPORT_SPEC: HookSpec = HookSpec {
    script_rel: QUALITY_REPORT_HOOK_SCRIPT_REL,
    settings_rel: SETTINGS_REL,
    event: "SessionEnd",
    command: QUALITY_REPORT_HOOK_COMMAND,
    marker: QUALITY_REPORT_HOOK_MARKER,
    script: QUALITY_REPORT_HOOK_SCRIPT,
};

/// The outcome of materializing a Claude Code hook — currently only the
/// [FR-IN-07] SessionEnd quality-report hook — a `Serialize` read-model the CLI
/// renders and `init` folds into its step list.
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
}

/// What the settings merge resolved to — a pure function of the existing file
/// content and `force`, isolated for unit testing.
#[derive(Debug, PartialEq, Eq)]
enum Merge {
    /// Our managed entry is already present and `force` was not given.
    AlreadyPresent,
    /// Write this serialized settings document; `forced` distinguishes a
    /// re-emit (entry was present) from a first install.
    Write { json: String, forced: bool },
    /// A foreign/unparseable settings file — never overwritten ([FR-IN-07]).
    Foreign { reason: String },
}

/// Materialize the [FR-IN-07] SessionEnd quality-report hook under `base`.
///
/// Writes `<base>/.claude/hooks/logos-quality-report.sh` and merges a
/// marker-tagged SessionEnd entry into `<base>/.claude/settings.json`.
/// **Idempotent and non-clobbering:** an existing managed entry (recognized by
/// its command path) is left untouched unless `force`; a foreign or unparseable
/// settings file is never overwritten. Installing the hook performs **no** LLM
/// call and opens **no** network connection ([NFR-SE-01]) — the hook only
/// shells out to the pure-read `scan`/`gate`/`check` commands at session end,
/// and always exits 0 ([FR-GV-05] report tier).
///
/// # Errors
/// Returns an error only when a Logos-owned path cannot be created or written.
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

    let summary_base = |action, notice| HookEmitSummary {
        script: spec.script_rel.to_string(),
        settings: spec.settings_rel.to_string(),
        action,
        notice,
    };

    match merge_settings(existing.as_deref(), force, spec) {
        Merge::AlreadyPresent => Ok(summary_base(EmitAction::Skipped, None)),
        Merge::Foreign { reason } => Ok(summary_base(EmitAction::Skipped, Some(reason))),
        Merge::Write { json, forced } => {
            // Write the script first so the wired entry never points at a
            // missing file, then commit the settings merge.
            write_script(base, spec)?;
            if let Some(parent) = settings_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            fs::write(&settings_path, json)
                .with_context(|| format!("writing {}", settings_path.display()))?;
            tracing::info!(
                script = spec.script_rel,
                settings = spec.settings_rel,
                event = spec.event,
                forced,
                "wiki hook materialized"
            );
            Ok(summary_base(
                if forced {
                    EmitAction::Forced
                } else {
                    EmitAction::Created
                },
                None,
            ))
        }
    }
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
    json!({
        "hooks": [ { "type": "command", "command": spec.command } ],
    })
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
/// `spec.event` array (e.g. `SessionEnd`) is the only key touched; every other
/// key and a foreign entry survive verbatim.
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
    if present && !force {
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

    // ── Generic `merge_settings` machinery, verified via the [FR-IN-07]
    // quality-report spec ([CR-070]: the retired augment spec was previously
    // the vehicle for this coverage) ──────────────────────────────────────────

    /// A pre-existing foreign SessionEnd entry survives our merge untouched —
    /// we append alongside it, never clobber it.
    #[test]
    fn merge_preserves_a_foreign_entry() {
        let existing = r#"{
            "hooks": {
                "SessionEnd": [
                    { "hooks": [ { "type": "command", "command": "my-own.sh" } ] }
                ]
            },
            "permissions": { "allow": ["Bash"] }
        }"#;
        let Merge::Write { json, forced } =
            merge_settings(Some(existing), false, &QUALITY_REPORT_SPEC)
        else {
            panic!("expected a write");
        };
        assert!(!forced, "a first install is not a forced re-emit");
        let value: Value = serde_json::from_str(&json).unwrap();
        let end = value["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(end.len(), 2, "the foreign entry is preserved alongside ours");
        assert!(end.iter().any(|e| e["hooks"][0]["command"] == "my-own.sh"));
        assert!(end.iter().any(|e| is_ours(e, QUALITY_REPORT_HOOK_MARKER)));
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

    /// A settings file whose shape is wrong anywhere on the `hooks.SessionEnd`
    /// path — including a valid-JSON-but-non-object root — is foreign and never
    /// overwritten.
    #[test]
    fn structurally_foreign_settings_is_refused() {
        let bad = r#"{ "hooks": { "SessionEnd": "not-an-array" } }"#;
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

    /// The materialized settings document is valid JSON ending in a newline,
    /// and the wired command uses the `${CLAUDE_PROJECT_DIR}` placeholder.
    #[test]
    fn merged_document_is_well_formed() {
        let Merge::Write { json, .. } = merge_settings(None, false, &QUALITY_REPORT_SPEC) else {
            panic!("expected a write");
        };
        assert!(json.ends_with('\n'), "trailing newline like .mcp.json");
        let value: Value = serde_json::from_str(&json).expect("valid JSON");
        let cmd = value["hooks"]["SessionEnd"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(cmd, QUALITY_REPORT_HOOK_COMMAND);
        assert!(cmd.contains("${CLAUDE_PROJECT_DIR}"), "uses the placeholder");
    }

    // ── [FR-IN-07] SessionEnd quality-report hook ────────────────────────────

    /// A fresh project gets the quality-report script (executable on Unix) and a
    /// **shared** `settings.json` carrying exactly one marker-tagged SessionEnd
    /// entry.
    #[test]
    fn materialize_quality_report_writes_script_and_merges_shared_settings() {
        let tmp = TempDir::new().unwrap();
        let summary = materialize_quality_report(tmp.path(), false).unwrap();
        assert_eq!(summary.action, EmitAction::Created);
        assert_eq!(summary.settings, SETTINGS_REL, "the shared settings.json");

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

        // The per-developer settings.local.json is untouched — this hook is shared.
        assert!(
            !tmp.path().join(".claude/settings.local.json").exists(),
            "the quality-report hook never writes the per-developer settings.local.json"
        );
        let settings: Value =
            serde_json::from_str(&fs::read_to_string(tmp.path().join(SETTINGS_REL)).unwrap())
                .unwrap();
        let end = settings["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(end.len(), 1, "exactly one SessionEnd entry");
        assert!(is_ours(&end[0], QUALITY_REPORT_HOOK_MARKER));
        // SessionEnd matches every session end — no tool matcher.
        assert!(
            end[0].get("matcher").is_none(),
            "a matcher-less SessionEnd entry fires on every session end"
        );
        let cmd = end[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, QUALITY_REPORT_HOOK_COMMAND);
        assert!(cmd.contains("${CLAUDE_PROJECT_DIR}"));
    }

    /// The quality-report merge is idempotent (skip + byte-identical) and
    /// `--force` re-emits exactly one managed SessionEnd entry, no duplicates.
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
        let ours = settings["hooks"]["SessionEnd"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| is_ours(e, QUALITY_REPORT_HOOK_MARKER))
            .count();
        assert_eq!(ours, 1, "force never duplicates the managed entry");
    }

    /// The quality-report merge preserves a foreign SessionEnd entry and an
    /// unrelated PostToolUse entry it shares the file with, and refuses an
    /// unparseable `settings.json` ([FR-IN-07] never-clobber).
    #[test]
    fn quality_report_merge_preserves_foreign_and_coexists_with_other_events() {
        // A settings.json that already carries an unrelated PostToolUse entry
        // (owned by some other tool) plus a foreign SessionEnd entry and an
        // unrelated key.
        let existing = r#"{
            "hooks": {
                "PostToolUse": [
                    { "matcher": "Bash", "hooks": [ { "type": "command", "command": "some-other-tool.sh" } ] }
                ],
                "SessionEnd": [
                    { "hooks": [ { "type": "command", "command": "their-cleanup.sh" } ] }
                ]
            },
            "permissions": { "allow": ["Bash"] }
        }"#;
        let Merge::Write { json, forced } =
            merge_settings(Some(existing), false, &QUALITY_REPORT_SPEC)
        else {
            panic!("expected a write");
        };
        assert!(!forced);
        let value: Value = serde_json::from_str(&json).unwrap();
        let end = value["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(end.len(), 2, "the foreign SessionEnd entry survives alongside ours");
        assert!(end
            .iter()
            .any(|e| e["hooks"][0]["command"] == "their-cleanup.sh"));
        assert!(end.iter().any(|e| is_ours(e, QUALITY_REPORT_HOOK_MARKER)));
        // The unrelated PostToolUse entry is untouched — only SessionEnd moved.
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
    /// documented off-switch. It reads via `scan`/`gate`/`check` only, prints the
    /// signal / baseline / violations to stderr, and always exits 0 — it never
    /// makes a network or LLM call ([NFR-SE-01]) and never blocks teardown
    /// ([FR-GV-05]).
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
        // Runs check + scan and reads the baseline via gate (FR-GV-02/05/09).
        assert!(QUALITY_REPORT_HOOK_SCRIPT.contains("logos scan --json"));
        assert!(QUALITY_REPORT_HOOK_SCRIPT.contains("logos check"));
        assert!(QUALITY_REPORT_HOOK_SCRIPT.contains("logos gate"));
        // Surfaces the signal, the baseline signal, and the delta.
        assert!(QUALITY_REPORT_HOOK_SCRIPT.contains("baseline_signal"));
        assert!(QUALITY_REPORT_HOOK_SCRIPT.contains("signal - baseline"));
        // No backtick command-substitution in a double-quoted `${:-}` default —
        // it would run an unwanted `logos index` as a side effect (regression guard).
        assert!(
            !QUALITY_REPORT_HOOK_SCRIPT.contains("`logos index`"),
            "no command-substitution side effect in the signal fallback"
        );
        // Report-only: the readout goes to stderr (SessionEnd stdout is dropped)
        // and the script always exits 0.
        assert!(QUALITY_REPORT_HOOK_SCRIPT.contains("} >&2"));
        assert!(
            QUALITY_REPORT_HOOK_SCRIPT.trim_end().ends_with("exit 0"),
            "the script always exits 0 — never blocks teardown"
        );
    }

    /// End-to-end behavior: run the materialized script against a fake `logos` on
    /// PATH and assert the actual readout — the current signal, the baseline, the
    /// delta, and the (escaped-quote-safe) violation messages — reaches stderr and
    /// the script exits 0; then assert the off-switch silences it entirely. This
    /// exercises the real script rather than string-matching the constant.
    #[cfg(unix)]
    #[test]
    fn quality_report_script_runs_reports_to_stderr_and_off_switch_silences() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let tmp = TempDir::new().unwrap();
        materialize_quality_report(tmp.path(), false).unwrap();
        let script = tmp.path().join(QUALITY_REPORT_HOOK_SCRIPT_REL);

        // A fake `logos` emitting compact single-line JSON. The check output
        // carries a message with an escaped quote, so the run also proves the
        // escaped-quote extraction fix (no truncation).
        let bin = tmp.path().join("fakebin");
        fs::create_dir_all(&bin).unwrap();
        let logos = bin.join("logos");
        fs::write(
            &logos,
            "#!/bin/sh\ncase \"$1\" in\n  \
             scan)  echo '{\"signal\":8234,\"violations\":[]}';;\n  \
             gate)  echo '{\"passed\":true,\"signal\":8234,\"baseline_signal\":8100}';;\n  \
             check) echo '{\"passed\":false,\"violations\":[{\"severity\":\"error\",\"message\":\"bad \\\"x\\\" import\"},{\"severity\":\"error\",\"message\":\"cc too high\"}]}';;\n\
             esac\n",
        )
        .unwrap();
        fs::set_permissions(&logos, fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );

        // Normal run: the readout reaches stderr and the script exits 0.
        let out = Command::new("sh")
            .arg(&script)
            .env("PATH", &path)
            .env("CLAUDE_PROJECT_DIR", tmp.path())
            .env_remove("LOGOS_QUALITY_REPORT_DISABLE")
            .output()
            .unwrap();
        assert!(out.status.success(), "always exits 0");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("signal:   8234"), "current signal: {err}");
        assert!(err.contains("baseline: 8100"), "baseline signal: {err}");
        assert!(err.contains("delta:    134"), "signal-vs-baseline delta: {err}");
        assert!(err.contains("rule violations: 2"), "violation count: {err}");
        // The escaped-quote message survives intact (regression guard for the
        // grep truncation fix), and both violations are listed.
        assert!(err.contains("bad \"x\" import"), "escaped-quote message intact: {err}");
        assert!(err.contains("cc too high"), "second violation listed: {err}");

        // Off-switch: the hook is silent and still exits 0.
        let off = Command::new("sh")
            .arg(&script)
            .env("PATH", &path)
            .env("CLAUDE_PROJECT_DIR", tmp.path())
            .env("LOGOS_QUALITY_REPORT_DISABLE", "1")
            .output()
            .unwrap();
        assert!(off.status.success(), "off-switch still exits 0");
        assert!(
            off.stderr.is_empty() && off.stdout.is_empty(),
            "the off-switch silences the hook entirely"
        );
    }
}
