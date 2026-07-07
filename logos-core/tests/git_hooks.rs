//! End-to-end proof for the git-hook sync trigger (S-022,
//! [FR-SY-05](../../docs/specs/requirements/FR-SY-05.md),
//! [UAT-IN-01](../../docs/specs/requirements/UAT-IN-01.md) hook half):
//! install the managed hooks into a real repository, put a *stub* `logos`
//! binary on `PATH` that records its invocations, then drive real
//! `git commit` / `git checkout` / `git merge` operations and assert the
//! hooks fired a `sync` with the files each event changed.
//!
//! The stub keeps the test hermetic: no dependence on a built `logos` binary
//! or an indexed project — the contract under test is "the right git event
//! invokes `logos sync <changed files>`", nothing more. The sync behaviour
//! itself is covered by the pipeline and watcher suites.

#![cfg(unix)] // The hook scripts are /bin/sh; the stub uses a shell shebang.

use std::fs;
use std::process::Command;

use tempfile::TempDir;

/// A git repo with identity configured, plus a PATH dir holding the stub
/// `logos` that appends `sync <args>` lines to a log beside itself.
struct HookedRepo {
    root: TempDir,
    bin_dir: TempDir,
}

impl HookedRepo {
    fn new() -> Self {
        let root = TempDir::new().expect("repo dir");
        let bin_dir = TempDir::new().expect("bin dir");

        let repo = Self { root, bin_dir };
        repo.git(&["init", "--quiet", "-b", "main"]);
        repo.git(&["config", "user.email", "test@example.invalid"]);
        repo.git(&["config", "user.name", "Logos Test"]);

        // The recording stub: every invocation logs its argv tail.
        let log = repo.bin_dir.path().join("logos-calls.log");
        let stub = repo.bin_dir.path().join("logos");
        fs::write(
            &stub,
            format!("#!/bin/sh\necho \"$@\" >> {}\n", log.display()),
        )
        .expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod stub");
        }

        // Through the Engine facade — the seam S-023's `init -i` consumes —
        // so the e2e suite covers the public contract, not just the module.
        logos_core::Engine::install_hooks(repo.root.path()).expect("hooks install");
        repo
    }

    /// Run git in the repo with the stub dir prepended to PATH (hooks inherit
    /// the environment of the git invocation that fires them).
    fn git(&self, args: &[&str]) {
        let path = format!(
            "{}:{}",
            self.bin_dir.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let output = Command::new("git")
            .args(args)
            .env("PATH", path)
            .current_dir(self.root.path())
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write(&self, rel: &str, contents: &str) {
        let path = self.root.path().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parents");
        }
        fs::write(path, contents).expect("write");
    }

    /// The `logos` invocations the hooks made so far, one per line.
    fn calls(&self) -> Vec<String> {
        match fs::read_to_string(self.bin_dir.path().join("logos-calls.log")) {
            Ok(text) => text.lines().map(str::to_owned).collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// FR-SY-05 acceptance (commit): committing a file triggers
/// `logos sync --quiet <that file>` via the post-commit hook.
#[test]
fn a_commit_triggers_a_targeted_sync() {
    let repo = HookedRepo::new();
    repo.write("src/lib.rs", "pub fn hello() {}\n");
    // A path with a space exercises the NUL-delimited xargs handoff.
    repo.write("src/with space.rs", "pub fn spaced() {}\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "--quiet", "-m", "add lib"]);

    let calls = repo.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].starts_with("sync --quiet"), "{calls:?}");
    assert!(calls[0].contains("src/lib.rs"), "{calls:?}");
    assert!(calls[0].contains("src/with space.rs"), "{calls:?}");
}

/// FR-SY-05 acceptance (checkout): switching to a branch whose code differs
/// triggers a sync of the files that changed between the two HEADs.
#[test]
fn a_branch_checkout_triggers_a_sync_of_the_diff() {
    let repo = HookedRepo::new();
    repo.write("src/lib.rs", "pub fn on_main() {}\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "--quiet", "-m", "main version"]);

    repo.git(&["checkout", "--quiet", "-b", "feature"]);
    repo.write("src/lib.rs", "pub fn on_feature() {}\n");
    repo.write("src/extra.rs", "pub fn extra() {}\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "--quiet", "-m", "feature version"]);

    let before = repo.calls().len();
    repo.git(&["checkout", "--quiet", "main"]);

    let calls = repo.calls();
    let checkout_call = &calls[before..];
    assert_eq!(checkout_call.len(), 1, "{calls:?}");
    assert!(checkout_call[0].contains("src/lib.rs"), "{calls:?}");
    assert!(checkout_call[0].contains("src/extra.rs"), "{calls:?}");
}

/// The post-checkout `$3 = 0` guard, executed for real: a *file* checkout
/// (not a branch switch) fires the hook with the flag at 0 and must sync
/// nothing — only a branch checkout moves enough to justify one.
#[test]
fn a_file_checkout_does_not_trigger_a_sync() {
    let repo = HookedRepo::new();
    repo.write("src/lib.rs", "pub fn original() {}\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "--quiet", "-m", "base"]);

    let before = repo.calls().len();
    // Dirty the file, then restore it from HEAD — a file checkout ($3 = 0).
    repo.write("src/lib.rs", "pub fn scribbled() {}\n");
    repo.git(&["checkout", "--quiet", "HEAD", "--", "src/lib.rs"]);

    assert_eq!(
        repo.calls().len(),
        before,
        "a file checkout must not trigger a sync: {:?}",
        repo.calls()
    );
}

/// FR-SY-05 acceptance (merge): merging a branch triggers a sync of what the
/// merge brought in.
#[test]
fn a_merge_triggers_a_sync_of_the_incoming_files() {
    let repo = HookedRepo::new();
    repo.write("src/lib.rs", "pub fn base() {}\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "--quiet", "-m", "base"]);

    repo.git(&["checkout", "--quiet", "-b", "feature"]);
    repo.write("src/merged_in.rs", "pub fn merged_in() {}\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "--quiet", "-m", "feature work"]);
    repo.git(&["checkout", "--quiet", "main"]);

    let before = repo.calls().len();
    repo.git(&["merge", "--quiet", "--no-edit", "feature"]);

    let calls = repo.calls();
    let merge_calls = &calls[before..];
    assert!(
        merge_calls.iter().any(|c| c.contains("src/merged_in.rs")),
        "{calls:?}"
    );
}

/// FR-SY-06 posture: with `logos` NOT on PATH the hooks are silent no-ops —
/// the git operation succeeds untouched.
#[test]
fn hooks_without_logos_on_path_never_block_git() {
    let root = TempDir::new().expect("repo dir");
    let run = |args: &[&str]| {
        // A PATH with only the system dirs — no stub, and (in this test env)
        // no real `logos` either: constrain to /usr/bin:/bin where git lives.
        let output = Command::new("git")
            .args(args)
            .env("PATH", "/usr/bin:/bin")
            .current_dir(root.path())
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
    logos_core::hooks::install(root.path()).expect("hooks install");

    fs::write(root.path().join("file.rs"), "pub fn f() {}\n").expect("write");
    run(&["add", "file.rs"]);
    run(&["commit", "--quiet", "-m", "no logos anywhere"]);
}

/// The uninstall round-trip leaves the repository hook-free: a later commit
/// invokes nothing.
#[test]
fn uninstalled_hooks_no_longer_fire() {
    let repo = HookedRepo::new();
    logos_core::hooks::uninstall(repo.root.path()).expect("uninstall");

    repo.write("src/lib.rs", "pub fn quiet() {}\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "--quiet", "-m", "after uninstall"]);

    assert!(repo.calls().is_empty(), "{:?}", repo.calls());
}

// ── The enforcing pre-push gate (S-218, FR-IN-06 / FR-GV-02 / FR-GV-03) ──────
//
// End-to-end proof that the `pre-push` hook `init --hooks` installs turns a
// real `git push` into an enforced non-regression checkpoint: it PROPAGATES
// `logos check`'s exit (block on a violation), lets a clean check through, and
// bails OPEN when `logos` is absent so a machine without the tool is never
// falsely blocked. A stub `logos` on PATH makes `check`'s verdict scriptable
// without a built binary or an indexed project.

/// A repo wired for push tests: managed hooks installed via the Engine facade,
/// a bare `origin` remote to push into, and a stub `logos` on PATH whose
/// `check` behaviour the caller controls. `check_case` is the body of the
/// stub's `check)` branch — e.g. `"exit 0"` (clean) or
/// `"echo '…violation…' >&2; exit 1"` (a `severity='error'` regression).
struct PushRepo {
    root: TempDir,
    bin_dir: TempDir,
    _remote: TempDir,
}

impl PushRepo {
    fn new(check_case: &str) -> Self {
        let root = TempDir::new().expect("repo dir");
        let bin_dir = TempDir::new().expect("bin dir");
        let remote = TempDir::new().expect("remote dir");

        // The stub: freshness `sync` (fired by post-commit) is a silent no-op;
        // `check` (fired by pre-push) behaves as the scenario dictates.
        let stub = bin_dir.path().join("logos");
        fs::write(
            &stub,
            format!("#!/bin/sh\ncase \"$1\" in\n  check) {check_case} ;;\n  *) exit 0 ;;\nesac\n"),
        )
        .expect("write stub");
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod stub");
        }

        let repo = Self {
            root,
            bin_dir,
            _remote: remote,
        };

        // A bare remote to push into.
        repo.run_git(repo._remote.path(), &["init", "--quiet", "--bare"]);
        // The work repo, on `main` with an identity.
        repo.run_git(repo.root.path(), &["init", "--quiet", "-b", "main"]);
        repo.run_git(repo.root.path(), &["config", "user.email", "test@example.invalid"]);
        repo.run_git(repo.root.path(), &["config", "user.name", "Logos Test"]);

        // Install the managed hooks (incl. the pre-push gate) through the same
        // Engine seam `init -i` consumes.
        logos_core::Engine::install_hooks(repo.root.path()).expect("hooks install");

        // A commit to push; post-commit fires and calls the stub's `sync` no-op.
        repo.write("f.rs", "pub fn f() {}\n");
        repo.run_git(repo.root.path(), &["add", "."]);
        repo.run_git(repo.root.path(), &["commit", "--quiet", "-m", "seed"]);
        repo.run_git(
            repo.root.path(),
            &["remote", "add", "origin", &repo._remote.path().display().to_string()],
        );
        repo
    }

    /// PATH with the stub dir prepended (so `command -v logos` finds the stub).
    fn path_with_stub(&self) -> String {
        format!(
            "{}:{}",
            self.bin_dir.path().display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }

    /// Run git with the stub on PATH, asserting success (setup steps).
    fn run_git(&self, dir: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .env("PATH", self.path_with_stub())
            .current_dir(dir)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write(&self, rel: &str, contents: &str) {
        fs::write(self.root.path().join(rel), contents).expect("write");
    }

    /// Attempt `git push origin main` with the given PATH; return the raw
    /// outcome (no success assertion — the exit code is what's under test).
    fn push_with_path(&self, path: &str) -> std::process::Output {
        Command::new("git")
            .args(["push", "origin", "main"])
            .env("PATH", path)
            .current_dir(self.root.path())
            .output()
            .expect("git runs")
    }
}

/// FR-GV-03 acceptance: a push carrying a `severity='error'` violation is
/// blocked — `git push` exits non-zero and the violation is named.
#[test]
fn pre_push_blocks_a_push_carrying_an_error_violation() {
    let repo = PushRepo::new(
        "echo 'logos check: FR-DEAD dead code src/f.rs [severity=error]' >&2; exit 1",
    );

    let out = repo.push_with_path(&repo.path_with_stub());

    assert!(
        !out.status.success(),
        "the gate must block the push on an error violation"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("dead code src/f.rs"),
        "the blocking violation must be named: {stderr}"
    );
}

/// A clean working set pushes successfully (exit 0): `check` passes, the gate
/// lets the push through.
#[test]
fn pre_push_allows_a_push_when_check_is_clean() {
    let repo = PushRepo::new("exit 0");

    let out = repo.push_with_path(&repo.path_with_stub());

    assert!(
        out.status.success(),
        "a clean check must let the push through: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// An absent `logos` binary lets the push through (exit 0, no false block) —
/// even though the stub would exit 1 *if* it were reachable, proving it is the
/// binary's absence (not a clean check) that opens the gate.
#[test]
fn pre_push_bails_open_when_logos_is_absent() {
    let repo = PushRepo::new("exit 1");

    // A PATH with git (Apple git lives in /usr/bin) but no stub `logos`.
    let out = repo.push_with_path("/usr/bin:/bin");

    assert!(
        out.status.success(),
        "an absent logos must never falsely block a push: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `git push --no-verify` bypasses the gate entirely (git skips `pre-push`),
/// even with a stub that would otherwise block.
#[test]
fn pre_push_honours_no_verify() {
    let repo = PushRepo::new("exit 1");

    let out = Command::new("git")
        .args(["push", "--no-verify", "origin", "main"])
        .env("PATH", repo.path_with_stub())
        .current_dir(repo.root.path())
        .output()
        .expect("git runs");

    assert!(
        out.status.success(),
        "--no-verify must bypass the gate: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
