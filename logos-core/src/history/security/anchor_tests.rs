//! A secret found only in history is anchored to its commit ([S-767],
//! [FR-SC-13] as amended by [CR-238] Amendment 1, [FR-SC-03]).
//!
//! Every test replays the history report Gitleaks 8.18.4 really wrote over the
//! fixture repository [`identity_tests`](super::identity_tests) rebuilds: the
//! settings secret sits on line 13 at C3 (and on line 3 at C1 and C2); the
//! legacy secret is in `legacy/client.py`, which C2 deletes.
//!
//! [S-767]: ../../../../docs/planning/journal.md#s-767-a-secret-found-only-in-history-is-anchored-to-its-commit
//! [FR-SC-13]: ../../../../docs/specs/requirements/FR-SC-13.md
//! [FR-SC-03]: ../../../../docs/specs/requirements/FR-SC-03.md
//! [CR-238]: ../../../../docs/requests/CR-238-sprint-106-review-follow-ups.md

use std::collections::BTreeSet;

use rusqlite::Connection;

use super::identity_tests::{
    constants, fixture, git, gitleaks_findings, gitleaks_repo, ingest_history, key, settings_at, settings_finding, write, C1,
    C2, C3, SECRET, SETTINGS,
};
use super::{read_latest, CommitAnchor, FindingBinding, SecurityFinding};
use crate::history::{FRESHNESS_FRESH, FRESHNESS_NA};

const LEGACY: &str = "legacy/client.py";

fn legacy_finding(root: &std::path::Path) -> SecurityFinding {
    let found: Vec<SecurityFinding> =
        gitleaks_findings(root).into_iter().filter(|f| f.report_path.as_deref() == Some(LEGACY)).collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    found.into_iter().next().unwrap()
}

/// The settings finding after a history ingest over a working tree whose
/// line 13 — where C3 has the secret — reads `line` instead.
fn settings_with_line_13(line: &str) -> SecurityFinding {
    let (_t, root) = gitleaks_repo(3);
    write(&root, SETTINGS, &format!("import os\n\n{}{line}\n", constants()));
    ingest_history(&root);
    settings_finding(&root)
}

fn historical_at(commit: &str, path: &str) -> (FindingBinding, Option<String>, Option<CommitAnchor>) {
    (FindingBinding::Historical, None, Some(CommitAnchor::new(commit.to_string(), path)))
}

/// A secret in a file deleted since its commit is `historical`: anchored to
/// that commit and its path there, with no file of today's tree, so nothing
/// is anchored or freshness-checked for it.
#[test]
fn a_secret_in_a_file_deleted_since_its_commit_is_historical_with_its_commit() {
    let (_t, root) = gitleaks_repo(3);
    let summary = ingest_history(&root);
    assert_eq!(summary.historical, 1, "the legacy secret alone: {summary:#?}");

    let legacy = legacy_finding(&root);
    assert_eq!((legacy.binding, legacy.path.clone(), legacy.anchor.clone()), historical_at(C1, LEGACY));
    assert_eq!(legacy.anchor.as_ref().map(|a| a.short_commit.as_str()), Some(&C1[..7]), "the short commit");
    assert_eq!(legacy.freshness, FRESHNESS_NA);
    let anchored: i64 = Connection::open(crate::history::db_path(&root))
        .unwrap()
        .query_row("SELECT count(*) FROM security_files WHERE path = ?1", [LEGACY], |r| r.get(0))
        .unwrap();
    assert_eq!(anchored, 0, "no file of today's tree is anchored for it");
}

/// A secret still on its line is bound to it, anchored to the commit whose
/// place that is — an edit elsewhere on the line, even before the secret,
/// keeps it there.
#[test]
fn a_secret_still_on_its_line_is_bound_to_it() {
    let (_t, root) = gitleaks_repo(3);
    ingest_history(&root);
    let settings = settings_finding(&root);
    assert_eq!((settings.binding, settings.path.as_deref()), (FindingBinding::Bound, Some(SETTINGS)));
    assert_eq!((settings.start_line, settings.freshness), (Some(13), FRESHNESS_FRESH));
    assert_eq!(settings.anchor, Some(CommitAnchor::new(C3.to_string(), SETTINGS)));

    for line in [format!("{}  # rotated 2026-10-19", key()), format!("    {}", key())] {
        let edited = settings_with_line_13(&line);
        assert_eq!((edited.binding, edited.path.as_deref()), (FindingBinding::Bound, Some(SETTINGS)), "{line:?}");
    }
}

/// A secret still on its line in a file the index does not hold (a `.env`, a
/// file excluded from the index) is not found only in history: it keeps its
/// `unmatched` binding, with its commit, and reads `historical` once its line
/// holds other text.
#[test]
fn a_secret_still_on_its_line_in_a_file_outside_the_index_is_not_historical() {
    let outside_the_index = |line_13: Option<&str>| {
        let (_t, root) = gitleaks_repo(3);
        if let Some(line) = line_13 {
            write(&root, SETTINGS, &format!("import os\n\n{}{line}\n", constants()));
        }
        super::ingest(&root, &fixture("history.sarif"), &["app/main.py".to_string()]).unwrap();
        settings_finding(&root)
    };
    let present = outside_the_index(None);
    assert_eq!((present.binding, present.path.clone()), (FindingBinding::Unmatched, None), "the file still holds it");
    assert_eq!(present.anchor, Some(CommitAnchor::new(C3.to_string(), SETTINGS)));
    let overwritten = outside_the_index(Some("API_KEY = os.environ[\"API_KEY\"]"));
    assert_eq!(overwritten.binding, FindingBinding::Historical, "its line holds other code now");
}

/// A secret whose line now holds other code is `historical`, anchored to the
/// commit that had it there — committed away, or a different secret on the
/// line one character off this one.
#[test]
fn a_secret_whose_line_now_holds_other_code_is_historical() {
    let (_t, root) = gitleaks_repo(3);
    write(&root, SETTINGS, &format!("import os\n\n{}API_KEY = os.environ[\"API_KEY\"]\n", constants()));
    git(&root, Some("2026-10-04T09:00:00+00:00"), &["commit", "-q", "-am", "C4: read the key from the environment"]);
    ingest_history(&root);
    let settings = settings_finding(&root);
    assert_eq!((settings.binding, settings.path.clone(), settings.anchor.clone()), historical_at(C3, SETTINGS));
    assert_eq!(settings.start_line, Some(13), "its place at that commit");
    assert_eq!(settings.commits, [C3, C2, C1]);
    let anchored: i64 = Connection::open(crate::history::db_path(&root))
        .unwrap()
        .query_row("SELECT count(*) FROM security_files WHERE path = ?1", [SETTINGS], |r| r.get(0))
        .unwrap();
    assert_eq!(anchored, 0, "the file is still there, but nothing of today's file is claimed for it");

    let rotated = key().replace(SECRET, &format!("{}x", &SECRET[..SECRET.len() - 1]));
    assert_ne!(rotated, key());
    let near_miss = settings_with_line_13(&format!("{rotated}  # rotate me"));
    assert_eq!(near_miss.binding, FindingBinding::Historical, "another secret on the line is not this one");
}

/// The finding sits where the secret still is: the newest commit whose place
/// still holds it today, when the newest commit's place does not. Its commits
/// stay listed newest first, and its fingerprint is the one secret's.
#[test]
fn the_finding_sits_at_the_newest_place_that_still_holds_the_secret() {
    let (_t, at_c3) = gitleaks_repo(3);
    ingest_history(&at_c3);
    let at_c3 = settings_finding(&at_c3);

    let (_t2, root) = gitleaks_repo(3);
    // C2's file again: the secret back on line 3, line 13 a constant.
    write(&root, SETTINGS, &settings_at(2));
    ingest_history(&root);
    let settings = settings_finding(&root);
    assert_eq!((settings.binding, settings.path.as_deref(), settings.start_line), (FindingBinding::Bound, Some(SETTINGS), Some(3)));
    assert_eq!(settings.anchor, Some(CommitAnchor::new(C2.to_string(), SETTINGS)), "C2 is the newest place that holds it");
    assert_eq!(settings.commits, [C3, C2, C1]);
    assert_eq!(settings.fingerprint, at_c3.fingerprint, "where it sits is no part of its identity");
}

/// [FR-SC-03]: the commit's author, email and message — each in the report
/// Gitleaks wrote — have no column in a security table, and no cell of the
/// store or of the read model holds one.
#[test]
fn no_author_email_or_commit_message_is_stored() {
    let (_t, root) = gitleaks_repo(3);
    ingest_history(&root);

    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture("history.sarif")).unwrap()).unwrap();
    let mut forbidden: BTreeSet<String> = BTreeSet::new();
    for result in report["runs"][0]["results"].as_array().unwrap() {
        for field in ["author", "email", "commitMessage"] {
            forbidden.insert(result["partialFingerprints"][field].as_str().unwrap().to_string());
        }
    }
    assert_eq!(forbidden.len(), 5, "one author, one email, three messages: {forbidden:?}");

    let conn = Connection::open(crate::history::db_path(&root)).unwrap();
    let columns: Vec<(String, String)> = conn
        .prepare(
            "SELECT m.name, c.name FROM sqlite_master m, pragma_table_info(m.name) c
              WHERE m.type = 'table' AND m.name LIKE 'security_%' ORDER BY m.name, c.cid",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(columns.iter().any(|(_, c)| c == "anchor_commit"), "the scan reads the security tables: {columns:?}");
    for (table, column) in &columns {
        for word in ["author", "email", "committer", "commit_message", "subject"] {
            assert!(!column.contains(word), "{table}.{column} names a commit's {word}");
        }
    }
    let rows = super::tests::dump(&root);
    let read = serde_json::to_string(&read_latest(&root).unwrap()).unwrap();
    for value in &forbidden {
        for row in &rows {
            assert!(!row.contains(value.as_str()), "{row} holds {value:?}");
        }
        assert!(!read.contains(value.as_str()), "the read model holds {value:?}");
    }
}

/// Every `security_findings` column migration 8 leaves alone, quoted, row by
/// row in key order: what its table rebuild must copy unchanged.
fn untouched_columns(conn: &Connection) -> Vec<String> {
    const COLUMNS: [&str; 15] = [
        "snapshot_id", "fingerprint", "rule_id", "severity", "cwes", "message", "redacted", "report_path",
        "start_line", "start_column", "end_line", "end_column", "scoped", "commits", "legacy_fingerprints",
    ];
    let row = COLUMNS.map(|c| format!("quote({c})")).join(" || '|' || ");
    conn.prepare(&format!("SELECT {row} FROM security_findings ORDER BY snapshot_id, fingerprint"))
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// `(fingerprint, binding, path, anchor_commit, scoped, commits)` of a stored row.
type MigratedRow = (String, String, Option<String>, Option<String>, i64, String);

/// history.db 7 → 8: `anchor_commit` arrives; every stored history-mode row is
/// anchored to its newest commit and `historical` until checked, every other
/// row is copied as it was, the bindings' CHECKs still hold, and the stored
/// Gitleaks snapshot's report hash is marked — so re-ingesting that very
/// report binds the secret still on its line again, under its fingerprint.
#[test]
fn migration_8_anchors_stored_history_rows_until_the_next_ingest_checks_them() {
    let (_s, scratch) = gitleaks_repo(3);
    ingest_history(&scratch);
    let known = settings_finding(&scratch);
    let gone = legacy_finding(&scratch);

    let (_t, root) = gitleaks_repo(3);
    let path = crate::history::db_path(&root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let report_hash = blake3::hash(&std::fs::read(fixture("history.sarif")).unwrap()).to_hex().to_string();
    {
        let conn = crate::history::db::open_at_version(&path, 7).unwrap();
        for (id, name, hash) in [(1, "gitleaks", report_hash.as_str()), (2, "Semgrep OSS", "other")] {
            conn.execute(
                "INSERT INTO security_snapshots (id, head_sha, scanner_name, scanner_version, report_hash, format, created_at, full_head_sha)
                 VALUES (?1, ?2, ?3, '1', ?4, 'sarif', 1, ?2)",
                rusqlite::params![id, C3, name, hash],
            )
            .unwrap();
        }
        let insert = "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding,
                          report_path, path, start_line, start_column, end_line, end_column, scoped, commits, legacy_fingerprints)
                      VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'bound', ?8, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)";
        let commits = format!("{C3},{C2},{C1}");
        let legacy = known.legacy_fingerprints.join(",");
        // Every row differs from the others in every column a copy could swap.
        for (id, fingerprint, rule, severity, cwes, message, redacted, file, region, scoped, commits, legacy) in [
            (1, known.fingerprint.as_str(), "generic-api-key", "high", "CWE-798", "redacted", 1, SETTINGS, [13, 1, 13, 41], 0, commits.as_str(), legacy.as_str()),
            (1, "scoped-row", "generic-api-key", "medium", "", "scoped", 0, "app/main.py", [2, 3, 4, 5], 1, "", "old-a,old-b"),
            (2, "semgrep-row", "python.sqli", "low", "CWE-89", "sql", 0, "app/main.py", [7, 8, 9, 10], 0, "", ""),
        ] {
            let [l1, c1, l2, c2] = region;
            conn.execute(
                insert,
                rusqlite::params![id, fingerprint, rule, severity, cwes, message, redacted, file, l1, c1, l2, c2, scoped, commits, legacy],
            )
            .unwrap();
        }
        // The deleted file's secret, in one commit: `unmatched`, no path.
        conn.execute(
            "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding,
                 report_path, start_line, start_column, end_line, end_column, commits, legacy_fingerprints)
             VALUES (1, ?1, 'generic-api-key', 'medium', '', 'redacted', 1, 'unmatched', ?2, 1, 1, 1, 42, ?3, ?4)",
            rusqlite::params![gone.fingerprint, LEGACY, C1, gone.legacy_fingerprints.join(",")],
        )
        .unwrap();
    }
    let before = untouched_columns(&Connection::open(&path).unwrap());

    // Through migration 8 alone: migration 9 marks every snapshot again.
    let migrated = crate::history::db::open_at_version(&path, 8).unwrap();
    assert_eq!(crate::history::db::current_version(&migrated).unwrap(), 8);
    assert_eq!(untouched_columns(&migrated), before, "every other column of every row is copied as it was");
    let index: i64 = migrated
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'security_findings_path' AND tbl_name = 'security_findings'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(index, 1, "the path index is rebuilt with the table");
    let rows: Vec<MigratedRow> = migrated
        .prepare(
            "SELECT fingerprint, binding, path, anchor_commit, scoped, commits FROM security_findings
              ORDER BY snapshot_id, fingerprint",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let main = Some("app/main.py".to_string());
    let mut expected = vec![
        (known.fingerprint.clone(), "historical".to_string(), None, Some(C3.to_string()), 0, format!("{C3},{C2},{C1}")),
        (gone.fingerprint.clone(), "historical".to_string(), None, Some(C1.to_string()), 0, C1.to_string()),
        ("scoped-row".to_string(), "bound".to_string(), main.clone(), None, 1, String::new()),
        ("semgrep-row".to_string(), "bound".to_string(), main, None, 0, String::new()),
    ];
    expected.sort();
    assert_eq!(rows, expected, "only the history-mode rows are anchored, each to its newest commit, and historical");
    let hashes: Vec<String> = migrated
        .prepare("SELECT report_hash FROM security_snapshots ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(hashes, [format!("pre-8:{report_hash}"), "other".to_string()], "only the Gitleaks snapshot is marked");
    for (binding, path) in [("bogus", None), ("historical", Some("a.py")), ("bound", None)] {
        let refused = migrated.execute(
            "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding, path)
             VALUES (2, 'checked', 'r', 'low', '', 'm', 0, ?1, ?2)",
            rusqlite::params![binding, path],
        );
        assert!(refused.is_err(), "{binding} with path {path:?} breaks a CHECK");
    }
    drop(migrated);

    let before = settings_finding(&root);
    assert_eq!((before.binding, before.path.clone(), before.anchor.clone()), historical_at(C3, SETTINGS), "read back before the next ingest");
    let one_commit = legacy_finding(&root);
    assert_eq!((one_commit.binding, one_commit.path.clone(), one_commit.anchor.clone()), historical_at(C1, LEGACY), "a row listing one commit");

    let summary = ingest_history(&root);
    assert!(!summary.already_ingested, "the marked snapshot's own report re-ingests");
    let after = settings_finding(&root);
    assert_eq!((after.binding, after.path.as_deref()), (FindingBinding::Bound, Some(SETTINGS)), "checked, and still on its line");
    assert_eq!(after.fingerprint, known.fingerprint, "the same finding");
}

/// history.db 6 → 8, the path a released store takes in one open: migration 7
/// records no commits, so a Gitleaks full-scan row stored before it — bound to
/// a line that may hold other code now — is `historical` too, anchored to no
/// commit; a scoped row, a row with no location and another scanner's row are
/// copied as they were. The next full scan checks each line again.
#[test]
fn a_store_from_before_migration_7_upgrades_its_gitleaks_full_scan_rows_to_historical() {
    let (_t, root) = gitleaks_repo(3);
    let path = crate::history::db_path(&root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let conn = crate::history::db::open_at_version(&path, 6).unwrap();
        for (id, name) in [(1, "gitleaks"), (2, "Semgrep OSS")] {
            conn.execute(
                "INSERT INTO security_snapshots (id, head_sha, scanner_name, scanner_version, report_hash, format, created_at, full_head_sha)
                 VALUES (?1, ?2, ?3, '1', 'r', 'sarif', 1, ?2)",
                rusqlite::params![id, C3, name],
            )
            .unwrap();
        }
        // Line 3 of today's file holds `X_1 = 1`: C1's place of the secret.
        let located = "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding,
                           report_path, path, start_line, start_column, end_line, end_column, scoped)
                       VALUES (?1, ?2, 'generic-api-key', 'medium', '', 'redacted', 1, 'bound', ?3, ?3, 3, 2, 3, 42, ?4)";
        for (id, fingerprint, file, scoped) in [(1, "full-row", SETTINGS, 0), (1, "scoped-row", SETTINGS, 1), (2, "semgrep-row", "app/main.py", 0)] {
            conn.execute(located, rusqlite::params![id, fingerprint, file, scoped]).unwrap();
        }
        conn.execute(
            "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding)
             VALUES (1, 'nowhere-row', 'generic-api-key', 'medium', '', 'redacted', 1, 'unbound')",
            [],
        )
        .unwrap();
    }

    let migrated = crate::history::db::open(&path).unwrap();
    let rows: Vec<(String, String, Option<String>, Option<String>)> = migrated
        .prepare("SELECT fingerprint, binding, path, anchor_commit FROM security_findings ORDER BY snapshot_id, fingerprint")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let at = |p: &str| Some(p.to_string());
    assert_eq!(
        rows,
        [
            ("full-row".to_string(), "historical".to_string(), None, None),
            ("nowhere-row".to_string(), "unbound".to_string(), None, None),
            ("scoped-row".to_string(), "bound".to_string(), at(SETTINGS), None),
            ("semgrep-row".to_string(), "bound".to_string(), at("app/main.py"), None),
        ],
        "only the located Gitleaks full-scan row was never checked"
    );
    drop(migrated);
    let full: Vec<SecurityFinding> = gitleaks_findings(&root).into_iter().filter(|f| f.fingerprint == "full-row").collect();
    assert_eq!((full[0].binding, full[0].anchor.clone()), (FindingBinding::Historical, None), "no commit is known for it");

    ingest_history(&root);
    let settings = settings_finding(&root);
    assert_eq!((settings.binding, settings.path.as_deref()), (FindingBinding::Bound, Some(SETTINGS)), "the next full scan checks it");
}
