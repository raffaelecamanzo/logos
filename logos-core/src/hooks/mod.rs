//! The optional `core.hooksPath` git-hook installer ([S-022], [FR-SY-05],
//! [FR-IN-03], [git integration]).
//!
//! Installs `post-commit` / `post-checkout` / `post-merge` hook scripts under
//! `<root>/.logos/hooks/` and points `git config core.hooksPath` at that
//! directory, so the relevant git events trigger a targeted `logos sync` of
//! exactly the files the event changed (computed via `git diff`), keeping
//! navigation fresh across commits, branch switches, and merges.
//!
//! Alongside those three **freshness** hooks it installs one **enforcing**
//! hook — a `pre-push` gate (S-218, [FR-IN-06]) that runs `logos check` and
//! propagates its non-zero exit to *block* the push on any regression. See
//! [`PRE_PUSH_HOOK`] / [`blocking_script_body`].
//!
//! # The freshness hooks are non-load-bearing for correctness ([FR-SY-06], [ADR-11])
//!
//! Like the watcher, the `post-*` hooks are a freshness optimization, never a
//! correctness dependency: each is best-effort by construction — it exits 0
//! unconditionally, bails out silently when `logos` is not on `PATH`, and
//! never blocks or fails the git operation that triggered it. A hook that
//! never fires costs at most a slightly stale navigation answer until the
//! next reconcile.
//!
//! The `pre-push` gate is the deliberate exception: it *does* block (that is
//! its purpose), but it too bails **open** when `logos` is genuinely absent,
//! so it can never falsely block a machine that lacks the tool, and it honours
//! `git push --no-verify`.
//!
//! # Non-clobbering ([FR-IN-01] posture)
//!
//! Installation refuses to redirect a `core.hooksPath` that already points
//! somewhere else (the user's hook manager owns it — husky, lefthook, a
//! custom dir): the result carries a warning naming the conflict and nothing
//! is changed. Re-running over our own installation is idempotent: scripts
//! are refreshed in place.
//!
//! # Consumed by S-023
//!
//! `logos init -i` surfaces this as its optional "install git hooks" step —
//! [`install`] is the seam that story calls; this module owns the mechanism.
//!
//! [S-022]: ../../../docs/planning/journal.md#s-022-incremental-sync-hardening-with-watcher-and-git-hooks
//! [FR-SY-05]: ../../../docs/specs/requirements/FR-SY-05.md
//! [FR-SY-06]: ../../../docs/specs/requirements/FR-SY-06.md
//! [FR-IN-01]: ../../../docs/specs/requirements/FR-IN-01.md
//! [FR-IN-03]: ../../../docs/specs/requirements/FR-IN-03.md
//! [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
//! [git integration]: ../../../docs/specs/architecture/integrations/git.md

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Serialize;

#[cfg(test)]
mod tests;

/// The hooks directory, relative to the project root. Inside `.logos/` so the
/// scripts travel with the policy directory and stay out of the user's way;
/// the S-023 `.gitignore` managed block ignores only `.logos/*.db*`, so hooks
/// remain committable if the team wants them shared.
///
/// Exported so [`crate::init`] can reference the same constant rather than
/// maintaining a duplicate string that could drift.
pub(crate) const HOOKS_RELDIR: &str = ".logos/hooks";

/// Marker line identifying a script as ours — the idempotency / ownership
/// check reads this before ever overwriting a file.
///
/// Exported so [`crate::init`] can use the same marker when writing its own
/// hook files during `logos init --hooks`, ensuring cross-recognition.
pub(crate) const MANAGED_MARKER: &str = "# logos-managed-hook";

/// The three hook events FR-SY-05 names, each with the `git diff` invocation
/// that computes exactly what the event changed.
///
/// Every script follows the same skeleton: bail out silently unless `logos`
/// is on PATH, compute the changed set, sync it quietly, and exit 0 no
/// matter what — a hook must never block or fail the git operation.
///
/// Exported so [`crate::init`] can iterate the same hook set without a
/// separate constant that risks drifting out of sync.
pub(crate) const HOOKS: &[(&str, &str)] = &[
    (
        "post-commit",
        // The files the commit just recorded (`--root` so the repository's
        // very first commit diffs against the empty tree instead of nothing).
        "changed=$(git diff-tree -r --root --name-only --no-commit-id HEAD 2>/dev/null) || exit 0",
    ),
    (
        "post-checkout",
        // $1=previous HEAD, $2=new HEAD, $3=1 for a branch checkout (0 is a
        // file checkout — nothing moved that the index doesn't already know).
        "[ \"$3\" = \"1\" ] || exit 0\n\
         changed=$(git diff --name-only \"$1\" \"$2\" 2>/dev/null) || exit 0",
    ),
    (
        "post-merge",
        // Everything the merge brought in relative to where we were.
        "changed=$(git diff --name-only ORIG_HEAD HEAD 2>/dev/null) || exit 0",
    ),
];

/// The one **enforcing** hook (S-218, [FR-IN-06], [FR-GV-02], [FR-GV-03]).
///
/// Unlike the freshness [`HOOKS`] above — best-effort scripts that always exit
/// 0 — the `pre-push` gate runs `logos check` and *propagates* its non-zero
/// exit to block the push on any rule / structural / admission / dead-code
/// regression. It is deliberately not exit-0-swallowed: enforcement at the
/// "code about to leave the machine" boundary is the entire point.
///
/// It still bails **open** (exit 0) when the `logos` binary is genuinely
/// absent, so a machine without the tool is never falsely blocked, and it
/// honours `git push --no-verify` natively (git skips the hook entirely).
///
/// [FR-IN-06]: ../../../docs/specs/requirements/FR-IN-06.md
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-03]: ../../../docs/specs/requirements/FR-GV-03.md
pub(crate) const PRE_PUSH_HOOK: &str = "pre-push";

/// Every managed hook file name — the freshness set plus the blocking gate.
///
/// The single source both install paths ([`install`] here and
/// [`crate::init`]'s wizard step) iterate for ownership vetoes and uninstall,
/// so the two implementations can never disagree on *which* files are ours.
pub(crate) fn all_hook_names() -> impl Iterator<Item = &'static str> {
    HOOKS
        .iter()
        .map(|(name, _)| *name)
        .chain(std::iter::once(PRE_PUSH_HOOK))
}

/// Every managed hook as `(name, full script body)`.
///
/// The single source both install paths render from, so the freshness scripts
/// and the blocking gate stay in lockstep rather than being re-templated (and
/// risking divergence) in each caller.
pub(crate) fn managed_scripts() -> Vec<(&'static str, String)> {
    let mut scripts: Vec<(&'static str, String)> = HOOKS
        .iter()
        .map(|(name, changed_cmd)| (*name, script_body(name, changed_cmd)))
        .collect();
    scripts.push((PRE_PUSH_HOOK, blocking_script_body()));
    scripts
}

/// Result read-model of [`install`] / [`uninstall`] (S-023 prints this from
/// its `init -i` step).
#[derive(Debug, Default, Serialize)]
pub struct HooksResult {
    /// The configured hooks directory (project-relative).
    pub hooks_dir: String,
    /// Hook scripts written (or refreshed) by this call.
    pub installed: Vec<String>,
    /// Why anything was skipped — e.g. a foreign `core.hooksPath`.
    pub warnings: Vec<String>,
}

/// Install the sync git hooks for the repository at `root` ([FR-SY-05]).
///
/// Writes the three managed hook scripts under `.logos/hooks/` and sets
/// `core.hooksPath` to that directory. Idempotent over our own installation;
/// non-clobbering over anyone else's (see the module docs).
///
/// # Errors
/// Returns an error if `root` is not inside a git work tree, `git` is not on
/// `PATH`, or the scripts/config cannot be written.
pub fn install(root: &Path) -> Result<HooksResult> {
    ensure_git_worktree(root)?;
    let mut result = HooksResult {
        hooks_dir: HOOKS_RELDIR.to_string(),
        ..HooksResult::default()
    };

    // Non-clobbering: a hooksPath we don't own is the user's hook manager.
    if let Some(existing) = configured_hooks_path(root)? {
        if existing != HOOKS_RELDIR {
            result.warnings.push(format!(
                "core.hooksPath already points at {existing:?} — leaving it alone; \
                 add `logos sync` calls to your own hooks to keep navigation fresh"
            ));
            return Ok(result);
        }
    }

    let hooks_dir = root.join(HOOKS_RELDIR);
    std::fs::create_dir_all(&hooks_dir)
        .with_context(|| format!("creating the hooks directory {}", hooks_dir.display()))?;

    for (name, body) in managed_scripts() {
        let path = hooks_dir.join(name);
        // Ownership check: never overwrite a hook script we didn't write.
        if path.exists() {
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            if !existing.contains(MANAGED_MARKER) {
                result.warnings.push(format!(
                    "{HOOKS_RELDIR}/{name} exists and is not logos-managed — left untouched"
                ));
                continue;
            }
        }
        std::fs::write(&path, &body).with_context(|| format!("writing the {name} hook"))?;
        make_executable(&path)?;
        result.installed.push(name.to_string());
    }

    git_config(root, "core.hooksPath", HOOKS_RELDIR)?;
    Ok(result)
}

/// Remove the managed hooks and unset `core.hooksPath` if (and only if) it
/// still points at our directory.
///
/// # Errors
/// Returns an error if `root` is not inside a git work tree or git/filesystem
/// operations fail.
pub fn uninstall(root: &Path) -> Result<HooksResult> {
    ensure_git_worktree(root)?;
    let mut result = HooksResult {
        hooks_dir: HOOKS_RELDIR.to_string(),
        ..HooksResult::default()
    };

    for name in all_hook_names() {
        let path = root.join(HOOKS_RELDIR).join(name);
        if !path.exists() {
            continue;
        }
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        if !body.contains(MANAGED_MARKER) {
            result.warnings.push(format!(
                "{HOOKS_RELDIR}/{name} is not logos-managed — left untouched"
            ));
            continue;
        }
        std::fs::remove_file(&path).with_context(|| format!("removing the {name} hook"))?;
        result.installed.push(name.to_string());
    }

    if configured_hooks_path(root)?.as_deref() == Some(HOOKS_RELDIR) {
        let status = git(root, &["config", "--unset", "core.hooksPath"])?;
        if !status.status.success() {
            bail!("could not unset core.hooksPath");
        }
    }
    Ok(result)
}

/// The full script for one hook: marker, PATH guard, changed-set computation,
/// quiet best-effort sync, unconditional success.
///
/// Exported so [`crate::init`] can generate the canonical body rather than
/// maintaining a separate (and divergence-prone) hook template.
pub(crate) fn script_body(name: &str, changed_cmd: &str) -> String {
    format!(
        "#!/bin/sh\n\
         {MANAGED_MARKER}: {name} (S-022, FR-SY-05)\n\
         # Best-effort navigation freshness — never blocks or fails the git\n\
         # operation; reconcile is the correctness backstop (FR-SY-06).\n\
         command -v logos >/dev/null 2>&1 || exit 0\n\
         {changed_cmd}\n\
         [ -n \"$changed\" ] || exit 0\n\
         # NUL-delimited so paths with spaces survive xargs (BSD + GNU).\n\
         printf '%s\\n' \"$changed\" | tr '\\n' '\\0' | xargs -0 logos sync --quiet >/dev/null 2>&1 || true\n\
         exit 0\n"
    )
}

/// The **blocking** pre-push gate script (S-218, [FR-IN-06], [FR-GV-02],
/// [FR-GV-03]).
///
/// Mirror image of [`script_body`]: it bails **open** (exit 0) only when the
/// `logos` binary is genuinely absent — never a false block on a machine
/// without the tool — and otherwise `exec`s `logos check`, whose non-zero exit
/// on any `severity='error'` violation *propagates* as the hook's exit and so
/// blocks the push. `check` prints the offending violation, so the reason a
/// push was refused is always named. `git push --no-verify` bypasses the hook
/// natively (git skips `pre-push` entirely), so no explicit flag handling is
/// needed here.
///
/// Exported so [`crate::init`] generates the canonical gate body rather than
/// carrying a divergence-prone copy.
///
/// [FR-IN-06]: ../../../docs/specs/requirements/FR-IN-06.md
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-03]: ../../../docs/specs/requirements/FR-GV-03.md
pub(crate) fn blocking_script_body() -> String {
    format!(
        "#!/bin/sh\n\
         {MANAGED_MARKER}: {PRE_PUSH_HOOK} (S-218, FR-GV-02, FR-GV-03)\n\
         # ENFORCING gate — unlike the freshness hooks this PROPAGATES a\n\
         # non-zero exit to block the push on any rule / structural / admission\n\
         # / dead-code regression (FR-GV-03). It bails OPEN (exit 0) only when\n\
         # `logos` is genuinely absent, so a machine without the binary is\n\
         # never falsely blocked; `git push --no-verify` bypasses it natively.\n\
         command -v logos >/dev/null 2>&1 || exit 0\n\
         exec logos check\n"
    )
}

/// `chmod +x` (Unix). On non-Unix targets git invokes hooks through `sh`,
/// which does not require the executable bit.
fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)
            .with_context(|| format!("reading permissions of {}", path.display()))?
            .permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        std::fs::set_permissions(path, permissions)
            .with_context(|| format!("marking {} executable", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// The repository's current `core.hooksPath`, if any.
fn configured_hooks_path(root: &Path) -> Result<Option<String>> {
    let output = git(root, &["config", "--get", "core.hooksPath"])?;
    match output.status.code() {
        Some(0) => {}
        // Exit 1 from `--get` simply means "not set".
        Some(1) => return Ok(None),
        // Exit ≥2 is a real fault (e.g. a corrupt config file): surface it
        // rather than mistaking it for "unset" — the non-clobbering check
        // must never proceed on a misread.
        code => bail!(
            "git config --get core.hooksPath failed (exit {code:?}): {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!value.is_empty()).then_some(value))
}

/// Fail with an actionable message unless `root` is inside a git work tree.
fn ensure_git_worktree(root: &Path) -> Result<()> {
    let output = git(root, &["rev-parse", "--is-inside-work-tree"])?;
    let inside =
        output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true";
    if !inside {
        bail!(
            "{} is not inside a git work tree — git hooks need a repository \
             (run `git init` first or skip hook installation)",
            root.display()
        );
    }
    Ok(())
}

/// Set one git config key in the repository at `root`.
fn git_config(root: &Path, key: &str, value: &str) -> Result<()> {
    let output = git(root, &["config", key, value])?;
    if !output.status.success() {
        bail!(
            "git config {key} {value} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Run `git <args>` with `root` as the working directory.
///
/// A missing `git` binary degrades to an actionable error rather than a raw
/// OS error (the git integration's "git absent on PATH" failure mode).
fn git(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("running `git {}` (is git on PATH?)", args.join(" ")))
}
