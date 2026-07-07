//! Integration tests for the temporal-tier surface (S-048, CR-006), exercised
//! end-to-end through the [`Engine`] façade over **real** git fixtures: the
//! hotspot ranking ([FR-GH-06], [UAT-GH-03]), the gate-immunity two-tier rule
//! ([FR-GH-07], [UAT-GH-02], [BR-26]), the tier-labeled scan detail
//! ([FR-GH-07]), and the degraded modes ([FR-GH-08], [UAT-GH-04]).
//!
//! The pure ranking logic is unit-tested inside `history::hotspot`; these prove
//! the cross-store join (`history.db` churn × `logos.db` complexity) and the
//! surface contracts against an actual `git` subprocess + a real index.

#![cfg(feature = "lang-rust")]

use std::path::Path;
use std::process::Command;

use logos_core::Engine;
use tempfile::TempDir;

// ── git fixture helpers (mirroring history::tests conventions) ───────────────

/// Run a git command in `cwd` with a fixed identity, panicking on failure.
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

/// Commit `rel` with `contents` as a single message.
fn commit(cwd: &Path, rel: &str, contents: &str, msg: &str) {
    let path = cwd.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    sh_git(cwd, &["add", rel]);
    sh_git(cwd, &["commit", "-q", "-m", msg]);
}

/// A high-cyclomatic-complexity Rust function body (many decision points → a
/// large per-function CC, summed into the file's structural-complexity axis).
fn complex_source(name: &str) -> String {
    format!(
        "pub fn {name}(x: i64) -> i64 {{\n\
         {body}\
         \n    x\n}}\n",
        body = (0..8)
            .map(|i| format!("    if x == {i} {{ return {i}; }}\n"))
            .collect::<String>(),
    )
}

/// A trivial (CC≈1) Rust function.
fn calm_source(name: &str) -> String {
    format!("pub fn {name}() -> i64 {{ 0 }}\n")
}

/// Build an indexed git fixture engineering four files across the churn ×
/// complexity quadrants. `hot.rs` is top on **both** axes, so it must rank #1
/// ([UAT-GH-03]). Returns the temp guard (kept alive for `.logos/`).
fn engineered_repo() -> TempDir {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);

    // calm.rs: low churn (1 commit), low complexity.
    commit(repo, "src/calm.rs", &calm_source("calm"), "add calm");
    // complex.rs: low churn (1 commit), high complexity.
    commit(
        repo,
        "src/complex.rs",
        &complex_source("complex"),
        "add complex",
    );
    // churny.rs: high churn (4 commits), low complexity.
    for n in 0..4 {
        commit(
            repo,
            "src/churny.rs",
            &format!("pub fn churny() -> i64 {{ {n} }}\n"),
            &format!("churny v{n}"),
        );
    }
    // hot.rs: high churn (5 commits) AND high complexity — the engineered #1.
    // One commit message matches the default defect_patterns (`\bfix\b`), so
    // hot.rs carries a non-zero defect-heuristic count for the scan-detail and
    // hotspot assertions ([FR-GH-05]).
    for n in 0..5 {
        let msg = if n == 2 {
            "fix: crash in hot".to_string()
        } else {
            format!("hot v{n}")
        };
        commit(
            repo,
            "src/hot.rs",
            &format!("{}// rev {n}\n", complex_source("hot")),
            &msg,
        );
    }

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    tmp
}

// ── FR-GH-06 / UAT-GH-03: hotspot ranking, limits, deterministic order ───────

#[test]
fn engineered_file_ranks_first_with_deterministic_order_and_heuristic_label() {
    let tmp = engineered_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.hotspots(None, false).expect("hotspots runs");
    assert!(report.degraded.is_none(), "a real git repo is not degraded");
    assert_eq!(
        report.files.first().map(|h| h.path.as_str()),
        Some("src/hot.rs"),
        "the high-churn + high-complexity file ranks first ({:?})",
        report.files
    );
    // The mandatory heuristic label rides the report (FR-GH-05, NFR-CC-04).
    assert_eq!(report.defect_label, "heuristic");
    assert!(report.tier.contains("non-gated"), "tier label is explicit");

    // Determinism: a second evaluation at the same HEAD + config is byte-for-byte
    // identical (NFR-RA-06). (first_mine is now false → no notice drift.)
    let again = engine.hotspots(None, false).expect("hotspots reruns");
    assert_eq!(
        serde_json::to_string(&report.files).unwrap(),
        serde_json::to_string(&again.files).unwrap(),
        "ranking is byte-identical across runs"
    );
}

#[test]
fn limit_truncates_the_ranked_board() {
    let tmp = engineered_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let full = engine.hotspots(None, false).expect("hotspots runs");
    assert!(full.ranked_files >= 2, "fixture has several ranked files");

    let limited = engine
        .hotspots(Some(1), false)
        .expect("hotspots --limit 1 runs");
    assert_eq!(limited.files.len(), 1, "--limit caps the returned board");
    assert_eq!(
        limited.ranked_files, full.ranked_files,
        "total is preserved"
    );
    assert_eq!(
        limited.files[0].path, full.files[0].path,
        "the top file is stable under --limit"
    );
}

// ── FR-GH-07 / UAT-GH-02 / BR-26: gate immunity — the two-tier rule ──────────

/// The gated verdict, stripped of provenance (freshness HEAD/sha, the snapshot
/// id in `message`) that legitimately tracks the commit — what BR-26 pins as a
/// pure function of tree + config.
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
fn gate_is_byte_identical_as_the_temporal_tier_advances() {
    let tmp = engineered_repo();
    let repo = tmp.path();
    let engine = Engine::start(repo).expect("engine starts");

    // Save a baseline, then record the gated verdict.
    engine.gate(None, true, true).expect("gate --save");
    let baseline = gated_verdict(&engine.gate(None, false, true).expect("gate"));

    // (1) History advances: a temporal evaluation populates history.db and a
    // commit to a NON-indexed path moves HEAD — neither touches the graph.
    engine
        .hotspots(None, false)
        .expect("temporal eval populates history.db");
    assert!(
        repo.join(".logos/history.db").exists(),
        "the temporal read created history.db"
    );
    commit(repo, "NOTES.md", "not indexed\n", "doc-only commit");
    engine
        .hotspots(None, false)
        .expect("temporal eval after the advance");
    let after_advance = gated_verdict(&engine.gate(None, false, true).expect("gate"));
    assert_eq!(
        baseline, after_advance,
        "history advancing never moves the gate"
    );

    // (2) history.db deleted: still byte-identical (the gate never reads it).
    std::fs::remove_file(repo.join(".logos/history.db")).expect("delete history.db");
    let after_delete = gated_verdict(&engine.gate(None, false, true).expect("gate"));
    assert_eq!(
        baseline, after_delete,
        "deleting history.db changes nothing gated"
    );

    // (3) A full re-index leaves history.db intact and the gate unchanged.
    engine.hotspots(None, false).expect("repopulate history.db");
    engine.index();
    assert!(
        repo.join(".logos/history.db").exists(),
        "history.db survives a full index (FR-GH-01)"
    );
    let after_index = gated_verdict(&engine.gate(None, false, true).expect("gate"));
    assert_eq!(baseline, after_index, "a full index never moves the gate");
}

#[test]
fn a_bare_gate_never_mines_history() {
    let tmp = engineered_repo();
    let repo = tmp.path();
    // A fresh engine that only ever runs `gate` must not create history.db —
    // mining is off the gate path (FR-GH-02, BR-26).
    let engine = Engine::start(repo).expect("engine starts");
    engine.gate(None, true, true).expect("gate --save");
    engine.gate(None, false, true).expect("gate");
    assert!(
        !repo.join(".logos/history.db").exists(),
        "gate spawns no mining — history.db must not exist"
    );
}

// ── FR-GH-07: tier-labeled temporal columns in scan detail ───────────────────

/// The gated half of a scan, isolated from the additive temporal tier.
fn gated_scan(s: &logos_core::models::quality::ScanResult) -> String {
    serde_json::to_string(&serde_json::json!({
        "signal": s.signal,
        "metrics": s.metrics,
        "violations": s.violations,
        "worst_offenders": s.worst_offenders,
    }))
    .unwrap()
}

#[test]
fn scan_renders_the_labeled_temporal_tier_without_moving_gated_columns() {
    let tmp = engineered_repo();
    let git = Engine::start(tmp.path()).expect("engine starts");
    let scan_git = git.scan(true).expect("scan runs");

    // The non-gated tier is present, labeled, and never gated (NFR-CC-04, BR-26).
    assert!(
        scan_git.temporal.tier.contains("non-gated"),
        "tier label is explicit"
    );
    assert!(!scan_git.temporal.gated, "the temporal tier is never gated");
    assert_eq!(
        scan_git.temporal.defect_label, "heuristic",
        "FR-GH-05 label"
    );
    // The defect-history column is populated (one `fix:` commit touched
    // hot.rs) AND carries the heuristic label — the label is proven on a
    // non-zero value, not a trivially-zero column ([FR-GH-05], [NFR-CC-04]).
    let hot = scan_git
        .temporal
        .files
        .iter()
        .find(|f| f.path == "src/hot.rs")
        .expect("the temporal columns carry the indexed files");
    assert!(
        hot.defect_commits > 0,
        "the heuristic-labeled defect column is populated: {hot:?}"
    );

    // A byte-identical NON-git copy of the same tree degrades the temporal tier
    // to n/a, while the gated columns stay byte-identical (FR-GH-07).
    let plain = TempDir::new().unwrap();
    for rel in [
        "src/calm.rs",
        "src/complex.rs",
        "src/churny.rs",
        "src/hot.rs",
    ] {
        let src = std::fs::read(tmp.path().join(rel)).unwrap();
        let dst = plain.path().join(rel);
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::write(dst, src).unwrap();
    }
    let nogit = Engine::start(plain.path()).expect("engine starts");
    nogit.index();
    let scan_nogit = nogit.scan(true).expect("scan runs");

    assert_eq!(
        gated_scan(&scan_git),
        gated_scan(&scan_nogit),
        "gated columns are byte-identical with the temporal tier present vs n/a"
    );
    assert!(
        scan_nogit.temporal.degraded.is_some() && scan_nogit.temporal.files.is_empty(),
        "a non-git tree's temporal tier is n/a, never fabricated"
    );
}

// ── FR-GH-08 / UAT-GH-04: degraded modes — n/a + notice, exit 0 ──────────────

#[test]
fn non_git_directory_reports_na_with_a_notice() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/lib.rs"), calm_source("a")).unwrap();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();

    // The call SUCCEEDS (maps to exit 0) and reports n/a, never an error.
    let report = engine
        .hotspots(None, false)
        .expect("hotspots succeeds in a non-git dir");
    assert!(report.files.is_empty(), "no fabricated hotspots");
    assert!(report.degraded.is_some(), "the tier is degraded");
    assert!(
        report.notice.as_deref().is_some_and(|n| !n.is_empty()),
        "a one-line notice explains the degrade"
    );
}

#[test]
fn shallow_clone_reports_na_without_partial_numbers() {
    let tmp = engineered_repo();
    // A shallow (`--depth 1`) local clone over file:// — no network (UAT-GH-04).
    let shallow_parent = TempDir::new().unwrap();
    let shallow = shallow_parent.path().join("shallow");
    let url = format!("file://{}", tmp.path().display());
    sh_git(
        shallow_parent.path(),
        &["clone", "-q", "--depth", "1", &url, "shallow"],
    );

    let engine = Engine::start(&shallow).expect("engine starts");
    engine.index();
    let report = engine
        .hotspots(None, false)
        .expect("hotspots succeeds on a shallow clone");
    assert!(
        report.files.is_empty(),
        "a shallow clone shows n/a, never misleadingly low churn"
    );
    assert!(report.degraded.is_some(), "the shallow clone degrades");
    assert!(
        report.notice.as_deref().is_some_and(|n| !n.is_empty()),
        "a one-line notice explains the shallow-clone degrade (UAT-GH-04)"
    );
}
