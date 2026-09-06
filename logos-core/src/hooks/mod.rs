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
//! # Reachable from every working tree (CR-106, [FR-WT-01])
//!
//! `core.hooksPath` is relative, and git resolves it against the top level of
//! the working tree the command runs in — so a linked worktree looks for its
//! *own* `.logos/hooks`, which [ADR-15]'s seed-from-main never created. The
//! path stays relative ([FR-IN-06] AC1); reachability comes from seeding each
//! working tree with symlinks to the one real copy of each script. See the
//! "Every-working-tree reachability" section below for the mechanism, the
//! copy fallback, and the accepted timing gap.
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
//! [FR-WT-01]: ../../../docs/specs/requirements/FR-WT-01.md
//! [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
//! [ADR-15]: ../../../docs/specs/architecture/decisions/ADR-15.md
//! [git integration]: ../../../docs/specs/architecture/integrations/git.md

use std::path::{Path, PathBuf};
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
    /// The **other** working trees this call made the hooks reachable from
    /// (CR-106) — seeded on [`install`], cleaned on [`uninstall`]. Empty in a
    /// single-checkout repository, which is why the hooks appearing to work
    /// there proved nothing about the linked worktrees where they did not.
    #[serde(default)]
    pub worktrees: Vec<String>,
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

    // Deliberately the RELATIVE path (FR-IN-06 AC1) — reachability from the
    // other working trees comes from seeding below, never from absolutising
    // this value (CR-106).
    git_config(root, "core.hooksPath", HOOKS_RELDIR)?;

    // Every working tree that exists *now* becomes reachable before this call
    // returns, so a worktree that predates the installation is covered by
    // construction rather than on some later visit.
    if crate::workspace::primary_root(root).is_some() {
        // …except from here, where there is no durable anchor to point them at.
        result.warnings.push(format!(
            "installed in a linked worktree, so the hooks fire here but not in the \
             repository's other working trees — run `logos init --hooks` in the primary \
             checkout to make them reachable everywhere ({HOOKS_RELDIR} is resolved \
             per working tree)"
        ));
    }
    for seeded in seed_all_worktrees(root) {
        if !seeded.is_empty() {
            result.worktrees.push(seeded.worktree);
        }
        result.warnings.extend(seeded.warnings);
    }
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

    let (removed, warnings) = purge_hooks_dir(&root.join(HOOKS_RELDIR), PurgeScope::Installation);
    result.installed = removed;
    result.warnings.extend(warnings);

    // CR-106: an installation is repo-global — `core.hooksPath` lives in the
    // shared config and is about to be unset for every working tree — so the
    // removal must be repo-global too. Purging the source scripts first leaves
    // the seeded symlinks dangling, and a dangling link is exactly what this
    // pass then clears, so an upgrade from a version that seeded nothing and
    // one from a version that seeded symlinks both end clean.
    for other in other_working_trees(root) {
        let (removed, warnings) = purge_hooks_dir(&other.join(HOOKS_RELDIR), PurgeScope::SeededOnly);
        if !removed.is_empty() {
            result.worktrees.push(other.display().to_string());
        }
        result.warnings.extend(warnings);
    }

    if configured_hooks_path(root)?.as_deref() == Some(HOOKS_RELDIR) {
        let status = git(root, &["config", "--unset", "core.hooksPath"])?;
        if !status.status.success() {
            bail!("could not unset core.hooksPath");
        }
    }
    Ok(result)
}

// ── Every-working-tree reachability (CR-106, [FR-WT-01]) ────────────────────
//
// `core.hooksPath` is deliberately the RELATIVE `.logos/hooks` ([FR-IN-06]
// AC1), and git resolves a relative `core.hooksPath` against the top level of
// **the working tree the command runs in**. In a linked worktree that is
// `<worktree>/.logos/hooks` — a directory [ADR-15]'s seed-from-main never
// created, because it seeds a database, not a hooks directory. So no logos
// hook ever fired in a linked worktree, while every observable signal read as
// success: `init` reported the hooks installed, `git config` returned a value,
// and the scripts were on disk.
//
// Reachability therefore comes from **seeding**, never from absolutising the
// path — absolutising is the exact breach [FR-IN-06] AC1 forbids, and it would
// trade this silent failure for another (a moved or re-cloned repo finding
// nothing at a recorded absolute path). Each working tree gets its own
// `.logos/hooks/` holding SYMLINKS to the one real copy of each script, so a
// re-install that updates a script cannot leave a worktree running a stale
// one. Where the platform or filesystem refuses symlinks the fallback is
// copies **plus** [`COPY_FALLBACK_MARKER`], which [`reachability_findings`]
// reads — never silent copies.
//
// [ADR-15]: ../../../docs/specs/architecture/decisions/ADR-15.md
// [FR-WT-01]: ../../../docs/specs/requirements/FR-WT-01.md

/// Marker file a copy-fallback seed leaves beside the copies it made, naming
/// the checkout they were copied from.
///
/// Its presence is the *only* thing that distinguishes a copy-seeded working
/// tree from a symlinked one, so [`reachability_findings`] can report copies
/// (and detect them going stale) instead of letting them pass silently
/// ([NFR-CC-04]) — the failure shape CR-106 exists to close.
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub(crate) const COPY_FALLBACK_MARKER: &str = ".seeded-by-copy";

/// The key line inside [`COPY_FALLBACK_MARKER`] naming the source checkout.
const COPY_FALLBACK_SOURCE_KEY: &str = "source: ";

/// How a seeded worktree hook is materialised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkMode {
    /// The contracted mechanism: a symlink to the source checkout's script,
    /// so exactly one copy of each script exists and none can go stale.
    Symlink,
    /// The fallback for a platform or filesystem that refuses symlinks —
    /// copies, always accompanied by [`COPY_FALLBACK_MARKER`].
    ///
    /// Production never *selects* this: [`place_hook`] discovers the refusal
    /// by attempting the symlink, because a filesystem's answer is not
    /// something a caller can know in advance. Naming it is what lets the
    /// tests drive the fallback on a machine whose filesystem is perfectly
    /// happy to make symlinks — the AC requires the path be exercised, not
    /// assumed to work wherever CI happens to run.
    #[cfg_attr(not(test), allow(dead_code))]
    Copy,
}

/// What became of one hook in one working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// A symlink to the source script — freshly made or already correct.
    Linked,
    /// A marked copy (the symlink fallback).
    Copied,
    /// A managed script already sitting in the worktree that this seed left
    /// exactly as it found it. The hooks directory is deliberately **not**
    /// gitignored ([FR-IN-04]) so a team may commit its hooks and let them
    /// travel through git; such a script arrives by checkout, already fires,
    /// and replacing it with a symlink would dirty every working tree's `git
    /// status` for no gain.
    Present,
    /// A script we did not write, left untouched ([FR-IN-01] posture).
    Foreign,
}

/// One working tree's hook-seed outcome (CR-106).
#[derive(Debug, Default, Serialize)]
pub struct WorktreeSeedResult {
    /// The working tree seeded.
    pub worktree: String,
    /// Hooks reachable there through a symlink to the source script.
    pub linked: Vec<String>,
    /// Hooks materialised as copies because symlinks were unavailable —
    /// always accompanied by the staleness marker `doctor` reads.
    pub copied: Vec<String>,
    /// Hooks already reachable there and left exactly as found — a script the
    /// team commits and lets travel through git ([FR-IN-04]). Reported, not
    /// replaced: it already fires, and rewriting it as a symlink would dirty
    /// every working tree's `git status` to no end.
    #[serde(default)]
    pub present: Vec<String>,
    /// Why anything was skipped. Never fatal: a failed seed costs a working
    /// tree its freshening, and `doctor` names it ([ADR-11] fail-soft).
    pub warnings: Vec<String>,
}

impl WorktreeSeedResult {
    /// Is no managed hook reachable in that working tree as a result of this
    /// call — nothing placed, and nothing already there?
    pub fn is_empty(&self) -> bool {
        self.linked.is_empty() && self.copied.is_empty() && self.present.is_empty()
    }
}

/// Seed `worktree_root`'s `.logos/hooks/` from the managed scripts in
/// `source_root`'s, so the relative `core.hooksPath` resolves to real hooks
/// there too (CR-106).
///
/// Idempotent: a hook already symlinked at the right target is left alone. A
/// managed *copy* or a managed real script found in the worktree is replaced
/// by a symlink, because a second copy is exactly what goes stale. A script
/// the worktree owns and we did not write is never touched, in a worktree
/// exactly as in the main checkout.
///
/// Seeds nothing when `source_root` has no managed hooks: opt-in stays opt-in,
/// and an unhooked project must not grow a hooks directory.
pub fn seed_worktree(source_root: &Path, worktree_root: &Path) -> WorktreeSeedResult {
    seed_worktree_with(source_root, worktree_root, LinkMode::Symlink)
}

/// [`seed_worktree`] with the materialisation mechanism pinned — the seam the
/// copy-fallback test drives, so the fallback path is *exercised* rather than
/// assumed to work on the one filesystem CI happens to run on.
fn seed_worktree_with(
    source_root: &Path,
    worktree_root: &Path,
    mode: LinkMode,
) -> WorktreeSeedResult {
    let mut result = WorktreeSeedResult {
        worktree: worktree_root.display().to_string(),
        ..WorktreeSeedResult::default()
    };
    if crate::workspace::paths_equal(source_root, worktree_root) {
        return result; // the real scripts already live here
    }

    let src_dir = source_root.join(HOOKS_RELDIR);
    let sources: Vec<(&str, PathBuf)> = all_hook_names()
        .map(|name| (name, src_dir.join(name)))
        .filter(|(_, path)| is_managed_script(path))
        .collect();
    if sources.is_empty() {
        return result; // nothing installed to make reachable
    }

    let dst_dir = worktree_root.join(HOOKS_RELDIR);
    if let Err(err) = std::fs::create_dir_all(&dst_dir) {
        result.warnings.push(format!(
            "could not create {} — logos git hooks will not fire in this working tree: {err}",
            dst_dir.display()
        ));
        return result;
    }

    // A marker beside the hooks says the copies there are OURS, so refreshing
    // them is a duty rather than a clobber. Without it, a managed script found
    // in the worktree came through git and stays put.
    let refresh_copies = dst_dir.join(COPY_FALLBACK_MARKER).exists();

    for (name, src) in sources {
        match place_hook(&src, &dst_dir.join(name), mode, refresh_copies) {
            Ok(Placement::Linked) => result.linked.push(name.to_string()),
            Ok(Placement::Copied) => result.copied.push(name.to_string()),
            Ok(Placement::Present) => result.present.push(name.to_string()),
            Ok(Placement::Foreign) => result.warnings.push(format!(
                "{HOOKS_RELDIR}/{name} in {} is not logos-managed — left untouched",
                worktree_root.display()
            )),
            Err(err) => result.warnings.push(format!(
                "could not seed the {name} hook into {}: {err:#}",
                worktree_root.display()
            )),
        }
    }

    // The marker exists exactly while copies do, so `doctor` can never mistake
    // a copy-seeded tree for a symlinked one.
    let marker = dst_dir.join(COPY_FALLBACK_MARKER);
    if result.copied.is_empty() {
        let _ = std::fs::remove_file(&marker);
    } else if let Err(err) = std::fs::write(&marker, copy_fallback_note(source_root)) {
        result.warnings.push(format!(
            "hooks were copied into {} but the staleness marker could not be written \
             ({err}) — `doctor` cannot report them as copies",
            worktree_root.display()
        ));
    }
    result
}

/// Materialise one hook at `dst` from the source script at `src`.
///
/// `refresh_copies` says whether a managed **regular file** at `dst` is ours
/// to replace — true only where [`COPY_FALLBACK_MARKER`] marks the directory
/// as copy-seeded. Everywhere else such a file is a checked-in hook that
/// travelled through git ([FR-IN-04]): it already fires, and replacing it
/// would show up as a modification in every working tree.
///
/// # Errors
/// Returns an error only for an I/O failure that leaves `dst` unusable; a
/// refused symlink is not one — it falls back to a marked copy.
fn place_hook(src: &Path, dst: &Path, mode: LinkMode, refresh_copies: bool) -> Result<Placement> {
    match std::fs::symlink_metadata(dst) {
        Ok(meta) => {
            let is_link = meta.file_type().is_symlink();
            // A DANGLING symlink cannot be read, but one under our own hooks
            // directory is ours by construction — it is what an uninstall that
            // purged the source first leaves behind — so it is replaceable,
            // not foreign.
            let managed = match std::fs::read_to_string(dst) {
                Ok(body) => body.contains(MANAGED_MARKER),
                Err(_) => is_link,
            };
            if !managed {
                return Ok(Placement::Foreign);
            }
            if is_link && mode == LinkMode::Symlink && links_to(dst, src) {
                return Ok(Placement::Linked); // already correct
            }
            if !is_link && !refresh_copies {
                return Ok(Placement::Present); // the team's own, via git
            }
            std::fs::remove_file(dst)
                .with_context(|| format!("replacing the seeded hook {}", dst.display()))?;
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err).with_context(|| format!("inspecting {}", dst.display()));
        }
    }

    if mode == LinkMode::Symlink {
        if symlink_file(src, dst).is_ok() {
            return Ok(Placement::Linked);
        }
        // A refused symlink has two very different causes demanding opposite
        // responses: the platform genuinely cannot make one — what the copy
        // fallback exists for — or another logos process placed the very link
        // we wanted between the `symlink_metadata` above and this call. Every
        // engine start seeds and nothing serialises them, so that race is
        // ordinary, not exotic. Re-read the destination instead of assuming
        // the first cause.
        if links_to(dst, src) {
            return Ok(Placement::Linked);
        }
    }

    // Never copy ONTO a symlink. `fs::copy` opens the destination with
    // truncate, so a link still resolving to `src` would truncate the one real
    // copy of the script — silently emptying the primary checkout's `pre-push`
    // and disabling the [FR-IN-06] gate for the entire repository, while every
    // call still returned `Ok`. Clear the path first; whatever is there is
    // ours, having passed the ownership check above or appeared concurrently
    // from another logos process seeding the same worktree.
    if std::fs::symlink_metadata(dst).is_ok() {
        std::fs::remove_file(dst)
            .with_context(|| format!("clearing {} before the copy", dst.display()))?;
    }
    std::fs::copy(src, dst)
        .with_context(|| format!("copying the hook script to {}", dst.display()))?;
    make_executable(dst)?;
    Ok(Placement::Copied)
}

/// Does the symlink at `dst` already point at `src`?
///
/// Compared symlink-resolved, because the two paths reach the same script by
/// different spellings routinely — an installer seeds from the root it was
/// handed while the bootstrap seeds from `git`'s canonical
/// `--git-common-dir` answer — and a literal mismatch would rewrite a
/// perfectly good link on every engine start.
fn links_to(dst: &Path, src: &Path) -> bool {
    std::fs::read_link(dst).is_ok_and(|target| crate::workspace::paths_equal(&target, src))
}

/// A symlink at `dst` pointing to `src`, or the platform's refusal.
///
/// The target is **absolute**, which is safe here in a way an absolute
/// `core.hooksPath` is not: this link lives in the gitignored, per-worktree
/// `.logos/`, [`seed_from_primary`] re-derives it on every engine start, and
/// [`reachability_findings`] reports it the moment it dangles — where a
/// recorded absolute config value is re-derived by nothing and reported by
/// nothing (CR-106 §3.3).
fn symlink_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(src, dst)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (src, dst);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "this platform does not support symlinks",
        ))
    }
}

/// Does `path` hold one of our hook scripts (following a symlink)?
fn is_managed_script(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|body| body.contains(MANAGED_MARKER))
}

/// The body of [`COPY_FALLBACK_MARKER`] — human-readable, and machine-readable
/// on its `source:` line so the staleness comparison knows what to diff against.
fn copy_fallback_note(source_root: &Path) -> String {
    format!(
        "{MANAGED_MARKER}: worktree seed fallback (CR-106)\n\
         # Symlinks were unavailable here, so the hooks beside this file are\n\
         # COPIES of the source checkout's scripts rather than links to them.\n\
         # Copies can go stale, so `logos doctor` reads this marker and reports\n\
         # them; seeding copies silently would recreate the very defect that\n\
         # every-working-tree reachability closed.\n\
         {COPY_FALLBACK_SOURCE_KEY}{}\n",
        source_root.display()
    )
}

/// Every working tree of `root`'s repository — the primary checkout and each
/// linked worktree — as absolute paths.
///
/// Empty when git cannot answer (binary absent, not a repository): the caller
/// then seeds nothing, which is the safe direction. A bare repository's entry
/// is dropped — it has no working tree for a hook to fire in.
fn working_trees(root: &Path) -> Vec<PathBuf> {
    let Ok(output) = git(root, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    let mut trees: Vec<PathBuf> = Vec::new();
    let mut current: Option<PathBuf> = None;
    for line in listing.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            trees.extend(current.take());
            current = Some(PathBuf::from(path));
        } else if line == "bare" {
            current = None;
        }
    }
    trees.extend(current);
    // A pruned-but-not-yet-removed entry no longer exists on disk.
    trees.retain(|path| path.is_dir());
    trees
}

/// The working trees of `root`'s repository other than `root` itself.
fn other_working_trees(root: &Path) -> Vec<PathBuf> {
    working_trees(root)
        .into_iter()
        .filter(|path| !crate::workspace::paths_equal(path, root))
        .collect()
}

/// Seed every OTHER working tree of `root`'s repository from the managed
/// scripts in `root`'s `.logos/hooks/` (CR-106).
///
/// This is what covers a worktree that existed **before** installation, by
/// construction rather than by a later visit: [`install`] calls it, so every
/// working tree in existence at install time is reachable the moment install
/// returns. A worktree created **after** installation is covered instead by
/// [`seed_from_primary`] on its first logos use.
///
/// Seeds nothing when `root` is itself a **linked worktree**. A seed points
/// every other tree at one checkout's scripts, and a linked worktree is the
/// one checkout that can be removed at any moment — anchoring the repository's
/// hooks there would leave dangling links behind `git worktree remove`. The
/// primary checkout is the only durable anchor, so an installation run from a
/// worktree installs *there* and says so ([`install`] warns); the other trees
/// then report themselves unreachable through [`reachability_findings`] rather
/// than being wired to something that may vanish.
pub fn seed_all_worktrees(root: &Path) -> Vec<WorktreeSeedResult> {
    if crate::workspace::primary_root(root).is_some() {
        return Vec::new();
    }
    other_working_trees(root)
        .into_iter()
        .map(|worktree| seed_worktree(root, &worktree))
        .filter(|result| !result.is_empty() || !result.warnings.is_empty())
        .collect()
}

/// Seed THIS working tree's hooks from the primary checkout, when `root` is a
/// linked worktree whose primary has managed hooks installed (CR-106).
///
/// The bootstrap seam [`crate::Engine`] calls on every start, which is what
/// covers a worktree created **after** installation: its first logos use makes
/// the hooks reachable. `None` — nothing to do — in the primary checkout,
/// outside a repository, in a submodule, and in any worktree whose primary has
/// no hooks installed.
///
/// **The accepted timing gap** ([CRA-04]): the seed runs on first logos use,
/// so a `git commit` made between `git worktree add` and the first logos
/// command in the new worktree still runs unhooked. Closing it would require
/// hooking `git worktree add`, which logos does not control;
/// [`reachability_findings`] surfaces the window rather than hiding it.
///
/// [CRA-04]: ../../../docs/requests/CR-106-git-hooks-never-fire-in-a-linked-worktree.md
pub fn seed_from_primary(root: &Path) -> Option<WorktreeSeedResult> {
    // Cheap discriminator, no subprocess: only a linked worktree (or a
    // submodule, which `primary_root` then rejects) has `.git` as a FILE. The
    // primary checkout and a non-repository both stop here, so the common
    // engine start pays one `stat` for this whole mechanism.
    if !root.join(".git").is_file() {
        return None;
    }
    let primary = crate::workspace::primary_root(root)?;
    let result = seed_worktree(&primary, root);
    (!result.is_empty() || !result.warnings.is_empty()).then_some(result)
}

/// How much of one hooks directory an uninstall may remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PurgeScope {
    /// Every managed hook — the installation itself, in the working tree the
    /// uninstall was asked for.
    Installation,
    /// Only what the worktree seeding placed: symlinks (dangling ones
    /// included) and copies the fallback marker claims. A checked-in script
    /// that travelled through git is the repository's, not this seed's, and
    /// deleting it in N worktrees is not what "remove what I seeded" means.
    SeededOnly,
}

/// Remove the managed hooks from one working tree's hooks directory, plus the
/// copy-fallback marker, leaving anything we did not write untouched.
///
/// Returns `(removed hook names, warnings)`. Under [`PurgeScope::SeededOnly`]
/// the now-empty directory goes too, so a seeded worktree is left with no
/// trace; the attempt fails harmlessly when the user keeps something else
/// there.
fn purge_hooks_dir(dir: &Path, scope: PurgeScope) -> (Vec<String>, Vec<String>) {
    let mut removed = Vec::new();
    let mut warnings = Vec::new();
    let seeded_copies = dir.join(COPY_FALLBACK_MARKER).exists();
    for name in all_hook_names() {
        let path = dir.join(name);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let is_link = meta.file_type().is_symlink();
        // As in `place_hook`: an unreadable symlink under our own directory is
        // a dangling link of ours — precisely what must not survive an
        // uninstall — not a foreign script to preserve.
        let managed = match std::fs::read_to_string(&path) {
            Ok(body) => body.contains(MANAGED_MARKER),
            Err(_) => is_link,
        };
        if !managed {
            warnings.push(format!(
                "{HOOKS_RELDIR}/{name} is not logos-managed — left untouched"
            ));
            continue;
        }
        if scope == PurgeScope::SeededOnly && !is_link && !seeded_copies {
            continue; // the repository's own checked-in hook
        }
        match std::fs::remove_file(&path) {
            Ok(()) => removed.push(name.to_string()),
            Err(err) => warnings.push(format!("could not remove {}: {err}", path.display())),
        }
    }
    let _ = std::fs::remove_file(dir.join(COPY_FALLBACK_MARKER));
    if scope == PurgeScope::SeededOnly {
        let _ = std::fs::remove_dir(dir);
    }
    (removed, warnings)
}

/// The hook-reachability findings for the working tree at `root` (CR-106,
/// [FR-IN-03]) — what `doctor` reports so an installation that cannot fire is
/// never again indistinguishable from one that can.
///
/// Empty — deliberately, and not as a degraded answer — when `core.hooksPath`
/// is unset or belongs to another hook manager: opt-in stays opt-in and an
/// unhooked project is not degraded by a finding it cannot act on. Empty too
/// when git cannot answer at all, since `doctor` must not manufacture a
/// finding from a misread ([NFR-CC-04]).
///
/// [FR-IN-03]: ../../../docs/specs/requirements/FR-IN-03.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub fn reachability_findings(root: &Path) -> Vec<String> {
    if configured_hooks_path(root).ok().flatten().as_deref() != Some(HOOKS_RELDIR) {
        return Vec::new();
    }
    let dir = root.join(HOOKS_RELDIR);
    let mut findings = Vec::new();

    // `is_file` follows the link, so a DANGLING symlink reads as missing —
    // which is exactly what it is to git: nothing runs.
    let missing: Vec<&str> = all_hook_names()
        .filter(|name| !dir.join(name).is_file())
        .collect();
    if !missing.is_empty() {
        findings.push(format!(
            "core.hooksPath is `{HOOKS_RELDIR}`, but {} is missing {} — those git hooks \
             do NOT fire in this working tree (CR-106). Run any `logos` command here to \
             seed them from the primary checkout, or `logos init --hooks` in the primary \
             checkout if the installation itself is gone.",
            dir.display(),
            missing.join(", ")
        ));
    }
    findings.extend(copy_fallback_findings(&dir));
    findings.extend(unseeded_foreign_hook_findings(root, &dir));
    findings
}

/// The finding for a hook script that is **not ours**, sits in the source
/// checkout's hooks directory, and is therefore configured to run — but is
/// absent from this working tree, so it does not.
///
/// Seeding deliberately carries only the hooks logos manages: distributing a
/// third party's script into a context its author never configured is a much
/// larger claim than making our own installation reachable, and it is not the
/// one CR-106 makes. But silence here would rebuild the defect one storey up —
/// hooks appearing to work in a worktree while the user's own `pre-commit`
/// quietly does not is *precisely* the "every signal reads as success" shape.
/// So logos reports what it will not carry.
fn unseeded_foreign_hook_findings(root: &Path, dir: &Path) -> Vec<String> {
    let Some(source) = crate::workspace::primary_root(root) else {
        return Vec::new(); // the primary checkout: the scripts are already here
    };
    let Ok(entries) = std::fs::read_dir(source.join(HOOKS_RELDIR)) else {
        return Vec::new();
    };
    let mut unreachable: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            !name.starts_with('.')
                && !all_hook_names().any(|managed| managed == name)
                && is_hook_script(&source.join(HOOKS_RELDIR).join(name))
                && !dir.join(name).is_file()
        })
        .collect();
    if unreachable.is_empty() {
        return Vec::new();
    }
    unreachable.sort(); // deterministic ordering (NFR-RA-06)
    vec![format!(
        "{} holds hook script(s) logos does not manage ({}) that are absent from {} — they \
         run in that checkout and NOT in this working tree. Seeding carries only the hooks \
         logos installed; copy or link the rest yourself if they are meant to run here.",
        source.join(HOOKS_RELDIR).display(),
        unreachable.join(", "),
        dir.display()
    )]
}

/// Is `path` a plain file that git would actually execute as a hook?
fn is_hook_script(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// The findings [`COPY_FALLBACK_MARKER`] exists to make possible: this working
/// tree's hooks are copies rather than symlinks, and whether they have gone
/// stale against the source scripts they were copied from.
fn copy_fallback_findings(dir: &Path) -> Vec<String> {
    let Ok(note) = std::fs::read_to_string(dir.join(COPY_FALLBACK_MARKER)) else {
        return Vec::new();
    };
    let source = note
        .lines()
        .find_map(|line| line.strip_prefix(COPY_FALLBACK_SOURCE_KEY))
        .map(PathBuf::from);
    let Some(source) = source else {
        return vec![format!(
            "{} records a copy-fallback hook seed but names no source checkout — the \
             copies cannot be checked for staleness; re-run `logos init --hooks`.",
            dir.join(COPY_FALLBACK_MARKER).display()
        )];
    };
    let src_dir = source.join(HOOKS_RELDIR);
    let stale: Vec<&str> = all_hook_names()
        .filter(
            |name| match (std::fs::read(src_dir.join(name)), std::fs::read(dir.join(name))) {
                (Ok(source), Ok(local)) => source != local,
                // An unreadable pair is not evidence of staleness; the missing
                // half is already reported above where it matters.
                _ => false,
            },
        )
        .collect();
    if stale.is_empty() {
        vec![format!(
            "the hooks in {} are COPIES of {}'s scripts, not symlinks (this filesystem \
             refused symlinks): they cannot follow a re-install, so re-run \
             `logos init --hooks` after upgrading logos.",
            dir.display(),
            source.display()
        )]
    } else {
        vec![format!(
            "the hooks in {} are STALE copies of {}'s scripts ({} differ): this working \
             tree runs out-of-date hooks — re-run `logos init --hooks`.",
            dir.display(),
            source.display(),
            stale.join(", ")
        )]
    }
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
