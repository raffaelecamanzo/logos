//! Black-box end-to-end confirmation of the NATIVE worktree lifecycle
//! (S-216, [CR-054], [FR-WT-05], [FR-WT-03], [FR-WT-01], [NFR-RA-06],
//! [ADR-48]) — driven entirely through REAL `git worktree`/`git merge` I/O
//! and the public `Engine` façade, exactly as the CLI/MCP surfaces use it,
//! with **no dev-harness skill or external orchestration involved**
//! ([CRA-03](../../docs/requests/CR-054-graph-update-admission-unification.md)).
//!
//! `tests/worktree.rs` (S-021/[ADR-15]) proves the seed/diff-reconcile
//! substrate in isolation; `tests/admission_convergence.rs` (S-214) proves
//! the admission gate on synthetic boundary/gitignore fixtures fed through
//! `Engine::sync`. This suite composes both into the single lifecycle S-216
//! is chartered to confirm end to end:
//!
//! 1. opening an engine in a linked worktree serves correct navigation via
//!    seed + diff-reconcile, never a cold index
//!    ([FR-WT-01](../../docs/specs/requirements/FR-WT-01.md),
//!    [FR-WT-03](../../docs/specs/requirements/FR-WT-03.md));
//! 2. a gitignored file created in the worktree is not indexed, and a
//!    worktree nested under the primary root is not folded into the primary
//!    graph — the S-214 admission wiring exercised on the REAL worktree
//!    substrate, not a synthetic fixture;
//! 3. `session_start`/`session_end` measure the worktree's OWN increment,
//!    anchored to a baseline stored in the worktree's own DB, isolated from
//!    the primary's;
//! 4. merging the worktree's branch back into the primary and reconciling
//!    (`scan(true)`) makes the primary's graph population equal a fresh
//!    `index` of the same tree — **reconcile-to-source**, no cross-DB graph
//!    copy — and `Engine::verify()` reports `ok`
//!    ([NFR-RA-06](../../docs/specs/requirements/NFR-RA-06.md)).
//!
//! Gated on `lang-rust`: integration tests share the crate's feature set and
//! a `--no-default-features` build excludes the Rust grammar these fixtures
//! need.
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

/// The absolute path of `<root>/<rel>` — the shape the watcher/hook hand to
/// `Engine::sync` (OS events carry absolute paths).
fn abs(root: &Path, rel: &str) -> PathBuf {
    root.join(rel)
}

/// Does a function named `name` exist in this engine's graph?
fn has_fn(engine: &Engine, name: &str) -> bool {
    engine
        .search(name, Some(logos_core::model::NodeKind::Function), Some(16))
        .hits
        .iter()
        .any(|h| h.name == name)
}

/// Does the graph hold any node whose defining file is `rel`?
fn has_nodes_for_file(engine: &Engine, rel: &str) -> bool {
    let rel = rel.to_string();
    engine
        .runtime()
        .expect("runtime present")
        .submit_read(move |store| {
            Ok(store
                .all_nodes()?
                .iter()
                .any(|n| n.file_path.as_deref() == Some(rel.as_str())))
        })
        .expect("read runs")
}

/// The canonical, rowid-independent identity of the graph: the sorted set of
/// node tuples `(symbol, kind, name, file, start, end)` and edge tuples
/// `(source_symbol, target_symbol, kind)`. Two indexes that admit the same
/// files produce the same canonical graph even though their autoincrement
/// rowids differ — the "population equals a fresh index" [NFR-RA-06] names.
type CanonicalGraph = (
    Vec<(String, i32, String, Option<String>, Option<i64>, Option<i64>)>,
    Vec<(String, String, i32)>,
);

fn canonical_graph(engine: &Engine) -> CanonicalGraph {
    engine
        .runtime()
        .expect("runtime present")
        .submit_read(|store| {
            let nodes = store.all_nodes()?;
            let edges = store.all_edges()?;
            let symbol_of: std::collections::HashMap<_, _> = nodes
                .iter()
                .map(|n| (n.id, n.symbol.as_str().to_string()))
                .collect();

            let mut node_rows: Vec<_> = nodes
                .iter()
                .map(|n| {
                    (
                        n.symbol.as_str().to_string(),
                        n.kind.as_i32(),
                        n.name.clone(),
                        n.file_path.clone(),
                        n.start_line,
                        n.end_line,
                    )
                })
                .collect();
            node_rows.sort();

            let mut edge_rows: Vec<_> = edges
                .iter()
                .map(|e| {
                    (
                        symbol_of[&e.source].clone(),
                        symbol_of[&e.target].clone(),
                        e.kind.as_i32(),
                    )
                })
                .collect();
            edge_rows.sort();

            Ok((node_rows, edge_rows))
        })
        .expect("canonical-graph read runs")
}

/// A committed primary repo mirroring the canonical layout: tracked source,
/// tracked `.logos/` policy, and the gitignored-DB posture of FR-IN-04
/// (`.logos/*.db*` never travels through git).
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
fn add_worktree(tmp: &TempDir, main: &Path, branch: &str) -> PathBuf {
    let wt = tmp.path().join("wt");
    sh_git(
        main,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", branch],
    );
    wt
}

/// The full S-216 narrative: open a worktree with no dev-harness skills
/// present, confirm seed + diff-reconcile navigation, confirm the gitignored
/// scratch it creates is excluded, run a session bracket over its own
/// increment, then merge its branch back into the primary and confirm
/// reconcile-to-source converges the primary's population to a fresh index
/// with `verify().ok`.
#[test]
fn worktree_lifecycle_end_to_end_seed_diff_reconcile_session_and_merge_back_parity() {
    let (tmp, main) = repo_fixture();
    // Canonicalize so every absolute path handed to `sync` below shares the
    // prefix the pipeline relativizes against (macOS `/var` → `/private/var`);
    // otherwise a path would be skipped as "outside the root" before the
    // admission gate ever runs, and the test would pass for the wrong reason.
    let main = main.canonicalize().expect("canonicalize main root");

    // Index the primary checkout, then release it (drop = writer torn down)
    // so the worktree bootstrap below can seed from its on-disk DB.
    {
        let engine = Engine::start(&main).expect("primary engine starts");
        let indexed = engine.index();
        assert!(indexed.files_indexed >= 1, "primary index ran");
    }
    assert!(main.join(".logos/logos.db").exists());

    let wt = add_worktree(&tmp, &main, "feature");
    let wt = wt.canonicalize().expect("canonicalize worktree root");

    // ── (1) FR-WT-01/FR-WT-03: seed + diff-reconcile, no cold index ──────────
    let engine_wt = Engine::start(&wt).expect("worktree engine starts");
    assert!(
        wt.join(".logos/logos.db").exists(),
        "the worktree owns its own DB after first use (FR-WT-01)"
    );
    let status = engine_wt.status();
    assert!(
        status.indexed,
        "the graph is populated from the seed before any navigation call \
         could auto-index: {status:?}"
    );
    assert!(
        status.last_full_index_at.is_none(),
        "no cold index ran — the bootstrap was seed + diff-reconcile (FR-WT-03)"
    );
    assert!(
        has_fn(&engine_wt, "seeded_fn"),
        "navigation on the worktree correctly serves the seeded primary graph"
    );

    // ── (3a) session_start baselines the WORKTREE's own graph ────────────────
    let baseline = engine_wt.session_start().expect("session_start runs");

    // ── (2) admission wiring on the REAL worktree substrate ──────────────────
    // A gitignored file created in the worktree...
    write(&wt, ".gitignore", ".logos/*.db\n.logos/*.db-*\nscratch/\n");
    write(&wt, "scratch/generated.rs", "pub fn generated_fn() {}\n");
    // ...alongside the worktree's own genuine increment: a new admitted file.
    write(&wt, "src/feature.rs", "pub fn feature_fn() {}\n");
    engine_wt.sync(&[
        abs(&wt, "scratch/generated.rs"),
        abs(&wt, "src/feature.rs"),
    ]);
    assert!(
        !has_nodes_for_file(&engine_wt, "scratch/generated.rs"),
        "a gitignored file created in the worktree is not indexed (S-214 admission wiring)"
    );
    assert!(
        has_fn(&engine_wt, "feature_fn"),
        "the worktree's own genuine increment IS indexed (the gate is not over-broad)"
    );

    // ── (3b) session_end measures the worktree's OWN increment ───────────────
    let ended = engine_wt.session_end().expect("session_end runs");
    assert_eq!(
        ended.baseline_signal, baseline.signal,
        "session_end compares against the SAME baseline session_start recorded, \
         stored in the worktree's own DB (per-worktree session accounting)"
    );
    // `baseline_signal` alone only proves the same baseline row round-tripped —
    // it would hold even if the reconcile below were a no-op. `signal.is_some()`
    // proves session_end computed a FRESH score over the worktree's own
    // reconciled graph (not a degraded "n/a" no-op) — jointly with the
    // `has_fn` check below, the reconcile ran AND the fresh score reflects it.
    assert!(
        ended.signal.is_some(),
        "session_end computed a fresh signal over the worktree's own graph: {ended:?}"
    );
    assert!(
        has_fn(&engine_wt, "feature_fn"),
        "session_end's reconcile (gate(reconcile=true)) still reflects the \
         worktree's increment after the session closes"
    );

    // The primary's graph never saw any of this (AA-05 / NFR-CC-02): a
    // fresh handle on main proves the worktree session left it untouched.
    {
        let main_engine = Engine::start(&main).expect("primary engine reopens");
        assert!(
            !has_fn(&main_engine, "feature_fn"),
            "the primary checkout's DB is isolated from the worktree's session"
        );
    }

    // Commit the worktree's tracked changes on its branch; the gitignored
    // scratch is never staged, so it never reaches the merge below.
    sh_git(&wt, &["add", "."]);
    sh_git(&wt, &["commit", "-q", "-m", "feature work"]);
    drop(engine_wt); // release the worktree's ephemeral DB — never copied anywhere.

    // ── (4) merge-back: reconcile-to-source, no cross-DB copy ────────────────
    sh_git(&main, &["merge", "-q", "feature"]);
    let main_engine = Engine::start(&main).expect("primary engine reopens after merge");
    main_engine
        .scan(true)
        .expect("full-walk reconcile after merge-back");
    assert!(
        has_fn(&main_engine, "feature_fn"),
        "reconcile-to-source picked up the merged-in file (FR-WT-05)"
    );
    assert!(
        !has_nodes_for_file(&main_engine, "scratch/generated.rs"),
        "the worktree's gitignored scratch never reached the primary — it was \
         never tracked, so the merge never carried it"
    );

    // A fresh index of an independent clone of the SAME merged tree is the
    // equivalence target: reconcile-to-source must converge the primary's
    // live population to exactly what a cold `index` would produce.
    let fresh = tmp.path().join("fresh");
    sh_git(
        tmp.path(),
        &["clone", "-q", main.to_str().unwrap(), fresh.to_str().unwrap()],
    );
    let fresh_engine = Engine::start(&fresh).expect("fresh clone engine starts");
    let fresh_indexed = fresh_engine.index();
    assert!(fresh_indexed.files_indexed >= 1, "fresh index ran");

    assert_eq!(
        canonical_graph(&main_engine),
        canonical_graph(&fresh_engine),
        "reconcile-to-source must converge the primary's population to a \
         fresh index — no cross-DB graph copy (FR-WT-05, NFR-RA-06)"
    );

    let report = main_engine.verify().expect("verify runs");
    assert!(
        report.ok,
        "verify must report ok after the merge-back reconcile: {report:?}"
    );
}

/// A worktree nested UNDER the primary's own working tree (e.g. a
/// `.worktrees/<name>` convention) must never be folded into the primary's
/// graph: the nested `.git` boundary excludes it structurally, and the
/// S-213 default `ignored_dirs` name-prune excludes it belt-and-suspenders,
/// even before the primary's next reconcile.
#[test]
fn a_worktree_nested_under_the_primary_root_is_not_folded_into_the_primary_graph() {
    let (_tmp, main) = repo_fixture();
    // Canonicalize for the same reason as the lifecycle test above: the
    // `sync` path below must share the root's prefix or it would be
    // skipped as "outside the root" rather than genuinely exercising the
    // boundary/ignored_dirs admission rules under test.
    let main = main.canonicalize().expect("canonicalize main root");
    let engine = Engine::start(&main).expect("primary engine starts");
    let indexed = engine.index();
    assert!(indexed.files_indexed >= 1, "primary index ran");
    assert!(has_fn(&engine, "seeded_fn"), "the control source is indexed");

    // A real `git worktree add` whose directory lives INSIDE the primary's
    // own working tree — the layout a `.worktrees/`-based dev harness
    // produces, but created here with no such harness present.
    let inner = main.join(".worktrees").join("inner");
    fs::create_dir_all(inner.parent().unwrap()).expect("create .worktrees/");
    sh_git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            inner.to_str().unwrap(),
            "-b",
            "inner",
        ],
    );
    write(&inner, "src/nested_only.rs", "pub fn nested_only_fn() {}\n");

    // A partial sync naming the nested worktree's file directly (the
    // watcher's spelling if a filesystem event ever fired for it)...
    engine.sync(&[abs(&inner, "src/nested_only.rs")]);
    assert!(
        !has_fn(&engine, "nested_only_fn"),
        "a partial sync of a path under a nested worktree creates no nodes"
    );

    // ...and a full-walk reconcile of the primary confirms the same
    // exclusion holds for the load-bearing path, not just the pre-filter.
    engine.scan(true).expect("full-walk reconcile of the primary");
    assert!(
        !has_fn(&engine, "nested_only_fn"),
        "a nested worktree under the primary root is never folded into the \
         primary graph, even after a full-walk reconcile (FR-WT-05)"
    );
    assert!(
        !has_nodes_for_file(&engine, ".worktrees/inner/src/nested_only.rs"),
        "no node carries the nested worktree's file path"
    );

    let report = engine.verify().expect("verify runs");
    assert!(
        report.ok,
        "the primary graph stays sound with a nested worktree present: {report:?}"
    );
}
