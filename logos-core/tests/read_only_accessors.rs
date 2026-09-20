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

/// The `check_run` marker as `(row count, every column as text)` — content,
/// not just a row count, so "unchanged" means the readout did not re-stamp a
/// marker over the one the last real run recorded (S-313, [FR-GV-21]).
///
/// `SELECT *` with a generic row reader, deliberately, rather than a column
/// list. An enumerated projection is the thing that goes stale: this helper
/// listed `(ran_at, commit_sha, violation_count)` and silently stopped covering
/// the marker when migration 21 (S-437, [CR-140]) widened it with the evaluated
/// set — the guard would have kept passing while three new fields went
/// unwatched. Reading whatever columns the table has keeps it honest as the
/// marker widens again ([CR-140] CRA-08). The same reasoning, and the same
/// shape, as `read_table` in `graph_store::migrate`'s tests.
///
/// Each value is rendered with its storage class, so a NULL is distinguishable
/// from the string `"NULL"` — which matters precisely here, since NULL in the
/// evaluated-set columns means "written before migration 21".
fn check_run_state(repo: &Path) -> (i64, Option<Vec<(String, String)>>) {
    let conn = Connection::open_with_flags(
        repo.join(".logos/logos.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open logos.db read-only");
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM check_run", [], |r| r.get(0))
        .expect("count check_run");
    let mut stmt = conn
        .prepare("SELECT * FROM check_run WHERE id = 1")
        .expect("prepare marker read");
    let columns = stmt.column_count();
    let names: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let marker = stmt
        .query_row([], |r| {
            (0..columns)
                .map(|i| {
                    let rendered = match r.get_ref(i)? {
                        rusqlite::types::ValueRef::Null => "NULL".to_string(),
                        rusqlite::types::ValueRef::Integer(v) => format!("i:{v}"),
                        rusqlite::types::ValueRef::Real(v) => format!("r:{v}"),
                        rusqlite::types::ValueRef::Text(v) => {
                            format!("t:{}", String::from_utf8_lossy(v))
                        }
                        rusqlite::types::ValueRef::Blob(v) => format!("b:{v:?}"),
                    };
                    Ok((names[i].clone(), rendered))
                })
                .collect::<rusqlite::Result<Vec<(String, String)>>>()
        })
        .ok();
    (rows, marker)
}

/// The columns `check_run_state` must be watching for the guards above to mean
/// what they say ([CR-140] CRA-08).
///
/// Without this, narrowing the helper's projection would **silently** shrink
/// what "the marker is unchanged" covers and every guard would keep passing —
/// the failure mode a widened table invites, and the reason the helper reads
/// `SELECT *`. This asserts the coverage rather than trusting it.
const MARKER_COLUMNS_UNDER_GUARD: [&str; 6] = [
    "ran_at",
    "commit_sha",
    "violation_count",
    "checked_rules",
    "rules_present",
    "operation",
];

/// Assert the captured marker genuinely covers every column the guards claim.
fn assert_marker_coverage(state: &(i64, Option<Vec<(String, String)>>)) {
    let captured: Vec<&str> = state
        .1
        .as_ref()
        .expect("the fixture recorded a marker")
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    for column in MARKER_COLUMNS_UNDER_GUARD {
        assert!(
            captured.contains(&column),
            "the write-free guard must watch `{column}` — captured {captured:?} \
             (CR-140 CRA-08: the guard is extended to the widened marker table)"
        );
    }
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
    assert_marker_coverage(&marker_before);

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
    assert_marker_coverage(&marker_before);

    for _ in 0..5 {
        engine.quality_readout().expect("readout");
        engine.quality_report_hook_payload().expect("hook payload");
    }

    assert_eq!(
        check_run_state(tmp.path()),
        marker_before,
        "every column of the marker — the evaluated set included — is untouched by \
         reading (FR-GV-21, CR-140 CRA-08)"
    );
    assert_eq!(violation_count(tmp.path()), violations_before);
    assert_eq!(metric_snapshot_count(tmp.path()), snapshots_before);
}

// ── the rendered evaluated set ([CR-140] §3.2, [FR-IN-07], S-437 T2) ────────
//
// The four states end to end, through a REAL store and the REAL rendering —
// `readout.rs`'s unit tests pin the wording against constructed read-models,
// and these pin that a genuine `check_rules` run over each fixture reaches the
// arm it should. Asserted against the rendered `additionalContext`, never
// against the marker row, which is T1's own falsifiability surface.

/// A `.logos/rules.toml` that exists and authors **no** rules — every example
/// commented out, which is exactly what `logos init`'s default template writes.
/// The `notes/sprint-test-72.md` Finding 1a second transcript reached this
/// state through `logos init` rather than by inventing a shape.
fn write_contract_authoring_zero_rules(repo: &Path) {
    std::fs::write(
        repo.join(".logos/rules.toml"),
        "# Everything is optional: an omitted constraint is simply not enforced.\n\
         # [constraints]\n\
         # max_cc = 15\n",
    )
    .expect("write rules.toml");
}

/// A contract authoring rules the fixture **satisfies** — the only genuinely
/// clean state, and the one that must carry its denominator.
fn write_satisfied_rules(repo: &Path) {
    std::fs::write(
        repo.join(".logos/rules.toml"),
        "[constraints]\nmax_cc = 100\nmax_fn_lines = 200\n",
    )
    .expect("write rules.toml");
}

/// Blank the three evaluated-set columns, leaving the rest of the marker — the
/// exact read-side shape of a store whose marker was written **before**
/// migration 21, which is what every existing install has.
fn blank_the_evaluated_set(repo: &Path) {
    let conn = Connection::open(repo.join(".logos/logos.db")).expect("open logos.db");
    conn.execute(
        "UPDATE check_run SET checked_rules = NULL, rules_present = NULL, operation = NULL",
        [],
    )
    .expect("blank the evaluated set");
}

/// The `rule violations:` line of the rendered session-start context — the
/// surface [CR-140] CRA-01 requires these assertions be made against.
fn rendered_violations_line(engine: &Engine) -> String {
    let context = engine
        .quality_report_hook_payload()
        .expect("payload renders")
        .hook_specific_output
        .additional_context;
    context
        .lines()
        .find(|line| line.trim_start().starts_with("rule violations:"))
        .unwrap_or_else(|| panic!("the readout carries a violations line: {context}"))
        .trim()
        .to_string()
}

/// **Arm (a), end to end** — `notes/sprint-test-72.md` Finding 1a: a real
/// `check_rules` run against a store with no contract exits `4` printing
/// *"nothing was evaluated"*, leaves a marker, and used to render that marker
/// as ``0 — clean `logos check```. It now names the state.
#[test]
fn a_real_run_with_no_contract_renders_as_that_state_not_as_clean() {
    let tmp = indexed_repo();
    assert!(
        !tmp.path().join(".logos/rules.toml").exists(),
        "the fixture must genuinely have no contract, or this proves nothing"
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.check_rules(None, true).expect("check runs");

    let line = rendered_violations_line(&engine);
    assert!(
        line.contains("no rules contract authored"),
        "the vacuous state is named (CRA-01): {line}"
    );
    assert!(
        !line.contains("clean"),
        "and the same binary no longer says 'nothing was evaluated' and 'clean' \
         in one breath (FR-GV-03): {line}"
    );
    assert!(
        !line.contains("rule violations: 0"),
        "nor renders it as a bare zero (FR-GV-22): {line}"
    );
}

/// **Arm (b), end to end** — a present contract authoring zero rules, the
/// `logos init` default. Exit `0`, and still not a pass.
#[test]
fn a_real_run_over_a_contract_authoring_zero_rules_is_its_own_state() {
    let tmp = indexed_repo();
    write_contract_authoring_zero_rules(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let report = engine.check_rules(None, true).expect("check runs");
    assert!(
        report.rules_present,
        "the fixture must genuinely carry a contract, or this is arm (a) again"
    );
    assert_eq!(
        report.checked_rules, 0,
        "authoring no rules must genuinely evaluate none: {report:?}"
    );

    let line = rendered_violations_line(&engine);
    assert!(
        line.contains("a rules contract present, authoring no rules"),
        "the ordinary state of a fresh project is named (CRA-02): {line}"
    );
    assert!(
        !line.contains("no rules contract authored"),
        "and never collapsed into the unconfigured state: {line}"
    );
    assert!(!line.contains("clean"), "a denominator of zero is no pass: {line}");
}

/// **Arm (c), end to end** — a real clean run over N > 0 rules states the clean
/// result **with N**, read from the report rather than hardcoded so the two
/// cannot drift ([NFR-CC-04], CRA-03).
#[test]
fn a_real_clean_run_states_its_denominator() {
    let tmp = indexed_repo();
    write_satisfied_rules(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let report = engine.check_rules(None, true).expect("check runs");
    assert!(report.violations.is_empty(), "the fixture passes its own contract");
    assert!(
        report.checked_rules > 0,
        "the contract must genuinely evaluate rules, or this is a vacuous arm"
    );

    let line = rendered_violations_line(&engine);
    assert!(
        line.contains(&format!("0 of {} rule(s) evaluated", report.checked_rules)),
        "the clean figure carries the denominator the run reported: {line}"
    );
    assert!(line.contains("clean"), "and this one IS clean: {line}");
}

/// **Arm (d), end to end** — a marker written **before** migration 21. An
/// existing install is the common case, and rendering its absent evaluated set
/// favourably is this story's own subject ([CR-140] CRA-05).
///
/// Built by blanking the three columns on a store whose run was genuinely
/// clean over a real contract — so the store is exactly one that WOULD render
/// as clean if the rendering fell back to zero, which is the mutation this
/// pins.
#[test]
fn a_pre_migration_marker_renders_as_unknown_never_clean_and_never_zero() {
    let tmp = indexed_repo();
    write_satisfied_rules(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.check_rules(None, true).expect("check runs");
    assert!(
        rendered_violations_line(&engine).contains("clean"),
        "the store must read as clean BEFORE the columns are blanked, or the \
         assertion below is about nothing"
    );

    blank_the_evaluated_set(tmp.path());
    let line = rendered_violations_line(&engine);
    assert!(
        line.contains("evaluated set unknown"),
        "an unrecorded evaluated set is unknown, not zero (CRA-05): {line}"
    );
    assert!(!line.contains("clean"), "and licenses no pass: {line}");
    assert!(
        !line.contains("rule violations: 0"),
        "and is never rendered as a zero: {line}"
    );
}

/// The rendered violations line **names no command**, on a real store, in every
/// state ([FR-IN-07], CRA-04).
///
/// The end-to-end half of the unit test of the same name. `scan` is run last on
/// purpose: it writes the marker too (`notes/sprint-test-72.md` Finding 1b),
/// which is exactly how a readout came to name a ``logos check`` that was never
/// invoked.
#[test]
fn the_rendered_violations_line_names_no_command_on_a_real_store() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let mut lines = vec![rendered_violations_line(&engine)];
    engine.check_rules(None, true).expect("check runs");
    lines.push(rendered_violations_line(&engine));
    write_satisfied_rules(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine restarts with the contract");
    engine.scan(true).expect("scan runs and writes the marker too");
    lines.push(rendered_violations_line(&engine));

    for line in &lines {
        for command in ["logos check", "logos scan", "logos gate", "`logos", "check_rules"] {
            assert!(
                !line.contains(command),
                "the violations line names no command, and named `{command}`: {line}"
            );
        }
    }
}

/// [CR-140] CRA-08, end to end: rendering every one of the four states leaves
/// the widened marker **byte-identical**, column for column.
///
/// `repeated_readouts_leave_the_marker_unchanged` proves it for one state; this
/// proves the arms added by T2 did not smuggle a write into any of the others.
#[test]
fn rendering_every_evaluated_set_state_persists_nothing() {
    for prepare in [
        // arm (a): no contract at all
        (|_: &Path| {}) as fn(&Path),
        // arm (b): a contract authoring nothing
        write_contract_authoring_zero_rules,
        // arm (c): a contract the fixture satisfies
        write_satisfied_rules,
        // arm (d): a marker with no evaluated set — blanked after the run below
        write_satisfied_rules,
    ] {
        let tmp = indexed_repo();
        prepare(tmp.path());
        let engine = Engine::start(tmp.path()).expect("engine starts");
        engine.check_rules(None, true).expect("check runs");

        let marker_before = check_run_state(tmp.path());
        assert_eq!(marker_before.0, 1, "every one of these states records a marker");
        assert_marker_coverage(&marker_before);
        let violations_before = violation_count(tmp.path());
        let snapshots_before = metric_snapshot_count(tmp.path());

        for _ in 0..3 {
            engine.quality_readout().expect("readout");
            engine.quality_report_hook_payload().expect("hook payload");
        }

        assert_eq!(
            check_run_state(tmp.path()),
            marker_before,
            "reading a state is not running one — every marker column is untouched \
             (FR-GV-21, CR-140 CRA-08)"
        );
        assert_eq!(violation_count(tmp.path()), violations_before);
        assert_eq!(metric_snapshot_count(tmp.path()), snapshots_before);
    }

    // The pre-migration arm separately: its fixture is defined by a write that
    // happens AFTER the run, so it cannot share the loop's shape above.
    let tmp = indexed_repo();
    write_satisfied_rules(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.check_rules(None, true).expect("check runs");
    blank_the_evaluated_set(tmp.path());
    let marker_before = check_run_state(tmp.path());
    assert_marker_coverage(&marker_before);

    for _ in 0..3 {
        engine.quality_report_hook_payload().expect("hook payload");
    }
    assert_eq!(
        check_run_state(tmp.path()),
        marker_before,
        "a pre-migration marker is not re-stamped by being read (CR-140 CRA-08)"
    );
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

/// The Health pair's two fields are projections of **one** read of the last
/// persisted snapshot, and each is byte-identical to what its standalone
/// accessor returns ([FR-UI-04], [CR-135] §3.2, [ADR-28]).
///
/// The equality half is the guard the story's AC names: `latest_gate` and
/// `latest_scan` keep working for their other callers *by projecting from the
/// same seam*, not by retaining reads of their own. Three sources of one
/// verdict is exactly how the three drift; this fails the moment they do.
///
/// It does **not** pin the one-read property itself — with no concurrent writer
/// a two-read bundle answers identically, and racing one is the flaky test
/// [CR-135] §7 rules out. That property is pinned structurally, at the handler
/// that composes the bundle (`the_health_handler_reads_the_snapshot_once`, in
/// the web crate).
#[test]
fn latest_health_projects_one_snapshot_into_both_fields() {
    let tmp = indexed_repo();
    // A graph that actually contains a test function, so the [FR-QM-08]
    // exclusion count asserted below is a real figure. Over the bare fixture it
    // is 0 on both sides, and review proved the consequence by running it: a
    // mutation zeroing the gate side survived the whole suite, because an
    // equality between two zeroes cannot notice a dropped field.
    std::fs::write(
        tmp.path().join("src/c.rs"),
        "#[cfg(test)]\nmod tests {\n    #[test]\n    fn covered() {\n        assert!(true);\n    }\n}\n",
    )
    .expect("write a test-bearing source file");
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    engine
        .gate(None, true, true)
        .expect("gate --save persists a snapshot and a baseline");

    // Record a real `check` result, so the no-write assertions below compare
    // populated values rather than two empty tables. Without this the marker is
    // absent and the violations table empty, and an equality between two
    // nothings cannot notice a write — the same false green review already hit
    // on the [FR-QM-08] exclusion count a few lines above.
    write_breaching_rules(tmp.path());
    engine.check_rules(None, true).expect("check runs");

    let before = metric_snapshot_count(tmp.path());
    let violations_before = violation_count(tmp.path());
    let marker_before = check_run_state(tmp.path());
    assert_eq!(
        marker_before.0, 1,
        "the fixture recorded a marker, so 'unchanged' below is a real comparison"
    );
    assert_marker_coverage(&marker_before);
    assert!(
        violations_before > 0,
        "the fixture recorded findings, so 'unchanged' below is a real comparison"
    );

    let health = engine.latest_health().expect("the Health pair");

    assert_eq!(
        serde_json::to_string(&health.gate).unwrap(),
        serde_json::to_string(&engine.latest_gate().unwrap()).unwrap(),
        "the bundled verdict is the standalone verdict — one seam, one source"
    );
    assert_eq!(
        serde_json::to_string(&health.scan).unwrap(),
        serde_json::to_string(&engine.latest_scan().unwrap()).unwrap(),
        "the bundled scan is the standalone scan — one seam, one source"
    );

    // Internal consistency: both halves describe the same snapshot, so the
    // figures they share are the same figures ([FR-EH-04]).
    assert_eq!(
        health.gate.signal, health.scan.signal,
        "the verdict gated on the signal the metric grid renders"
    );
    assert!(
        health.gate.test_function_count > 0,
        "the fixture carries a test function, so the exclusion count below compares real \
         figures rather than two zeroes"
    );
    assert_eq!(
        health.gate.test_function_count, health.scan.metrics.test_function_count,
        "and on the same FR-QM-08 production-scope exclusion count"
    );
    assert!(
        !health.scan.metrics.empty,
        "the fixture scanned a non-empty graph"
    );
    assert!(
        !health.gate.message.contains("no snapshot yet"),
        "a populated grid cannot sit beside a no-snapshot verdict (CR-135 §2.1): {}",
        health.gate.message
    );

    // ADR-28: reading the pair, once or repeatedly, persists nothing.
    //
    // "Nothing" is the whole store this module writes, not the snapshot table
    // alone. [S-314] established `metric_snapshots` + `violations` + marker
    // *content* as the complete set for the readout path; `latest_health` is a
    // second read path through the same module, reached by a plain page GET,
    // and it inherits that contract rather than a third of it. Guarded here
    // because a snapshot-only assertion let a `persist_violations` call inside
    // `latest_health` pass the whole suite — every `/api/v1/health` load would
    // have cleared the last real `check` result and stamped a fabricated
    // recorded-clean marker over it ([FR-GV-21], [BR-41]).
    for _ in 0..3 {
        engine.latest_health().unwrap();
    }
    assert_eq!(
        metric_snapshot_count(tmp.path()),
        before,
        "reading the Health pair appended no snapshot"
    );
    assert_eq!(
        violation_count(tmp.path()),
        violations_before,
        "reading the Health pair neither cleared nor rewrote the violations table"
    );
    assert_eq!(
        check_run_state(tmp.path()),
        marker_before,
        "reading a run is not running one — every column of the marker is untouched \
         by reading the Health pair (FR-GV-21, ADR-49, CR-140 CRA-08)"
    );
}

/// On a never-`scan`-ned store **both** halves of the Health pair take the
/// empty branch, because there is one snapshot value and both read it.
///
/// This is the reproduced symptom stated as an invariant ([CR-135] §2.1): the
/// no-signal callout beside a fully populated quality grid is the pairing of a
/// `None` snapshot on the gate side with a `Some` one on the scan side, and a
/// single read cannot produce it in either direction.
#[test]
fn never_scanned_store_health_pair_is_empty_on_both_sides() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let health = engine.latest_health().expect("the Health pair");

    assert!(
        health.scan.metrics.empty,
        "no snapshot → the empty sentinel on the scan side, never zeros (NFR-CC-04)"
    );
    assert!(health.scan.signal.is_none(), "no fabricated signal");
    assert!(
        health.gate.signal.is_none(),
        "and none on the verdict side either — one read, one answer"
    );
    assert!(health.gate.passed, "no snapshot cannot regress — informational pass");
    assert!(
        health.gate.message.contains("no snapshot yet"),
        "the verdict names the producing command: {}",
        health.gate.message
    );

    assert_eq!(
        metric_snapshot_count(tmp.path()),
        0,
        "reading the Health pair created no snapshot"
    );
}

// ── the one-read invariant, pinned at the seam (CR-135 §3.2) ─────────────────

/// Production code only — the doc comments in `governance/mod.rs` name these
/// accessors constantly, and a brace in prose would unbalance the body walk.
///
/// A deliberate twin of the helper of the same name in `web/src/lib.rs`'s test
/// module: the two crates cannot share a test helper without a new dev-only
/// crate, and a shared crate for eight lines buys less than it costs. Both are
/// exercised by their own pin on every run, so a drifted copy fails rather than
/// silently mis-measuring.
fn production_code(source: &str) -> String {
    let code = source
        .split_once("\n#[cfg(test)]\nmod tests {")
        .map_or(source, |(before, _)| before);
    code.lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of the function whose header begins at the first occurrence of
/// `header` — opening brace to matching close, by brace depth, skipping string
/// literals so a brace inside one (this file has `format!("… {current} …")`)
/// cannot unbalance the walk. Panics on an unbalanced walk rather than
/// returning a short slice: a pin that silently scans the wrong region is worse
/// than one that fails loudly.
fn fn_body<'a>(code: &'a str, header: &str) -> &'a str {
    let start = code
        .find(header)
        .unwrap_or_else(|| panic!("`{header}` is not in this source"));
    let open = start
        + code[start..]
            .find('{')
            .unwrap_or_else(|| panic!("`{header}` has no body"));
    let bytes = code.as_bytes();
    let (mut depth, mut i, mut in_str) = (0usize, open, false);
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'{' if !in_str => depth += 1,
            b'}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return &code[open + 1..i];
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("unbalanced braces walking the body of `{header}`");
}

/// The Health path takes **exactly one** read of the last persisted snapshot —
/// wherever an extra read might be added ([CR-135] §3.2, [FR-UI-04]).
///
/// # Why this exists beside the web-side pin
///
/// `the_health_handler_reads_the_snapshot_once` (in the `web` crate) proves
/// which accessor the handler *names*. It says nothing about how many reads
/// that accessor then takes, and review demonstrated the gap by running it:
/// two mutations restore the [CR-135] defect — a second `latest_metrics` call
/// inside `latest_health`, and a re-read inside `scan_from_snapshot` that
/// ignores the value it was handed — and **both left the whole suite green**.
/// The comment in `latest_health` saying "do not add a second `latest_metrics`
/// call here, in either projection or at a caller" was prose with nothing
/// enforcing it. This is the enforcement.
///
/// # Why a source scan and not a counter
///
/// Counting the reads at runtime is not available here, and each leg was
/// checked rather than assumed: `ReaderPool` holds concrete `SqliteGraphStore`
/// connections, so a counting `GraphStore` cannot be injected through
/// `submit_read`'s `&dyn GraphStore`; `Runtime` exposes no read counter; and
/// `observability::traced` wraps only `Engine` methods, so the internal
/// `governance::latest_metrics` calls emit no telemetry to count. Racing a
/// concurrent `scan` is ruled out by [CR-135] §7 — against broken code it fails
/// only *sometimes*, which is the one direction a regression test must not be
/// unreliable in. A source scan is deterministic and answers exactly the
/// question asked.
///
/// The per-function expectations are stated for **every** function on the path,
/// not just the seam: an extra read is a defect wherever it lands, and naming
/// each one is what makes the roster a closed list rather than a spot check.
///
/// # What this pin does NOT cover, stated so it is not over-read
///
/// It scans `governance/mod.rs` only. `latest_metrics` is `pub(crate)`, so a
/// reader added to a sibling module — `governance/readout.rs` is already a
/// `pub mod` — and called from `latest_health` would escape both this discovery
/// and the web-side handler pin. Making `latest_metrics` module-private would
/// let the compiler enforce the file scope this test assumes; that is a
/// production visibility change and is left for a story that owns the file.
#[test]
fn the_health_path_reads_the_snapshot_exactly_once() {
    let code = production_code(include_str!("../src/governance/mod.rs"));

    /// How many times `body` reads the last persisted snapshot.
    ///
    /// Both spellings count. The invariant is "one read of the store", not "one
    /// call to one wrapper": `latest_metrics` is a one-line convenience over
    /// `submit_read(|store| store.latest_metric_snapshot())`, so a function that
    /// calls the store accessor directly takes exactly the same read, in its own
    /// transaction. Keying only on the wrapper's name let the [CR-135] defect be
    /// restored verbatim with this pin green — a helper doing the `submit_read`
    /// itself, called from `latest_health`, gave one Health response two reads of
    /// the same row and every expectation below stayed correct.
    ///
    /// Whole-identifier only: `latest_metrics(` is a substring of
    /// `prior_latest_metrics(`. The two accessor names do not overlap each other
    /// (`latest_metrics(` needs `s(` where `latest_metric_snapshot(` has `_s`),
    /// so nothing is double-counted.
    fn snapshot_reads(body: &str) -> usize {
        ["latest_metrics(", "latest_metric_snapshot("]
            .iter()
            .map(|accessor| {
                body.match_indices(accessor)
                    .filter(|(at, _)| {
                        body[..*at]
                            .chars()
                            .next_back()
                            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
                    })
                    .count()
            })
            .sum()
    }

    /// Is `line` a module-scope function header, whatever qualifies it?
    ///
    /// Column 0 is module scope; everything before `fn` must be a qualifier. A
    /// whitelist of three visibility spellings is not that, and the difference
    /// ships a reader: `pub(super) fn` walked straight past the discovery below
    /// with the suite green. `async fn`, `const fn` and `pub(in …) fn` are the
    /// same class. Probed with decoys at the end of this test, because a matcher
    /// nobody ran data through is the recurring defect in this file.
    fn is_module_fn_header(line: &str) -> bool {
        const QUALIFIERS: [&str; 6] = ["pub", "async", "const", "unsafe", "extern", "\""];
        if line.starts_with(char::is_whitespace) {
            return false;
        }
        let Some(fn_at) = line.find("fn ") else {
            return false;
        };
        // Drop parenthesised scopes before tokenising: `pub(in crate::governance)`
        // carries a space, so a bare `split_whitespace` sees `crate::governance)`
        // as its own token and refuses a valid header. The decoy list below
        // caught exactly that on this function's first cut.
        let mut prefix = String::new();
        let mut depth = 0usize;
        for ch in line[..fn_at].chars() {
            match ch {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ if depth == 0 => prefix.push(ch),
                _ => {}
            }
        }
        prefix
            .split_whitespace()
            .all(|token| QUALIFIERS.iter().any(|q| token.starts_with(q)))
    }

    for (header, expected, why) in [
        (
            "fn latest_health(",
            1,
            "the seam is THE read — both Health fields are projections of its one value",
        ),
        (
            "fn scan_from_snapshot(",
            0,
            "a projection consumes the snapshot it is handed; re-reading reopens the window \
             inside the seam",
        ),
        (
            "fn gate_from_snapshot(",
            0,
            "likewise on the verdict side — it reads the baseline, never the snapshot again",
        ),
        (
            "fn latest_scan(",
            1,
            "the standalone accessor takes its own single read and projects it",
        ),
        (
            "fn latest_gate(",
            1,
            "and so does its sibling — one read each, never two",
        ),
        (
            "fn latest_metrics(",
            1,
            "the wrapper IS the one store read the three above project from; it holds the \
             `submit_read`, so a second one here doubles every caller at once",
        ),
    ] {
        assert_eq!(
            snapshot_reads(fn_body(&code, header)),
            expected,
            "`{header}` must read the last persisted snapshot {expected}×: {why} (CR-135 §3.2)"
        );
    }

    // The roster above is a closed list over the functions it NAMES, which is not
    // the same as a closed list over the module — and the difference is the whole
    // property. Story review proved it: a one-line helper calling `latest_metrics`
    // once, called from `latest_health`, leaves every expectation above exactly
    // correct and the suite green at 21/21, while a Health response takes **two**
    // reads of the snapshot. The web-side pin cannot see it either — the handler
    // still names only `latest_health`.
    //
    // So the readers are DISCOVERED rather than named, and the roster is asserted
    // to be all of them. A fourth reader must now be classified here before it can
    // ship, which is what "an extra read is a defect wherever it lands" requires.
    let mut readers: Vec<&str> = Vec::new();
    for line in code.lines() {
        if !is_module_fn_header(line) {
            continue;
        }
        // The parameter list opens AFTER the name — `line.find('(')` would take
        // the one inside `pub(crate)` and slice a header that matches the wrong
        // function entirely (it did, on the first cut of this assertion).
        let Some(name_at) = line.find("fn ").map(|at| at + 3) else { continue };
        let Some(paren) = line[name_at..].find('(').map(|at| name_at + at) else {
            continue;
        };
        if snapshot_reads(fn_body(&code, &line[..=paren])) > 0 {
            readers.push(line[name_at..paren].trim());
        }
    }
    readers.sort_unstable();
    assert_eq!(
        readers,
        [
            "latest_gate",
            "latest_health",
            "latest_metrics",
            "latest_scan"
        ],
        "exactly these functions read the last persisted snapshot. A new reader is a \
         new place the Health path can take a second read, so it is classified in the \
         roster above before it ships — not discovered later as a torn readout \
         (CR-135 §3.2); found {readers:?}"
    );

    // The matcher probed with its near misses. A source-scanning pin is only
    // worth what its predicate is worth, and this file has twice shipped one
    // that checked nothing until data was run through it — the `pub(crate)`
    // paren bug noted above, and the three-spelling whitelist this replaces.
    for accepted in [
        "fn f(",
        "pub fn f(",
        "pub(crate) fn f(",
        "pub(super) fn f(",
        "pub(in crate::governance) fn f(",
        "async fn f(",
        "pub(crate) async fn f(",
        "const fn f(",
        "pub unsafe fn f(",
    ] {
        assert!(
            is_module_fn_header(accepted),
            "`{accepted}` is a module-scope function header and must be discovered"
        );
    }
    for refused in [
        "    fn nested(",      // not module scope
        "        pub fn f(",   // ditto
        "struct Fn(",          // no `fn ` token
        "let f = |x| fn_x(x);" // `fn_x` is not `fn `
    ] {
        assert!(
            !is_module_fn_header(refused),
            "`{refused}` is not a module-scope function header"
        );
    }
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
