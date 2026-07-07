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
