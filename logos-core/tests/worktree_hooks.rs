//! End-to-end proof that installed git hooks fire in a **linked worktree**
//! (CR-106, [FR-IN-03](../../docs/specs/requirements/FR-IN-03.md),
//! [FR-SY-05](../../docs/specs/requirements/FR-SY-05.md),
//! [FR-IN-06](../../docs/specs/requirements/FR-IN-06.md),
//! [FR-WT-01](../../docs/specs/requirements/FR-WT-01.md)).
//!
//! `core.hooksPath` is the relative `.logos/hooks`, and git resolves a relative
//! `core.hooksPath` against the top level of the working tree the command runs
//! in — so before this suite existed, **no logos git hook had ever fired in a
//! linked worktree**: not the commit/checkout/merge freshening, and not the
//! blocking `pre-push` gate.
//!
//! # Every assertion here is an observable side effect of a hook *body*
//!
//! Never the presence of `core.hooksPath`, never the presence of a script
//! file. Those two are exactly what read as success for the entire life of the
//! defect — the installer reported the hooks installed, `git config` returned a
//! value, and the scripts were on disk, while nothing ran. A test that asserts
//! either one would have passed then and proves nothing now.
//!
//! The stub `logos` on `PATH` writes its invocation into
//! `$(git rev-parse --show-toplevel)/.logos/hook-log` — resolving its root the
//! same way `logos` itself does ([FR-WT-01]) — so the log's *location* answers
//! "which graph moved", not merely "something ran".
//!
//! [FR-WT-01]: ../../docs/specs/requirements/FR-WT-01.md

#![cfg(unix)] // The hook scripts are /bin/sh; the stub uses a shell shebang.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// A repository with a primary checkout, a stub `logos` on `PATH`, and a bare
/// remote to push into. The linked worktree is added by the test, so each one
/// controls whether it is created before or after the hook installation.
struct Repo {
    main: TempDir,
    bin: TempDir,
    remote: TempDir,
    /// Where linked worktrees are created — outside the primary checkout, so a
    /// worktree is never a subdirectory of the tree it links to.
    trees: TempDir,
}

impl Repo {
    /// `check_case` is the body of the stub's `check)` branch — `"exit 0"` for
    /// a clean gate, `"...; exit 1"` for a `severity='error'` regression.
    fn new(check_case: &str) -> Self {
        let repo = Self {
            main: TempDir::new().expect("main dir"),
            bin: TempDir::new().expect("bin dir"),
            remote: TempDir::new().expect("remote dir"),
            trees: TempDir::new().expect("worktrees dir"),
        };

        // The stub resolves its own root exactly as `logos` does — through
        // `git rev-parse --show-toplevel` — and records the invocation there.
        // Which `.logos/` the line lands in IS the "which graph moved" answer.
        let stub = repo.bin.path().join("logos");
        fs::write(
            &stub,
            format!(
                "#!/bin/sh\n\
                 top=$(git rev-parse --show-toplevel 2>/dev/null)\n\
                 [ -n \"$top\" ] && mkdir -p \"$top/.logos\" && echo \"$*\" >> \"$top/.logos/hook-log\"\n\
                 case \"$1\" in\n  check) {check_case} ;;\nesac\n\
                 exit 0\n"
            ),
        )
        .expect("write stub");
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod stub");
        }

        repo.git(repo.remote.path(), &["init", "--quiet", "--bare"]);
        repo.git(repo.main.path(), &["init", "--quiet", "-b", "main"]);
        repo.git(repo.main.path(), &["config", "user.email", "t@example.invalid"]);
        repo.git(repo.main.path(), &["config", "user.name", "Logos Test"]);
        repo.commit(repo.main.path(), "base.rs", "pub fn base() {}\n", "base");
        repo.git(
            repo.main.path(),
            &["remote", "add", "origin", &repo.remote.path().display().to_string()],
        );
        repo
    }

    fn path_with_stub(&self) -> String {
        format!(
            "{}:{}",
            self.bin.path().display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }

    /// Run git with the stub on `PATH` (hooks inherit git's environment),
    /// asserting success — for setup steps whose failure is not under test.
    fn git(&self, dir: &Path, args: &[&str]) {
        let out = self.try_git(dir, args, &self.path_with_stub());
        assert!(
            out.status.success(),
            "git {args:?} in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Run git and hand back the raw outcome — the exit code is what the
    /// `pre-push` tests are asserting on.
    fn try_git(&self, dir: &Path, args: &[&str], path: &str) -> Output {
        Command::new("git")
            .args(args)
            .env("PATH", path)
            .current_dir(dir)
            .output()
            .expect("git runs")
    }

    fn write(&self, dir: &Path, rel: &str, contents: &str) {
        fs::write(dir.join(rel), contents).expect("write");
    }

    /// Write one file and commit exactly it. Deliberately never `git add .`:
    /// `.logos/hooks/` is not gitignored ([FR-IN-04]), so a blanket add would
    /// commit the hook scripts themselves and turn every later checkout and
    /// merge into a fight over them — a fixture artefact, not the behaviour
    /// under test.
    fn commit(&self, dir: &Path, rel: &str, contents: &str, message: &str) {
        self.write(dir, rel, contents);
        self.git(dir, &["add", "--", rel]);
        self.git(dir, &["commit", "--quiet", "-m", message]);
    }

    /// Install the managed hooks through the same `Engine` seam `init -i`
    /// consumes.
    fn install(&self) -> logos_core::hooks::HooksResult {
        logos_core::Engine::install_hooks(self.main.path()).expect("hooks install")
    }

    /// Add a linked worktree on a new branch and return its root.
    fn add_worktree(&self, name: &str) -> PathBuf {
        let path = self.trees.path().join(name);
        self.git(
            self.main.path(),
            &["worktree", "add", "--quiet", "-b", name, &path.display().to_string()],
        );
        path
    }

    /// The hook invocations recorded in one working tree's own `.logos/`.
    fn log(&self, tree: &Path) -> Vec<String> {
        match fs::read_to_string(tree.join(".logos/hook-log")) {
            Ok(text) => text.lines().map(str::to_owned).collect(),
            Err(_) => Vec::new(),
        }
    }

    fn hooks_path(&self, dir: &Path) -> String {
        let out = self.try_git(dir, &["config", "core.hooksPath"], &self.path_with_stub());
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
}

/// "First logos use in this worktree" — the bootstrap seam `Engine::start`
/// runs. Tests that are not specifically about the engine call it directly, so
/// the hook behaviour under test is not entangled with runtime startup.
fn first_logos_use(worktree: &Path) {
    logos_core::hooks::seed_from_primary(worktree);
}

// ── The freshening hooks (FR-SY-05, FR-IN-03) ───────────────────────────────

/// A commit in a linked worktree runs `post-commit` — asserted by the sync the
/// hook body performs, which is the thing that never happened.
#[test]
fn a_commit_in_a_linked_worktree_triggers_a_targeted_sync() {
    let repo = Repo::new("exit 0");
    repo.install();
    let wt = repo.add_worktree("feature");
    first_logos_use(&wt);

    repo.commit(&wt, "only_here.rs", "pub fn only_here() {}\n", "in the worktree");

    let calls = repo.log(&wt);
    assert_eq!(calls.len(), 1, "post-commit must fire in a worktree: {calls:?}");
    assert!(calls[0].starts_with("sync --quiet"), "{calls:?}");
    assert!(calls[0].contains("only_here.rs"), "{calls:?}");
}

/// A branch checkout in a linked worktree runs `post-checkout` with the diff
/// between the two HEADs.
#[test]
fn a_branch_checkout_in_a_linked_worktree_triggers_a_sync_of_the_diff() {
    let repo = Repo::new("exit 0");
    repo.install();
    let wt = repo.add_worktree("feature");
    first_logos_use(&wt);

    repo.commit(&wt, "moved.rs", "pub fn on_feature() {}\n", "feature version");

    let before = repo.log(&wt).len();
    repo.git(&wt, &["checkout", "--quiet", "-b", "sidebranch", "HEAD~1"]);

    let calls = repo.log(&wt);
    let checkout = &calls[before..];
    assert_eq!(checkout.len(), 1, "post-checkout must fire in a worktree: {calls:?}");
    assert!(checkout[0].contains("moved.rs"), "{calls:?}");
}

/// A merge in a linked worktree runs `post-merge` with what the merge brought
/// in.
#[test]
fn a_merge_in_a_linked_worktree_triggers_a_sync_of_the_incoming_files() {
    let repo = Repo::new("exit 0");
    repo.install();
    let wt = repo.add_worktree("feature");
    first_logos_use(&wt);

    // A commit on `main` (made from the primary checkout) for the worktree's
    // branch to merge in.
    repo.commit(repo.main.path(), "merged_in.rs", "pub fn merged_in() {}\n", "on main");

    let before = repo.log(&wt).len();
    repo.git(&wt, &["merge", "--quiet", "--no-edit", "main"]);

    let calls = repo.log(&wt);
    let merge = &calls[before..];
    assert!(
        merge.iter().any(|c| c.contains("merged_in.rs")),
        "post-merge must fire in a worktree: {calls:?}"
    );
}

/// [FR-WT-01]: the hook acts on **that worktree's** graph. Asserted by *where*
/// the hook's own root resolution put its side effect — the worktree's
/// `.logos/`, and not the primary checkout's.
///
/// [FR-WT-01]: ../../docs/specs/requirements/FR-WT-01.md
#[test]
fn a_worktree_hook_acts_on_that_worktrees_graph_not_the_main_checkouts() {
    let repo = Repo::new("exit 0");
    repo.install();
    let wt = repo.add_worktree("feature");
    first_logos_use(&wt);

    repo.commit(&wt, "worktree_only.rs", "pub fn worktree_only() {}\n", "in the worktree");

    assert!(
        repo.log(&wt).iter().any(|c| c.contains("worktree_only.rs")),
        "the worktree's own graph must be the one that moved: {:?}",
        repo.log(&wt)
    );
    assert!(
        !repo
            .log(repo.main.path())
            .iter()
            .any(|c| c.contains("worktree_only.rs")),
        "the main checkout's graph must NOT have moved: {:?}",
        repo.log(repo.main.path())
    );
}

// ── The mechanism: relative hooksPath, symlinks, no staleness ───────────────

/// [FR-IN-06] AC1 passes unmodified: reachability came from seeding, so
/// `core.hooksPath` is still the relative `.logos/hooks` — read from the
/// worktree as well as the primary checkout.
///
/// [FR-IN-06]: ../../docs/specs/requirements/FR-IN-06.md
#[test]
fn core_hooks_path_stays_the_relative_path_in_every_working_tree() {
    let repo = Repo::new("exit 0");
    repo.install();
    let wt = repo.add_worktree("feature");
    first_logos_use(&wt);

    assert_eq!(repo.hooks_path(repo.main.path()), ".logos/hooks");
    assert_eq!(repo.hooks_path(&wt), ".logos/hooks");
}

/// Seeded hooks are symlinks to the primary's scripts, and the point of that
/// choice holds end to end: rewriting a primary script changes what the
/// worktree runs, with no re-seed. Copies could not do this, which is why they
/// are the fallback and not the mechanism.
#[test]
fn seeded_worktree_hooks_are_symlinks_that_cannot_go_stale() {
    let repo = Repo::new("exit 0");
    repo.install();
    let wt = repo.add_worktree("feature");
    first_logos_use(&wt);

    let seeded = wt.join(".logos/hooks/post-commit");
    let meta = fs::symlink_metadata(&seeded).expect("seeded hook exists");
    assert!(meta.file_type().is_symlink(), "the seed must be a symlink");
    assert_eq!(
        fs::read_link(&seeded).unwrap().canonicalize().unwrap(),
        repo.main
            .path()
            .join(".logos/hooks/post-commit")
            .canonicalize()
            .unwrap(),
    );

    // Re-write the primary's script; the worktree must follow it immediately.
    fs::write(
        repo.main.path().join(".logos/hooks/post-commit"),
        "#!/bin/sh\n# logos-managed-hook: post-commit\nlogos rewritten-body\nexit 0\n",
    )
    .expect("rewrite the primary script");

    repo.commit(&wt, "f.rs", "pub fn f() {}\n", "after the rewrite");

    assert!(
        repo.log(&wt).iter().any(|c| c == "rewritten-body"),
        "the worktree must run the CURRENT script, never a stale copy: {:?}",
        repo.log(&wt)
    );
}

// ── The blocking pre-push gate (FR-IN-06, Must) ─────────────────────────────

/// A push from a linked worktree carrying a `severity='error'` violation is
/// blocked, exactly as from the main checkout. A gate whose enforcement
/// depends on which working tree the push is made from is not a gate.
#[test]
fn a_push_from_a_linked_worktree_carrying_a_violation_is_blocked() {
    let repo = Repo::new("echo 'logos check: FR-DEAD dead code src/f.rs [severity=error]' >&2; exit 1");
    repo.install();
    let wt = repo.add_worktree("feature");
    first_logos_use(&wt);

    repo.commit(&wt, "f.rs", "pub fn f() {}\n", "to push");

    let out = repo.try_git(
        &wt,
        &["push", "origin", "feature"],
        &repo.path_with_stub(),
    );

    assert!(
        !out.status.success(),
        "the gate must block a push made from a linked worktree"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("dead code src/f.rs"),
        "the blocking violation must be named: {stderr}"
    );
}

/// A clean push from a linked worktree still succeeds — the gate fires there,
/// it does not simply refuse there.
#[test]
fn a_clean_push_from_a_linked_worktree_succeeds() {
    let repo = Repo::new("exit 0");
    repo.install();
    let wt = repo.add_worktree("feature");
    first_logos_use(&wt);

    repo.commit(&wt, "f.rs", "pub fn f() {}\n", "to push");

    let out = repo.try_git(&wt, &["push", "origin", "feature"], &repo.path_with_stub());

    assert!(
        out.status.success(),
        "a clean check must let a worktree push through: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        repo.log(&wt).iter().any(|c| c == "check"),
        "the gate must actually have run: {:?}",
        repo.log(&wt)
    );
}

/// The exit-code contract is unchanged in a worktree too: an absent `logos`
/// binary bails the gate **open** rather than falsely blocking.
#[test]
fn a_push_from_a_linked_worktree_bails_open_when_logos_is_absent() {
    let repo = Repo::new("exit 1");
    repo.install();
    let wt = repo.add_worktree("feature");
    first_logos_use(&wt);

    repo.commit(&wt, "f.rs", "pub fn f() {}\n", "to push");

    // git lives in /usr/bin; the stub `logos` is deliberately off this PATH.
    let out = repo.try_git(&wt, &["push", "origin", "feature"], "/usr/bin:/bin");

    assert!(
        out.status.success(),
        "an absent logos must never falsely block a worktree push: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── Coverage of both worktree lifecycles, and the accepted gap ──────────────

/// A worktree that existed **before** the installation is covered *by the
/// installation itself* — reachable the moment `install` returns, not on some
/// later visit.
#[test]
fn a_worktree_that_predates_the_installation_is_seeded_by_the_install() {
    let repo = Repo::new("exit 0");
    let wt = repo.add_worktree("feature");

    let result = repo.install();

    assert_eq!(
        result.worktrees.len(),
        1,
        "install must report the worktree it seeded: {result:?}"
    );
    // No `first_logos_use` here: the install is the whole coverage story.
    repo.commit(&wt, "predates.rs", "pub fn predates() {}\n", "predating worktree");

    assert!(
        repo.log(&wt).iter().any(|c| c.contains("predates.rs")),
        "{:?}",
        repo.log(&wt)
    );
}

/// A worktree created **after** the installation is covered on its first logos
/// use — and the window before that is the accepted, documented timing gap
/// (CRA-04), surfaced as a `doctor` finding rather than hidden.
#[test]
fn a_worktree_created_after_the_installation_is_unhooked_until_first_logos_use() {
    let repo = Repo::new("exit 0");
    repo.install();
    let wt = repo.add_worktree("feature");

    // The gap: a commit between `git worktree add` and the first logos command.
    repo.commit(&wt, "in_the_gap.rs", "pub fn in_the_gap() {}\n", "inside the timing gap");
    assert!(
        repo.log(&wt).is_empty(),
        "the documented gap: no hook fires before the first logos use: {:?}",
        repo.log(&wt)
    );

    // …and it is a finding, not a silence.
    let findings = logos_core::hooks::reachability_findings(&wt);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].contains("post-commit"), "{findings:?}");
    assert!(findings[0].contains("pre-push"), "{findings:?}");

    first_logos_use(&wt);
    assert!(
        logos_core::hooks::reachability_findings(&wt).is_empty(),
        "the seed must clear the finding"
    );

    repo.commit(&wt, "after_the_gap.rs", "pub fn after_the_gap() {}\n", "after the first logos use");
    assert!(
        repo.log(&wt).iter().any(|c| c.contains("after_the_gap.rs")),
        "{:?}",
        repo.log(&wt)
    );
}

/// The bootstrap seam is wired: starting an `Engine` in a linked worktree —
/// what *any* logos command does there — is what makes the hooks fire.
#[test]
fn starting_an_engine_in_a_linked_worktree_makes_the_hooks_fire() {
    let repo = Repo::new("exit 0");
    repo.install();
    let wt = repo.add_worktree("feature");

    drop(logos_core::Engine::start(&wt).expect("engine starts in the worktree"));

    repo.commit(&wt, "after_engine.rs", "pub fn after_engine() {}\n", "after engine start");

    assert!(
        repo.log(&wt).iter().any(|c| c.contains("after_engine.rs")),
        "an engine start must have seeded the hooks: {:?}",
        repo.log(&wt)
    );
}

/// **The same seam, through the phase-timed twin** — the sprint-level check
/// that the two hand-mirrored cold paths did not drift apart.
///
/// `Engine::start_with_phase_report` (S-368, [CR-116]) is a deliberate
/// near-duplicate of `Engine::start`'s call sequence, written so the timing
/// instrumentation never lands on the production path. S-338 then added the
/// hook-reachability step to `start_with_configs` in the *same* iteration, and
/// the twin was not extended — so the attribution silently omitted a real cold
/// path step, and no assertion noticed: the existing equivalence check compares
/// the loaded registry, which this step does not touch, and the phase-sum
/// reconciliation runs over a bare `TempDir`, where the step is a single `stat`
/// that costs nothing.
///
/// So the guard has to be an **observable side effect in a linked worktree**,
/// exactly as the rest of this file argues. A commit after a phase-reported
/// start must fire the hook, which it can only do if the twin seeded.
///
/// [CR-116]: ../../docs/requests/CR-116-cold-start-budget-and-its-guard-disagree.md
#[test]
fn a_phase_reported_engine_start_seeds_the_worktree_hooks_too() {
    let repo = Repo::new("exit 0");
    repo.install();
    let wt = repo.add_worktree("feature");

    let (engine, _phases) =
        logos_core::Engine::start_with_phase_report(&wt).expect("instrumented engine starts");
    drop(engine);

    repo.commit(
        &wt,
        "after_phase_report.rs",
        "pub fn after_phase_report() {}\n",
        "after a phase-reported engine start",
    );

    assert!(
        repo.log(&wt).iter().any(|c| c.contains("after_phase_report.rs")),
        "the phase-timed twin must seed the hooks exactly as Engine::start does: {:?}",
        repo.log(&wt)
    );
}

// ── Uninstall, and the non-clobber posture ──────────────────────────────────

/// `uninstall` leaves no seeded worktree hooks behind — no dangling symlinks,
/// no directory, and nothing that still fires.
#[test]
fn uninstall_removes_the_seeded_worktree_hooks_and_they_stop_firing() {
    let repo = Repo::new("exit 0");
    repo.install();
    let wt = repo.add_worktree("feature");
    first_logos_use(&wt);
    assert!(wt.join(".logos/hooks/post-commit").is_file());

    let result = logos_core::hooks::uninstall(repo.main.path()).expect("uninstall");
    assert_eq!(result.worktrees.len(), 1, "{result:?}");

    assert!(
        !wt.join(".logos/hooks").exists(),
        "the seeded hooks directory must be gone, dangling links included"
    );
    assert_eq!(repo.hooks_path(&wt), "", "core.hooksPath must be unset");

    repo.commit(&wt, "after_uninstall.rs", "pub fn after() {}\n", "after uninstall");
    assert!(repo.log(&wt).is_empty(), "{:?}", repo.log(&wt));
}

/// An upgrade from the previous version — which wrote plain script files and
/// never seeded a worktree — uninstalls cleanly, leaving no un-removable
/// configuration behind.
#[test]
fn uninstall_removes_a_previous_version_installation() {
    let repo = Repo::new("exit 0");
    let wt = repo.add_worktree("feature");

    // Exactly what the previous version left: managed scripts in the primary
    // checkout, the relative `core.hooksPath`, and an untouched worktree.
    repo.install();
    fs::remove_dir_all(wt.join(".logos/hooks")).ok();
    assert!(!wt.join(".logos/hooks").exists());

    logos_core::hooks::uninstall(repo.main.path()).expect("uninstall");

    for name in ["post-commit", "post-checkout", "post-merge", "pre-push"] {
        assert!(!repo.main.path().join(".logos/hooks").join(name).exists());
    }
    assert_eq!(repo.hooks_path(repo.main.path()), "");
}

/// The non-clobber posture reaches into the worktree: a same-named hook file
/// the seed did not write is left untouched, and named as a warning.
#[test]
fn seeding_leaves_a_foreign_hook_script_in_a_worktree_untouched() {
    let repo = Repo::new("exit 0");
    repo.install();
    let wt = repo.add_worktree("feature");

    let foreign = wt.join(".logos/hooks/post-commit");
    fs::create_dir_all(foreign.parent().unwrap()).unwrap();
    fs::write(&foreign, "#!/bin/sh\n# someone else's hook\n").unwrap();

    let seeded = logos_core::hooks::seed_from_primary(&wt).expect("a seed ran");

    assert_eq!(fs::read_to_string(&foreign).unwrap(), "#!/bin/sh\n# someone else's hook\n");
    assert!(
        seeded.warnings.iter().any(|w| w.contains("not logos-managed")),
        "{seeded:?}"
    );
    assert!(seeded.linked.contains(&"pre-push".to_string()), "{seeded:?}");
}

// ── Detection: `doctor` asks the question the three success signals cannot ──

/// `doctor` reports an installed-but-unreachable hook configuration, and does
/// not move its verdict doing so.
///
/// The state under test is the one an uninstall of the source scripts leaves:
/// `core.hooksPath` set, the scripts "present" in this working tree as
/// symlinks — and resolving to nothing. Presence is exactly what read as
/// success while nothing ran, so presence is exactly what this must not
/// accept.
#[test]
fn doctor_reports_unreachable_hooks_without_moving_its_verdict() {
    let repo = Repo::new("exit 0");
    repo.install();
    let wt = repo.add_worktree("feature");
    first_logos_use(&wt);
    assert!(logos_core::hooks::reachability_findings(&wt).is_empty());

    fs::remove_dir_all(repo.main.path().join(".logos/hooks")).expect("remove the source scripts");

    let engine = logos_core::Engine::start(&wt).expect("engine starts");
    let report = engine.doctor().expect("doctor runs");

    assert_eq!(report.hook_warnings.len(), 1, "{:?}", report.hook_warnings);
    assert!(
        report.hook_warnings[0].contains("do NOT fire in this working tree"),
        "{:?}",
        report.hook_warnings
    );
    assert!(
        report.ok && report.faults.is_empty(),
        "the finding is diagnostic — it must not move doctor's verdict: {report:?}"
    );
}

/// …and reports nothing when hooks are absent by choice: opt-in stays opt-in
/// and an unhooked project is not degraded by a finding it cannot act on.
#[test]
fn doctor_is_silent_about_hooks_in_a_worktree_of_an_unhooked_project() {
    let repo = Repo::new("exit 0");
    let wt = repo.add_worktree("feature"); // no `install` anywhere

    let engine = logos_core::Engine::start(&wt).expect("engine starts");
    let report = engine.doctor().expect("doctor runs");

    assert!(report.hook_warnings.is_empty(), "{:?}", report.hook_warnings);
    assert!(!wt.join(".logos/hooks").exists());
}

/// A hook script logos does **not** manage, sitting in the configured hooks
/// directory, runs in the primary checkout and not in a worktree — seeding
/// carries only what logos installed. That is a deliberate limit, so it is
/// *reported*: hooks appearing to work in a worktree while the user's own
/// script quietly does not is the same "reads as success" trap one storey up.
#[test]
fn doctor_names_a_third_party_hook_that_the_seed_does_not_carry() {
    let repo = Repo::new("exit 0");
    repo.install();
    let theirs = repo.main.path().join(".logos/hooks/pre-commit");
    fs::write(&theirs, "#!/bin/sh\n# somebody else's gate\nexit 0\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&theirs, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let wt = repo.add_worktree("feature");
    first_logos_use(&wt);

    let findings = logos_core::hooks::reachability_findings(&wt);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].contains("pre-commit"), "{findings:?}");
    assert!(findings[0].contains("does not manage"), "{findings:?}");
    // The managed four are reachable all the same — this is a report, not a
    // refusal to seed.
    assert!(wt.join(".logos/hooks/pre-push").is_file());
    assert!(!wt.join(".logos/hooks/pre-commit").exists());
    assert!(
        logos_core::hooks::reachability_findings(repo.main.path()).is_empty(),
        "the script is not unreachable in the checkout that holds it"
    );
}

/// Installing from inside a linked worktree installs *there* and says so,
/// rather than anchoring the whole repository's hooks to a checkout that
/// `git worktree remove` can delete out from under them.
#[test]
fn installing_from_a_linked_worktree_seeds_nothing_and_names_the_limit() {
    let repo = Repo::new("exit 0");
    let wt = repo.add_worktree("feature");

    let result = logos_core::hooks::install(&wt).expect("install succeeds");

    assert!(result.worktrees.is_empty(), "{result:?}");
    assert!(
        result.warnings.iter().any(|w| w.contains("primary checkout")),
        "{result:?}"
    );
    assert!(
        !repo.main.path().join(".logos/hooks").exists(),
        "the primary must not be wired to a worktree's scripts"
    );

    // The tree it ran in does get working hooks.
    repo.commit(&wt, "here.rs", "pub fn here() {}\n", "installed from here");
    assert!(repo.log(&wt).iter().any(|c| c.contains("here.rs")), "{:?}", repo.log(&wt));
}
