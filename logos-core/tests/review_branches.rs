//! `review --base <ref> --branches …` (S-757, [FR-GV-28], [UAT-GV-18]),
//! end-to-end through `Engine::review_branches` over real git fixtures.
//!
//! Coverage by acceptance criterion:
//! - two branches that each add one outgoing module edge to a module at
//!   `max_fan_out − 1` are reported as exceeding it together, each with a
//!   share of 1, and neither alone is;
//! - a limit no combination exceeds is not listed, and a branch that cannot be
//!   measured — no such ref, no common history, a repeat of a summed branch's
//!   commit — is named with why and left out of the sum;
//! - a project-wide count (`max_dead`) is projected as a module's fan-out
//!   is, and a `"report"` drift budget is named as not projected;
//! - the answer is byte-stable, and the second run reads every side back from
//!   the cache.
//!
//! The exit code and the CLI/MCP parity are surface contracts:
//! `cli/tests/review_cli.rs` and `mcp/tests/review_parity.rs`.
//!
//! Every fixture is a throw-away repository with `GIT_CEILING_DIRECTORIES` set
//! and git maintenance off, so nothing escapes into an enclosing repository.
//!
//! [FR-GV-28]: ../../docs/specs/requirements/FR-GV-28.md
//! [UAT-GV-18]: ../../docs/specs/requirements/UAT-GV-18.md

#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::Path;
use std::process::Command;

use logos_core::models::quality::{BranchShare, BranchesProjection, DriftReference};
use logos_core::Engine;
use tempfile::TempDir;

/// Run `git -C <cwd> <args…>` hermetically, asserting success.
fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args([
            "-c", "user.email=dev@logos", "-c", "user.name=Logos Dev",
            "-c", "commit.gpgsign=false", "-c", "core.hooksPath=",
            "-c", "maintenance.auto=false", "-c", "gc.auto=0",
        ])
        .args(args)
        .env("GIT_CEILING_DIRECTORIES", cwd.parent().unwrap())
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", message]);
}

/// `src/engine.rs` calling one function in each of `modules`.
fn engine_calling(modules: &[&str]) -> String {
    let calls: String = modules.iter().map(|m| format!("    crate::{m}::work_{m}();\n")).collect();
    format!("pub fn run() {{\n{calls}}}\n")
}

/// Sprint 106's I2-INT case in miniature: on `main` the module `engine` calls
/// into `a` and `b` — a module fan-out of 2 under `max_fan_out = 3` — and the
/// modules `c` and `d` exist uncalled. `b1` adds a call into `c`, `b2` one
/// into `d`: one new outgoing module edge each. The engine is not indexed:
/// the projection measures committed trees only.
fn repo() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    write(root, ".gitignore", ".logos/\n");
    write(root, "src/lib.rs", "pub mod engine;\npub mod a;\npub mod b;\npub mod c;\npub mod d;\n");
    for m in ["a", "b", "c", "d"] {
        write(root, &format!("src/{m}.rs"), &format!("pub fn work_{m}() {{}}\n"));
    }
    write(root, "src/engine.rs", &engine_calling(&["a", "b"]));
    write(root, ".logos/rules.toml", "[constraints]\nmax_fan_out = 3\n");
    commit(root, "base");
    git(root, &["branch", "-M", "main"]);
    for (branch, extra) in [("b1", "c"), ("b2", "d")] {
        git(root, &["checkout", "-q", "-b", branch, "main"]);
        write(root, "src/engine.rs", &engine_calling(&["a", "b", extra]));
        commit(root, &format!("engine calls {extra}"));
    }
    git(root, &["checkout", "-q", "main"]);
    tmp
}

fn project(root: &Path, branches: &[&str]) -> BranchesProjection {
    let engine = Engine::start(root).expect("engine starts");
    let branches: Vec<String> = branches.iter().map(|b| b.to_string()).collect();
    engine.review_branches(Some("main"), &branches).expect("the projection runs")
}

fn share(branch: &str, share: i64) -> BranchShare {
    BranchShare {
        branch: branch.to_string(),
        share,
        exceeds_alone: false,
    }
}

#[test]
fn two_branches_that_each_fit_a_module_fan_out_exceed_it_together() {
    let tmp = repo();
    let root = tmp.path();

    for alone in ["b1", "b2"] {
        let one = project(root, &[alone]);
        assert!(one.measured, "{one:#?}");
        assert!(one.exceeded.is_empty(), "{alone} alone fits max_fan_out: {one:#?}");
        assert_eq!(one.limits, ["max_fan_out"]);
    }

    let both = project(root, &["b1", "b2"]);
    assert!(both.measured, "{both:#?}");
    assert!(both.branches.iter().all(|b| b.measured), "{both:#?}");
    assert_eq!(both.exceeded.len(), 1, "only the engine module is pushed past its ceiling: {both:#?}");
    let engine = &both.exceeded[0];
    assert_eq!(engine.rule, "max_fan_out");
    assert_eq!(engine.subject, "module:src/`engine.rs`/", "the module key, without a coordinate");
    assert_eq!(engine.file, "src/engine.rs");
    assert_eq!(engine.trust, "resolved");
    assert_eq!((engine.base, engine.limit, engine.projected), (2, 3, 4), "{engine:#?}");
    assert_eq!(engine.shares, [share("b1", 1), share("b2", 1)]);
    assert_eq!(both.subjects, 1, "the one module with an outgoing edge is the denominator: {both:#?}");
    assert!(both.drift.is_none());
}

#[test]
fn an_unmeasurable_branch_is_named_and_left_out_of_the_sum() {
    let tmp = repo();
    let root = tmp.path();
    git(root, &["branch", "b1-again", "b1"]);
    // A branch with no history in common with `main`.
    git(root, &["checkout", "-q", "--orphan", "orphan"]);
    commit(root, "unrelated");
    git(root, &["checkout", "-q", "main"]);

    let answer = project(root, &["b1", "no-such-branch", "orphan", "b1-again", "--b2"]);
    assert!(answer.measured, "{answer:#?}");
    let named: Vec<(&str, bool)> = answer.branches.iter().map(|b| (b.name.as_str(), b.measured)).collect();
    assert_eq!(
        named,
        [("b1", true), ("no-such-branch", false), ("orphan", false), ("b1-again", false), ("--b2", false)]
    );
    let why: Vec<&str> = answer.branches[1..]
        .iter()
        .map(|b| b.not_measured_because.as_deref().unwrap())
        .collect();
    assert!(why[0].contains("does not resolve"), "{why:?}");
    assert!(why[1].contains("no common ancestor"), "{why:?}");
    assert!(why[2].contains("all in b1's share"), "{why:?}");
    assert!(why[3].contains("not a ref name"), "{why:?}");
    assert!(answer.exceeded.is_empty(), "b1 counted once fits the ceiling: {answer:#?}");

    let none = project(root, &["no-such-branch"]);
    assert!(!none.measured, "{none:#?}");
    assert!(none.not_measured_because.unwrap().contains("no branch could be measured"));
}

#[test]
fn the_projection_is_byte_stable_and_reads_its_sides_back_from_the_cache() {
    let tmp = repo();
    let root = tmp.path();
    let first = serde_json::to_string(&project(root, &["b1", "b2"])).unwrap();
    let cache = root.join(".logos/review-cache");
    let entries = |dir: &Path| {
        let mut names: Vec<String> =
            fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        names
    };
    let built = entries(&cache);
    // The base, and each branch tip; both merge-bases are the base.
    assert_eq!(built.len(), 3, "{built:?}");
    assert!(built.iter().all(|n| n.ends_with(".projection.json")), "{built:?}");
    let mtimes: Vec<_> = built.iter().map(|n| fs::metadata(cache.join(n)).unwrap().modified().unwrap()).collect();

    let second = serde_json::to_string(&project(root, &["b1", "b2"])).unwrap();
    assert_eq!(second, first, "a rerun is byte-identical");
    let again: Vec<_> = built.iter().map(|n| fs::metadata(cache.join(n)).unwrap().modified().unwrap()).collect();
    assert_eq!(again, mtimes, "no side was measured again");
}

#[test]
fn without_a_limit_nothing_is_projected_and_undeclared_limits_are_not_named() {
    let tmp = repo();
    let root = tmp.path();
    write(root, ".logos/rules.toml", "[constraints]\nmax_clone_ratio = 0.5\n[review]\n");
    let answer = project(root, &["b1", "b2"]);
    assert!(!answer.measured, "{answer:#?}");
    assert!(answer.not_measured_because.as_deref().unwrap().contains("no limit to project"), "{answer:#?}");
    let rules: Vec<&str> = answer.not_projected.iter().map(|l| l.rule.as_str()).collect();
    assert_eq!(rules, ["max_clone_ratio", "drift_budget"], "a \"report\" drift budget never fails");
    assert!(answer.limits.is_empty());
}

/// A project-wide count: `main` has no dead function under `max_dead = 1`;
/// `d1` adds an uncalled private function to `a`, `d2` one to `b` — one dead
/// function each, two together.
#[test]
fn two_branches_that_each_add_a_dead_function_exceed_max_dead_together() {
    let tmp = repo();
    let root = tmp.path();
    write(root, ".logos/rules.toml", "[constraints]\nmax_dead = 1\n");
    for (branch, m) in [("d1", "a"), ("d2", "b")] {
        git(root, &["checkout", "-q", "-b", branch, "main"]);
        write(root, &format!("src/{m}.rs"), &format!("pub fn work_{m}() {{}}\n\nfn unused_{m}() {{}}\n"));
        commit(root, &format!("a dead function in {m}"));
    }
    git(root, &["checkout", "-q", "main"]);

    let alone = project(root, &["d1"]);
    assert!(alone.measured && alone.exceeded.is_empty(), "{alone:#?}");
    let none = project(root, &["b1"]);
    assert_eq!((none.subjects, none.exceeded.len()), (1, 0), "a count of 0 everywhere is still evaluated");
    let both = project(root, &["d1", "d2"]);
    assert_eq!(both.limits, ["max_dead"]);
    assert_eq!(both.exceeded.len(), 1, "{both:#?}");
    let dead = &both.exceeded[0];
    assert_eq!((dead.rule.as_str(), dead.subject.as_str(), dead.file.as_str()), ("max_dead", "", ""));
    assert_eq!((dead.base, dead.limit, dead.projected), (0, 1, 2), "{dead:#?}");
    assert_eq!(dead.shares, [share("d1", 1), share("d2", 1)]);
}

/// A branch built on a summed branch is measured from that branch's tip, and a
/// branch whose commits a summed one already holds adds nothing: the shared
/// commits count once, in either order. `stacked` is `b1` plus a README.
#[test]
fn a_stacked_branch_counts_the_commits_it_shares_once() {
    let tmp = repo();
    let root = tmp.path();
    git(root, &["checkout", "-q", "-b", "stacked", "b1"]);
    write(root, "README.md", "docs only\n");
    commit(root, "docs");
    git(root, &["checkout", "-q", "-b", "on-both", "b1"]);
    // Ancestry is what is judged, so the merged tree may be b1's own.
    git(root, &["merge", "-q", "--no-edit", "-s", "ours", "b2"]);
    git(root, &["checkout", "-q", "main"]);

    let on_top = project(root, &["b1", "stacked"]);
    assert!(on_top.exceeded.is_empty(), "the merge has fan-out 3: {on_top:#?}");
    assert!(on_top.branches.iter().all(|b| b.measured), "{on_top:#?}");
    assert_eq!(on_top.branches[1].stacked_on.as_deref(), Some("b1"));

    let under = project(root, &["stacked", "b1"]);
    assert!(under.exceeded.is_empty(), "{under:#?}");
    assert!(!under.branches[1].measured, "{under:#?}");
    assert!(under.branches[1].not_measured_because.as_deref().unwrap().contains("all in stacked's share"));

    let both = project(root, &["b1", "b2", "on-both"]);
    assert_eq!(both.exceeded.len(), 1, "b1 and b2 still exceed together: {both:#?}");
    assert!(!both.branches[2].measured, "{both:#?}");
    assert!(both.branches[2].not_measured_because.as_deref().unwrap().contains("b1 and b2"), "{both:#?}");
}

/// The share is the branch's own change, from its merge-base, and the
/// projection starts from the base tip: `b1` forks while `engine` calls only
/// `a`, then `main` adds a call into `b`, and `b2` forks from there. Each adds
/// one more edge, so the merge has fan-out 4 under `max_fan_out = 3`.
#[test]
fn a_branch_forked_before_the_base_moved_brings_only_its_own_change() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    write(root, ".gitignore", ".logos/\n");
    write(root, "src/lib.rs", "pub mod engine;\npub mod a;\npub mod b;\npub mod c;\npub mod d;\n");
    for m in ["a", "b", "c", "d"] {
        write(root, &format!("src/{m}.rs"), &format!("pub fn work_{m}() {{}}\n"));
    }
    write(root, "src/engine.rs", &engine_calling(&["a"]));
    write(root, ".logos/rules.toml", "[constraints]\nmax_fan_out = 3\n");
    commit(root, "base");
    git(root, &["branch", "-M", "main"]);
    git(root, &["checkout", "-q", "-b", "b1"]);
    write(root, "src/engine.rs", &engine_calling(&["a", "c"]));
    commit(root, "engine calls c");
    git(root, &["checkout", "-q", "main"]);
    write(root, "src/engine.rs", &engine_calling(&["a", "b"]));
    commit(root, "main moves on: engine calls b");
    git(root, &["checkout", "-q", "-b", "b2"]);
    write(root, "src/engine.rs", &engine_calling(&["a", "b", "d"]));
    commit(root, "engine calls d");
    git(root, &["checkout", "-q", "main"]);

    let both = project(root, &["b1", "b2"]);
    assert_ne!(both.branches[0].merge_base, both.base_commit, "b1 forked before the base moved");
    assert_eq!(both.branches[1].merge_base, both.base_commit);
    assert_eq!(both.exceeded.len(), 1, "{both:#?}");
    let engine = &both.exceeded[0];
    assert_eq!((engine.base, engine.projected), (2, 4), "{engine:#?}");
    assert_eq!(engine.shares, [share("b1", 1), share("b2", 1)]);
}

/// More commits than the cache keeps per kind: a rerun still reads every side
/// back, and the projection evicts no review entry.
#[test]
fn a_projection_past_the_cache_bound_reruns_from_the_cache() {
    let tmp = repo();
    let root = tmp.path();
    let review_entry = root.join(".logos/review-cache/0000000000000000000000000000000000000000.json");
    write(root, ".logos/review-cache/0000000000000000000000000000000000000000.json", "{}");
    let names: Vec<String> = (0..17).map(|i| format!("t{i:02}")).collect();
    for name in &names {
        git(root, &["checkout", "-q", "-b", name, "main"]);
        write(root, &format!("notes/{name}.md"), "a note\n");
        commit(root, name);
    }
    git(root, &["checkout", "-q", "main"]);
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let cache = root.join(".logos/review-cache");
    let stamps = || {
        let mut all: Vec<(String, std::time::SystemTime)> = fs::read_dir(&cache)
            .unwrap()
            .map(|e| e.unwrap())
            .map(|e| (e.file_name().into_string().unwrap(), e.metadata().unwrap().modified().unwrap()))
            .collect();
        all.sort();
        all
    };

    let first = project(root, &refs);
    let built = stamps();
    assert_eq!(built.len(), 19, "18 sides and the review entry: {built:?}");
    let second = project(root, &refs);
    assert_eq!(second, first);
    assert_eq!(stamps(), built, "no side was measured again");
    assert!(review_entry.exists(), "a projection never evicts a review's entry");
}

/// When the only limit is a numeric `drift_budget` and the base has no
/// signal, nothing is projected, and the answer says so rather than reading
/// as checked and clean.
#[test]
fn a_projection_that_projects_nothing_is_not_measured() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    write(root, ".gitignore", ".logos/\n");
    write(root, "README.md", "nothing to measure\n");
    write(root, ".logos/rules.toml", "[review]\ndrift_budget = 5\n");
    commit(root, "base");
    git(root, &["branch", "-M", "main"]);
    git(root, &["checkout", "-q", "-b", "b1"]);
    write(root, "src/lib.rs", "pub fn first() {}\n");
    commit(root, "code");
    git(root, &["checkout", "-q", "main"]);

    let answer = project(root, &["b1"]);
    assert!(!answer.measured, "{answer:#?}");
    assert!(answer.not_measured_because.as_deref().unwrap().contains("no declared limit could be projected"));
    assert!(answer.limits.is_empty() && answer.exceeded.is_empty());
    let rules: Vec<&str> = answer.not_projected.iter().map(|l| l.rule.as_str()).collect();
    assert_eq!(rules, ["drift_budget"]);
    assert!(answer.not_projected[0].reason.contains("has no signal"), "{answer:#?}");
}

/// A numeric `drift_budget` alone is projected: each of `d1` and `d2` adds a
/// dead function, which lowers the signal, and the summed drop is judged by
/// the budget with each branch's drop named.
#[test]
fn a_numeric_drift_budget_is_projected_from_each_branch_drop() {
    let tmp = repo();
    let root = tmp.path();
    write(root, ".logos/rules.toml", "[review]\ndrift_budget = 0\n");
    for (branch, m) in [("d1", "a"), ("d2", "b")] {
        git(root, &["checkout", "-q", "-b", branch, "main"]);
        write(root, &format!("src/{m}.rs"), &format!("pub fn work_{m}() {{}}\n\nfn unused_{m}() {{}}\n"));
        commit(root, &format!("a dead function in {m}"));
    }
    git(root, &["checkout", "-q", "main"]);

    let answer = project(root, &["d1", "d2"]);
    assert!(answer.measured, "{answer:#?}");
    assert_eq!(answer.limits, ["drift_budget"]);
    assert!(answer.exceeded.is_empty() && answer.not_projected.is_empty(), "{answer:#?}");
    let drift = answer.drift.as_ref().expect("two drops past a budget of 0");
    let named: Vec<&str> = drift.shares.iter().map(|s| s.branch.as_str()).collect();
    assert_eq!(named, ["d1", "d2"]);
    assert!(drift.shares.iter().all(|s| s.share > 0 && s.exceeds_alone), "{drift:#?}");
    let total: i64 = drift.shares.iter().map(|s| s.share).sum();
    assert_eq!((drift.drift.drop, drift.drift.budget, drift.drift.exceeded), (total, Some(0.0), true));
    assert_eq!(i64::from(drift.drift.baseline) - i64::from(drift.drift.current), total);
    assert!(answer.notes[0].contains("from the base commit's signal"), "{:?}", answer.notes);
    assert_eq!(
        (drift.reference.kind, drift.reference.reason.as_deref(), drift.reference.snapshot_id),
        (DriftReference::BASE, Some("no gate baseline is saved in this checkout"), None),
        "no gate --save ran here"
    );
}

/// With a comparable blessed baseline the drift is projected from it, as the
/// gate after the merge judges it. The baseline is saved at the first `main`;
/// `main` then loses points to three dead functions, and `d1` and `d2` each
/// add one more. Under `drift_budget` = the branches' summed drop, the base
/// commit alone would read "within"; from the baseline it is past, and the
/// gate over the merged tree agrees. A baseline saved under other thresholds
/// is not comparable, and the base commit is the reference again.
#[test]
fn the_drift_is_projected_from_a_comparable_baseline_as_the_merged_gate_judges_it() {
    let tmp = repo();
    let root = tmp.path();
    write(root, ".logos/rules.toml", "[review]\ndrift_budget = 0\n");
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let saved = engine.gate(None, true, true).expect("gate --save");
    let blessed = saved.signal.expect("a baseline signal");
    write(root, "src/c.rs", "pub fn work_c() {}\n\nfn gone_1() {}\n\nfn gone_2() {}\n\nfn gone_3() {}\n");
    commit(root, "main loses points");
    for (branch, m) in [("d1", "a"), ("d2", "b")] {
        git(root, &["checkout", "-q", "-b", branch, "main"]);
        write(root, &format!("src/{m}.rs"), &format!("pub fn work_{m}() {{}}\n\nfn unused_{m}() {{}}\n"));
        commit(root, &format!("a dead function in {m}"));
    }
    git(root, &["checkout", "-q", "main"]);

    let first = project(root, &["d1", "d2"]);
    let drift = first.drift.as_ref().expect("past a budget of 0");
    assert_eq!(drift.reference.kind, DriftReference::BASELINE, "{drift:#?}");
    assert!(drift.reference.snapshot_id.is_some() && drift.reference.reason.is_none());
    assert_eq!(drift.drift.baseline, blessed, "judged from the blessed signal");
    let summed: i64 = drift.shares.iter().map(|s| s.share).sum();
    assert!(drift.drift.drop > summed, "the base is already below the baseline: {drift:#?}");
    let id = drift.reference.snapshot_id.expect("the baseline's snapshot");
    assert_eq!(
        first.notes[0],
        format!(
            "drift_budget is projected from this checkout's blessed gate baseline (snapshot {id}), as the gate after \
             the merge would judge it: another checkout's baseline can give another answer"
        )
    );

    write(root, ".logos/rules.toml", &format!("[review]\ndrift_budget = {summed}\n"));
    let at_sum = project(root, &["d1", "d2"]);
    let drift = at_sum.drift.as_ref().expect("from the baseline the merge falls past the summed drop");
    assert!(drift.shares.iter().all(|s| s.exceeds_alone), "each share plus the base's own fall: {drift:#?}");
    git(root, &["checkout", "-q", "-b", "merged", "main"]);
    git(root, &["merge", "-q", "--no-edit", "d1", "d2"]);
    engine.index();
    let gate = engine.gate(None, false, true).expect("gate over the merged tree");
    assert!(gate.drift.is_some_and(|d| d.exceeded), "the merged gate agrees: {:?}", gate.drift);
    git(root, &["checkout", "-q", "main"]);

    write(root, ".logos/rules.toml", "[review]\ndrift_budget = 0\n[metric_thresholds]\nnesting_depth = 9\n");
    let other = project(root, &["d1", "d2"]);
    let drift = other.drift.as_ref().expect("past a budget of 0");
    assert_eq!(
        (drift.reference.kind, drift.reference.reason.as_deref()),
        (DriftReference::BASE, Some("the gate baseline was saved under other thresholds"))
    );
    assert_eq!(drift.drift.drop, drift.shares.iter().map(|s| s.share).sum::<i64>(), "from the base commit");
}
