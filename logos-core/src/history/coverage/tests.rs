//! Integration tests for the coverage ingest pipeline over **real** git fixtures
//! ([S-049] acceptance, [FR-CV-01]..[FR-CV-04], [FR-CV-09], [ADR-23]).
//!
//! The pure parser/detection logic is unit-tested in [`super::parse`] and the
//! path mapper in [`super::pathmap`]; these exercise the end-to-end ingest seam
//! against an actual `git` HEAD: format equivalence, atomic rejection of a corrupt
//! report, same-HEAD merge (summed hits), new-HEAD snapshot retention, idempotent
//! re-ingest, per-file content-hash anchoring + stale rejection, the snapshot
//! provenance schema, and re-index survival.

use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::Connection;
use tempfile::TempDir;

use crate::config::{EffectiveCoverage, EffectiveCoverageIngest};

use super::{combined_config_hash, ingest, CoverageFormat};

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

/// Commit `relpath` with `contents`.
fn commit(cwd: &Path, relpath: &str, contents: &str, msg: &str) {
    if let Some(parent) = Path::new(relpath).parent() {
        std::fs::create_dir_all(cwd.join(parent)).unwrap();
    }
    std::fs::write(cwd.join(relpath), contents).unwrap();
    sh_git(cwd, &["add", relpath]);
    sh_git(cwd, &["commit", "-q", "-m", msg]);
}

/// A git repo at `<tmp>/repo` with `src/lib.rs` and `src/util.rs` committed.
fn repo() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("temp root");
    let r = tmp.path().join("repo");
    std::fs::create_dir_all(&r).unwrap();
    sh_git(&r, &["init", "-q", "-b", "main"]);
    commit(
        &r,
        "src/lib.rs",
        "pub fn one() {}\npub fn two() {}\n",
        "lib",
    );
    commit(&r, "src/util.rs", "pub fn util() {}\n", "util");
    (tmp, r)
}

fn indexed() -> Vec<String> {
    vec!["src/lib.rs".to_string(), "src/util.rs".to_string()]
}

fn write(root: &Path, name: &str, body: &str) -> PathBuf {
    let p = root.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

/// Every `(path, line_no, hits)` row across all snapshots, sorted — the coverage
/// "values" two equivalent reports must agree on.
fn coverage_rows(conn: &Connection) -> Vec<(String, i64, i64)> {
    let mut stmt = conn
        .prepare(
            "SELECT cf.path, cl.line_no, cl.hits
             FROM coverage_files cf JOIN coverage_lines cl ON cl.file_id = cf.id
             ORDER BY cf.path, cl.line_no",
        )
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

const LCOV: &str = "TN:suite\n\
                    SF:/build/ci/src/lib.rs\n\
                    DA:1,5\n\
                    DA:2,0\n\
                    end_of_record\n\
                    SF:/build/ci/src/util.rs\n\
                    DA:1,3\n\
                    end_of_record\n";

const COBERTURA: &str = r#"<?xml version="1.0"?>
<coverage>
  <packages><package name="p"><classes>
    <class filename="src/lib.rs"><lines>
      <line number="1" hits="5"/>
      <line number="2" hits="0"/>
    </lines></class>
    <class filename="src/util.rs"><lines>
      <line number="1" hits="3"/>
    </lines></class>
  </classes></package></packages>
</coverage>"#;

/// [UAT-CV-01] / [FR-CV-01]: an LCOV report and an equivalent Cobertura report
/// ingest to the **same** coverage values, both without `--format`.
#[test]
fn lcov_and_cobertura_ingest_to_the_same_values() {
    let (_t1, r1) = repo();
    let (_t2, r2) = repo();
    let lcov = write(&r1, "cov.info", LCOV);
    let cobertura = write(&r2, "cov.xml", COBERTURA);

    let s1 = ingest(&r1, &lcov, None, &EffectiveCoverage::default(), &EffectiveCoverageIngest::default(), &indexed()).unwrap();
    let s2 = ingest(
        &r2,
        &cobertura,
        None,
        &EffectiveCoverage::default(),
        &EffectiveCoverageIngest::default(), &indexed(),
    )
    .unwrap();

    assert_eq!(s1.format, CoverageFormat::Lcov);
    assert_eq!(s2.format, CoverageFormat::Cobertura);
    // Both auto-detected, both bound both files, identical line totals.
    assert_eq!(
        (s1.matched_files, s1.instrumented_lines, s1.covered_lines),
        (2, 3, 2)
    );
    assert_eq!(
        (s2.matched_files, s2.instrumented_lines, s2.covered_lines),
        (2, 3, 2)
    );

    let rows1 = coverage_rows(&super::super::open(&r1).unwrap());
    let rows2 = coverage_rows(&super::super::open(&r2).unwrap());
    assert_eq!(
        rows1,
        vec![
            ("src/lib.rs".to_string(), 1, 5),
            ("src/lib.rs".to_string(), 2, 0),
            ("src/util.rs".to_string(), 1, 3),
        ]
    );
    assert_eq!(
        rows1, rows2,
        "LCOV and Cobertura yield identical coverage values"
    );
}

/// [FR-CV-01] / [UAT-CV-01]: a corrupt report is rejected with a non-zero result
/// (an `Err`) and **no** partial write — on a fresh repo the store is never even
/// created; on an existing store the row counts are unchanged.
#[test]
fn corrupt_report_is_rejected_atomically() {
    // Fresh repo: a corrupt LCOV must not create history.db at all.
    let (_t, r) = repo();
    let bad = write(&r, "bad.info", "SF:src/lib.rs\nDA:1\nend_of_record\n"); // truncated DA
    let err = ingest(&r, &bad, None, &EffectiveCoverage::default(), &EffectiveCoverageIngest::default(), &indexed());
    assert!(err.is_err(), "a truncated LCOV is rejected");
    assert!(
        !super::super::db_path(&r).exists(),
        "a rejected report never creates the store (byte-identical: absent)"
    );

    // Existing store: a corrupt ingest leaves the row counts untouched.
    let good = write(&r, "good.info", LCOV);
    ingest(&r, &good, None, &EffectiveCoverage::default(), &EffectiveCoverageIngest::default(), &indexed()).unwrap();
    let conn = super::super::open(&r).unwrap();
    let (snaps, files, lines) = (
        count(&conn, "coverage_snapshots"),
        count(&conn, "coverage_files"),
        count(&conn, "coverage_lines"),
    );
    let corrupt = write(
        &r,
        "corrupt.xml",
        "<coverage><classes><class filename=\"x\"><lines><line number=\"1\" hits=\"1\"/>",
    );
    assert!(
        ingest(
            &r,
            &corrupt,
            None,
            &EffectiveCoverage::default(),
            &EffectiveCoverageIngest::default(), &indexed()
        )
        .is_err(),
        "malformed XML is rejected"
    );
    let conn = super::super::open(&r).unwrap();
    assert_eq!(
        (
            count(&conn, "coverage_snapshots"),
            count(&conn, "coverage_files"),
            count(&conn, "coverage_lines")
        ),
        (snaps, files, lines),
        "a rejected ingest writes nothing (no partial store mutation)"
    );
}

/// [FR-CV-01]: an unrecognized (non-coverage) file is rejected unless `--format`
/// forces a parser.
#[test]
fn unrecognized_format_is_rejected_but_force_overrides() {
    let (_t, r) = repo();
    let html = write(&r, "page.html", "<html><body>not coverage</body></html>");
    assert!(
        ingest(&r, &html, None, &EffectiveCoverage::default(), &EffectiveCoverageIngest::default(), &indexed()).is_err(),
        "a non-coverage XML is rejected without --format"
    );

    // `--format lcov` forces the LCOV parser on an otherwise-undetected file.
    let forced = write(&r, "forced.txt", "SF:src/util.rs\nDA:1,1\nend_of_record\n");
    let s = ingest(
        &r,
        &forced,
        Some(CoverageFormat::Lcov),
        &EffectiveCoverage::default(),
        &EffectiveCoverageIngest::default(), &indexed(),
    )
    .unwrap();
    assert_eq!(s.format, CoverageFormat::Lcov);
    assert_eq!(s.matched_files, 1);

    // Forcing the *wrong* format does not fabricate coverage: LCOV text parsed as
    // Cobertura has no XML elements, so it yields zero matched files (not bad data).
    let lcov = write(&r, "real.info", LCOV);
    let wrong = ingest(
        &r,
        &lcov,
        Some(CoverageFormat::Cobertura),
        &EffectiveCoverage::default(),
        &EffectiveCoverageIngest::default(), &indexed(),
    )
    .unwrap();
    assert_eq!(wrong.format, CoverageFormat::Cobertura);
    assert_eq!(
        wrong.matched_files, 0,
        "LCOV forced as Cobertura yields nothing, never fabricated coverage"
    );
}

/// [FR-CV-03] / [NFR-RA-05]: a report path that maps to an indexed file which
/// cannot be read on disk (so it cannot be content-hash anchored) is surfaced as
/// unmatched, never anchored with a fabricated hash.
#[test]
fn mapped_but_unreadable_file_is_unmatched() {
    let (_t, r) = repo();
    // `src/ghost.rs` is in the index but absent from the working tree.
    let idx = vec!["src/ghost.rs".to_string()];
    let report = write(
        &r,
        "cov.info",
        "SF:/build/src/ghost.rs\nDA:1,1\nend_of_record\n",
    );
    let s = ingest(&r, &report, None, &EffectiveCoverage::default(), &EffectiveCoverageIngest::default(), &idx).unwrap();
    assert_eq!(s.matched_files, 0, "an unreadable file is not anchored");
    assert_eq!(
        s.unmatched,
        vec!["src/ghost.rs".to_string()],
        "it is surfaced as unmatched, never fabricated"
    );
}

/// [FR-CV-04]: two different reports at the **same** HEAD merge into one snapshot
/// with summed line hits on overlap.
#[test]
fn same_head_reports_merge_with_summed_hits() {
    let (_t, r) = repo();
    let a = write(
        &r,
        "a.info",
        "SF:src/lib.rs\nDA:1,1\nDA:2,0\nend_of_record\n",
    );
    let b = write(
        &r,
        "b.info",
        "SF:src/lib.rs\nDA:1,1\nend_of_record\nSF:src/util.rs\nDA:1,2\nend_of_record\n",
    );

    let sa = ingest(&r, &a, None, &EffectiveCoverage::default(), &EffectiveCoverageIngest::default(), &indexed()).unwrap();
    assert!(!sa.merged_into_existing, "first ingest opens a snapshot");
    let sb = ingest(&r, &b, None, &EffectiveCoverage::default(), &EffectiveCoverageIngest::default(), &indexed()).unwrap();
    assert!(sb.merged_into_existing, "same-HEAD second ingest merges");
    assert_eq!(sa.snapshot_id, sb.snapshot_id, "one snapshot at this HEAD");

    let conn = super::super::open(&r).unwrap();
    assert_eq!(count(&conn, "coverage_snapshots"), 1);
    assert_eq!(
        coverage_rows(&conn),
        vec![
            ("src/lib.rs".to_string(), 1, 2), // 1 + 1 summed across the two reports
            ("src/lib.rs".to_string(), 2, 0),
            ("src/util.rs".to_string(), 1, 2),
        ]
    );
}

/// [UAT-CV-01]: re-ingesting the **same** report at the same HEAD is idempotent —
/// the summary flags it and the store is byte-identical (hits not doubled).
#[test]
fn reingesting_the_same_report_is_a_noop() {
    let (_t, r) = repo();
    let a = write(&r, "a.info", LCOV);
    ingest(&r, &a, None, &EffectiveCoverage::default(), &EffectiveCoverageIngest::default(), &indexed()).unwrap();
    let before = coverage_rows(&super::super::open(&r).unwrap());

    let again = ingest(&r, &a, None, &EffectiveCoverage::default(), &EffectiveCoverageIngest::default(), &indexed()).unwrap();
    assert!(again.already_ingested, "the duplicate report is detected");
    let after = coverage_rows(&super::super::open(&r).unwrap());
    assert_eq!(
        before, after,
        "an idempotent re-ingest leaves the store identical"
    );
    assert_eq!(
        count(&super::super::open(&r).unwrap(), "coverage_reports"),
        1,
        "the duplicate report is not recorded twice"
    );
}

/// [FR-CV-04]: an ingest at a **new** HEAD opens a new snapshot; the prior one is
/// retained as queryable provenance.
#[test]
fn new_head_starts_a_new_snapshot_retaining_prior() {
    let (_t, r) = repo();
    let a = write(&r, "a.info", LCOV);
    let first = ingest(&r, &a, None, &EffectiveCoverage::default(), &EffectiveCoverageIngest::default(), &indexed()).unwrap();

    // Advance HEAD, then ingest again.
    commit(&r, "src/new.rs", "pub fn n() {}\n", "advance");
    let second = ingest(&r, &a, None, &EffectiveCoverage::default(), &EffectiveCoverageIngest::default(), &indexed()).unwrap();

    assert!(
        !second.merged_into_existing,
        "a new HEAD opens a new snapshot"
    );
    assert_ne!(first.snapshot_id, second.snapshot_id);
    let conn = super::super::open(&r).unwrap();
    assert_eq!(
        count(&conn, "coverage_snapshots"),
        2,
        "the prior snapshot is retained"
    );
    assert_ne!(first.head_sha, second.head_sha);
}

/// [FR-CV-04] / [FR-CV-05]: on a same-HEAD merge, a file whose content changed
/// since the anchor is **rejected** (never apply stale line data); other files in
/// the same report still merge.
#[test]
fn content_hash_mismatch_rejects_file_on_merge() {
    let (_t, r) = repo();
    let a = write(&r, "a.info", "SF:src/lib.rs\nDA:1,5\nend_of_record\n");
    ingest(&r, &a, None, &EffectiveCoverage::default(), &EffectiveCoverageIngest::default(), &indexed()).unwrap();

    // Edit lib.rs in the working tree (HEAD unchanged) so its content hash drifts
    // from the anchor, then merge a second report touching lib.rs *and* util.rs.
    std::fs::write(
        r.join("src/lib.rs"),
        "pub fn one() {}\npub fn two() {}\npub fn three() {}\n",
    )
    .unwrap();
    let b = write(
        &r,
        "b.info",
        "SF:src/lib.rs\nDA:1,9\nend_of_record\nSF:src/util.rs\nDA:1,1\nend_of_record\n",
    );
    let sb = ingest(&r, &b, None, &EffectiveCoverage::default(), &EffectiveCoverageIngest::default(), &indexed()).unwrap();

    assert_eq!(
        sb.rejected_stale,
        vec!["src/lib.rs".to_string()],
        "the drifted file is rejected"
    );
    let conn = super::super::open(&r).unwrap();
    assert_eq!(
        coverage_rows(&conn),
        vec![
            ("src/lib.rs".to_string(), 1, 5), // unchanged — B's 9 was rejected, not summed
            ("src/util.rs".to_string(), 1, 1), // the in-sync file still merged
        ]
    );
}

/// [FR-CV-03]: absolute report paths map by suffix; an ambiguous basename is
/// reported unmatched, never bound.
#[test]
fn absolute_paths_map_and_ambiguous_is_unmatched() {
    let (_t, r) = repo();
    commit(&r, "tests/util.rs", "fn t() {}\n", "test util");
    let idx = vec!["src/util.rs".to_string(), "tests/util.rs".to_string()];
    // One absolute path that uniquely suffix-matches `src/util.rs`, and a bare
    // `util.rs` that is ambiguous across the two `util.rs` files.
    let report = write(
        &r,
        "cov.info",
        "SF:/abs/build/src/util.rs\nDA:1,1\nend_of_record\nSF:util.rs\nDA:2,2\nend_of_record\n",
    );
    let s = ingest(&r, &report, None, &EffectiveCoverage::default(), &EffectiveCoverageIngest::default(), &idx).unwrap();
    assert_eq!(s.matched_files, 1, "only the unambiguous path binds");
    assert_eq!(
        s.unmatched,
        vec!["util.rs".to_string()],
        "the ambiguous basename is unmatched"
    );
}

/// [FR-CV-02] / [FR-CV-09]: every snapshot carries HEAD SHA + the `[coverage]`
/// config hash; each report carries its hash + format; each file carries a content
/// hash anchor.
#[test]
fn snapshot_carries_required_provenance() {
    let (_t, r) = repo();
    let cfg = EffectiveCoverage::default();
    let report = write(&r, "cov.info", LCOV);
    let summary = ingest(&r, &report, None, &cfg, &EffectiveCoverageIngest::default(), &indexed()).unwrap();

    let conn = super::super::open(&r).unwrap();
    let (head, config_hash): (String, String) = conn
        .query_row(
            "SELECT head_sha, config_hash FROM coverage_snapshots WHERE id = ?1",
            [summary.snapshot_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(head, summary.head_sha);
    assert_eq!(
        config_hash,
        combined_config_hash(&cfg, &EffectiveCoverageIngest::default()),
        "the combined [coverage] + [coverage_ingest] hash is recorded (FR-CV-09)"
    );

    let (rhash, fmt): (String, String) = conn
        .query_row(
            "SELECT report_hash, format FROM coverage_reports WHERE snapshot_id = ?1",
            [summary.snapshot_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(rhash, summary.report_hash);
    assert_eq!(fmt, "lcov");

    // Every covered file carries a non-empty content-hash anchor.
    let anchorless: i64 = conn
        .query_row(
            "SELECT count(*) FROM coverage_files WHERE content_hash = ''",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        anchorless, 0,
        "every file is anchored to a content hash (FR-CV-02)"
    );
}

/// [FR-CV-09]: changing `path_strip_prefixes` changes the config hash recorded at
/// the next ingest (the provenance the snapshot carries).
#[test]
fn changing_strip_prefixes_changes_recorded_config_hash() {
    let (_t1, r1) = repo();
    let (_t2, r2) = repo();
    let report1 = write(&r1, "cov.info", LCOV);
    let report2 = write(&r2, "cov.info", LCOV);

    let default_cfg = EffectiveCoverage::default();
    let tuned_cfg = EffectiveCoverage {
        path_strip_prefixes: vec!["/build/ci/".to_string()],
    };
    let s1 = ingest(&r1, &report1, None, &default_cfg, &EffectiveCoverageIngest::default(), &indexed()).unwrap();
    let s2 = ingest(&r2, &report2, None, &tuned_cfg, &EffectiveCoverageIngest::default(), &indexed()).unwrap();
    assert_ne!(
        s1.config_hash, s2.config_hash,
        "the strip-prefix change is visible in the snapshot hash"
    );
}

/// [FR-CV-09] / [CR-036]: the `[coverage_ingest]` side of the combined snapshot
/// hash is exercised end-to-end — holding `[coverage]` constant and varying the
/// ingest config changes the recorded `config_hash`, proving `ingest_cfg.hash()`
/// is actually folded into the snapshot (not silently dropped on the ingest path).
#[test]
fn changing_coverage_ingest_config_changes_recorded_config_hash() {
    let (_t1, r1) = repo();
    let (_t2, r2) = repo();
    let report1 = write(&r1, "cov.info", LCOV);
    let report2 = write(&r2, "cov.info", LCOV);

    let cov = EffectiveCoverage::default();
    let default_ingest = EffectiveCoverageIngest::default();
    let tuned_ingest = EffectiveCoverageIngest {
        artifact_glob: vec!["ci/cov.xml".to_string()],
        format: "auto".to_string(),
        refresh_cmd: None,
    };
    // Same [coverage] config, different [coverage_ingest] config.
    let s1 = ingest(&r1, &report1, None, &cov, &default_ingest, &indexed()).unwrap();
    let s2 = ingest(&r2, &report2, None, &cov, &tuned_ingest, &indexed()).unwrap();
    assert_ne!(
        s1.config_hash, s2.config_hash,
        "a [coverage_ingest] change folds into the snapshot config hash (FR-CV-09)"
    );
}

/// [FR-CV-02]: ingested coverage survives a full `index` rebuild — `history.db` is
/// a separate file the graph rebuild never touches. Driven through the
/// [`Engine`](crate::Engine) seam.
#[cfg(feature = "lang-rust")]
#[test]
fn coverage_survives_a_full_index() {
    let (_t, r) = repo();
    let report = write(&r, "cov.info", LCOV);
    ingest(&r, &report, None, &EffectiveCoverage::default(), &EffectiveCoverageIngest::default(), &indexed()).unwrap();
    let before = coverage_rows(&super::super::open(&r).unwrap());
    assert!(!before.is_empty());

    let engine = crate::Engine::start(&r).expect("engine starts");
    let _ = engine.index();

    let after = coverage_rows(&super::super::open(&r).unwrap());
    assert_eq!(
        after, before,
        "coverage survives a full logos.db rebuild (FR-CV-02)"
    );
}
