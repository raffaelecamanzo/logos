//! Black-box integration tests for worktree-aware operation (S-021,
//! [ADR-15](../../docs/specs/architecture/decisions/ADR-15.md)), driven on
//! REAL `git worktree` I/O through the public `Engine` façade exactly as the
//! CLI/MCP surfaces use it — validating
//! [AA-05](../../docs/specs/architecture.md#24-assumptions) (one server per
//! worktree root) end to end.
//!
//! Coverage by acceptance criterion:
//! - a linked worktree resolves and uses ITS OWN DB, seeding from the primary
//!   checkout on first use, and the engine's results reflect the worktree's
//!   code, never main's
//!   ([FR-WT-01](../../docs/specs/requirements/FR-WT-01.md),
//!   [FR-WT-03](../../docs/specs/requirements/FR-WT-03.md),
//!   [NFR-CC-02](../../docs/specs/requirements/NFR-CC-02.md),
//!   [UAT-WT-01](../../docs/specs/requirements/UAT-WT-01.md),
//!   [UAT-WT-02](../../docs/specs/requirements/UAT-WT-02.md));
//! - checked-in policy travels into the worktree and is honoured there; the
//!   derived DB does not travel
//!   ([FR-WT-02](../../docs/specs/requirements/FR-WT-02.md),
//!   [NFR-DM-04](../../docs/specs/requirements/NFR-DM-04.md));
//! - a missing primary DB falls back to a full index
//!   ([ADR-15](../../docs/specs/architecture/decisions/ADR-15.md) fallback);
//! - `.logos/` lands at the working-tree root even when the engine is opened
//!   on a subdirectory
//!   ([FR-WT-01](../../docs/specs/requirements/FR-WT-01.md)).
//!
//! Gated on `lang-rust`: integration tests share the crate's feature set and a
//! `--no-default-features` build excludes the Rust grammar these tests need.
#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use logos_core::Engine;

/// Run a git command in `cwd`, panicking on failure — fixtures only.
fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=test@logos", "-c", "user.name=logos-test"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Write `contents` to `<root>/<rel>`, creating parent directories.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write fixture file");
}

/// A committed repo at `<tmp>/main` mirroring the canonical layout: tracked
/// source, tracked `.logos/` policy, and the gitignored-DB posture of
/// FR-IN-04 (`.logos/*.db*` never travels through git).
fn repo_fixture() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("temp root");
    let main = tmp.path().join("main");
    write(&main, "src/lib.rs", "pub fn seeded_fn() {}\n");
    write(&main, ".gitignore", ".logos/*.db\n.logos/*.db-*\n");
    write(&main, ".logos/config.toml", "");
    sh_git(&main, &["init", "-q", "-b", "main"]);
    sh_git(&main, &["add", "."]);
    sh_git(&main, &["commit", "-q", "-m", "initial"]);
    (tmp, main)
}

/// Add a linked worktree at `<tmp>/wt` on a new branch and return its root.
fn add_worktree(tmp: &TempDir, main: &Path) -> PathBuf {
    let wt = tmp.path().join("wt");
    sh_git(
        main,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "feature",
        ],
    );
    wt
}

/// Does a function named `name` exist in this engine's graph?
fn has_fn(engine: &Engine, name: &str) -> bool {
    engine
        .search(name, Some(logos_core::model::NodeKind::Function), Some(16))
        .hits
        .iter()
        .any(|h| h.name == name)
}

/// UAT-WT-01 / FR-WT-01..03 / NFR-CC-02 — the worktree acceptance test:
/// index main, add a worktree, edit a file there; the engine started at the
/// worktree seeds its OWN `.logos/logos.db` from the primary and its results
/// reflect the worktree's code, while main's graph stays untouched.
#[test]
fn a_worktree_seeds_from_main_and_reflects_its_own_code() {
    let (tmp, main) = repo_fixture();

    // Index the primary checkout, then release it (drop = writer torn down).
    {
        let engine = Engine::start(&main).expect("primary engine starts");
        let indexed = engine.index();
        assert!(indexed.files_indexed >= 1, "primary index ran");
    }
    assert!(main.join(".logos/logos.db").exists());

    let wt = add_worktree(&tmp, &main);
    // NFR-DM-04 / FR-WT-02: policy travels through git, the derived DB does not.
    assert!(
        wt.join(".logos/config.toml").exists(),
        "checked-in policy travels into the worktree"
    );
    assert!(
        !wt.join(".logos/logos.db").exists(),
        "the gitignored derived DB does NOT travel"
    );

    // Diverge the worktree: a tracked edit and an untracked new file.
    write(
        &wt,
        "src/lib.rs",
        "pub fn seeded_fn() {}\npub fn worktree_only_fn() {}\n",
    );
    write(&wt, "src/fresh.rs", "pub fn fresh_fn() {}\n");

    // First use in the DB-less worktree: seed + diff-reconcile (FR-WT-03).
    let engine = Engine::start(&wt).expect("worktree engine starts");
    assert!(
        wt.join(".logos/logos.db").exists(),
        "the worktree owns its own DB after first use (FR-WT-01)"
    );
    // `status` reads WITHOUT the auto-index prologue, so a populated graph
    // here proves the seed + diff-reconcile happened at start — and
    // `last_full_index_at` being empty proves it was O(diff-from-main), not a
    // full O(repo) index (FR-WT-03).
    let status = engine.status();
    assert!(
        status.indexed,
        "the graph is populated straight from the seed, before any \
         navigation call could auto-index: {status:?}"
    );
    assert!(
        status.last_full_index_at.is_none(),
        "no full index ran — the bootstrap was seed + diff-reconcile"
    );
    assert!(
        has_fn(&engine, "seeded_fn"),
        "the seeded graph carries main's symbols without a re-index"
    );
    assert!(
        has_fn(&engine, "worktree_only_fn"),
        "the diff-reconcile picked up the worktree's tracked edit (UAT-WT-02)"
    );
    assert!(
        has_fn(&engine, "fresh_fn"),
        "the diff-reconcile picked up the untracked new file"
    );
    drop(engine);

    // One server per worktree root (AA-05): main's graph never saw the
    // worktree's symbols (NFR-CC-02 — and conversely a server at the worktree
    // never serves main's graph).
    let main_engine = Engine::start(&main).expect("primary engine restarts");
    assert!(
        !has_fn(&main_engine, "worktree_only_fn"),
        "the primary checkout's DB is isolated from the worktree's"
    );
}

/// The ADR-15 fallback: no primary DB to seed from → the worktree starts on a
/// fresh store and the first evaluation performs a FULL index instead of
/// failing.
#[test]
fn a_worktree_without_a_primary_db_full_indexes() {
    let (tmp, main) = repo_fixture();
    let wt = add_worktree(&tmp, &main); // primary never indexed

    let engine = Engine::start(&wt).expect("worktree engine starts without a seed");
    assert!(
        !engine.status().indexed,
        "no seed → the store starts empty (nothing copied from nowhere)"
    );
    let result = engine.ensure_indexed();
    assert!(
        result.files_indexed >= 1,
        "no seed → the auto-index prologue performs a full index, got {result:?}"
    );
    assert!(
        has_fn(&engine, "seeded_fn"),
        "the full index built the graph"
    );
}

/// FR-WT-02 / NFR-DM-04: the worktree's own checked-in `config.toml` is the
/// one the engine honours — an exclude added on the branch keeps the excluded
/// tree out of the worktree's index.
#[test]
fn worktree_policy_is_honoured_by_the_worktree_engine() {
    let (tmp, main) = repo_fixture();
    write(&main, "vendor/skip.rs", "pub fn vendored_fn() {}\n");
    sh_git(&main, &["add", "."]);
    sh_git(&main, &["commit", "-q", "-m", "vendored file"]);

    let wt = add_worktree(&tmp, &main);
    // Branch-local policy: exclude vendor/ in the WORKTREE only.
    write(&wt, ".logos/config.toml", "exclude = [\"vendor/**\"]\n");

    let engine = Engine::start(&wt).expect("worktree engine starts");
    let result = engine.index();
    assert!(result.files_indexed >= 1, "index ran: {result:?}");
    assert!(has_fn(&engine, "seeded_fn"));
    assert!(
        !has_fn(&engine, "vendored_fn"),
        "the worktree's own config.toml governs its index (FR-WT-02)"
    );
}

/// FR-WT-01: starting the engine on a SUBDIRECTORY of a repo roots `.logos/`
/// at the working-tree toplevel, not at the subdirectory.
#[test]
fn engine_roots_at_the_working_tree_toplevel_from_a_subdirectory() {
    let (_tmp, main) = repo_fixture();
    let engine = Engine::start(main.join("src")).expect("engine starts on a subdir");
    drop(engine);
    assert!(
        main.join(".logos/logos.db").exists(),
        ".logos/ resolves to the working-tree root"
    );
    assert!(
        !main.join("src/.logos").exists(),
        "no stray .logos/ at the subdirectory"
    );
}

/// Outside git entirely, the hint IS the root — the cwd/--project fallback —
/// and nothing about S-021 disturbs the plain-directory workflow.
#[test]
fn outside_git_the_hint_directory_is_the_root() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", "pub fn plain_fn() {}\n");
    let engine = Engine::start(tmp.path()).expect("engine starts outside git");
    let result = engine.index();
    assert!(result.files_indexed >= 1);
    assert!(has_fn(&engine, "plain_fn"));
    assert!(tmp.path().join(".logos/logos.db").exists());
}
