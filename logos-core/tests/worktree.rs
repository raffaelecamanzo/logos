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

// ── FR-WT-06 / CR-112: worktree seeding carries the governance contract ───

/// A committed repo with `.logos/` **wholesale** gitignored — the common,
/// naive convention CR-112 exists for (a blanket `.logos/` line, not the
/// granular `.logos/.gitignore` `logos init` generates). Unlike
/// [`repo_fixture`], no policy file here is ever git-tracked, so a linked
/// worktree's only path to one is the seed copying it.
fn gitignored_repo_fixture() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("temp root");
    let main = tmp.path().join("main");
    write(&main, "src/lib.rs", "pub fn seeded_fn() {}\n");
    write(&main, ".gitignore", ".logos/\n");
    sh_git(&main, &["init", "-q", "-b", "main"]);
    sh_git(&main, &["add", "."]);
    sh_git(&main, &["commit", "-q", "-m", "initial"]);
    (tmp, main)
}

/// A minimal but real layered contract — two layers plus a boundary between
/// them — the same shape `logos-core/tests/governance.rs` uses.
const LAYERED_RULES: &str = "\
[[layers]]
name  = \"domain\"
paths = [\"src/domain_*.rs\"]
order = 1

[[layers]]
name  = \"presentation\"
paths = [\"src/ui_*.rs\"]
order = 2

[[boundaries]]
from   = \"domain\"
to     = \"presentation\"
reason = \"the domain must not reach upward into presentation\"
";

/// [FR-WT-06] AC1: seeding a DB-less worktree from a primary checkout that
/// has `.logos/rules.toml` yields a worktree where `check_rules` reports the
/// SAME `checked_rules` count as the primary at the same commit. Neither
/// policy file traveled through git (both are gitignored), so parity here
/// can only come from the seed's own copy — and both copies are
/// byte-identical, never rewritten ([FR-IN-06]).
#[test]
fn worktree_seed_carries_the_governance_contract_with_checked_rules_parity() {
    let (tmp, main) = gitignored_repo_fixture();
    write(&main, ".logos/rules.toml", LAYERED_RULES);
    let config_body = "max_file_size = 4096\n";
    write(&main, ".logos/config.toml", config_body);

    let primary_checked_rules = {
        let engine = Engine::start(&main).expect("primary engine starts");
        let report = engine
            .check_rules(None, true)
            .expect("primary check_rules runs");
        assert!(report.rules_present, "the primary has an authored contract");
        report.checked_rules
    };
    assert!(main.join(".logos/logos.db").exists());

    let wt = add_worktree(&tmp, &main);
    assert!(
        !wt.join(".logos/rules.toml").exists(),
        "rules.toml is gitignored — it must not have traveled through git"
    );
    assert!(
        !wt.join(".logos/config.toml").exists(),
        "config.toml is gitignored — it must not have traveled through git"
    );

    // First use in the DB-less worktree: the seed copies the graph AND the
    // governance contract (FR-WT-06) — both policy files, not only rules.toml.
    let engine = Engine::start(&wt).expect("worktree engine starts");
    assert_eq!(
        fs::read_to_string(wt.join(".logos/rules.toml")).unwrap(),
        fs::read_to_string(main.join(".logos/rules.toml")).unwrap(),
        "the seeded rules.toml is byte-identical to the primary's — never rewritten (FR-IN-06)"
    );
    assert_eq!(
        fs::read_to_string(wt.join(".logos/config.toml")).unwrap(),
        config_body,
        "the seeded config.toml is byte-identical to the primary's — never rewritten (FR-IN-06)"
    );

    let wt_report = engine
        .check_rules(None, true)
        .expect("worktree check_rules runs");
    assert!(wt_report.rules_present, "the seeded worktree has a contract");
    assert_eq!(
        wt_report.checked_rules, primary_checked_rules,
        "the worktree evaluates the SAME rule count as the primary at the same commit"
    );
}

/// [FR-WT-06]: contract seeding is independent of whether the primary
/// checkout has ever run `Engine::start` itself. A primary that authored
/// `rules.toml` but has no `.logos/logos.db` yet (so `seed_source` — which
/// requires a primary DB — resolves to `None`) still seeds its contract into
/// a fresh worktree, because the bootstrap resolves the primary root for
/// contract-seeding independently of the graph-store seed.
#[test]
fn worktree_seed_carries_the_contract_even_when_the_primary_has_no_db() {
    let (tmp, main) = gitignored_repo_fixture();
    write(&main, ".logos/rules.toml", LAYERED_RULES);
    assert!(
        !main.join(".logos/logos.db").exists(),
        "the primary never ran Engine::start"
    );

    let wt = add_worktree(&tmp, &main);
    let engine = Engine::start(&wt).expect("worktree engine starts without a primary DB");
    assert_eq!(
        fs::read_to_string(wt.join(".logos/rules.toml")).unwrap(),
        LAYERED_RULES,
        "the contract is seeded even though the primary itself was never indexed"
    );
    let report = engine
        .check_rules(None, true)
        .expect("worktree check_rules runs");
    assert!(report.rules_present, "the seeded worktree has a contract");
}

/// `Engine::start_with_phase_report` — the hand-mirrored, phase-timed twin of
/// `Engine::start`'s cold path — must seed a DB-less worktree the same way
/// `Engine::start` does: BOTH the graph store and the governance contract
/// (CR-116 §9 item 5, S-369).
///
/// The twin exists to attribute the production cold path's cost, so it is only
/// honest while it does the same work. S-369 made both paths resolve the
/// primary checkout with **one** `git rev-parse --git-common-dir` instead of
/// two, feeding that one resolution to the graph seed and the contract seed
/// alike — a change that had to land in both twins or the attribution would
/// over-report `other` by exactly the saving production made.
///
/// **What this test can and cannot catch.** It fails if the twin stops
/// performing either seed: the graph seed is proved by `status()` (which
/// deliberately skips the auto-index prologue) reporting a populated graph with
/// no full index, and the contract seed by the byte-identical `rules.toml`.
/// It does **not** guard the de-duplication itself — a twin restored to two
/// `--git-common-dir` subprocesses still performs both seeds, and subprocess
/// count is not observable from this seam. So this covers the *omitted-step*
/// failure mode, the same one
/// `worktree_hooks.rs::a_phase_reported_engine_start_seeds_the_worktree_hooks_too`
/// covers, and not the de-duplication regressing.
#[test]
fn a_phase_reported_engine_start_seeds_the_store_and_the_contract_too() {
    let (tmp, main) = gitignored_repo_fixture();
    write(&main, ".logos/rules.toml", LAYERED_RULES);

    // Give the primary a real DB so the graph seed has something to copy —
    // this exercises the arm where both seeds fire off one resolution.
    {
        let primary = Engine::start(&main).expect("primary engine starts");
        assert!(primary.index().files_indexed >= 1, "primary index ran");
    }
    assert!(
        main.join(".logos/logos.db").is_file(),
        "the primary now has a DB to seed from"
    );

    let wt = add_worktree(&tmp, &main);
    let (engine, phases) =
        Engine::start_with_phase_report(&wt).expect("the phase-reported twin starts");

    // The graph seed fired. `status` reads WITHOUT the auto-index prologue, so
    // a populated graph here proves the seed happened at start, and an empty
    // `last_full_index_at` proves it was seed + diff-reconcile rather than a
    // full index — the same discipline
    // `a_worktree_seeds_from_main_and_reflects_its_own_code` uses, and the
    // reason `has_fn` alone will not do: `has_fn` navigates, navigation runs
    // the FR-IX-07 prologue, and a full index would satisfy it with no seed at
    // all (which is exactly what `a_worktree_without_a_primary_db_full_indexes`
    // asserts). Read status BEFORE any navigation call.
    let status = engine.status();
    assert!(
        status.indexed,
        "the twin seeded the worktree store from the primary checkout, before \
         any navigation call could auto-index: {status:?}"
    );
    assert!(
        status.last_full_index_at.is_none(),
        "no full index ran — the twin's bootstrap was seed + diff-reconcile"
    );
    assert!(
        has_fn(&engine, "seeded_fn"),
        "the seeded graph carries main's symbols without a re-index"
    );
    // The contract seed fired off the same primary resolution.
    assert_eq!(
        fs::read_to_string(wt.join(".logos/rules.toml")).unwrap(),
        LAYERED_RULES,
        "the twin seeded the governance contract too, byte-identically"
    );
    let report = engine
        .check_rules(None, true)
        .expect("worktree check_rules runs");
    assert!(report.rules_present, "the seeded worktree has a contract");

    // And the attribution it exists to produce timed real work in the phases
    // this seam is the only place to check them. `enumerated_phases() + other
    // == sum()` is NOT asserted here: it holds for every possible field
    // assignment by construction, so it can never fail — the arithmetic is
    // pinned against literals in `engine::tests::cold_start_phase_totals_are_exact`
    // instead. What only this test can check is that the seed work actually
    // landed somewhere: unlike the bare-`TempDir` cold starts the attribution
    // harness measures, this root really seeds, so `other` (which absorbs the
    // seed, the contract copy and the reconcile) must be non-zero.
    assert!(
        phases.query_compilation > std::time::Duration::ZERO,
        "a real registry load was timed, not a skipped one"
    );
    assert!(
        phases.store_open > std::time::Duration::ZERO,
        "a real store open was timed"
    );
    assert!(
        phases.other > std::time::Duration::ZERO,
        "the seed and contract-copy work landed in `other`, as NFR-PE-05's \
         incidental-work clause describes: {phases:?}"
    );
}

/// [FR-WT-06] AC2: a primary checkout with no contract seeds a worktree with
/// no contract — the seed copies, it never fabricates.
#[test]
fn worktree_seed_with_no_primary_contract_fabricates_nothing() {
    let (tmp, main) = gitignored_repo_fixture(); // no rules.toml ever written

    {
        let engine = Engine::start(&main).expect("primary engine starts");
        let report = engine
            .check_rules(None, true)
            .expect("primary check_rules runs");
        assert!(!report.rules_present, "no contract was ever authored");
    }

    let wt = add_worktree(&tmp, &main);
    let engine = Engine::start(&wt).expect("worktree engine starts");
    assert!(
        !wt.join(".logos/rules.toml").exists(),
        "nothing to copy, and the seed fabricates nothing"
    );
    let report = engine
        .check_rules(None, true)
        .expect("worktree check_rules runs");
    assert!(
        !report.rules_present,
        "the seeded worktree reports the same absent-contract state as the primary"
    );
    assert_eq!(report.checked_rules, 0);
}
