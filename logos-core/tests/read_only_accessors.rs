//! Read-only read-model accessor contract (S-082, CR-018, ADR-28, FR-UI-03).
//!
//! The web dashboard's Health/Overview/Metrics/Hotspots/Commits views read the
//! engine through the `latest_*` accessors so a page GET reflects the **last
//! persisted** snapshot/mine and never triggers an evaluate-and-persist write.
//! These tests prove that no-write invariant end-to-end through the [`Engine`]
//! façade over a real indexed + scanned + mined git fixture: repeated `latest_*`
//! calls leave the `metric_snapshots` and `temporal_snapshots` row counts
//! byte-for-byte unchanged, the read-only figures match the persisting paths',
//! and the CLI/MCP `scan`/`hotspots`/`temporal_report` paths still persist
//! (byte-unchanged behaviour, [ADR-28] consequence note).

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

/// A small **indexed** (not yet scanned/mined) git repo: one branchy function
/// and one churny file with a defect-matching commit subject, so the metric and
/// temporal tiers both have something honest to report.
fn indexed_repo() -> TempDir {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(
        repo,
        "src/a.rs",
        "pub fn a(x: i64) -> i64 { if x > 0 { 1 } else { 0 } }\n",
        "add a",
    );
    for n in 0..3 {
        commit(
            repo,
            "src/b.rs",
            &format!("pub fn b() -> i64 {{ {n} }}\n"),
            &format!("fix: b v{n}"),
        );
    }
    Engine::start(repo).expect("engine starts").index();
    tmp
}

/// Count `metric_snapshots` rows by opening `logos.db` read-only — the
/// FR-UI-03/CR-018 snapshot-count invariant the read-only accessors must hold.
fn metric_snapshot_count(repo: &Path) -> i64 {
    let conn = Connection::open_with_flags(
        repo.join(".logos/logos.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open logos.db read-only");
    conn.query_row("SELECT count(*) FROM metric_snapshots", [], |r| r.get(0))
        .expect("count metric_snapshots")
}

/// Count `temporal_snapshots` rows; `0` when `history.db` has not been created.
fn temporal_snapshot_count(repo: &Path) -> i64 {
    let db = repo.join(".logos/history.db");
    if !db.exists() {
        return 0;
    }
    let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open history.db read-only");
    conn.query_row("SELECT count(*) FROM temporal_snapshots", [], |r| r.get(0))
        .expect("count temporal_snapshots")
}

/// Count `violations` rows — the table `check_rules` clears and rewrites.
fn violation_count(repo: &Path) -> i64 {
    let conn = Connection::open_with_flags(
        repo.join(".logos/logos.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open logos.db read-only");
    conn.query_row("SELECT count(*) FROM violations", [], |r| r.get(0))
        .expect("count violations")
}

/// The `check_run` marker as `(row count, ran_at, commit_sha, violation_count)`
/// — content, not just a row count, so "unchanged" means the readout did not
/// re-stamp a marker over the one the last real run recorded (S-313,
/// [FR-GV-21]).
fn check_run_state(repo: &Path) -> (i64, Option<(i64, Option<String>, i64)>) {
    let conn = Connection::open_with_flags(
        repo.join(".logos/logos.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open logos.db read-only");
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM check_run", [], |r| r.get(0))
        .expect("count check_run");
    let marker = conn
        .query_row(
            "SELECT ran_at, commit_sha, violation_count FROM check_run WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    (rows, marker)
}

// ── report tier: quality_readout (CR-095) ────────────────────────────────────

/// `quality_readout` computes a **fresh** signal and writes **nothing**
/// ([FR-IN-07], [CR-095]).
///
/// This is the invariant the whole accessor exists for. The report tier fires at
/// every session boundary — every start, resume and `/clear` — so a single write
/// per call would take the graph-DB write lock and append a row to the
/// [FR-GV-06] `evolution` series on every one of them, drowning the signal
/// history in session-open noise and causing exactly the lock contention the
/// readout is supposed to tolerate. `gate` (which persists, [FR-GV-09]) is
/// called first here purely to establish a baseline and a non-zero row count, so
/// the assertions below prove the readout adds nothing on top.
#[test]
fn quality_readout_computes_fresh_and_writes_nothing() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    // Establish a persisted baseline + a recorded check, then freeze the counts.
    engine.gate(None, true, true).expect("baseline saved");
    engine.check_rules(None, true).expect("check runs");
    let snapshots_before = metric_snapshot_count(tmp.path());
    let violations_before = violation_count(tmp.path());
    let marker_before = check_run_state(tmp.path());
    assert!(snapshots_before > 0, "the fixture has a persisted snapshot");
    assert_eq!(marker_before.0, 1, "the check recorded its marker (FR-GV-21)");

    // Several readouts back to back — the shape a /clear-heavy session produces.
    let first = engine.quality_readout().expect("readout");
    for _ in 0..3 {
        engine.quality_readout().expect("readout");
    }

    assert_eq!(
        metric_snapshot_count(tmp.path()),
        snapshots_before,
        "the readout must not append to the evolution series"
    );
    assert_eq!(
        violation_count(tmp.path()),
        violations_before,
        "the readout must not re-persist violations"
    );
    assert_eq!(
        check_run_state(tmp.path()),
        marker_before,
        "the readout must not stamp a run marker of its own — reading is not running \
         (FR-GV-21, ADR-49: the report leg does not write)"
    );

    // Fresh computation, not a replay of the last persisted snapshot: the signal
    // is present and agrees with what the persisting gate reports.
    let gate = engine.latest_gate().expect("read-only verdict");
    assert_eq!(
        first.signal, gate.signal,
        "the readout's freshly computed signal agrees with the recorded one on an unchanged tree"
    );
    assert!(first.baseline_signal.is_some(), "the blessed baseline is reported");
    assert_eq!(
        first.delta,
        Some(0),
        "an unchanged tree sits exactly on its baseline"
    );
    assert!(
        first.freshness.contains("assumed-fresh"),
        "the readout never reconciles, and says so: {}",
        first.freshness
    );
}

/// On a never-checked store the readout reports "nothing recorded" as `None`,
/// never as a clean bill of health — `check_rules` clears and rewrites the
/// table, so an empty one is left equally by a clean check and by no check at
/// all, and the readout must not resolve that ambiguity by guessing ([CR-095]).
#[test]
fn quality_readout_never_fabricates_a_clean_check() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let readout = engine.quality_readout().expect("readout");
    assert!(
        readout.violations.is_none(),
        "no check has run — the readout reports nothing recorded, not zero violations"
    );
    assert!(readout.violation_count.is_none());
    assert_eq!(
        violation_count(tmp.path()),
        0,
        "reading the readout recorded no violations of its own"
    );
    assert_eq!(
        check_run_state(tmp.path()),
        (0, None),
        "and left the store with no marker — no check has run (FR-GV-21)"
    );
    assert!(
        readout.check.is_none(),
        "no marker and no rows: the readout knows of no run at all, which is what \
         lets the rendering say 'no check has run' rather than a disjunction (CR-096)"
    );
}

// ── the dated readout ([CR-096], [FR-GV-21], [UAT-GV-13]) ───────────────────

/// Write a `rules.toml` that the fixture graph **breaches**, so a real
/// `check_rules` run records findings rather than a clean marker. `max_cc = 0`
/// is failed by any function with a branch, which `src/a.rs` has.
fn write_breaching_rules(repo: &Path) {
    // `.logos/rules.toml` is the only path `check_rules` loads from.
    std::fs::write(repo.join(".logos/rules.toml"), "[constraints]\nmax_cc = 0\n")
        .expect("write rules.toml");
}

/// Delete the marker row while leaving the violation rows intact — the exact
/// read-side shape of a store written **before** the S-313 migration: findings
/// that carry their own `created_at`, and no marker.
fn drop_check_run_marker(repo: &Path) {
    let conn = Connection::open(repo.join(".logos/logos.db")).expect("open logos.db");
    conn.execute("DELETE FROM check_run", [])
        .expect("delete the marker");
}

/// The repo's actual `HEAD`, read independently of the engine — so a test can
/// assert the readout reports the real sha rather than merely *a* sha. Asserting
/// `is_some()` let a hardcoded `"deadbeef…"` pass every test in this file.
fn head_sha(repo: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git is on PATH");
    assert!(out.status.success(), "git rev-parse HEAD failed");
    String::from_utf8(out.stdout).expect("utf8").trim().to_string()
}

/// The readout's own record of the last check, or a panic naming what it said
/// instead — every assertion below is about this record's content.
fn check_record(engine: &Engine) -> logos_core::models::quality::CheckRun {
    engine
        .quality_readout()
        .expect("readout")
        .check
        .expect("the readout knows of a check run")
}

/// A recorded **clean** run is reported as clean, carrying the `HEAD` and the
/// age it was measured at — the assertion [FR-IN-07] previously forbade
/// outright. The distinction is only expressible because the marker exists:
/// the `violations` table is byte-identical to a never-checked store's.
#[test]
fn a_recorded_clean_check_is_reported_as_clean_with_its_head() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.check_rules(None, true).expect("check runs");

    assert_eq!(
        violation_count(tmp.path()),
        0,
        "the fixture satisfies its (absent) rules, so the check is clean"
    );
    let record = check_record(&engine);
    assert_eq!(
        record.recorded_count,
        Some(0),
        "a recorded clean run — the state the empty table cannot express (FR-GV-21)"
    );
    assert_eq!(
        record.commit_sha.as_deref(),
        Some(head_sha(tmp.path()).as_str()),
        "the run recorded the HEAD it actually saw, not merely some sha"
    );
    assert_eq!(
        record.head_sha.as_deref(),
        Some(head_sha(tmp.path()).as_str()),
        "and the readout compares against the real current HEAD"
    );
    assert!(
        !record.tree_moved,
        "nothing was committed since the check, so the tree has not moved"
    );
    assert!(
        record.age_seconds >= 0 && record.age_seconds < 300,
        "the run just happened: {}s",
        record.age_seconds
    );
}

/// A run that finds violations records a marker whose count equals the rows it
/// wrote and equals `check`'s own output, and the readout dates those findings
/// against the `HEAD` they were measured at.
#[test]
fn a_breaching_check_records_a_count_that_matches_its_own_output() {
    let tmp = indexed_repo();
    write_breaching_rules(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.check_rules(None, true).expect("check runs");
    let reported = report.violations.len() as i64;
    assert!(reported > 0, "the max_cc = 0 contract is breached by the fixture");

    let readout = engine.quality_readout().expect("readout");
    let record = readout.check.expect("a run is known of");
    assert_eq!(
        record.recorded_count,
        Some(reported),
        "the marker's count is `logos check`'s own output (CR-096 AC)"
    );
    assert_eq!(
        readout.violation_count.map(|c| c as i64),
        Some(reported),
        "and the rows agree with it, so no disagreement warning is raised"
    );
    assert!(
        readout.warnings.iter().all(|w| !w.contains("disagrees with itself")),
        "a consistent store raises no disagreement warning: {:?}",
        readout.warnings
    );
    assert_eq!(
        record.commit_sha.as_deref(),
        Some(head_sha(tmp.path()).as_str()),
        "the findings are attributed to the HEAD they were measured at"
    );
}

/// Committing after a check moves `HEAD` without re-running it. The readout
/// then reports the **same recorded result**, marked as measured against a
/// different tree — the case a bare timestamp cannot distinguish, and the whole
/// reason `commit_sha` is recorded ([UAT-GV-13] step 3).
#[test]
fn a_commit_after_the_check_marks_the_finding_as_measured_against_another_tree() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.check_rules(None, true).expect("check runs");

    let checked_at_head = head_sha(tmp.path());
    let before = check_record(&engine);
    assert!(!before.tree_moved, "no commit yet, so the tree has not moved");

    commit(tmp.path(), "src/c.rs", "pub fn c() -> i64 { 1 }\n", "add c");
    let moved_head = head_sha(tmp.path());
    assert_ne!(moved_head, checked_at_head, "the fixture really did move HEAD");

    let after = check_record(&engine);
    assert!(
        after.tree_moved,
        "HEAD moved since the check, so the recorded result describes another tree"
    );
    assert_eq!(
        after.ran_at, before.ran_at,
        "the recorded run itself is untouched — only the tree moved"
    );
    // Both shas pinned to the real commits, not merely asserted to differ: a
    // hardcoded constant in either field satisfies `assert_ne!` forever.
    assert_eq!(
        after.commit_sha.as_deref(),
        Some(checked_at_head.as_str()),
        "the marker still names the commit the check actually ran against"
    );
    assert_eq!(
        after.head_sha.as_deref(),
        Some(moved_head.as_str()),
        "and the comparison is against the commit HEAD actually moved to"
    );
}

/// A store written before the marker migration: violation rows with no marker.
/// Its findings are dated immediately from the `created_at` the rows already
/// carry — no re-index and nothing invented — but no `HEAD` was ever recorded
/// for them, so none is claimed and no clean run can be asserted.
#[test]
fn a_pre_migration_store_dates_its_rows_without_a_marker() {
    let tmp = indexed_repo();
    write_breaching_rules(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.check_rules(None, true).expect("check runs");
    let rows_before = violation_count(tmp.path());
    assert!(rows_before > 0, "the fixture recorded findings to date");

    drop_check_run_marker(tmp.path());
    assert_eq!(check_run_state(tmp.path()).0, 0, "the marker is gone; the rows are not");

    let readout = engine.quality_readout().expect("readout");
    let record = readout.check.expect("the rows still date their own run");
    assert_eq!(
        record.recorded_count, None,
        "no marker, so no recorded total — and no clean assertion is possible"
    );
    assert_eq!(record.commit_sha, None, "no HEAD was recorded for these rows");
    assert!(!record.tree_moved, "a comparison needs a recorded HEAD to compare against");
    assert!(record.ran_at > 0, "the rows' own created_at dates the run");
    assert_eq!(
        violation_count(tmp.path()),
        rows_before,
        "and reading it re-persisted nothing"
    );
}

/// The marker and the rows can only disagree on a store edited underneath
/// Logos. The readout **reports** that rather than resolving it — silently
/// preferring the marker would let a stale clean marker over a table holding
/// findings render as a pass, the precise fabrication [BR-41] forbids.
#[test]
fn a_marker_disagreeing_with_its_rows_is_warned_about_not_resolved() {
    let tmp = indexed_repo();
    write_breaching_rules(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.check_rules(None, true).expect("check runs");
    assert!(violation_count(tmp.path()) > 0, "findings are stored");

    // Forge a clean marker over a table that holds findings.
    let conn = Connection::open(tmp.path().join(".logos/logos.db")).expect("open logos.db");
    conn.execute("UPDATE check_run SET violation_count = 0 WHERE id = 1", [])
        .expect("forge a clean marker");

    let readout = engine.quality_readout().expect("readout");
    assert!(
        readout.warnings.iter().any(|w| w.contains("disagrees with itself")),
        "the disagreement is surfaced: {:?}",
        readout.warnings
    );
    assert!(
        readout.violation_count.is_some_and(|c| c > 0),
        "and the stored findings are still reported, not the flattering marker"
    );
}

/// A corrupted `ran_at` degrades to a named age — it never panics. The report
/// tier's whole contract is that it reports and never blocks ([FR-GV-05]), and
/// a crash on read is the most extreme form of blocking there is. Unchecked
/// `now - ran_at` panicked here in a debug build and wrapped silently in a
/// release one, so the two builds disagreed about what a corrupt store does.
#[test]
fn a_corrupted_run_timestamp_is_named_never_panicked_on() {
    let tmp = indexed_repo();
    write_breaching_rules(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.check_rules(None, true).expect("check runs");

    let conn = Connection::open(tmp.path().join(".logos/logos.db")).expect("open logos.db");
    conn.execute("UPDATE check_run SET ran_at = ?1 WHERE id = 1", [i64::MIN])
        .expect("corrupt the marker's timestamp");

    // The assertion is that this returns at all — the pre-fix code panicked.
    let readout = engine.quality_readout().expect("a corrupt timestamp is not an error");
    let record = readout.check.expect("the run is still known of");
    assert_eq!(
        record.age_seconds,
        i64::MAX,
        "the age saturates rather than overflowing"
    );

    // And the rendering names it rather than counting a hundred million days.
    let payload = engine.quality_report_hook_payload().expect("payload renders");
    let context = payload.hook_specific_output.additional_context;
    assert!(
        context.contains("implausibly old"),
        "the impossible age is named: {context}"
    );
    assert!(
        !context.contains("days ago"),
        "and never rendered as a confident age: {context}"
    );
}

/// [UAT-GV-13] step 5: repeated readouts leave the marker byte-identical. The
/// [CR-095] row-counting guard, extended to the table [CR-096] added — reading
/// a run is not running one.
#[test]
fn repeated_readouts_leave_the_marker_unchanged() {
    let tmp = indexed_repo();
    write_breaching_rules(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.check_rules(None, true).expect("check runs");

    let marker_before = check_run_state(tmp.path());
    let violations_before = violation_count(tmp.path());
    let snapshots_before = metric_snapshot_count(tmp.path());
    assert_eq!(marker_before.0, 1, "exactly one marker row, upserted not appended (BR-40)");

    for _ in 0..5 {
        engine.quality_readout().expect("readout");
        engine.quality_report_hook_payload().expect("hook payload");
    }

    assert_eq!(
        check_run_state(tmp.path()),
        marker_before,
        "ran_at, commit_sha and violation_count are all untouched by reading"
    );
    assert_eq!(violation_count(tmp.path()), violations_before);
    assert_eq!(metric_snapshot_count(tmp.path()), snapshots_before);
}

// ── metric side: latest_metrics / latest_scan / latest_gate ──────────────────

/// On a never-`scan`-ned store the read-only accessors honestly report "no
/// snapshot" — `latest_metrics` is `None`, `latest_scan` carries the empty
/// sentinel, `latest_gate` is an informational pass naming the producing command
/// — and **reading them writes no snapshot** ([NFR-CC-04], [ADR-28]).
#[test]
fn never_scanned_store_reads_empty_and_writes_nothing() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    assert!(
        engine.latest_metrics().unwrap().is_none(),
        "a never-scanned store has no persisted snapshot"
    );
    let scan = engine.latest_scan().unwrap();
    assert!(scan.metrics.empty, "no snapshot → the empty sentinel, never zeros");
    assert!(scan.signal.is_none(), "no fabricated signal");
    let gate = engine.latest_gate().unwrap();
    assert!(gate.signal.is_none());
    assert!(gate.passed, "no snapshot cannot regress — informational pass");
    assert!(
        gate.message.contains("scan"),
        "the verdict names the producing command: {}",
        gate.message
    );

    // The temporal read-only twins on a never-mined store: a headless `n/a`
    // report and an empty board, computed without mining or persisting
    // ([NFR-RA-05], [ADR-28]).
    let temporal = engine.latest_temporal_report().unwrap();
    assert!(temporal.head_sha.is_none(), "never-mined → headless n/a report");
    assert!(temporal.files.is_empty(), "no in-window facts → no files, never fabricated");
    assert!(
        engine.latest_hotspots(Some(50), false, false).unwrap().files.is_empty(),
        "never-mined → empty hotspot board"
    );

    assert_eq!(
        metric_snapshot_count(tmp.path()),
        0,
        "reading the read-only metric accessors created no snapshot"
    );
    assert_eq!(
        temporal_snapshot_count(tmp.path()),
        0,
        "reading the read-only temporal accessors mined and appended nothing"
    );
}

/// `latest_metrics` returns the last persisted snapshot's full breakdown, its
/// figures trace to that snapshot, and calling the read-only accessors
/// repeatedly leaves the `metric_snapshots` count unchanged ([FR-UI-03] AC,
/// [ADR-28]).
#[test]
fn latest_metrics_reflects_last_snapshot_without_writing() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let scanned = engine.scan(false).expect("scan persists one snapshot");
    let before = metric_snapshot_count(tmp.path());
    assert_eq!(before, 1, "scan wrote exactly one snapshot");

    let latest = engine
        .latest_metrics()
        .unwrap()
        .expect("a snapshot now exists");
    // Every figure traces to the persisted snapshot ([NFR-RA-05]): the read-only
    // breakdown is byte-for-byte the fresh-computed one the scan persisted. This
    // also guards the 29-column reader against any SELECT column-offset bug — a
    // swapped dimension would diverge here.
    assert_eq!(
        serde_json::to_string(&latest).unwrap(),
        serde_json::to_string(&scanned.metrics).unwrap(),
        "latest_metrics reconstructs the persisted snapshot exactly"
    );
    // The Cohesion/Focus applicability drop-out is preserved, never fabricated
    // as a zero ([ADR-21], [NFR-CC-04]): a class-less Rust repo drops both.
    assert_eq!(
        latest.cohesion.is_none(),
        scanned.metrics.cohesion.is_none(),
        "cohesion drop-out preserved across the read-only round-trip"
    );

    // latest_scan composes the same metrics; its signal matches.
    assert_eq!(engine.latest_scan().unwrap().signal, scanned.metrics.aggregate_signal);

    // Repeated read-only calls write nothing.
    for _ in 0..3 {
        engine.latest_metrics().unwrap();
        engine.latest_scan().unwrap();
        engine.latest_gate().unwrap();
    }
    assert_eq!(
        metric_snapshot_count(tmp.path()),
        before,
        "no read-only accessor ever appended a snapshot"
    );
}

/// The read-only verdict mirrors a non-saving `gate` comparison: with a saved
/// baseline at the same tree it is a PASS holding the baseline, and reading it
/// persists nothing extra ([ADR-28]).
#[test]
fn latest_gate_compares_to_baseline_without_writing() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    engine.gate(None, true, true).expect("gate --save sets a baseline");
    let after_save = metric_snapshot_count(tmp.path());

    let verdict = engine.latest_gate().expect("read-only verdict");
    assert!(verdict.passed, "the snapshot holds its own freshly-saved baseline");
    assert_eq!(verdict.signal, verdict.baseline_signal, "current == baseline");
    assert_eq!(
        metric_snapshot_count(tmp.path()),
        after_save,
        "the read-only verdict appended no snapshot"
    );
}

/// A scanned store with **no saved baseline** (the common new-user state) yields
/// an informational pass naming the producing command, never a fabricated FAIL —
/// the distinct no-baseline branch of the read-only verdict ([ADR-28]).
#[test]
fn latest_gate_without_saved_baseline_is_informational_pass() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    engine.scan(false).expect("scan persists a snapshot but saves no baseline");
    let after_scan = metric_snapshot_count(tmp.path());

    let verdict = engine.latest_gate().expect("read-only verdict");
    assert!(verdict.passed, "no baseline cannot regress — informational pass");
    assert!(verdict.baseline_signal.is_none(), "there is no baseline to compare against");
    assert!(
        verdict.message.contains("no baseline"),
        "the verdict names the missing baseline: {}",
        verdict.message
    );
    assert_eq!(
        metric_snapshot_count(tmp.path()),
        after_scan,
        "the read-only verdict appended no snapshot"
    );
}

/// CLI/MCP `scan` keeps persisting on every call — the read-only seam is
/// additive and leaves the evaluate-and-persist path byte-unchanged ([ADR-28]).
#[test]
fn cli_scan_still_persists_each_call() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.scan(false).unwrap();
    engine.scan(false).unwrap();
    assert_eq!(
        metric_snapshot_count(tmp.path()),
        2,
        "scan persists a snapshot every call, as before"
    );
}

// ── temporal side: latest_temporal_report / latest_hotspots ──────────────────

/// The read-only temporal accessors recompute from the last-mined facts and
/// append **no** `temporal_snapshots` row, while the persisting
/// `temporal_report`/`hotspots` still append one — and the read-only figures
/// match the persisting board ([ADR-28], [NFR-RA-06]).
#[test]
fn latest_temporal_reads_never_append_a_snapshot() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    // Prime the mine the way the CLI does — this is allowed to persist.
    engine
        .hotspots(None, false, false)
        .expect("hotspots mines + appends one temporal snapshot");
    let primed = temporal_snapshot_count(tmp.path());
    assert!(primed >= 1, "the CLI hotspots read appended a temporal snapshot");

    // The read-only twin reflects the same per-file figures …
    let read_only = engine.latest_temporal_report().unwrap();
    let persisting = engine.temporal_report().unwrap(); // appends another, on purpose
    assert_eq!(
        serde_json::to_string(&read_only.files).unwrap(),
        serde_json::to_string(&persisting.files).unwrap(),
        "read-only temporal figures match the persisting report"
    );
    let after_persisting = temporal_snapshot_count(tmp.path());
    assert_eq!(
        after_persisting,
        primed + 1,
        "the persisting temporal_report appended exactly one"
    );

    // … and repeated read-only temporal/hotspot reads append nothing.
    for _ in 0..3 {
        engine.latest_temporal_report().unwrap();
        engine.latest_hotspots(Some(50), false, false).unwrap();
        engine.latest_hotspots(Some(20), true, false).unwrap();
    }
    assert_eq!(
        temporal_snapshot_count(tmp.path()),
        after_persisting,
        "no read-only temporal accessor ever appended a snapshot"
    );
}

/// The read-only hotspot board ranks identically to the persisting one at the
/// same HEAD — the dashboard reflects the last mine without re-mining ([ADR-28]).
#[test]
fn latest_hotspots_matches_the_persisting_board() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let persisting = engine.hotspots(Some(50), false, false).unwrap();
    let read_only = engine.latest_hotspots(Some(50), false, false).unwrap();
    assert_eq!(
        serde_json::to_string(&persisting.files).unwrap(),
        serde_json::to_string(&read_only.files).unwrap(),
        "the read-only board ranks identically to the mined board"
    );
    assert_eq!(persisting.ranked_files, read_only.ranked_files);
}

// ── language composition: FR-UI-10 / CR-021 / ADR-28 ─────────────────────────

/// Read the canonical store's bytes — the strongest no-write invariant: a
/// read-only accessor must leave `logos.db` byte-for-byte identical.
fn logos_db_bytes(repo: &Path) -> Vec<u8> {
    std::fs::read(repo.join(".logos/logos.db")).expect("read logos.db")
}

/// An un-indexed root (no `logos.db`) returns an empty composition — the
/// Dashboard's honest empty state, never an error ([FR-UI-10], [NFR-CC-04]).
#[test]
fn language_composition_on_unindexed_root_is_empty() {
    let tmp = TempDir::new().expect("temp root");
    sh_git(tmp.path(), &["init", "-q", "-b", "main"]);
    commit(tmp.path(), "src/a.rs", "pub fn a() {}\n", "add a, never indexed");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(
        engine.language_composition().unwrap().languages.is_empty(),
        "a never-indexed root has no indexed nodes → empty composition"
    );
}

/// On a Rust-only indexed graph the composition reports exactly `rust` with its
/// node/file counts; a registered-but-unused grammar (e.g. `python`) is absent,
/// and repeated reads leave `logos.db` byte-for-byte unchanged ([FR-UI-10],
/// [FR-UI-03], [ADR-28]).
#[test]
fn language_composition_reflects_the_indexed_graph_without_writing() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let comp = engine.language_composition().unwrap();
    assert_eq!(comp.languages.len(), 1, "the fixture indexes only rust files");
    let rust = &comp.languages[0];
    assert_eq!(rust.language, "rust");
    assert_eq!(rust.files, 2, "src/a.rs and src/b.rs both carry nodes");
    assert!(rust.nodes > 0, "every count is a graph fact, not fabricated");
    assert!(
        !comp.languages.iter().any(|e| e.language == "python"),
        "a registered-but-unused grammar never appears (distinct from `languages`)"
    );

    // The composition lists only languages the project actually uses, unlike the
    // registry listing which surfaces every loaded grammar (FR-PL-06).
    let registered = engine.languages();
    assert!(
        registered.languages.len() > comp.languages.len(),
        "more grammars are registered than the project uses"
    );
    // A healthy cached-registry load (the `Engine::start` path) must never carry
    // a load failure (S-340) — pins the common path so a future edit to
    // `Engine::languages_from` can't silently regress it.
    assert!(
        registered.load_error.is_none(),
        "a healthy load must not report load_error, got {:?}",
        registered.load_error
    );

    // Repeated reads mutate no store: logos.db is byte-identical and no metric
    // snapshot is appended ([FR-UI-03] AC, [ADR-28]).
    let before = logos_db_bytes(tmp.path());
    let snapshots_before = metric_snapshot_count(tmp.path());
    for _ in 0..3 {
        engine.language_composition().unwrap();
    }
    assert_eq!(
        logos_db_bytes(tmp.path()),
        before,
        "reading the composition left logos.db byte-for-byte unchanged"
    );
    assert_eq!(
        metric_snapshot_count(tmp.path()),
        snapshots_before,
        "reading the composition appended no snapshot"
    );
}
