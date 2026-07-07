//! Integration tests for the symbol-level **reachability × runtime-coverage
//! cross** read-model (S-141, [FR-UI-17], [CR-036]), exercised end-to-end through
//! the [`Engine`] façade over a real git fixture + a real index + a real coverage
//! ingest.
//!
//! These prove, against actual `git` + a real graph + a real coverage store:
//! - every non-test function/method gets a `(reachable_from_test,
//!   runtime_exec_fraction)` pair and a Q1–Q4 classification matching
//!   hand-computed expectations ([FR-UI-17]);
//! - a symbol with no instrumented line in its span (or no fresh coverage) is
//!   `n/a` on the runtime axis — never a guessed `0` ([NFR-RA-05]);
//! - the read-model is deterministic at a fixed HEAD + store state ([NFR-RA-06]);
//! - reading the cross persists nothing — `gate`/`scan` stay byte-identical and
//!   no snapshot is appended (write-free on read, [BR-28], [ADR-28]).
//!
//! The pure attribution + classification arithmetic is unit-tested inside
//! `history::coverage::cross`; these prove the cross-store join (graph spans ×
//! coverage lines) and the api-layer reachability supply against the real stack.

#![cfg(feature = "lang-rust")]

use std::path::Path;
use std::process::Command;

use logos_core::history::Quadrant;
use logos_core::Engine;
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;

// ── git fixture helpers (mirroring tests/coverage_surface.rs) ────────────────

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

/// A `src/lib.rs` with five production functions at **known start lines** and two
/// tests that call the first two — one production function per quadrant plus an
/// uninstrumented one for the `n/a` case. Start lines (1-based):
///
/// Quadrant labels are the Gartner numbering (CR-040): the `qN_*` fn names keep
/// their pre-remap suffixes (fixture identifiers), but the cells they fall in are
/// the renumbered labels (e.g. reachable+executed = trust = **Q4**).
///
/// | line | fn | reachable | runtime |
/// |------|----|-----------|---------|
/// | 1  | `q1_reached_executed`  | yes (called by `t_q1`) | executed → **Q4** (trust) |
/// | 4  | `q2_reached_dead`      | yes (called by `t_q2`) | 0% → **Q2** (dead edge) |
/// | 7  | `q3_orphan_executed`   | no                     | executed → **Q1** (false-green) |
/// | 10 | `q4_orphan_dead`       | no                     | 0% → **Q3** (true gap) |
/// | 13 | `na_no_coverage_lines` | no                     | no instrumented line → **n/a** |
const FIXTURE: &str = "\
pub fn q1_reached_executed() -> i64 {
    1
}
pub fn q2_reached_dead() -> i64 {
    2
}
pub fn q3_orphan_executed() -> i64 {
    3
}
pub fn q4_orphan_dead() -> i64 {
    4
}
pub fn na_no_coverage_lines() -> i64 {
    5
}

#[test]
fn t_q1() {
    let _ = q1_reached_executed();
}

#[test]
fn t_q2() {
    let _ = q2_reached_dead();
}
";

/// One DA record per production function's **signature line** — guaranteed to
/// fall inside that function's `[start, end]` span and no other's. `na_*` gets no
/// record, so its span carries no instrumented line ([NFR-RA-05] `n/a`).
const REPORT: &str = "\
TN:suite
SF:src/lib.rs
DA:1,5
DA:4,0
DA:7,2
DA:10,0
end_of_record
";

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

/// Build an indexed fixture with fresh coverage and return the started engine.
fn indexed_fixture(repo: &Path) -> Engine {
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/lib.rs", FIXTURE, "add lib");
    let engine = Engine::start(repo).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "clean index: {:?}", result.warnings);
    let path = repo.join("coverage.info");
    std::fs::write(&path, REPORT).unwrap();
    let summary = engine
        .coverage_ingest(&path, None)
        .expect("coverage ingest succeeds");
    assert_eq!(summary.matched_files, 1, "src/lib.rs bound: {summary:?}");
    engine
}

// ── FR-UI-17: Q1–Q4 classification against hand-computed expectations ────────

#[test]
fn every_function_lands_in_its_hand_computed_quadrant() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    let engine = indexed_fixture(repo);

    // Sanity: reachability is exactly as the fixture intends, via the SAME signal
    // the cross consumes (test_gaps BFS) — q1/q2 reached, the rest are gaps.
    let gaps = engine.test_gaps(None, true).expect("test_gaps");
    let untested: std::collections::HashSet<&str> =
        gaps.untested.iter().map(|g| g.name.as_str()).collect();
    assert!(
        !untested.contains("q1_reached_executed") && !untested.contains("q2_reached_dead"),
        "the test-called functions are reachable: {untested:?}"
    );
    assert!(
        untested.contains("q3_orphan_executed") && untested.contains("q4_orphan_dead"),
        "the un-called functions are gaps: {untested:?}"
    );

    let report = engine.coverage_cross().expect("coverage cross");
    assert!(report.has_fresh_coverage, "the fixture has fresh coverage");
    assert!(report.notice.is_none(), "a populated store has no empty-state notice");
    assert_eq!(
        report.head_sha,
        engine.coverage_status().unwrap().head_sha,
        "the cross is anchored to the same snapshot HEAD as coverage status"
    );

    let by_name = |name: &str| {
        report
            .symbols
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} present in the cross: {:?}", report.symbols))
            .clone()
    };

    // Gartner labels ([CR-040]): the underlying classification is unchanged, only
    // the quadrant number each `(reachable, executed)` pair carries flipped.
    let q1 = by_name("q1_reached_executed"); // reachable + executed → trust (Q4)
    assert!(q1.reachable_from_test);
    assert_eq!(q1.runtime_exec_bp, Some(10_000), "1/1 instrumented line covered");
    assert_eq!(q1.quadrant, Some(Quadrant::Q4));

    let q2 = by_name("q2_reached_dead"); // reachable + 0% → dead edge (Q2)
    assert!(q2.reachable_from_test);
    assert_eq!(q2.runtime_exec_bp, Some(0), "instrumented but unexecuted (measured 0)");
    assert_eq!(q2.quadrant, Some(Quadrant::Q2));

    let q3 = by_name("q3_orphan_executed"); // unreachable + executed → false-green (Q1)
    assert!(!q3.reachable_from_test);
    assert_eq!(q3.runtime_exec_bp, Some(10_000));
    assert_eq!(q3.quadrant, Some(Quadrant::Q1));

    let q4 = by_name("q4_orphan_dead"); // unreachable + 0% → true gap (Q3)
    assert!(!q4.reachable_from_test);
    assert_eq!(q4.runtime_exec_bp, Some(0));
    assert_eq!(q4.quadrant, Some(Quadrant::Q3));

    // The uninstrumented function: n/a on the runtime axis, never a guessed 0.
    let na = by_name("na_no_coverage_lines");
    assert_eq!(na.runtime_exec_bp, None, "no instrumented line in span → n/a (NFR-RA-05)");
    assert_eq!(na.quadrant, None, "no runtime axis → no quadrant");

    // The tallies match the five hand-placed symbols.
    assert_eq!(report.totals.q1, 1);
    assert_eq!(report.totals.q2, 1);
    assert_eq!(report.totals.q3, 1);
    assert_eq!(report.totals.q4, 1);
    assert_eq!(report.totals.na_runtime, 1);
    assert_eq!(report.totals.total, 5);

    // Test functions are never symbols in the cross.
    assert!(
        !report.symbols.iter().any(|s| s.name == "t_q1" || s.name == "t_q2"),
        "test nodes are excluded from the cross"
    );
}

// ── FR-UI-17 / NFR-RA-05: no fresh coverage → n/a + the empty-state notice ───

#[test]
fn without_coverage_every_symbol_is_na_on_the_runtime_axis() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/lib.rs", FIXTURE, "add lib");
    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    // No ingest at all: the runtime axis is n/a for everyone, the reachability
    // axis still stands, and the honest empty-state notice is set (NFR-CC-04).
    let report = engine.coverage_cross().expect("coverage cross");
    assert!(!report.symbols.is_empty(), "the symbols (Y axis) still exist");
    assert!(
        report.symbols.iter().all(|s| s.runtime_exec_bp.is_none() && s.quadrant.is_none()),
        "no coverage → no runtime axis for any symbol"
    );
    assert!(
        report.symbols.iter().any(|s| s.reachable_from_test),
        "reachability is still computed without coverage"
    );
    assert!(!report.has_fresh_coverage);
    assert!(report.notice.is_some(), "the empty-state notice names the ingest command");
    assert_eq!(report.totals.na_runtime, report.totals.total);
}

// ── NFR-RA-06: determinism at a fixed HEAD + store state ─────────────────────

#[test]
fn the_cross_is_deterministic_at_a_fixed_head_and_store() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    let engine = indexed_fixture(repo);

    let a = engine.coverage_cross().expect("cross #1");
    let b = engine.coverage_cross().expect("cross #2");
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap(),
        "byte-identical output at a fixed HEAD + store state (NFR-RA-06)"
    );
}

// ── BR-28 / ADR-28: write-free on read; the gate is unmoved by the cross ─────

#[test]
fn reading_the_cross_persists_nothing_and_never_moves_the_gate() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    let engine = indexed_fixture(repo);

    // Baseline gate verdict on the indexed tree (the BR-28 pure-function pins).
    engine.gate(None, true, true).expect("gate --save");
    let gated = |e: &Engine| {
        let g = e.gate(None, false, true).expect("gate");
        serde_json::to_string(&serde_json::json!({
            "passed": g.passed,
            "signal": g.signal,
            "baseline_signal": g.baseline_signal,
            "regressions": g.regressions,
            "test_function_count": g.test_function_count,
        }))
        .unwrap()
    };
    let baseline = gated(&engine);
    let snapshots_before = coverage_snapshot_count(repo);

    // Reading the cross repeatedly appends no snapshot and never moves the gate.
    for _ in 0..3 {
        engine.coverage_cross().expect("coverage cross");
    }
    assert_eq!(
        coverage_snapshot_count(repo),
        snapshots_before,
        "the cross is write-free on read — no coverage snapshot appended (ADR-28)"
    );
    assert_eq!(
        baseline,
        gated(&engine),
        "the cross is never read by the gated path — gate is byte-identical (BR-28)"
    );
}

// ── FR-UI-17 / FR-CV-05: an all-stale snapshot → n/a, a partial (not empty) state

#[test]
fn all_stale_coverage_yields_na_without_the_empty_state_notice() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    let engine = indexed_fixture(repo);

    // Break src/lib.rs's content hash so the ingested snapshot goes fully stale.
    // No re-index: the graph keeps its symbols; only coverage freshness flips, so
    // this exercises the `read_latest_line_hits` Some(view)-with-empty-fresh_files
    // branch end-to-end — distinct from the no-coverage-ever path.
    std::fs::write(
        repo.join("src/lib.rs"),
        format!("{FIXTURE}// edited after ingest\n"),
    )
    .unwrap();

    let report = engine.coverage_cross().expect("coverage cross");
    assert!(!report.has_fresh_coverage, "the only covered file is now stale");
    // A populated-but-stale snapshot is a PARTIAL state, not an empty one: the
    // provenance (head_sha) is still shown and the no-coverage notice stays unset.
    assert!(
        report.notice.is_none(),
        "the empty-state notice is for no-coverage-ever only, not for staleness"
    );
    assert!(
        report.head_sha.is_some(),
        "stale coverage still carries its snapshot provenance"
    );
    assert!(
        report
            .symbols
            .iter()
            .all(|s| s.runtime_exec_bp.is_none() && s.quadrant.is_none()),
        "stale coverage attributes no runtime fraction (FR-CV-05, NFR-RA-05)"
    );
    assert!(report.totals.total > 0, "the symbols (Y axis) still exist");
    assert_eq!(
        report.totals.na_runtime, report.totals.total,
        "every symbol is n/a on the runtime axis when coverage is all-stale"
    );
}
