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

/// The Claude Code settings file the `SessionStart` entry merges into,
/// repo-relative.
pub const SETTINGS_REL: &str = ".claude/settings.json";

// ── The session-start quality-report hook ([FR-IN-07], [FR-GV-02], [FR-GV-05], [ADR-49], [CR-055], [CR-095]) ──

/// The quality-report hook script, repo-relative ([FR-IN-07]).
pub const QUALITY_REPORT_HOOK_SCRIPT_REL: &str = ".claude/hooks/logos-quality-open.sh";

/// The quality-report hook command wired into the **shared** `.claude/settings.json`
/// ([FR-IN-07] — a project-wide readout). Uses the same `${CLAUDE_PROJECT_DIR}`
/// placeholder convention as the other hooks.
const QUALITY_REPORT_HOOK_COMMAND: &str = "${CLAUDE_PROJECT_DIR}/.claude/hooks/logos-quality-open.sh";

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
    /// The **exact** command a prior Logos version wrote, matched verbatim.
    ///
    /// Not a substring: the command we wrote is fully known, so loose matching
    /// buys nothing and costs correctness. `contains("logos-quality-report.sh")`
    /// would also claim a user's vendored fork
    /// (`hooks/vendor/logos-quality-report.sh`), a wrapper invoking it
    /// (`my-wrapper.sh --after logos-quality-report.sh`), and even a reminder
    /// that merely names the file — and then delete them.
    command: &'static str,
    /// The retired script artifact, repo-relative — deleted only when it is a
    /// regular file carrying [`MANAGED_MARKER`].
    script_rel: &'static str,
}

/// The retired SessionEnd quality-report hook ([CR-095]).
const RETIRED_HOOKS: &[RetiredHook] = &[RetiredHook {
    event: "SessionEnd",
    command: "${CLAUDE_PROJECT_DIR}/.claude/hooks/logos-quality-report.sh",
    script_rel: ".claude/hooks/logos-quality-report.sh",
}];

/// The tag every Logos-authored hook script carries in its header.
///
/// The sweep requires it in a file's contents before deleting: Logos no longer
/// writes `logos-quality-report.sh`, so that path is unclaimed and a user may
/// legitimately own it now. Deleting on path alone would destroy their file.
const MANAGED_MARKER: &str = "logos:quality-report:managed";

/// The marker-tagged session-start quality-report hook script ([FR-IN-07],
/// [ADR-49], [CR-095]). POSIX `sh`, **report-only** by construction: it ALWAYS
/// exits 0, so it can never block a session.
///
/// Deliberately a **launcher, not a program** ([CR-095]). It gates on the
/// off-switch, checks the binary is present, enters the project, and execs
/// `logos quality-report --hook-json`; the readout and its JSON are built in the
/// binary by `serde_json`. An earlier revision assembled the payload in shell
/// and got every escaping edge wrong — raw control bytes produced invalid JSON
/// (which makes the host discard the whole readout silently), a stderr line
/// containing `"signal":N` was parsed as the signal and reported as truth, and
/// `grep`-based field extraction was silently coupled to compact JSON. None of
/// that is expressible here, because there is no parsing and no escaping left in
/// the script.
///
/// Makes **no** network or LLM call ([NFR-SE-01]) — the one command it runs is a
/// pure local read that writes nothing ([FR-GV-06]: the report tier must not grow
/// the evolution series). Deliberately **not** `exec`: the command's failure is
/// swallowed and the script exits 0 regardless, so a PATH binary too old to know
/// the subcommand degrades to silence rather than to a visible failed hook
/// ([FR-GV-05] report tier, distinct from the enforcing `pre-push` gate).
/// `LOGOS_QUALITY_REPORT_DISABLE` disables it without uninstalling.
const QUALITY_REPORT_HOOK_SCRIPT: &str = r#"#!/bin/sh
# logos:quality-report:managed — Claude Code SessionStart quality-report hook (FR-IN-07, ADR-49, CR-095).
#
# Launches the readout; it does not compute or format it. `logos
# quality-report --hook-json` emits the whole payload as one JSON object on
# stdout — systemMessage for the user, hookSpecificOutput.additionalContext for
# the agent, hookEventName fixed to SessionStart — built with serde_json in the
# binary. Nothing is parsed or escaped here, deliberately: a malformed payload
# makes the host discard the readout entirely, and shell is the wrong tool for
# JSON.
#
# Report-only: this script always exits 0, so a session is never blocked. The
# command writes nothing — no metric snapshot, no graph write lock — so firing
# at every session start, resume and /clear leaves the evolution series
# untouched.
#
#   off-switch: export LOGOS_QUALITY_REPORT_DISABLE=1
#
# Regenerate with `logos wiki hook --emit --force` (or re-run `logos init -i`).

# Off-switch: disable the report without uninstalling the hook.
[ "${LOGOS_QUALITY_REPORT_DISABLE:-0}" = "1" ] && exit 0

# Best-effort: a missing binary is nothing to report.
command -v logos >/dev/null 2>&1 || exit 0

# Report on the project the host named; its own cwd is not dependable.
cd "${CLAUDE_PROJECT_DIR:-$(pwd)}" 2>/dev/null || exit 0

# Deliberately NOT `exec`: the hook script and the binary on PATH can be
# different vintages (a script emitted by a new `logos wiki hook --emit` while
# an older release is still PATH-promoted, which is the normal state mid-
# upgrade). An older binary does not know this subcommand and would exit 2 with
# a clap usage error on stderr — which the host surfaces to the user as a failed
# hook, turning a missing readout into a visible defect. Swallowing it keeps the
# report tier's contract: it reports when it can and is silent when it cannot,
# but it is never itself the problem.
logos --json quality-report --hook-json 2>/dev/null || exit 0

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
    /// The event-field matcher, compared by the host as an exact-match
    /// alternation list.
    ///
    /// Not an `Option`: a matcher-less entry fires on **every** occurrence of the
    /// event, which for `SessionStart` includes `compact` — mid-task
    /// auto-compaction, the readout-as-noise pattern [CR-095] deliberately
    /// excludes. Making the axis unrepresentable is worth more than the
    /// generality; the retired [FR-WK-14] spec that once justified `None` is gone.
    matcher: &'static str,
    /// The declared per-hook timeout in seconds — never inherit a host default
    /// ([CR-095]).
    timeout_secs: u64,
    /// The wired command (uses the `${CLAUDE_PROJECT_DIR}` placeholder). Also
    /// the ownership token: an entry is ours when a hook's command equals this
    /// exactly ([CR-095] — never a substring test).
    command: &'static str,
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
    matcher: QUALITY_REPORT_HOOK_MATCHER,
    timeout_secs: QUALITY_REPORT_HOOK_TIMEOUT_SECS,
    command: QUALITY_REPORT_HOOK_COMMAND,
    script: QUALITY_REPORT_HOOK_SCRIPT,
    retired: RETIRED_HOOKS,
};

/// The outcome of materializing a Claude Code hook — currently only the
/// [FR-IN-07] session-start quality-report hook — a `Serialize` read-model the
/// CLI renders and `init` folds into its step list.
///
/// `action` reuses [`EmitAction`] for a uniform CLI JSON shape with the skill
/// (`"action":"created"|"forced"|"reconciled"|"skipped"`). A
/// [`EmitAction::Skipped`] is disambiguated by `notice`: `None` means "already
/// present" (idempotent re-run); `Some(reason)` means a foreign/unsafe
/// `.claude/settings.json` was left untouched.
///
/// [`EmitAction::Reconciled`] is this materializer's own variant: unlike the
/// skill, it rewrites a present artifact without `--force` when Logos needs the
/// shape changed, and a consumer must be able to tell that from the destructive
/// [`EmitAction::Forced`] ([CR-095]).
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
    /// Retired artifacts removed by this emit, repo-relative ([CR-095]).
    ///
    /// Covers **both** halves of the sweep — the settings entry and the script
    /// file — because either alone is a destructive edit the user must be told
    /// about. Reporting only deleted files would stay silent in the common case
    /// where `.claude/settings.json` is committed and `.claude/hooks/` is
    /// ignored: the entry disappears from a shared file with nothing said.
    /// Empty on a clean install and on every subsequent re-run.
    pub retired_removed: Vec<String>,
    /// Retired artifacts left in place because the settings shape around them
    /// was unrecognized, so removing the script would orphan a live entry
    /// ([CR-095]). Surfaced rather than silently skipped.
    pub retired_skipped: Vec<String>,
}

/// What the settings merge resolved to — a pure function of the existing file
/// content and `force`, isolated for unit testing.
#[derive(Debug, PartialEq, Eq)]
enum Merge {
    /// Our managed entry is already present, `force` was not given, and there
    /// was nothing retired left to sweep.
    AlreadyPresent,
    /// Write this serialized settings document. `action` is the reported outcome,
    /// resolved **here** rather than from a bool at the call site: three
    /// different situations reach this arm — a first install, a `--force`
    /// re-emit, and an unforced rewrite Logos needed (drift reconciliation or a
    /// retirement sweep) — and only the middle one may claim
    /// [`EmitAction::Forced`], which promises local edits were overwritten.
    /// Carrying `forced: bool` conflated the last two ([CR-095]).
    ///
    /// `swept` names the retired scripts whose settings hooks this merge removed,
    /// and `unsweepable` those whose event shape it did not recognize — the file
    /// sweep must skip the latter or it would orphan a surviving entry
    /// ([CR-095]).
    Write {
        json: String,
        action: EmitAction,
        swept: Vec<&'static str>,
        unsweepable: Vec<&'static str>,
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
/// call and opens **no** network connection ([NFR-SE-01]) — the hook only shells
/// out to `logos quality-report`, which computes the signal without persisting a
/// snapshot ([FR-GV-06]) and always exits 0 ([FR-GV-05] report tier).
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

    let summary_base = |action, notice, removed: Vec<String>, skipped: Vec<String>| {
        HookEmitSummary {
            script: spec.script_rel.to_string(),
            settings: spec.settings_rel.to_string(),
            action,
            notice,
            retired_removed: removed,
            retired_skipped: skipped,
        }
    };

    match merge_settings(existing.as_deref(), force, spec) {
        // A settings file we refuse to parse is also one we refuse to sweep:
        // leave every artifact exactly as it is ([FR-IN-07] never-overwrite).
        Merge::Foreign { reason } => Ok(summary_base(
            EmitAction::Skipped,
            Some(reason),
            Vec::new(),
            Vec::new(),
        )),
        // Nothing to write, but an orphaned retired script can still be on disk
        // (a prior sweep that removed the entry, or a hand-edited settings file).
        Merge::AlreadyPresent => {
            let sweep = sweep_retired_scripts(base, spec, &[]);
            Ok(summary_base(
                EmitAction::Skipped,
                None,
                sweep.removed,
                sweep.skipped,
            ))
        }
        Merge::Write {
            json,
            action,
            swept,
            unsweepable,
        } => {
            // Write the script first so the wired entry never points at a
            // missing file, then commit the settings merge.
            write_script(base, spec)?;
            if let Some(parent) = settings_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            fs::write(&settings_path, json)
                .with_context(|| format!("writing {}", settings_path.display()))?;
            let sweep = sweep_retired_scripts(base, spec, &unsweepable);
            // Both halves of the sweep are reported: the settings hooks removed
            // by the merge (`swept`) and the script files removed here.
            let mut removed = sweep.removed;
            for script_rel in swept {
                let entry = format!("{script_rel} (settings entry)");
                if !removed.contains(&entry) {
                    removed.push(entry);
                }
            }
            tracing::info!(
                script = spec.script_rel,
                settings = spec.settings_rel,
                event = spec.event,
                ?action,
                retired_removed = removed.len(),
                retired_skipped = sweep.skipped.len(),
                "wiki hook materialized"
            );
            Ok(summary_base(action, None, removed, sweep.skipped))
        }
    }
}

/// What the file-level sweep did ([CR-095]).
#[derive(Debug, Default, PartialEq, Eq)]
struct ScriptSweep {
    /// Repo-relative paths actually deleted.
    removed: Vec<String>,
    /// Paths deliberately left alone, each with the reason — a file Logos did
    /// not write, a non-regular path, an unrecognized settings shape, or a
    /// failed unlink.
    skipped: Vec<String>,
}

/// Delete the retired hooks' orphaned script artifacts ([CR-095]).
///
/// Deliberately conservative on all four axes, because this is the one place
/// Logos deletes a file:
///
/// - **Ownership is verified, not assumed.** Logos no longer writes
///   `logos-quality-report.sh`, so that path is unclaimed and a user may
///   legitimately own it now. The file must contain [`MANAGED_MARKER`] — the tag
///   every Logos-authored hook script carries — or it is left alone. Deleting on
///   path alone would destroy a file the user wrote and Logos never did.
/// - **Regular files only.** `symlink_metadata` is used rather than `exists()`,
///   which follows links: a symlink at that path is left in place (removing the
///   link would break the user's wiring), and a *dangling* symlink — which
///   `exists()` reports as absent — is reported as skipped rather than silently
///   ignored.
/// - **An unrecognized settings shape blocks the delete** (`unsweepable`): if
///   the merge declined to touch a retired entry, the script it references must
///   stay, or a live registration is left pointing at nothing.
/// - **Failure degrades, it never aborts.** A failed unlink is recorded and the
///   install still succeeds. Propagating it would abort `init::run` *after* the
///   settings write already landed, discarding the whole step list and telling
///   the user a successful install failed.
fn sweep_retired_scripts(base: &Path, spec: &HookSpec, unsweepable: &[&str]) -> ScriptSweep {
    let mut sweep = ScriptSweep::default();
    for retired in spec.retired {
        let rel = retired.script_rel;
        if unsweepable.contains(&rel) {
            sweep
                .skipped
                .push(format!("{rel} (left: its settings entry was not recognized)"));
            continue;
        }
        let path = base.join(rel);
        let Ok(meta) = fs::symlink_metadata(&path) else {
            // Genuinely absent — the common, silent case.
            continue;
        };
        if !meta.is_file() {
            sweep
                .skipped
                .push(format!("{rel} (left: not a regular file)"));
            continue;
        }
        match fs::read_to_string(&path) {
            Ok(body) if body.contains(MANAGED_MARKER) => match fs::remove_file(&path) {
                Ok(()) => sweep.removed.push(rel.to_string()),
                Err(err) => sweep.skipped.push(format!("{rel} (left: {err})")),
            },
            Ok(_) => sweep
                .skipped
                .push(format!("{rel} (left: not a Logos-authored script)")),
            Err(err) => sweep.skipped.push(format!("{rel} (left: {err})")),
        }
    }
    sweep
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

/// The settings entry this hook installs: the spec's source matcher wrapping one
/// `command` hook.
///
/// Both fields are unconditional. The matcher bounds *which* session starts fire
/// the hook (see [`HookSpec::matcher`]), and the declared timeout is never
/// omitted — inheriting an undocumented host default is what broke the retired
/// SessionEnd hook ([CR-095]).
fn hook_entry(spec: &HookSpec) -> Value {
    json!({
        "matcher": spec.matcher,
        "hooks": [ { "type": "command", "command": spec.command, "timeout": spec.timeout_secs } ],
    })
}

/// Is this individual hook object one Logos wrote, i.e. does its command match
/// `command` exactly?
///
/// Exact, not `contains`: the command Logos writes is fully known, so a substring
/// test only widens the blast radius onto commands that merely *mention* our
/// script name.
fn is_our_hook(hook: &Value, command: &str) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| c == command)
}

/// Does this hook entry contain a hook Logos wrote?
///
/// Note what this does **not** license: an entry containing one of ours may also
/// contain the user's own hooks, so it must be *pruned* at hook granularity —
/// never dropped wholesale. See [`prune_our_hooks`].
fn is_ours(entry: &Value, command: &str) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| hooks.iter().any(|h| is_our_hook(h, command)))
}

/// Remove Logos-authored hooks from `arr` at **hook** granularity, returning
/// whether anything was removed ([CR-095]).
///
/// The granularity is the whole point. A settings entry is a `{matcher, hooks:
/// [...]}` group, and a user may append their own command to an entry Logos
/// installed — or, historically, to the retired one. Dropping the entry because
/// it contains one of ours would silently unregister their hook: real
/// configuration loss, in a file that is usually shared and version-controlled,
/// caused by a command the user ran to *fix* something. So each entry keeps
/// every hook that is not ours, and only an entry left with no hooks at all is
/// dropped.
fn prune_our_hooks(arr: &mut Vec<Value>, command: &str) -> bool {
    let mut removed = false;
    let mut kept = Vec::with_capacity(arr.len());
    for mut entry in std::mem::take(arr) {
        if let Some(hooks) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
            let before = hooks.len();
            hooks.retain(|hook| !is_our_hook(hook, command));
            let shrank = hooks.len() != before;
            removed |= shrank;
            // Drop only an entry **we** emptied. One that arrived empty is the
            // user's business, not ours to tidy.
            if shrank && hooks.is_empty() {
                continue;
            }
        }
        kept.push(entry);
    }
    *arr = kept;
    removed
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
    // Sweep hooks retired by a prior version ([CR-095]), at hook granularity so
    // a user command sharing an entry with the retired one survives.
    let mut swept = Vec::new();
    let mut unsweepable = Vec::new();
    for retired in spec.retired {
        // An event we cannot recognize is left verbatim — and `sweepable` stays
        // false for it, so the file-level sweep declines to delete the script
        // that surviving entry still references. The entry and its script go
        // together or not at all; deleting one and keeping the other would
        // leave a registration pointing at nothing, which is worse than either.
        let sweepable = match hooks_obj.get_mut(retired.event) {
            None => true,
            Some(event) => match event.as_array_mut() {
                None => false,
                Some(arr) => {
                    if prune_our_hooks(arr, retired.command) {
                        swept.push(retired.script_rel);
                    }
                    // Drop an event key we just emptied rather than leaving
                    // `"SessionEnd": []` behind — the retirement should be
                    // invisible, not archaeological.
                    if arr.is_empty() {
                        hooks_obj.remove(retired.event);
                    }
                    true
                }
            },
        };
        if !sweepable {
            unsweepable.push(retired.script_rel);
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

    let present = arr.iter().any(|e| is_ours(e, spec.command));
    // Reconcile a present entry whose *shape* drifted from the spec — a matcher
    // or timeout from an older emit, or a hand edit. Without this the sweep is
    // only half self-healing: the retired event heals, while an entry carrying
    // (say) `timeout: 1` survives and reproduces the very cancellation
    // [CR-095] exists to remove.
    let current = present && arr.iter().any(|e| *e == hook_entry(spec));
    // Idempotent only when our entry is already exactly right AND nothing
    // retired is left to remove.
    if current && !force && swept.is_empty() {
        return Merge::AlreadyPresent;
    }
    // Re-emit: prune our prior hooks before re-adding so a refresh never
    // accumulates duplicates. Pruning is per-hook, so a user command sharing
    // our entry is preserved — the entry is only dropped if we emptied it.
    if present {
        prune_our_hooks(arr, spec.command);
    }
    arr.push(hook_entry(spec));

    // Which of the three writes this is. `Forced` claims local edits were
    // overwritten, so it needs *both* an entry to overwrite and a user who asked
    // for it; an unforced rewrite of a present entry is a `Reconciled`, which is
    // what drift healing and the retirement sweep produce ([CR-095]).
    let action = match (present, force) {
        (false, _) => EmitAction::Created,
        (true, true) => EmitAction::Forced,
        (true, false) => EmitAction::Reconciled,
    };

    Merge::Write {
        json: serialize(&config),
        action,
        swept,
        unsweepable,
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

    /// A retired script body as a prior Logos version wrote it — carrying the
    /// managed marker, which is what licenses the sweep to delete it.
    const RETIRED_SCRIPT_BODY: &str =
        "#!/bin/sh\n# logos:quality-report:managed — retired\nexit 0\n";

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
                    .filter(|e| is_ours(e, QUALITY_REPORT_HOOK_COMMAND))
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
        let Merge::Write { json, action, swept, .. } =
            merge_settings(Some(existing), false, &QUALITY_REPORT_SPEC)
        else {
            panic!("expected a write");
        };
        assert_eq!(
            action,
            EmitAction::Created,
            "no entry of ours was present, so this is a first install"
        );
        assert!(swept.is_empty(), "nothing retired to sweep here");
        let value: Value = serde_json::from_str(&json).unwrap();
        let start = value["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 2, "the foreign entry is preserved alongside ours");
        assert!(start.iter().any(|e| e["hooks"][0]["command"] == "my-own.sh"));
        assert!(start.iter().any(|e| is_ours(e, QUALITY_REPORT_HOOK_COMMAND)));
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
        let Merge::Write { action, .. } = merge_settings(None, false, &QUALITY_REPORT_SPEC) else {
            panic!("expected a write for an absent file");
        };
        assert_eq!(action, EmitAction::Created);
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

    /// The declared timeout is pinned to a **literal** value, not to its own
    /// constant: a comparison against `QUALITY_REPORT_HOOK_TIMEOUT_SECS` passes
    /// for any value, including a 1 that would reproduce the very cancellation
    /// [CR-095] exists to remove. The band is what matters — comfortably above
    /// the readout's real cost, comfortably below a wedged session.
    #[test]
    fn the_declared_timeout_is_pinned_to_a_usable_band() {
        assert_eq!(QUALITY_REPORT_HOOK_TIMEOUT_SECS, 30);
        let Merge::Write { json, .. } = merge_settings(None, false, &QUALITY_REPORT_SPEC) else {
            panic!("expected a write");
        };
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["hooks"]["SessionStart"][0]["hooks"][0]["timeout"], 30);
    }

    /// An entry that is present but whose **shape** drifted — an older emit's
    /// timeout, or a hand edit — is reconciled rather than left alone. Without
    /// this the self-healing is only half done: the retired event heals while a
    /// `timeout: 1` entry survives and keeps cancelling.
    #[test]
    fn a_drifted_managed_entry_is_reconciled() {
        let stale = format!(
            r#"{{ "hooks": {{ "SessionStart": [
                {{ "matcher": "startup", "hooks": [
                    {{ "type": "command", "command": "{QUALITY_REPORT_HOOK_COMMAND}", "timeout": 1 }}
                ] }}
            ] }} }}"#
        );
        let Merge::Write { json, action, .. } =
            merge_settings(Some(&stale), false, &QUALITY_REPORT_SPEC)
        else {
            panic!("a drifted managed entry must be rewritten, not accepted as current");
        };
        assert_eq!(
            action,
            EmitAction::Reconciled,
            "an unforced rewrite Logos initiated is not a `--force` overwrite: no local \
             edit was discarded and the caller asked for nothing ([CR-095])"
        );
        let value: Value = serde_json::from_str(&json).unwrap();
        let mine = ours(&value, "SessionStart");
        assert_eq!(mine.len(), 1, "reconciled in place, not duplicated: {value}");
        assert_eq!(mine[0]["hooks"][0]["timeout"], 30, "the stale timeout is corrected");
        assert_eq!(mine[0]["matcher"], QUALITY_REPORT_HOOK_MATCHER, "and the matcher");
    }

    /// A user command appended to the entry Logos installed survives a re-emit.
    /// The prune is per-**hook**, never per-entry: dropping the group because it
    /// contains one of ours would silently unregister their hook — real
    /// configuration loss, in a shared file, caused by a command run to fix
    /// something.
    #[test]
    fn a_user_hook_sharing_our_entry_survives_a_re_emit() {
        let shared = format!(
            r#"{{ "hooks": {{ "SessionStart": [
                {{ "matcher": "{QUALITY_REPORT_HOOK_MATCHER}", "hooks": [
                    {{ "type": "command", "command": "{QUALITY_REPORT_HOOK_COMMAND}", "timeout": 30 }},
                    {{ "type": "command", "command": "their-own.sh" }}
                ] }}
            ] }} }}"#
        );
        let value = written(merge_settings(Some(&shared), true, &QUALITY_REPORT_SPEC));
        let start = value["hooks"]["SessionStart"].as_array().unwrap();
        let commands: Vec<String> = start
            .iter()
            .flat_map(|e| e["hooks"].as_array().cloned().unwrap_or_default())
            .filter_map(|h| h["command"].as_str().map(str::to_string))
            .collect();
        assert!(
            commands.iter().any(|c| c == "their-own.sh"),
            "the user's hook is never unregistered: {value}"
        );
        assert_eq!(
            commands.iter().filter(|c| *c == QUALITY_REPORT_HOOK_COMMAND).count(),
            1,
            "and ours is re-emitted exactly once: {value}"
        );
    }

    /// An entry that arrived with an empty `hooks` array is the user's business,
    /// not ours to tidy: the prune drops only groups **it** emptied.
    #[test]
    fn an_entry_that_arrived_empty_is_left_alone() {
        let mut arr: Vec<Value> = serde_json::from_str(
            r#"[ { "matcher": "startup", "hooks": [] },
                 { "hooks": [ { "type": "command", "command": "ours" } ] } ]"#,
        )
        .unwrap();
        assert!(prune_our_hooks(&mut arr, "ours"), "ours was removed");
        assert_eq!(arr.len(), 1, "the pre-existing empty entry survives: {arr:?}");
        assert_eq!(arr[0]["matcher"], "startup");
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
        assert_eq!(swept, vec![".claude/hooks/logos-quality-report.sh"]);
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
        assert_eq!(swept, vec![".claude/hooks/logos-quality-report.sh"]);
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
        fs::write(&retired_script, RETIRED_SCRIPT_BODY).unwrap();
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
        // BOTH halves are reported: the deleted script and the removed settings
        // entry. Reporting only the file would stay silent when `.claude/hooks/`
        // is ignored but `settings.json` is committed — an entry vanishing from a
        // shared file with nothing said.
        assert_eq!(
            summary.retired_removed,
            vec![
                ".claude/hooks/logos-quality-report.sh".to_string(),
                ".claude/hooks/logos-quality-report.sh (settings entry)".to_string(),
            ],
            "the sweep is reported, not silent"
        );
        assert!(summary.retired_skipped.is_empty(), "nothing was left behind");
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
        fs::write(&orphan, RETIRED_SCRIPT_BODY).unwrap();

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
        fs::write(&retired_script, RETIRED_SCRIPT_BODY).unwrap();
        fs::write(tmp.path().join(SETTINGS_REL), "{ not json").unwrap();

        let summary = materialize_quality_report(tmp.path(), false).unwrap();
        assert_eq!(summary.action, EmitAction::Skipped);
        let notice = summary.notice.expect("the refusal is explained");
        assert!(notice.contains(SETTINGS_REL), "the notice names the file: {notice}");
        assert!(notice.contains("not valid JSON"), "and the reason: {notice}");
        assert!(notice.contains("left untouched"), "and what it did: {notice}");
        assert!(summary.retired_removed.is_empty());
        assert!(
            retired_script.exists(),
            "a file we will not parse is a file we will not sweep"
        );
    }

    /// The file sweep is the one place Logos deletes a file, so it verifies
    /// ownership rather than assuming it. A file at the retired path that Logos
    /// did not write — the path is unclaimed now, so a user may legitimately own
    /// it — is kept, and the reason is reported rather than swallowed.
    #[test]
    fn the_sweep_never_deletes_a_file_logos_did_not_write() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".claude/hooks")).unwrap();
        let theirs = tmp.path().join(".claude/hooks/logos-quality-report.sh");
        fs::write(&theirs, "#!/bin/sh\n# my own script, same name\necho hi\n").unwrap();

        let summary = materialize_quality_report(tmp.path(), false).unwrap();
        assert!(theirs.exists(), "an unmarked file at that path is not ours to delete");
        assert!(summary.retired_removed.is_empty());
        assert_eq!(summary.retired_skipped.len(), 1, "{summary:?}");
        assert!(
            summary.retired_skipped[0].contains("not a Logos-authored script"),
            "the reason is named: {:?}",
            summary.retired_skipped
        );
    }

    /// A non-regular path at the retired location is left in place and reported.
    /// `symlink_metadata` is used rather than `exists()` precisely so a symlink is
    /// never followed and unlinked (that would break the user's wiring) and a
    /// *dangling* one — which `exists()` calls absent — is still surfaced.
    #[cfg(unix)]
    #[test]
    fn the_sweep_leaves_a_symlink_or_directory_in_place() {
        // A dangling symlink: `exists()` reports absent, `symlink_metadata` does not.
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".claude/hooks")).unwrap();
        let link = tmp.path().join(".claude/hooks/logos-quality-report.sh");
        std::os::unix::fs::symlink(tmp.path().join("nowhere"), &link).unwrap();

        let summary = materialize_quality_report(tmp.path(), false).unwrap();
        assert!(
            fs::symlink_metadata(&link).is_ok(),
            "the symlink itself is never unlinked"
        );
        assert!(summary.retired_removed.is_empty());
        assert_eq!(summary.retired_skipped.len(), 1);
        assert!(
            summary.retired_skipped[0].contains("not a regular file"),
            "{:?}",
            summary.retired_skipped
        );

        // A directory at the same path: likewise left, likewise reported.
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".claude/hooks/logos-quality-report.sh");
        fs::create_dir_all(&dir).unwrap();
        let summary = materialize_quality_report(tmp.path(), false).unwrap();
        assert!(dir.is_dir(), "a directory is never removed");
        assert!(summary.retired_skipped[0].contains("not a regular file"));
    }

    /// A retired settings shape the merge declined to touch blocks the file
    /// delete: the entry and its script go together or not at all. Deleting the
    /// script while a live registration survives would leave that entry pointing
    /// at nothing — worse than leaving both.
    #[test]
    fn an_unrecognized_retired_entry_blocks_its_script_delete() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".claude/hooks")).unwrap();
        let retired_script = tmp.path().join(".claude/hooks/logos-quality-report.sh");
        fs::write(&retired_script, RETIRED_SCRIPT_BODY).unwrap();
        // A `SessionEnd` that is not an array — the merge leaves it verbatim.
        fs::write(
            tmp.path().join(SETTINGS_REL),
            r#"{ "hooks": { "SessionEnd": "not-an-array" } }"#,
        )
        .unwrap();

        let summary = materialize_quality_report(tmp.path(), false).unwrap();
        assert!(
            retired_script.exists(),
            "the script a surviving entry still references is kept"
        );
        assert!(summary.retired_removed.is_empty());
        assert_eq!(summary.retired_skipped.len(), 1);
        assert!(
            summary.retired_skipped[0].contains("settings entry was not recognized"),
            "{:?}",
            summary.retired_skipped
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
        assert!(is_ours(&start[0], QUALITY_REPORT_HOOK_COMMAND));
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

    /// A sweep-driven rewrite reports [`EmitAction::Reconciled`], never
    /// `Forced` ([CR-095]).
    ///
    /// This is the shape of the actual upgrade path — a current `SessionStart`
    /// entry alongside a retired `SessionEnd` one — and the only variant that
    /// could describe it before was `forced`, which promises `--force`
    /// overwrote local edits. Nobody passed `--force` here and nothing of the
    /// user's was touched, so a consumer acting on `forced` (a CI diff, a
    /// wrapper warning "Logos replaced your hook") would act on a fiction.
    #[test]
    fn a_sweep_driven_rewrite_is_reconciled_not_forced() {
        let tmp = TempDir::new().unwrap();
        materialize_quality_report(tmp.path(), false).unwrap();

        // Re-introduce the retired artifacts an older install would have left,
        // leaving our own entry exactly current.
        let retired_script = tmp.path().join(".claude/hooks/logos-quality-report.sh");
        fs::write(&retired_script, RETIRED_SCRIPT_BODY).unwrap();
        let mut settings: Value =
            serde_json::from_str(&fs::read_to_string(tmp.path().join(SETTINGS_REL)).unwrap())
                .unwrap();
        settings["hooks"]["SessionEnd"] =
            json!([serde_json::from_str::<Value>(RETIRED_ENTRY).unwrap()]);
        fs::write(tmp.path().join(SETTINGS_REL), serialize(&settings)).unwrap();

        let summary = materialize_quality_report(tmp.path(), false).unwrap();
        assert_eq!(
            summary.action,
            EmitAction::Reconciled,
            "an unforced sweep is not a `--force` overwrite: {summary:?}"
        );
        assert!(!summary.retired_removed.is_empty(), "the sweep did happen");
        assert!(!retired_script.exists());
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
        let Merge::Write { json, action, .. } =
            merge_settings(Some(existing), false, &QUALITY_REPORT_SPEC)
        else {
            panic!("expected a write");
        };
        assert_eq!(action, EmitAction::Created);
        let value: Value = serde_json::from_str(&json).unwrap();
        let start = value["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 2, "the foreign SessionStart entry survives alongside ours");
        assert!(start
            .iter()
            .any(|e| e["hooks"][0]["command"] == "their-startup.sh"));
        assert!(start.iter().any(|e| is_ours(e, QUALITY_REPORT_HOOK_COMMAND)));
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

    /// The script is a **launcher**, not a program ([CR-095]): it gates on the
    /// off-switch, enters the project, runs one command, and exits 0. Every
    /// property that made the shell-assembled predecessor fragile is asserted
    /// absent — no parsing, no field extraction, no hand-rolled JSON — because
    /// re-introducing any of them re-introduces the escaping bugs the move into
    /// the binary removed.
    #[test]
    fn quality_report_script_is_a_launcher_not_a_program() {
        // Assert against the executable body only. Matching the whole constant
        // would match its own prose — the comment explaining that nothing is
        // "parsed" here contains `sed`.
        let body = script_body();
        assert!(
            body.contains("logos --json quality-report --hook-json"),
            "the launcher delegates the whole payload to the binary: {body}"
        );
        // No parsing, extraction or JSON assembly in shell. The payload's field
        // names must appear in the binary's `HookPayload`, never here.
        for shell_ism in [
            "grep", "sed", "awk", "cut", "tr ", "systemMessage", "additionalContext",
            "hookEventName", "baseline_signal",
        ] {
            assert!(
                !body.contains(shell_ism),
                "the launcher neither parses nor assembles the payload ({shell_ism}): {body}"
            );
        }
        // No graph command that computes or reconciles: `quality-report` is the
        // single entry point, and it never pays for a reconcile or a write.
        for cmd in ["logos scan", "logos gate", "logos check", "logos index"] {
            assert!(
                !body.contains(cmd),
                "the launcher runs no second graph command ({cmd}): {body}"
            );
        }
        // Not `exec`: an older PATH binary rejecting the subcommand must degrade
        // to silence, not to a visible failed hook (CR-095).
        assert!(
            !body.contains("exec "),
            "the command's failure is swallowed, so the shell must survive it: {body}"
        );
        assert!(
            body.trim_end().ends_with("exit 0"),
            "the script always exits 0 — never blocks a session: {body}"
        );
    }

    /// The script's executable lines — comments and blanks stripped — so a
    /// shape assertion tests the code rather than the prose describing it.
    fn script_body() -> String {
        QUALITY_REPORT_HOOK_SCRIPT
            .lines()
            .filter(|line| !line.trim_start().starts_with('#') && !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Offline and report-only ([NFR-SE-01], [FR-GV-05]): no network client, no
    /// agent spawn, and the documented off-switch.
    #[test]
    fn quality_report_script_is_offline_report_only() {
        for net in ["curl", "wget", "nc ", "http://", "https://"] {
            assert!(
                !QUALITY_REPORT_HOOK_SCRIPT.contains(net),
                "the quality-report script invokes no network client ({net})"
            );
        }
        assert!(
            !QUALITY_REPORT_HOOK_SCRIPT.contains("claude "),
            "the quality-report hook spawns no agent — it only reports"
        );
        assert!(
            QUALITY_REPORT_HOOK_SCRIPT.contains("LOGOS_QUALITY_REPORT_DISABLE"),
            "off-switch env var"
        );
        // The old stderr readout is gone: on SessionStart, stdout is the channel.
        assert!(
            !QUALITY_REPORT_HOOK_SCRIPT.contains(">&2"),
            "the readout is no longer written to stderr"
        );
    }

    /// Install a fake `logos` on PATH, returning the PATH to run the hook under.
    /// It logs its argv to `<dir>/argv` and emits `stdout`/`stderr` verbatim, so a
    /// caller can prove *what the launcher does with the binary* without a graph.
    #[cfg(unix)]
    fn fake_logos(dir: &Path, stdout: &str, stderr: &str, exit_code: i32) -> String {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("fakebin");
        fs::create_dir_all(&bin).unwrap();
        let logos = bin.join("logos");
        fs::write(
            &logos,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{argv}'\n\
                 printf '%s' '{stdout}'\n\
                 printf '%s' '{stderr}' >&2\n\
                 exit {exit_code}\n",
                argv = dir.join("argv").display(),
            ),
        )
        .unwrap();
        fs::set_permissions(&logos, fs::Permissions::from_mode(0o755)).unwrap();
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default())
    }

    /// Run the materialized script, returning (stdout, stderr). Asserts exit 0 in
    /// every case — the report tier never blocks a session ([FR-GV-05]).
    #[cfg(unix)]
    fn run_hook(tmp: &Path, path: &str, project_dir: &Path, off_switch: bool) -> (String, String) {
        use std::process::Command;
        // An absolute interpreter: `path` is the *hook's* PATH, and one of the
        // cases below is deliberately empty — resolving `sh` through it would
        // fail the harness rather than exercise the script.
        let mut cmd = Command::new("/bin/sh");
        cmd.arg(tmp.join(QUALITY_REPORT_HOOK_SCRIPT_REL))
            .env("PATH", path)
            .env("CLAUDE_PROJECT_DIR", project_dir);
        if off_switch {
            cmd.env("LOGOS_QUALITY_REPORT_DISABLE", "1");
        } else {
            cmd.env_remove("LOGOS_QUALITY_REPORT_DISABLE");
        }
        let out = cmd.output().unwrap();
        assert_eq!(out.status.code(), Some(0), "the hook always exits 0");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// The launcher hands the binary's stdout through **verbatim** and adds
    /// nothing of its own to either stream: it is a pipe, not a formatter. It
    /// invokes exactly the JSON report subcommand, from the project the host
    /// named.
    #[cfg(unix)]
    #[test]
    fn the_launcher_passes_the_payload_through_verbatim() {
        let tmp = TempDir::new().unwrap();
        materialize_quality_report(tmp.path(), false).unwrap();
        let payload = r#"{"systemMessage":"logos quality report: signal 8234","hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"detail"}}"#;
        let path = fake_logos(tmp.path(), payload, "", 0);

        let (stdout, stderr) = run_hook(tmp.path(), &path, tmp.path(), false);
        assert_eq!(stdout, payload, "stdout is the binary's payload, unaltered");
        assert!(stderr.is_empty(), "the launcher adds nothing to stderr: {stderr:?}");
        // Parseable by the host, and the right event.
        let value: Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
        assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");

        // Exactly one invocation, of exactly the report subcommand.
        let argv = fs::read_to_string(tmp.path().join("argv")).unwrap();
        assert_eq!(
            argv.lines().collect::<Vec<_>>(),
            vec!["--json quality-report --hook-json"],
            "one invocation, no second graph command"
        );
    }

    /// The off-switch silences the hook entirely — no stdout, no stderr, and the
    /// binary is never invoked at all.
    #[cfg(unix)]
    #[test]
    fn off_switch_silences_the_hook() {
        let tmp = TempDir::new().unwrap();
        materialize_quality_report(tmp.path(), false).unwrap();
        let path = fake_logos(tmp.path(), r#"{"systemMessage":"x"}"#, "", 0);
        let (stdout, stderr) = run_hook(tmp.path(), &path, tmp.path(), true);
        assert!(
            stdout.is_empty() && stderr.is_empty(),
            "the off-switch silences the hook entirely: stdout={stdout:?} stderr={stderr:?}"
        );
        assert!(
            !tmp.path().join("argv").exists(),
            "the off-switch short-circuits before the binary is invoked"
        );
    }

    /// Three ways the environment can be wrong, all of which must degrade to
    /// silence rather than to a visible failed hook: no `logos` on PATH, a
    /// `CLAUDE_PROJECT_DIR` that cannot be entered, and — the normal mid-upgrade
    /// state — a PATH binary too old to know the subcommand, which exits non-zero
    /// with a clap usage error on stderr.
    #[cfg(unix)]
    #[test]
    fn a_broken_environment_degrades_to_silence() {
        let tmp = TempDir::new().unwrap();
        materialize_quality_report(tmp.path(), false).unwrap();

        // 1. No `logos` on PATH at all.
        let empty_dir = tmp.path().join("emptybin");
        fs::create_dir_all(&empty_dir).unwrap();
        let (stdout, stderr) =
            run_hook(tmp.path(), &empty_dir.display().to_string(), tmp.path(), false);
        assert!(
            stdout.is_empty() && stderr.is_empty(),
            "a missing binary is nothing to report: stdout={stdout:?} stderr={stderr:?}"
        );

        // 2. A project directory that cannot be entered.
        let path = fake_logos(tmp.path(), r#"{"systemMessage":"x"}"#, "", 0);
        let (stdout, stderr) =
            run_hook(tmp.path(), &path, &tmp.path().join("no-such-dir"), false);
        assert!(
            stdout.is_empty() && stderr.is_empty(),
            "an unusable project dir is silent: stdout={stdout:?} stderr={stderr:?}"
        );
        assert!(!tmp.path().join("argv").exists(), "and never runs the binary");

        // 3. An older binary that rejects the subcommand (clap exits 2 with a
        //    usage error on stderr). The host renders a hook's stderr on failure,
        //    so leaking this would turn a missing readout into a visible defect.
        let old = fake_logos(
            tmp.path(),
            "",
            "error: unrecognized subcommand 'quality-report'",
            2,
        );
        let (stdout, stderr) = run_hook(tmp.path(), &old, tmp.path(), false);
        assert!(
            stdout.is_empty() && stderr.is_empty(),
            "an older binary's usage error is swallowed: stdout={stdout:?} stderr={stderr:?}"
        );
    }
}
