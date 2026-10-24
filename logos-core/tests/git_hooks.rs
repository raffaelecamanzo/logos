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
/// `logos` that records each invocation's exact argv in its own file under
/// `calls/` beside itself.
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

        // The recording stub: every invocation writes its argv — NUL-terminated
        // per argument, so a tab, a quote or a non-ASCII byte arrives exactly as
        // the hook passed it — to its own numbered file under `calls/`.
        let calls = repo.bin_dir.path().join("calls");
        fs::create_dir(&calls).expect("calls dir");
        let stub = repo.bin_dir.path().join("logos");
        fs::write(
            &stub,
            format!(
                "#!/bin/sh\n\
                 dir={dir}\n\
                 n=$(ls \"$dir\" | wc -l | tr -d ' ')\n\
                 printf '%s\\0' \"$@\" > \"$dir/call-$(printf %04d \"$n\")\"\n",
                dir = calls.display()
            ),
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

    /// The `logos` invocations the hooks made so far, each as its exact argv
    /// (bytes, in call order).
    fn argvs(&self) -> Vec<Vec<Vec<u8>>> {
        let dir = self.bin_dir.path().join("calls");
        let mut files: Vec<_> = fs::read_dir(dir)
            .expect("calls dir")
            .map(|entry| entry.expect("entry").path())
            .collect();
        files.sort();
        files
            .into_iter()
            .map(|file| {
                let raw = fs::read(file).expect("call record");
                raw.split_inclusive(|byte| *byte == 0)
                    .map(|arg| arg[..arg.len() - 1].to_vec())
                    .collect()
            })
            .collect()
    }

    /// The `logos` invocations the hooks made so far, one lossy line each —
    /// enough for the assertions that do not care about exact bytes.
    fn calls(&self) -> Vec<String> {
        self.argvs()
            .iter()
            .map(|argv| {
                argv.iter()
                    .map(|arg| String::from_utf8_lossy(arg).into_owned())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    /// Run git in the repo and return its stdout bytes — what git itself says,
    /// for the tests that compare the hook's argv against it.
    fn git_out(&self, args: &[&str]) -> Vec<u8> {
        let output = Command::new("git")
            .args(args)
            .current_dir(self.root.path())
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}");
        output.stdout
    }

    /// The paths of the `sync` invocations recorded after the first `before`
    /// calls, flattened in order: each argv minus `sync --quiet` and the `--`
    /// that ends option parsing, which [`Self::argvs`] callers assert themselves.
    fn synced_since(&self, before: usize) -> Vec<Vec<u8>> {
        self.argvs()
            .into_iter()
            .skip(before)
            .flat_map(|argv| {
                assert_eq!(argv[..2], [b"sync".to_vec(), b"--quiet".to_vec()]);
                argv[2..].iter().filter(|arg| *arg != b"--").cloned().collect::<Vec<_>>()
            })
            .collect()
    }
}

/// `git`'s NUL-separated path list, split into the paths themselves.
fn nul_split(raw: &[u8]) -> Vec<Vec<u8>> {
    raw.split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

/// FR-SY-05 acceptance (commit): committing a file triggers
/// `logos sync --quiet <that file>` via the post-commit hook.
#[test]
fn a_commit_triggers_a_targeted_sync() {
    let repo = HookedRepo::new();
    repo.write("src/lib.rs", "pub fn hello() {}\n");
    // A path with a space exercises the NUL-delimited handoff.
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

/// A path git records with a quote, a tab or non-ASCII bytes reaches `logos
/// sync` exactly as git stores it. Without `-z` git C-quotes all three
/// (`"caf\303\251.rs"`, `"a\"b.rs"`, `"tab\there.rs"`), so the sync is handed a
/// path that names no file and the symbol never enters the index.
#[test]
fn a_commit_hands_over_each_path_exactly_as_git_records_it() {
    let repo = HookedRepo::new();
    for name in ["café.rs", "a\"b.rs", "tab\there.rs"] {
        repo.write(name, "pub fn f() {}\n");
    }
    repo.git(&["add", "."]);
    let before = repo.calls().len();
    repo.git(&["commit", "--quiet", "-m", "awkward names"]);

    let recorded = nul_split(&repo.git_out(&[
        "diff-tree", "-r", "--root", "--name-only", "--no-renames", "-z", "--no-commit-id", "HEAD",
    ]));
    // The installed hooks sit in the same commit (`git add .`); git's list is
    // the oracle, and the three awkward names are in it.
    for name in ["a\"b.rs", "tab\there.rs"] {
        assert!(recorded.contains(&name.as_bytes().to_vec()), "{recorded:?}");
    }
    for argv in &repo.argvs()[before..] {
        assert_eq!(
            argv[..3],
            [b"sync".to_vec(), b"--quiet".to_vec(), b"--".to_vec()],
            "paths follow `--`, so a leading `-` is never read as an option"
        );
    }
    assert!(recorded.contains(&"caf\u{e9}.rs".as_bytes().to_vec()), "{recorded:?}");
    assert_eq!(
        repo.synced_since(before),
        recorded,
        "the stub must receive git's own bytes, one argument per path"
    );
}

/// Sort a synced-path list for comparison with the expected set.
fn sorted(paths: &[&str]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = paths.iter().map(|p| p.as_bytes().to_vec()).collect();
    out.sort();
    out
}

/// A repo whose `feature` branch adds `café.rs` and renames `old.rs` to
/// `new.rs`, left checked out on `feature`.
fn repo_with_a_feature_branch_that_renames() -> HookedRepo {
    let repo = HookedRepo::new();
    repo.write("old.rs", "pub fn renamed_thing() { /* enough body to detect a rename */ }\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "--quiet", "-m", "base"]);
    repo.git(&["checkout", "--quiet", "-b", "feature"]);
    repo.git(&["mv", "old.rs", "new.rs"]);
    repo.write("café.rs", "pub fn cafe() {}\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "--quiet", "-m", "rename and add"]);
    repo
}

/// A branch checkout syncs a non-ASCII path — git C-quotes it without `-z`.
#[test]
fn a_checkout_syncs_a_non_ascii_path() {
    let repo = repo_with_a_feature_branch_that_renames();
    let before = repo.calls().len();
    repo.git(&["checkout", "--quiet", "main"]);

    assert!(
        repo.synced_since(before).contains(&"caf\u{e9}.rs".as_bytes().to_vec()),
        "{:?}",
        repo.calls()
    );
}

/// A merge syncs a non-ASCII path it brought in.
#[test]
fn a_merge_syncs_a_non_ascii_path() {
    let repo = repo_with_a_feature_branch_that_renames();
    repo.git(&["checkout", "--quiet", "main"]);
    let before = repo.calls().len();
    repo.git(&["merge", "--quiet", "--no-edit", "feature"]);

    assert!(
        repo.synced_since(before).contains(&"caf\u{e9}.rs".as_bytes().to_vec()),
        "{:?}",
        repo.calls()
    );
}

/// A rename on a checkout syncs **both** sides: `git diff --name-only` collapses
/// a rename to its new path by default, so the old path's nodes would never be
/// removed from the index.
#[test]
fn a_checkout_syncs_both_sides_of_a_rename() {
    let repo = repo_with_a_feature_branch_that_renames();
    let before = repo.calls().len();
    repo.git(&["checkout", "--quiet", "main"]);

    let mut synced = repo.synced_since(before);
    synced.sort();
    assert_eq!(synced, sorted(&["caf\u{e9}.rs", "new.rs", "old.rs"]), "{:?}", repo.calls());
}

/// The same for what a merge brought in.
#[test]
fn a_merge_syncs_both_sides_of_a_rename() {
    let repo = repo_with_a_feature_branch_that_renames();
    repo.git(&["checkout", "--quiet", "main"]);
    let before = repo.calls().len();
    repo.git(&["merge", "--quiet", "--no-edit", "feature"]);

    let mut synced = repo.synced_since(before);
    synced.sort();
    assert_eq!(synced, sorted(&["caf\u{e9}.rs", "new.rs", "old.rs"]), "{:?}", repo.calls());
}

/// A commit that renames syncs both paths too — the post-commit command never
/// detected renames, and `--no-renames` keeps it that way on purpose.
#[test]
fn a_renaming_commit_syncs_both_sides() {
    let repo = HookedRepo::new();
    repo.write("old.rs", "pub fn renamed_thing() { /* enough body to detect a rename */ }\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "--quiet", "-m", "base"]);
    repo.git(&["mv", "old.rs", "new.rs"]);

    let before = repo.calls().len();
    repo.git(&["commit", "--quiet", "-m", "rename"]);

    let mut synced = repo.synced_since(before);
    synced.sort();
    assert_eq!(synced, sorted(&["new.rs", "old.rs"]));
}

/// An empty change set runs no sync: an empty commit, a checkout that lands on
/// a different commit with the same tree, and a checkout to the commit already
/// checked out.
#[test]
fn an_empty_change_set_runs_no_sync() {
    let repo = HookedRepo::new();
    repo.write("src/lib.rs", "pub fn base() {}\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "--quiet", "-m", "base"]);

    let before = repo.calls().len();
    repo.git(&["commit", "--quiet", "--allow-empty", "-m", "empty"]);
    repo.git(&["checkout", "--quiet", "-b", "same-tree"]);
    repo.git(&["checkout", "--quiet", "main"]);
    repo.git(&["checkout", "--quiet", "HEAD~1"]);

    assert_eq!(
        repo.calls().len(),
        before,
        "an empty set must not call `logos sync`: {:?}",
        repo.calls()
    );
}

/// The empty-set guard lives in the hook's own shell. A `xargs` that — like
/// GNU's without `-r` — runs its command once even on empty input must still
/// produce no sync, so the guard cannot be borrowed from the `xargs` flavour on
/// the machine.
#[test]
fn the_empty_set_guard_does_not_rely_on_xargs() {
    let repo = HookedRepo::new();
    repo.write("src/lib.rs", "pub fn base() {}\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "--quiet", "-m", "base"]);

    // A `xargs` that always runs its command, exactly once, and reads nothing.
    let fake_xargs = repo.bin_dir.path().join("xargs");
    fs::write(&fake_xargs, "#!/bin/sh\ncat >/dev/null\nshift\nexec \"$@\"\n").expect("xargs stub");
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_xargs, fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    let before = repo.calls().len();
    repo.git(&["commit", "--quiet", "--allow-empty", "-m", "empty"]);

    assert_eq!(
        repo.calls().len(),
        before,
        "the guard must live in the hook: {:?}",
        repo.calls()
    );
}

/// A hook whose listing fails stays silent and exits 0, and syncs nothing:
/// `post-merge` run with no `ORIG_HEAD` makes git print a fatal ambiguity error,
/// which must never reach the terminal of the user whose git operation fired it.
#[test]
fn a_failing_listing_is_silent_and_syncs_nothing() {
    let repo = HookedRepo::new();
    repo.write("src/lib.rs", "pub fn base() {}\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "--quiet", "-m", "base"]);
    let before = repo.calls().len();

    // A fresh repository has no ORIG_HEAD; run the installed hook directly.
    let path = format!(
        "{}:{}",
        repo.bin_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new("sh")
        .arg(repo.root.path().join(".logos/hooks/post-merge"))
        .env("PATH", path)
        .current_dir(repo.root.path())
        .output()
        .expect("hook runs");

    assert!(output.status.success(), "a freshness hook always exits 0");
    assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stdout.is_empty(), "{}", String::from_utf8_lossy(&output.stdout));
    assert_eq!(repo.calls().len(), before, "{:?}", repo.calls());
}

/// A merge of a diverged branch — a real merge commit, not a fast-forward —
/// syncs the incoming non-ASCII path and both sides of the rename.
#[test]
fn a_true_merge_commit_syncs_what_it_brought_in() {
    let repo = repo_with_a_feature_branch_that_renames();
    repo.git(&["checkout", "--quiet", "main"]);
    repo.write("main_only.rs", "pub fn main_only() {}\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "--quiet", "-m", "main moves on"]);

    let before = repo.calls().len();
    repo.git(&["merge", "--quiet", "--no-edit", "--no-ff", "feature"]);

    let mut synced = repo.synced_since(before);
    synced.sort();
    assert_eq!(
        synced,
        sorted(&["caf\u{e9}.rs", "new.rs", "old.rs"]),
        "{:?}",
        repo.calls()
    );
}

/// A name beginning with `-` and a name holding a newline reach the sync as
/// single, intact arguments. (That `--` precedes them is pinned by the exact-argv
/// assertions elsewhere: the recording stub does not parse options, so this test
/// alone cannot tell the `--` is missing.)
#[test]
fn a_dash_name_and_a_newline_name_arrive_intact() {
    let repo = HookedRepo::new();
    repo.write("-dash.rs", "pub fn dash() {}\n");
    repo.write("new\nline.rs", "pub fn newline() {}\n");
    repo.git(&["add", "--", "-dash.rs", "new\nline.rs"]);
    let before = repo.calls().len();
    repo.git(&["commit", "--quiet", "-m", "dash and newline"]);

    let mut synced = repo.synced_since(before);
    synced.sort();
    assert_eq!(synced, sorted(&["-dash.rs", "new\nline.rs"]));
}

/// A change set larger than one `xargs` batch is synced in full, every batch
/// carrying `sync --quiet --`. The stub `xargs` forces two paths per batch.
#[test]
fn a_change_set_split_across_batches_is_synced_in_full() {
    let repo = HookedRepo::new();
    let batching_xargs = repo.bin_dir.path().join("xargs");
    fs::write(&batching_xargs, "#!/bin/sh\nexec /usr/bin/xargs -n 2 \"$@\"\n").expect("xargs stub");
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&batching_xargs, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    for n in 0..5 {
        repo.write(&format!("f{n}.rs"), "pub fn f() {}\n");
    }
    repo.git(&["add", "--", "f0.rs", "f1.rs", "f2.rs", "f3.rs", "f4.rs"]);
    let before = repo.calls().len();
    repo.git(&["commit", "--quiet", "-m", "five files"]);

    let argvs = repo.argvs();
    let batches = &argvs[before..];
    assert!(batches.len() >= 3, "five paths at two per batch: {:?}", repo.calls());
    let mut synced = repo.synced_since(before);
    synced.sort();
    assert_eq!(synced, sorted(&["f0.rs", "f1.rs", "f2.rs", "f3.rs", "f4.rs"]));
    for argv in batches {
        assert_eq!(argv[..3], [b"sync".to_vec(), b"--quiet".to_vec(), b"--".to_vec()]);
    }
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
