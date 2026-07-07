//! Integration tests for the coverage evidence-tier **surfaces** (S-051, CR-007),
//! exercised end-to-end through the [`Engine`] façade over real git fixtures + a
//! real index + real coverage ingests:
//!
//! - per-file freshness on read, hash-based, with the stale flip ([FR-CV-05],
//!   [UAT-CV-03]);
//! - `coverage status` provenance + the freshness fraction ([FR-CV-06]);
//! - the untested-hotspots join: the coverage column and the `--untested` filter,
//!   plus the labeled static-reachability fallback ([FR-CV-07]);
//! - gate-immunity: coverage ingest / staleness / `history.db` deletion never move
//!   the gate ([BR-28], [UAT-CV-02]).
//!
//! The pure ranking + freshness arithmetic is unit-tested inside
//! `history::hotspot` / `history::coverage`; these prove the cross-store join and
//! the surface contracts against actual `git` + a real coverage store.

#![cfg(feature = "lang-rust")]

use std::path::Path;
use std::process::Command;

use logos_core::Engine;
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;

// ── git fixture helpers (mirroring tests/hotspots.rs conventions) ────────────

fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit(cwd: &Path, rel: &str, contents: &str, msg: &str) {
    let path = cwd.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    sh_git(cwd, &["add", rel]);
    sh_git(cwd, &["commit", "-q", "-m", msg]);
}

/// A branchy function body (decision points → a non-trivial per-function CC).
fn branchy(name: &str, ifs: usize) -> String {
    let body: String = (0..ifs)
        .map(|i| format!("    if x == {i} {{ return {i}; }}\n"))
        .collect();
    format!("pub fn {name}(x: i64) -> i64 {{\n{body}    x\n}}\n")
}

/// Write `report` under `root` and ingest it through the `api` (auto-detect).
fn ingest(engine: &Engine, root: &Path, report: &str) -> logos_core::history::IngestSummary {
    let path = root.join("coverage.info");
    std::fs::write(&path, report).unwrap();
    engine
        .coverage_ingest(&path, None)
        .expect("coverage ingest succeeds")
}

// ── FR-CV-05 / FR-CV-06 / UAT-CV-03: freshness on read, the stale flip ───────

#[test]
fn editing_a_covered_file_flips_it_to_stale_and_drops_the_freshness_fraction() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", "pub fn a() -> i64 { 0 }\n", "add a");
    commit(repo, "src/b.rs", "pub fn b() -> i64 { 0 }\n", "add b");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    // Both files covered → both fresh, fraction 100% (10000 bp).
    ingest(
        &engine,
        repo,
        "TN:suite\nSF:src/a.rs\nDA:1,1\nend_of_record\n\
         SF:src/b.rs\nDA:1,1\nend_of_record\n",
    );
    let status = engine.coverage_status().expect("coverage status");
    assert_eq!(status.total_files, 2);
    assert_eq!(status.fresh_files, 2);
    assert_eq!(status.stale_files, 0);
    assert_eq!(
        status.freshness_bp,
        Some(10_000),
        "all covered files are fresh"
    );
    assert_eq!(
        status.overall_coverage_bp,
        Some(10_000),
        "(1+1)/(1+1) lines covered over the fresh files = 100% (FR-CV-06)"
    );
    assert!(
        status.notice.is_none(),
        "a populated store has no n/a notice"
    );
    assert!(
        status.head_sha.is_some() && !status.formats.is_empty() && status.report_count == 1,
        "provenance is present: {status:?}"
    );
    let a = status.files.iter().find(|f| f.path == "src/a.rs").unwrap();
    assert_eq!(a.freshness, "fresh");
    assert_eq!(a.coverage_bp, Some(10_000), "1/1 lines covered");

    // Edit a.rs on disk → its content hash no longer matches the anchor.
    std::fs::write(repo.join("src/a.rs"), "pub fn a() -> i64 { 1 }\n").unwrap();
    let status = engine
        .coverage_status()
        .expect("coverage status after edit");
    let a = status.files.iter().find(|f| f.path == "src/a.rs").unwrap();
    assert_eq!(a.freshness, "stale", "the edited file flips to stale");
    assert_eq!(
        a.coverage_bp, None,
        "stale line data is never rendered (FR-CV-05)"
    );
    assert_eq!(a.instrumented_lines, 0, "no stale line counts surface");
    let b = status.files.iter().find(|f| f.path == "src/b.rs").unwrap();
    assert_eq!(b.freshness, "fresh", "the untouched file stays fresh");
    assert_eq!(
        status.freshness_bp,
        Some(5_000),
        "exactly one of two files went stale → 50%"
    );
    // The aggregate counts only the fresh files' line ratio — distinct from the
    // freshness fraction: the stale file drops out, the surviving fresh file is
    // still 1/1 covered → 100%, while freshness fell to 50% (FR-CV-06).
    assert_eq!(
        status.overall_coverage_bp,
        Some(10_000),
        "overall coverage aggregates only fresh files, never the freshness fraction"
    );
}

/// [FR-CV-06] / [CR-021]: the overall aggregate is a raw covered ÷ instrumented
/// ratio over the fresh files — a real percentage, not just 0/100.
#[test]
fn overall_coverage_aggregate_is_the_raw_fresh_ratio() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", "pub fn a() -> i64 { 0 }\n", "add a");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    // 4 instrumented lines, 1 covered → 1/4 = 25.00% (2500 bp), raw and ungraded.
    ingest(
        &engine,
        repo,
        "TN:suite\nSF:src/a.rs\nDA:1,1\nDA:2,0\nDA:3,0\nDA:4,0\nend_of_record\n",
    );
    let status = engine.coverage_status().expect("coverage status");
    assert_eq!(
        status.overall_coverage_bp,
        Some(2_500),
        "1 of 4 instrumented lines covered = 25% (FR-CV-06)"
    );
}

/// Count `coverage_snapshots` rows by opening `history.db` read-only; `0` when
/// the store has not been created — the FR-CV-06 "reading it mutates no store"
/// invariant the status read-model must hold ([ADR-28], mirroring the
/// `metric_snapshot_count` pattern in `read_only_accessors.rs`).
fn coverage_snapshot_count(repo: &Path) -> i64 {
    let db = repo.join(".logos/history.db");
    if !db.exists() {
        return 0;
    }
    let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open history.db read-only");
    conn.query_row("SELECT count(*) FROM coverage_snapshots", [], |r| r.get(0))
        .expect("count coverage_snapshots")
}

/// [FR-CV-06] / [ADR-28]: reading `coverage status` (including the new overall
/// aggregate) mutates no store — repeated calls leave the `coverage_snapshots`
/// row count byte-for-byte unchanged.
#[test]
fn reading_coverage_status_appends_no_snapshot() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", "pub fn a() -> i64 { 0 }\n", "add a");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    ingest(&engine, repo, "TN:suite\nSF:src/a.rs\nDA:1,1\nend_of_record\n");

    let before = coverage_snapshot_count(repo);
    assert_eq!(before, 1, "the ingest wrote exactly one snapshot");

    // The aggregate is derived on read; repeated status reads append nothing.
    for _ in 0..3 {
        let status = engine.coverage_status().expect("coverage status");
        assert_eq!(status.overall_coverage_bp, Some(10_000));
    }
    assert_eq!(
        coverage_snapshot_count(repo),
        before,
        "reading coverage status appended no coverage snapshot (FR-CV-06, ADR-28)"
    );
}

#[test]
fn coverage_status_reports_na_with_a_notice_when_nothing_is_ingested() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();

    let status = engine.coverage_status().expect("status succeeds (exit 0)");
    assert!(status.head_sha.is_none() && status.total_files == 0);
    assert_eq!(
        status.overall_coverage_bp, None,
        "no coverage → the overall aggregate is n/a, not 0% (FR-CV-06)"
    );
    assert!(
        status.notice.as_deref().is_some_and(|n| !n.is_empty()),
        "a one-line n/a notice explains the empty store"
    );
}

// ── FR-CV-07: the untested-hotspots join (coverage column + --untested) ──────

#[test]
fn untested_filter_ranks_uncovered_hotspots_first_and_excludes_covered() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);

    // hot.rs: top churn (4 commits) + top complexity (8 branches), NO coverage.
    for n in 0..4 {
        commit(
            repo,
            "src/hot.rs",
            &format!("{}// rev {n}\n", branchy("hot", 8)),
            &format!("hot v{n}"),
        );
    }
    // tested.rs: high churn (3 commits) + complexity (6 branches), FULLY covered.
    for n in 0..3 {
        commit(
            repo,
            "src/tested.rs",
            &format!("{}// rev {n}\n", branchy("tested", 6)),
            &format!("tested v{n}"),
        );
    }
    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    // Cover every instrumented line of tested.rs with hits → fresh & positive.
    let tested_lines: String = (1..=9).map(|n| format!("DA:{n},1\n")).collect();
    ingest(
        &engine,
        repo,
        &format!("TN:suite\nSF:src/tested.rs\n{tested_lines}end_of_record\n"),
    );

    // The coverage column rides every hotspot row (FR-CV-05/07).
    let all = engine.hotspots(None, false).expect("hotspots");
    assert_eq!(all.coverage_basis, "coverage", "a snapshot exists");
    assert_eq!(
        all.coverage_label, None,
        "no fallback label when coverage exists"
    );
    let tested = all
        .files
        .iter()
        .find(|h| h.path == "src/tested.rs")
        .unwrap();
    assert_eq!(tested.coverage.state, "fresh");
    assert_eq!(tested.coverage.coverage_bp, Some(10_000));
    let hot = all.files.iter().find(|h| h.path == "src/hot.rs").unwrap();
    assert_eq!(hot.coverage.state, "n/a", "hot.rs was never covered");

    // --untested: tested.rs (fresh-covered) is excluded; the uncovered hottest
    // file ranks first (FR-CV-07).
    let untested = engine.hotspots(None, true).expect("hotspots --untested");
    assert!(untested.untested);
    let paths: Vec<&str> = untested.files.iter().map(|h| h.path.as_str()).collect();
    assert_eq!(
        paths.first(),
        Some(&"src/hot.rs"),
        "the uncovered hotspot ranks first under --untested: {paths:?}"
    );
    assert!(
        !paths.contains(&"src/tested.rs"),
        "a fresh-covered file is excluded under --untested: {paths:?}"
    );

    // Stale-as-absent end-to-end ([FR-CV-07], [NFR-RA-05]): editing the covered
    // file (without committing — churn/complexity are unchanged) flips its
    // coverage to stale, so `--untested` now RETAINS it. The ranking is built
    // from honest inputs only — never from the shifted line data.
    std::fs::write(
        repo.join("src/tested.rs"),
        format!("{}// edited\n", branchy("tested", 6)),
    )
    .unwrap();
    let after_edit = engine.hotspots(None, true).expect("hotspots --untested");
    let stale = after_edit
        .files
        .iter()
        .find(|h| h.path == "src/tested.rs")
        .expect("the now-stale file is retained under --untested");
    assert_eq!(
        stale.coverage.state, "stale",
        "the edited file's coverage is stale, not fresh"
    );
    assert_eq!(
        stale.coverage.coverage_bp, None,
        "stale coverage carries no number on the hotspot column"
    );
}

#[test]
fn untested_without_coverage_carries_the_static_reachability_fallback_label() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    for n in 0..3 {
        commit(
            repo,
            "src/hot.rs",
            &format!("{}// rev {n}\n", branchy("hot", 5)),
            &format!("hot v{n}"),
        );
    }
    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    // No coverage ingested → the fallback basis + label, and every column n/a.
    let report = engine.hotspots(None, true).expect("hotspots --untested");
    assert_eq!(report.coverage_basis, "static-reachability");
    assert_eq!(
        report.coverage_label,
        Some("static reachability, not execution coverage"),
        "the two signals are never blended without a label (FR-CV-07)"
    );
    assert!(
        report.files.iter().all(|h| h.coverage.state == "n/a"),
        "no coverage → every column is n/a"
    );
}

// ── BR-28 / UAT-CV-02: gate-immunity across all coverage states ──────────────

/// The gated verdict, stripped of provenance that legitimately tracks the commit
/// — what BR-28 pins as a pure function of tree + config (mirrors tests/hotspots.rs).
fn gated_verdict(g: &logos_core::models::quality::GateResult) -> String {
    serde_json::to_string(&serde_json::json!({
        "passed": g.passed,
        "signal": g.signal,
        "baseline_signal": g.baseline_signal,
        "regressions": g.regressions,
        "test_function_count": g.test_function_count,
    }))
    .unwrap()
}

#[test]
fn gate_is_byte_identical_across_coverage_ingest_staleness_and_deletion() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-b", "main", "-q"]);
    commit(
        repo,
        "src/covered.rs",
        &branchy("covered", 4),
        "add covered",
    );
    commit(repo, "src/other.rs", &branchy("other", 2), "add other");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    // Baseline on the pristine indexed tree.
    engine.gate(None, true, true).expect("gate --save");
    let baseline = gated_verdict(&engine.gate(None, false, true).expect("gate"));

    // (1) After ingest (touches only history.db): byte-identical.
    let lines: String = (1..=6).map(|n| format!("DA:{n},1\n")).collect();
    ingest(
        &engine,
        repo,
        &format!("TN:suite\nSF:src/covered.rs\n{lines}end_of_record\n"),
    );
    assert!(
        repo.join(".logos/history.db").exists(),
        "the ingest created history.db"
    );
    let after_ingest = gated_verdict(&engine.gate(None, false, true).expect("gate"));
    assert_eq!(
        baseline, after_ingest,
        "coverage ingest never moves the gate"
    );

    // (2) Make the coverage STALE by editing the covered file, then prove the
    // gate on that edited tree is identical WITH stale coverage and WITHOUT any
    // coverage — i.e. the coverage tier (stale or absent) never enters the gate.
    std::fs::write(repo.join("src/covered.rs"), branchy("covered", 5)).unwrap();
    // Coverage is now stale (anchor mismatch); confirm via the read surface.
    let cov = engine.coverage_status().expect("coverage status");
    assert_eq!(
        cov.stale_files, 1,
        "editing the covered file made its coverage stale"
    );
    let with_stale = gated_verdict(&engine.gate(None, false, true).expect("gate"));

    std::fs::remove_file(repo.join(".logos/history.db")).expect("delete history.db");
    let without_coverage = gated_verdict(&engine.gate(None, false, true).expect("gate"));
    assert_eq!(
        with_stale, without_coverage,
        "stale coverage vs no coverage on the same tree → identical gate (BR-28)"
    );
}

// ── FR-CV-10 / ADR-38: automatic ingest, refresh, and artifact-vs-HEAD staleness

/// [FR-CV-10] / [ADR-38]: the watcher-driven auto-ingest path (`coverage_ingest_auto`)
/// ingests an artifact through the same path as the manual command — a local
/// read + parse + store, no test run — and `coverage status` reflects it. This is
/// the engine seam the watcher worker drives on an artifact change.
#[test]
fn auto_ingest_populates_coverage_through_the_shared_path() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", "pub fn a() -> i64 { 0 }\n", "add a");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    // Drop a conventional artifact and auto-ingest it (no manual command).
    let artifact = repo.join("lcov.info");
    std::fs::write(&artifact, "TN:suite\nSF:src/a.rs\nDA:1,1\nend_of_record\n").unwrap();
    let summary = engine
        .coverage_ingest_auto(&artifact)
        .expect("auto-ingest succeeds");
    assert_eq!(summary.matched_files, 1, "src/a.rs bound");

    let status = engine.coverage_status().expect("coverage status");
    assert_eq!(status.total_files, 1);
    assert_eq!(status.fresh_files, 1);
    assert_eq!(status.overall_coverage_bp, Some(10_000));
    assert!(status.notice.is_none(), "auto-ingest populated the store");
}

/// [FR-CV-10] / [ADR-11]: an auto-ingest of a malformed artifact fails (the
/// watcher degrades it to a warning) WITHOUT a partial write — the store is left
/// byte-identical (here: never even created), so a bad artifact can never block
/// or corrupt the sync.
#[test]
fn auto_ingest_of_a_malformed_artifact_fails_without_writing() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", "pub fn a() -> i64 { 0 }\n", "add a");
    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    // A truncated LCOV (DA with no hit count) is rejected atomically.
    let artifact = repo.join("lcov.info");
    std::fs::write(&artifact, "SF:src/a.rs\nDA:1\nend_of_record\n").unwrap();
    assert!(
        engine.coverage_ingest_auto(&artifact).is_err(),
        "a malformed artifact is rejected (the watcher degrades this to a warning)"
    );
    assert_eq!(
        coverage_snapshot_count(repo),
        0,
        "a rejected auto-ingest wrote nothing (byte-identical store, ADR-11)"
    );
}

/// [FR-CV-10] / [ADR-38]: `coverage refresh` runs the configured `refresh_cmd` as
/// the lone explicit subprocess and ingests the artifact it produced. The command
/// here writes a conventional `lcov.info`; discovery + ingest follow.
#[test]
fn coverage_refresh_runs_the_command_and_ingests_its_output() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", "pub fn a() -> i64 { 0 }\n", "add a");

    // Configure a refresh command that materializes a conventional artifact.
    std::fs::create_dir_all(repo.join(".logos")).unwrap();
    std::fs::write(
        repo.join(".logos/config.toml"),
        "[coverage_ingest]\n\
         refresh_cmd = \"printf 'TN:suite\\\\nSF:src/a.rs\\\\nDA:1,1\\\\nend_of_record\\\\n' > lcov.info\"\n",
    )
    .unwrap();

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    let summary = engine.coverage_refresh().expect("coverage refresh succeeds");
    assert_eq!(summary.artifact, "lcov.info", "the convention artifact was ingested");
    assert_eq!(summary.ingest.matched_files, 1);

    let status = engine.coverage_status().expect("coverage status");
    assert_eq!(status.fresh_files, 1);
    assert_eq!(status.overall_coverage_bp, Some(10_000));
}

/// [FR-CV-10]: `coverage refresh` with no configured `refresh_cmd` fails loud
/// (exit non-zero) rather than silently doing nothing.
#[test]
fn coverage_refresh_without_a_configured_command_errs() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", "pub fn a() -> i64 { 0 }\n", "add a");
    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    let err = engine.coverage_refresh().expect_err("no refresh_cmd → error");
    assert!(
        err.to_string().contains("refresh_cmd"),
        "the error names the missing config: {err}"
    );
}

/// [FR-CV-06] / [FR-CV-10] / [ADR-38]: `coverage status` surfaces an
/// artifact-vs-HEAD staleness prompt once the tree moves past the ingest HEAD —
/// independent of per-file content freshness (the covered file is untouched, so it
/// stays content-fresh while the artifact still lags the new commit).
#[test]
fn coverage_status_flags_artifact_vs_head_staleness() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", "pub fn a() -> i64 { 0 }\n", "add a");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    ingest(&engine, repo, "TN:suite\nSF:src/a.rs\nDA:1,1\nend_of_record\n");

    // At the ingest HEAD: not stale, no prompt.
    let fresh = engine.coverage_status().expect("coverage status");
    assert!(!fresh.head_stale, "coverage ingested at the current HEAD is not stale");
    assert!(fresh.staleness_prompt.is_none());
    assert!(fresh.current_head.is_some(), "HEAD resolves in a git repo");

    // Advance HEAD with an unrelated commit; src/a.rs is untouched on disk.
    commit(repo, "src/b.rs", "pub fn b() -> i64 { 0 }\n", "add b");
    let stale = engine.coverage_status().expect("coverage status");
    assert!(
        stale.head_stale,
        "the artifact now lags HEAD (ingested at the prior commit)"
    );
    let prompt = stale.staleness_prompt.expect("a staleness prompt is surfaced");
    assert!(
        prompt.contains("lags HEAD") && prompt.contains("refresh"),
        "the prompt explains the lag and names the remedy: {prompt}"
    );
    // The covered file's content never changed, so it stays content-fresh — the
    // two staleness notions are independent (FR-CV-05 vs FR-CV-06).
    let a = stale.files.iter().find(|f| f.path == "src/a.rs").unwrap();
    assert_eq!(a.freshness, "fresh", "per-file freshness is unchanged by a HEAD move");
}

/// [BR-28] / [UAT-CV-02]: the gate is byte-identical after an AUTO-ingest — the
/// auto path writes only `history.db`, never `logos.db`, so the gated verdict is
/// unmoved (the auto-ingested coverage state of the byte-identity invariant).
#[test]
fn gate_is_byte_identical_after_auto_ingest() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-b", "main", "-q"]);
    commit(repo, "src/covered.rs", &branchy("covered", 4), "add covered");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    engine.gate(None, true, true).expect("gate --save");
    let baseline = gated_verdict(&engine.gate(None, false, true).expect("gate"));

    // Auto-ingest (the watcher path) touches only history.db.
    let artifact = repo.join("lcov.info");
    let lines: String = (1..=6).map(|n| format!("DA:{n},1\n")).collect();
    std::fs::write(
        &artifact,
        format!("TN:suite\nSF:src/covered.rs\n{lines}end_of_record\n"),
    )
    .unwrap();
    engine
        .coverage_ingest_auto(&artifact)
        .expect("auto-ingest succeeds");

    let after_auto = gated_verdict(&engine.gate(None, false, true).expect("gate"));
    assert_eq!(
        baseline, after_auto,
        "auto coverage ingest never moves the gate (BR-28, UAT-CV-02)"
    );
}
