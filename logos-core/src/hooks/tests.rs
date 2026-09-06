//! Tests for the `core.hooksPath` git-hook installer ([FR-SY-05],
//! [FR-IN-03]): installation, idempotency, the non-clobbering contract, and
//! uninstall. The end-to-end "a commit triggers `logos sync`" proof lives in
//! `tests/git_hooks.rs` (real `git commit` against a stub `logos` binary).
//!
//! [FR-SY-05]: ../../../docs/specs/requirements/FR-SY-05.md
//! [FR-IN-03]: ../../../docs/specs/requirements/FR-IN-03.md

use super::*;

use tempfile::TempDir;

/// A throwaway git repository with one identity-configured commit-ready tree.
fn git_repo() -> TempDir {
    let tmp = TempDir::new().expect("temp dir");
    let run = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(tmp.path())
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(&["init", "--quiet"]);
    run(&["config", "user.email", "test@example.invalid"]);
    run(&["config", "user.name", "Logos Test"]);
    tmp
}

#[test]
fn install_writes_all_four_hooks_and_sets_hooks_path() {
    let repo = git_repo();
    let result = install(repo.path()).expect("install succeeds");

    assert_eq!(
        result.installed,
        vec!["post-commit", "post-checkout", "post-merge", "pre-push"]
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.hooks_dir, ".logos/hooks");

    // The three freshness hooks: best-effort, unconditional success.
    for name in ["post-commit", "post-checkout", "post-merge"] {
        let path = repo.path().join(".logos/hooks").join(name);
        let body = std::fs::read_to_string(&path).expect("hook exists");
        assert!(body.starts_with("#!/bin/sh"), "{name}: {body}");
        assert!(body.contains(MANAGED_MARKER), "{name} carries the marker");
        assert!(body.contains("logos sync --quiet"), "{name} syncs");
        // Never block the git operation: unconditional success.
        assert!(body.trim_end().ends_with("exit 0"), "{name} exits 0");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_ne!(mode & 0o111, 0, "{name} is executable");
        }
    }

    // The enforcing pre-push gate: marker, bail-open guard, and a PROPAGATED
    // `logos check` — it must NOT be exit-0-swallowed like the freshness hooks.
    let gate = repo.path().join(".logos/hooks/pre-push");
    let body = std::fs::read_to_string(&gate).expect("pre-push exists");
    assert!(body.starts_with("#!/bin/sh"), "pre-push: {body}");
    assert!(body.contains(MANAGED_MARKER), "pre-push carries the marker");
    assert!(body.contains("exec logos check"), "pre-push runs the gate");
    assert!(
        body.contains("command -v logos >/dev/null 2>&1 || exit 0"),
        "pre-push bails open when logos is absent"
    );
    assert!(
        !body.trim_end().ends_with("exit 0"),
        "pre-push must propagate check's exit, not swallow it: {body}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&gate).unwrap().permissions().mode();
        assert_ne!(mode & 0o111, 0, "pre-push is executable");
    }

    assert_eq!(
        configured_hooks_path(repo.path()).unwrap().as_deref(),
        Some(".logos/hooks")
    );
}

#[test]
fn install_is_idempotent_over_its_own_hooks() {
    let repo = git_repo();
    install(repo.path()).expect("first install");
    let second = install(repo.path()).expect("second install");

    // Re-running refreshes the managed scripts rather than warning or duplicating.
    assert_eq!(second.installed.len(), 4, "{second:?}");
    assert!(second.warnings.is_empty(), "{second:?}");
}

#[test]
fn install_never_clobbers_a_foreign_hooks_path() {
    let repo = git_repo();
    git_config(repo.path(), "core.hooksPath", ".husky").expect("preset hooksPath");

    let result = install(repo.path()).expect("install returns");

    assert!(result.installed.is_empty(), "{result:?}");
    assert_eq!(result.warnings.len(), 1, "{result:?}");
    assert!(result.warnings[0].contains(".husky"), "{result:?}");
    // The user's configuration is untouched and our directory was not created.
    assert_eq!(
        configured_hooks_path(repo.path()).unwrap().as_deref(),
        Some(".husky")
    );
    assert!(!repo.path().join(".logos/hooks").exists());
}

#[test]
fn install_never_overwrites_a_foreign_script_in_our_dir() {
    let repo = git_repo();
    let hooks_dir = repo.path().join(".logos/hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    std::fs::write(hooks_dir.join("post-commit"), "#!/bin/sh\n# hand-written\n").unwrap();

    let result = install(repo.path()).expect("install returns");

    // The foreign script is preserved verbatim; the other three install fine.
    let body = std::fs::read_to_string(hooks_dir.join("post-commit")).unwrap();
    assert_eq!(body, "#!/bin/sh\n# hand-written\n");
    assert_eq!(
        result.installed,
        vec!["post-checkout", "post-merge", "pre-push"]
    );
    assert_eq!(result.warnings.len(), 1, "{result:?}");
}

#[test]
fn install_outside_a_git_repo_is_an_actionable_error() {
    let tmp = TempDir::new().unwrap();
    // Note: a temp dir under a developer's home could still be inside SOME
    // work tree only if the temp root is — `/tmp`/`$TMPDIR` never is.
    let err = install(tmp.path()).expect_err("no repo, no hooks");
    assert!(err.to_string().contains("git init"), "{err:#}");
}

#[test]
fn uninstall_removes_managed_hooks_and_unsets_hooks_path() {
    let repo = git_repo();
    install(repo.path()).expect("install");

    let result = uninstall(repo.path()).expect("uninstall");

    assert_eq!(result.installed.len(), 4, "{result:?}");
    assert!(!repo.path().join(".logos/hooks/post-commit").exists());
    assert!(!repo.path().join(".logos/hooks/pre-push").exists());
    assert_eq!(configured_hooks_path(repo.path()).unwrap(), None);
}

#[test]
fn uninstall_leaves_foreign_scripts_and_foreign_hooks_path() {
    let repo = git_repo();
    install(repo.path()).expect("install");
    // The user repointed hooksPath afterwards: that is theirs now.
    git_config(repo.path(), "core.hooksPath", ".husky").unwrap();
    // And hand-edited one script (dropping our marker).
    let edited = repo.path().join(".logos/hooks/post-merge");
    std::fs::write(&edited, "#!/bin/sh\n# customized\n").unwrap();

    let result = uninstall(repo.path()).expect("uninstall");

    assert!(edited.exists(), "hand-edited script preserved");
    assert!(result.warnings.iter().any(|w| w.contains("post-merge")));
    assert_eq!(
        configured_hooks_path(repo.path()).unwrap().as_deref(),
        Some(".husky"),
        "a repointed hooksPath is never unset"
    );
}

/// The post-checkout script ignores file checkouts (`$3 = 0`) — only a
/// branch switch moves enough to justify a sync.
#[test]
fn post_checkout_script_guards_on_branch_flag() {
    let (_, changed_cmd) = HOOKS
        .iter()
        .find(|(name, _)| *name == "post-checkout")
        .unwrap();
    let body = script_body("post-checkout", changed_cmd);
    assert!(body.contains("[ \"$3\" = \"1\" ] || exit 0"), "{body}");
}

/// The pre-push gate is the mirror image of the freshness scripts: it bails
/// open when `logos` is absent but otherwise `exec`s `logos check` so the
/// gate's exit code becomes the hook's — a regression blocks the push.
#[test]
fn blocking_script_propagates_check_and_bails_open_on_absence() {
    let body = blocking_script_body();
    assert!(body.starts_with("#!/bin/sh"), "{body}");
    assert!(body.contains(MANAGED_MARKER), "{body}");
    // Bail open only when the binary is genuinely missing.
    assert!(
        body.contains("command -v logos >/dev/null 2>&1 || exit 0"),
        "{body}"
    );
    // Propagate, never swallow: `exec` hands the exit code straight through.
    assert!(body.contains("exec logos check"), "{body}");
    assert!(
        !body.trim_end().ends_with("exit 0"),
        "the gate must not end on an unconditional success: {body}"
    );
}

/// The managed set is exactly the three freshness hooks plus the one gate, and
/// the two accessors agree on the roster.
#[test]
fn managed_set_is_the_three_freshness_hooks_plus_the_gate() {
    let names: Vec<&str> = all_hook_names().collect();
    assert_eq!(
        names,
        vec!["post-commit", "post-checkout", "post-merge", "pre-push"]
    );
    let scripted: Vec<&str> = managed_scripts().into_iter().map(|(n, _)| n).collect();
    assert_eq!(scripted, names, "both accessors list the same roster");
}

// ── Every-working-tree reachability (CR-106) ────────────────────────────────
//
// The end-to-end proof — real `git worktree add`, real commits and pushes,
// hooks asserted by the side effect of their own bodies — lives in
// `tests/worktree_hooks.rs`. What is left here is the mechanism the e2e suite
// cannot reach: the copy fallback (a filesystem that refuses symlinks is not
// something CI can be asked to provide) and the `doctor` findings.

/// A primary checkout with hooks installed, plus an empty directory standing
/// in for a linked worktree — enough for the seeding mechanism, which never
/// asks git what a worktree is.
fn installed_repo_and_worktree() -> (TempDir, TempDir) {
    let primary = git_repo();
    install(primary.path()).expect("install succeeds");
    (primary, TempDir::new().expect("worktree dir"))
}

#[test]
fn the_copy_fallback_writes_real_scripts_and_a_staleness_marker() {
    let (primary, worktree) = installed_repo_and_worktree();

    let seeded = seed_worktree_with(primary.path(), worktree.path(), LinkMode::Copy);

    assert_eq!(seeded.copied.len(), 4, "{seeded:?}");
    assert!(seeded.linked.is_empty(), "{seeded:?}");
    let copy = worktree.path().join(HOOKS_RELDIR).join("post-commit");
    assert!(
        !std::fs::symlink_metadata(&copy).unwrap().file_type().is_symlink(),
        "the fallback must produce a real file"
    );
    assert_eq!(
        std::fs::read_to_string(&copy).unwrap(),
        std::fs::read_to_string(primary.path().join(HOOKS_RELDIR).join("post-commit")).unwrap(),
    );

    // Never silent: the marker is what lets `doctor` say these are copies.
    let note = std::fs::read_to_string(worktree.path().join(HOOKS_RELDIR).join(COPY_FALLBACK_MARKER))
        .expect("the staleness marker exists");
    assert!(note.contains(&primary.path().display().to_string()), "{note}");
}

#[test]
fn a_copy_fallback_seed_is_reported_by_doctor_and_flagged_when_it_goes_stale() {
    let (primary, worktree) = installed_repo_and_worktree();
    seed_worktree_with(primary.path(), worktree.path(), LinkMode::Copy);
    // `reachability_findings` reads `core.hooksPath`, which is repo-global; the
    // stand-in worktree is a bare directory, so point it at the primary's
    // configuration by asking about the primary's own tree.
    let findings = copy_fallback_findings(&worktree.path().join(HOOKS_RELDIR));
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].contains("are COPIES"), "{findings:?}");
    assert!(!findings[0].contains("STALE"), "{findings:?}");

    // A re-install that updates the primary's scripts is exactly what copies
    // cannot follow — and exactly what the marker exists to make visible.
    std::fs::write(
        primary.path().join(HOOKS_RELDIR).join("post-commit"),
        format!("#!/bin/sh\n{MANAGED_MARKER}: post-commit\n# a newer body\nexit 0\n"),
    )
    .expect("rewrite the primary script");

    let findings = copy_fallback_findings(&worktree.path().join(HOOKS_RELDIR));
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].contains("STALE"), "{findings:?}");
    assert!(findings[0].contains("post-commit"), "{findings:?}");
}

#[test]
fn a_second_copy_fallback_seed_refreshes_its_own_copies() {
    let (primary, worktree) = installed_repo_and_worktree();
    seed_worktree_with(primary.path(), worktree.path(), LinkMode::Copy);
    let newer = format!("#!/bin/sh\n{MANAGED_MARKER}: post-commit\n# a newer body\nexit 0\n");
    std::fs::write(primary.path().join(HOOKS_RELDIR).join("post-commit"), &newer).unwrap();

    let seeded = seed_worktree_with(primary.path(), worktree.path(), LinkMode::Copy);

    assert_eq!(seeded.copied.len(), 4, "{seeded:?}");
    assert_eq!(
        std::fs::read_to_string(worktree.path().join(HOOKS_RELDIR).join("post-commit")).unwrap(),
        newer,
        "a marked copy is ours to refresh"
    );
    assert!(copy_fallback_findings(&worktree.path().join(HOOKS_RELDIR))
        .first()
        .is_some_and(|f| !f.contains("STALE")));
}

#[test]
fn seeding_over_a_checked_in_hook_leaves_it_exactly_as_it_found_it() {
    let (primary, worktree) = installed_repo_and_worktree();
    // The `.logos/hooks` directory is deliberately not gitignored, so a team
    // may commit its hooks; a worktree checkout then already has them.
    let checked_in = worktree.path().join(HOOKS_RELDIR).join("post-commit");
    std::fs::create_dir_all(checked_in.parent().unwrap()).unwrap();
    let body = format!("#!/bin/sh\n{MANAGED_MARKER}: post-commit\n# shared through git\nexit 0\n");
    std::fs::write(&checked_in, &body).unwrap();

    let seeded = seed_worktree(primary.path(), worktree.path());

    assert_eq!(seeded.present, vec!["post-commit".to_string()], "{seeded:?}");
    assert_eq!(
        std::fs::read_to_string(&checked_in).unwrap(),
        body,
        "replacing it would dirty every working tree's `git status`"
    );
    assert_eq!(seeded.linked.len(), 3, "the rest are still seeded: {seeded:?}");
}

#[test]
fn seeding_a_repo_without_hooks_creates_nothing() {
    let primary = git_repo(); // no `install`
    let worktree = TempDir::new().expect("worktree dir");

    let seeded = seed_worktree(primary.path(), worktree.path());

    assert!(seeded.is_empty() && seeded.warnings.is_empty(), "{seeded:?}");
    assert!(
        !worktree.path().join(".logos").exists(),
        "opt-in stays opt-in: an unhooked project must not grow a hooks directory"
    );
}

#[test]
fn reachability_reports_an_installed_configuration_whose_hooks_are_gone() {
    let repo = git_repo();
    install(repo.path()).expect("install succeeds");
    assert!(
        reachability_findings(repo.path()).is_empty(),
        "a reachable installation is not a finding"
    );

    // Exactly the shape a linked worktree had: `core.hooksPath` set and
    // resolving to a directory that does not exist.
    std::fs::remove_dir_all(repo.path().join(HOOKS_RELDIR)).unwrap();

    let findings = reachability_findings(repo.path());
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].contains("do NOT fire in this working tree"), "{findings:?}");
    for name in all_hook_names() {
        assert!(findings[0].contains(name), "{findings:?}");
    }
}

#[test]
fn reachability_reports_a_dangling_symlink_as_the_nothing_ran_that_it_is() {
    let (primary, worktree) = installed_repo_and_worktree();
    seed_worktree(primary.path(), worktree.path());
    // Uninstalling the source first is precisely what leaves dangling links.
    std::fs::remove_file(primary.path().join(HOOKS_RELDIR).join("post-commit")).unwrap();

    let dir = worktree.path().join(HOOKS_RELDIR);
    assert!(
        std::fs::symlink_metadata(dir.join("post-commit")).is_ok(),
        "the link is still there — which is exactly why presence proves nothing"
    );
    assert!(!dir.join("post-commit").is_file(), "…and it resolves to nothing");
}

#[test]
fn reachability_is_silent_when_hooks_are_absent_by_choice_or_foreign() {
    let repo = git_repo();
    assert!(
        reachability_findings(repo.path()).is_empty(),
        "an unhooked project must not be degraded by a finding it cannot act on"
    );

    git_config(repo.path(), "core.hooksPath", ".husky").expect("set a foreign hooks path");
    assert!(
        reachability_findings(repo.path()).is_empty(),
        "another hook manager's configuration is not ours to report on"
    );
}

#[test]
fn uninstall_clears_the_seeded_worktree_hooks_and_leaves_a_checked_in_one() {
    let (primary, worktree) = installed_repo_and_worktree();
    let dir = worktree.path().join(HOOKS_RELDIR);
    seed_worktree(primary.path(), worktree.path());
    // One hook the repository shares through git, not one this seed placed.
    std::fs::remove_file(dir.join("post-merge")).unwrap();
    std::fs::write(
        dir.join("post-merge"),
        format!("#!/bin/sh\n{MANAGED_MARKER}: post-merge\nexit 0\n"),
    )
    .unwrap();

    let (removed, warnings) = purge_hooks_dir(&dir, PurgeScope::SeededOnly);

    assert_eq!(removed.len(), 3, "{removed:?} {warnings:?}");
    assert!(
        dir.join("post-merge").is_file(),
        "a checked-in hook belongs to the repository, not to this seed"
    );
    assert!(!dir.join(COPY_FALLBACK_MARKER).exists());
}

#[test]
fn concurrent_seeds_never_truncate_the_source_scripts() {
    // Every engine start seeds, and nothing serialises them: two logos
    // processes starting in the same fresh worktree race on `place_hook`. The
    // invariant that matters is not who wins — it is that the PRIMARY's single
    // real copy of each script survives intact. A seed that copied onto a
    // symlink still resolving to the source would truncate it to zero bytes,
    // and an empty executable exits 0, silently disabling the `pre-push` gate
    // for the whole repository while every call returned `Ok`.
    let (primary, worktree) = installed_repo_and_worktree();
    let before: Vec<(String, String)> = all_hook_names()
        .map(|name| {
            let path = primary.path().join(HOOKS_RELDIR).join(name);
            (name.to_string(), std::fs::read_to_string(path).expect("source script"))
        })
        .collect();

    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                seed_worktree(primary.path(), worktree.path());
            });
        }
    });

    for (name, body) in &before {
        assert!(!body.is_empty(), "fixture sanity: {name} was non-empty to begin with");
        assert_eq!(
            &std::fs::read_to_string(primary.path().join(HOOKS_RELDIR).join(name))
                .expect("the source script must survive every racing seed"),
            body,
            "{name} in the primary checkout was corrupted by a concurrent seed"
        );
        // …and the worktree ends up reachable, not merely uncorrupted.
        assert_eq!(
            &std::fs::read_to_string(worktree.path().join(HOOKS_RELDIR).join(name))
                .expect("the seeded hook resolves"),
            body,
            "{name} is not reachable from the worktree after the race"
        );
    }
}

#[test]
fn an_unreadable_foreign_hook_is_neither_replaced_nor_removed() {
    // `.logos/hooks/` is a directory the user may keep their own scripts in,
    // so the non-clobbering posture must hold for the ones we cannot READ too.
    // Only a `NotFound` read may be taken as "a dangling link of ours"; a
    // symlink to a directory, or a script that is not valid UTF-8, says
    // nothing about ownership — and treating either as ours would silently
    // delete somebody else's hook.
    let (primary, worktree) = installed_repo_and_worktree();
    let dir = worktree.path().join(HOOKS_RELDIR);
    std::fs::create_dir_all(&dir).unwrap();

    // A symlink pointing at a directory: readable as a link, unreadable as text.
    let to_a_dir = dir.join("post-commit");
    std::os::unix::fs::symlink(worktree.path(), &to_a_dir).unwrap();
    // A hook that is simply not valid UTF-8.
    let not_utf8 = dir.join("post-merge");
    std::fs::write(&not_utf8, [0x23, 0x21, 0xff, 0xfe, 0x0a]).unwrap();

    let seeded = seed_worktree(primary.path(), worktree.path());

    assert_eq!(seeded.warnings.len(), 2, "both must be reported, not swallowed: {seeded:?}");
    assert!(std::fs::symlink_metadata(&to_a_dir).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read(&not_utf8).unwrap(), [0x23, 0x21, 0xff, 0xfe, 0x0a]);

    // …and an uninstall must not delete them either.
    let (removed, warnings) = purge_hooks_dir(&dir, PurgeScope::SeededOnly);
    assert!(!removed.contains(&"post-commit".to_string()), "{removed:?}");
    assert!(!removed.contains(&"post-merge".to_string()), "{removed:?}");
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(std::fs::symlink_metadata(&to_a_dir).is_ok(), "foreign symlink survived");
    assert!(not_utf8.is_file(), "foreign non-UTF-8 hook survived");
}
