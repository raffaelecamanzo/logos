//! One identity for a Gitleaks finding across scan modes and commits ([S-765],
//! [FR-SC-02], [FR-SC-17], [FR-SC-03], [UAT-SC-10]).
//!
//! Every test replays the reports Gitleaks 8.18.4 really wrote, with the
//! registry's `--redact`, over a fixture repository it rebuilds commit for
//! commit (`tests/fixtures/sarif/gitleaks-8.18.4/record.sh`; no scanner runs —
//! Sprint 106 Decision 40). One secret, `API_KEY` in
//! `tests/fixtures/settings.py`, is added in C1, edited on its line in C2 and
//! moved down ten lines in C3, so history mode reports it three times, at
//! columns `2..42`, `1..41` and `1..41`, while a scoped run reads `2..42` on
//! line 3 at C1 and on line 13 at C3. A second secret is in a file C2 deletes.
//!
//! [S-765]: ../../../../docs/planning/journal.md#s-765-a-gitleaks-finding-keeps-one-identity-across-scan-modes-and-commits
//! [FR-SC-02]: ../../../../docs/specs/requirements/FR-SC-02.md
//! [FR-SC-03]: ../../../../docs/specs/requirements/FR-SC-03.md
//! [FR-SC-17]: ../../../../docs/specs/requirements/FR-SC-17.md
//! [UAT-SC-10]: ../../../../docs/specs/requirements/UAT-SC-10.md

use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::NaiveDate;
use rusqlite::Connection;
use tempfile::TempDir;

use super::{ingest, ingest_scoped, label, read_latest, FindingBinding, IgnoreFile, IngestScope, SecurityFinding};

pub(super) const SECRET: &str = "lgs_pl4nted_S3cr3t_q7Vx9Rz2Kw";
pub(super) const LEGACY_SECRET: &str = "lgs_l3gacy_R8wY2pQ5nM4kT7";
pub(super) const SETTINGS: &str = "tests/fixtures/settings.py";
pub(super) const C1: &str = "05ea98774379a22652c0c3b1b69faf9506c34b2d";
pub(super) const C2: &str = "f8ee3bea8cc926e1c055faa25e1d17974e12016f";
pub(super) const C3: &str = "906f13898d55a9c395e64e94588643268f7c4d3a";

pub(super) fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sarif/gitleaks-8.18.4").join(name)
}

pub(super) fn key() -> String {
    format!("API_KEY = \"{SECRET}\"")
}

pub(super) fn constants() -> String {
    (1..=10).map(|n| format!("X_{n} = {n}\n")).collect()
}

/// `tests/fixtures/settings.py` at commit `n` (1..=3), as `record.sh` writes it.
pub(super) fn settings_at(n: usize) -> String {
    match n {
        1 => format!("import os\n\n{}\n{}", key(), constants()),
        2 => format!("import os\n\n{}  # rotate me\n{}", key(), constants()),
        _ => format!("import os\n\n{}{}  # rotate me\n", constants(), key()),
    }
}

/// git in `cwd` with `record.sh`'s identity, the ceiling above the fixture and
/// auto-maintenance off; `date` dates a commit.
pub(super) fn git(cwd: &Path, date: Option<&str>, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(cwd)
        .env("GIT_CEILING_DIRECTORIES", cwd.parent().unwrap())
        .args(["-c", "user.name=Fixture Dev", "-c", "user.email=dev@example.test", "-c", "commit.gpgsign=false"])
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        .args(args);
    if let Some(date) = date {
        cmd.env("GIT_AUTHOR_DATE", date).env("GIT_COMMITTER_DATE", date);
    }
    let out = cmd.output().expect("git is on PATH");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub(super) fn write(root: &Path, path: &str, body: &str) {
    let full = root.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, body).unwrap();
}

/// The fixture repository committed through commit `through` (1..=3) — the
/// commits `record.sh` makes, so they get the SHAs the reports carry.
pub(super) fn gitleaks_repo(through: usize) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("temp root");
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, None, &["init", "-q", "-b", "main"]);
    // The store and the ignore file a test writes stay out of every commit.
    std::fs::write(root.join(".git/info/exclude"), ".logos/\n").unwrap();
    write(&root, "app/main.py", "print(\"hello\")\n");
    write(&root, SETTINGS, &settings_at(1));
    write(&root, "legacy/client.py", &format!("CLIENT_TOKEN = \"{LEGACY_SECRET}\"\n"));
    git(&root, None, &["add", "-A"]);
    git(&root, Some("2026-10-01T09:00:00+00:00"), &["commit", "-q", "-m", "C1: add the api client"]);
    for n in 2..=through {
        commit(&root, n);
    }
    (tmp, root)
}

/// Make commit `n` (2 or 3) of the fixture on top of the one before it.
fn commit(root: &Path, n: usize) {
    write(root, SETTINGS, &settings_at(n));
    if n == 2 {
        std::fs::remove_file(root.join("legacy/client.py")).unwrap();
    }
    git(root, None, &["add", "-A"]);
    let (date, message) = match n {
        2 => ("2026-10-02T09:00:00+00:00", "C2: note the rotation"),
        _ => ("2026-10-03T09:00:00+00:00", "C3: move the key below the constants"),
    };
    git(root, Some(date), &["commit", "-q", "-m", message]);
}

pub(super) fn indexed(root: &Path) -> Vec<String> {
    ["app/main.py", SETTINGS, "legacy/client.py"].iter().filter(|p| root.join(p).exists()).map(|p| p.to_string()).collect()
}

pub(super) fn ingest_history(root: &Path) -> super::SecurityIngestSummary {
    ingest(root, &fixture("history.sarif"), &indexed(root)).unwrap()
}

fn scope() -> Vec<String> {
    vec!["tests/fixtures".to_string()]
}

fn ingest_scoped_run(root: &Path, report: &str) -> super::SecurityIngestSummary {
    let paths = scope();
    let scope = IngestScope { paths: &paths, keeps_full_findings: true };
    ingest_scoped(root, &fixture(report), &indexed(root), Some(scope)).unwrap()
}

pub(super) fn gitleaks_findings(root: &Path) -> Vec<SecurityFinding> {
    read_latest(root).unwrap().findings.into_iter().filter(|f| f.scanner_name == "gitleaks").collect()
}

pub(super) fn settings_finding(root: &Path) -> SecurityFinding {
    let mut found: Vec<SecurityFinding> =
        gitleaks_findings(root).into_iter().filter(|f| f.report_path.as_deref() == Some(SETTINGS)).collect();
    assert_eq!(found.len(), 1, "one finding for the secret: {found:#?}");
    found.remove(0)
}

/// The test repository is the one the reports were recorded on: its commits
/// have the SHAs the history report names, so every blob a test reads is the
/// one Gitleaks read.
#[test]
fn the_rebuilt_repository_has_the_recorded_commits() {
    let (_t, root) = gitleaks_repo(3);
    assert_eq!(git(&root, None, &["log", "--format=%H"]).lines().collect::<Vec<_>>(), [C3, C2, C1]);
    let report = std::fs::read_to_string(fixture("history.sarif")).unwrap();
    for sha in [C1, C2, C3] {
        assert!(report.contains(sha), "{sha} is in the recorded report");
    }
    assert!(report.contains("\"text\": \"REDACTED\"") && !report.contains(SECRET), "recorded with --redact");
}

/// One secret present in three commits is one finding listing the three,
/// newest first, at the place the newest commit has it; the secret in the
/// deleted file is one finding of its own. Each keeps the report keys of its
/// results as legacy fingerprints.
#[test]
fn one_secret_in_three_commits_is_one_finding_listing_them() {
    let (_t, root) = gitleaks_repo(3);
    let summary = ingest_history(&root);
    assert_eq!((summary.findings, summary.scanners[0].findings), (2, 2), "four results, two secrets");

    let settings = settings_finding(&root);
    assert_eq!(settings.commits, [C3, C2, C1]);
    assert_eq!((settings.binding, settings.path.as_deref()), (FindingBinding::Bound, Some(SETTINGS)));
    assert_eq!((settings.start_line, settings.start_column, settings.end_column), (Some(13), Some(1), Some(41)), "C3's place");
    assert_eq!(settings.legacy_fingerprints.len(), 6, "one report key per commit, as keyed now and before migration 11");

    let legacy: Vec<SecurityFinding> =
        gitleaks_findings(&root).into_iter().filter(|f| f.report_path.as_deref() == Some("legacy/client.py")).collect();
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].commits, [C1]);
    assert_eq!(legacy[0].binding, FindingBinding::Historical, "the file is gone at HEAD (S-767)");
    assert_ne!(legacy[0].fingerprint, settings.fingerprint);
}

/// The fingerprint is one across modes, commits and lines: the scoped run at
/// C1 (line 3) and at C3 (line 13), each in a store of its own, and the full
/// scan's finding of three commits all key the secret alike — and a scoped
/// re-scan over the full scan adds nothing and keeps the full scan's row.
#[test]
fn the_fingerprint_is_unchanged_by_a_scoped_rescan_its_commit_and_a_line_move() {
    let (_t1, at_c1) = gitleaks_repo(1);
    ingest_scoped_run(&at_c1, "scoped-c1.sarif");
    let on_line_3 = settings_finding(&at_c1);
    assert_eq!((on_line_3.start_line, on_line_3.commits.len()), (Some(3), 0));

    let (_t3, at_c3) = gitleaks_repo(3);
    ingest_scoped_run(&at_c3, "scoped-c3.sarif");
    let on_line_13 = settings_finding(&at_c3);
    assert_eq!(on_line_13.start_line, Some(13));
    assert_eq!(on_line_13.fingerprint, on_line_3.fingerprint, "the line moved ten lines down");

    let (_t, root) = gitleaks_repo(3);
    ingest_history(&root);
    let full = settings_finding(&root);
    assert_eq!(full.fingerprint, on_line_3.fingerprint, "history mode keys it as content mode does");

    let rescan = ingest_scoped_run(&root, "scoped-c3.sarif");
    assert!(!rescan.already_ingested);
    let after = gitleaks_findings(&root);
    assert_eq!(after.len(), 2, "no copy is added: {after:#?}");
    assert_eq!(settings_finding(&root), full, "the full scan's row is kept as it was");
}

/// [UAT-SC-10] / CR-238 D1: a secret first recorded by a scoped run, before it
/// is committed, and suppressed by its fingerprint, is still suppressed — the
/// same fingerprint — once committed and found by a full scan.
#[test]
fn a_suppressed_secret_first_recorded_by_a_scoped_run_stays_suppressed_after_its_commit() {
    let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
    let (_t, root) = gitleaks_repo(2);
    write(&root, SETTINGS, &settings_at(3));
    ingest_scoped_run(&root, "scoped-c3.sarif");
    let recorded = settings_finding(&root).fingerprint;
    super::suppress(&root, recorded.as_str(), "fixture secret, never deployed", None, today).unwrap();

    commit(&root, 3);
    assert_eq!(git(&root, None, &["rev-parse", "HEAD"]).trim(), C3);
    ingest_history(&root);
    let ignore = IgnoreFile::load(&root).unwrap();
    let labelled = label(&gitleaks_findings(&root), &ignore, &root, today);
    let settings: Vec<_> =
        labelled.findings.iter().filter(|l| l.finding.report_path.as_deref() == Some(SETTINGS)).collect();
    assert_eq!(settings.len(), 1);
    assert_eq!(settings[0].finding.fingerprint, recorded, "the same fingerprint after its commit");
    assert_eq!(settings[0].finding.commits, [C3, C2, C1]);
    assert!(settings[0].is_suppressed(), "still suppressed");
    assert!(labelled.entries.iter().all(|e| !e.orphaned), "the entry still names a finding");
}

/// [FR-SC-03]: after every ingest, no `history.db` column holds either secret,
/// the text Gitleaks matched, an unkeyed hash of either, a keyed digest or
/// the key — every cell of every table and every byte of the file is read.
#[test]
fn no_secret_and_no_unkeyed_hash_of_one_is_stored() {
    let (_t, root) = gitleaks_repo(3);
    ingest_history(&root);
    ingest_scoped_run(&root, "scoped-c3.sarif");
    let reads = [(C3, SETTINGS, region(13, 1, 41)), (C1, "legacy/client.py", region(1, 1, 42))];
    assert_holds_none(&root, &secret_material(&root, &reads), &[]);
}

/// The keys the old identity gave the history report's results, as a store
/// written before migration 7 holds them: each finding's legacy fingerprints,
/// learned by ingesting into a scratch repository.
fn old_keys() -> Vec<(String, String)> {
    let (_t, root) = gitleaks_repo(3);
    ingest_history(&root);
    gitleaks_findings(&root)
        .into_iter()
        .flat_map(|f| f.legacy_fingerprints.into_iter().map(move |old| (old, f.fingerprint.clone())))
        .collect()
}

/// history.db 6 → 7: the columns arrive empty and the stored Gitleaks
/// snapshot's report hash is marked; the next Gitleaks ingest — of the very
/// report that snapshot came from — is no no-op: it re-keys the rows of an
/// older snapshot, merging the per-commit repeats of one secret into one, and
/// the ratchet baseline's fingerprints, so nothing reads as new.
#[test]
fn migration_7_rekeys_stored_gitleaks_rows_and_the_baseline_on_the_next_ingest() {
    let old = old_keys();
    assert_eq!(old.len(), 8, "four results, each with its report key now and before migration 11");
    let (_t, root) = gitleaks_repo(3);
    let path = crate::history::db_path(&root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let report = std::fs::read(fixture("history.sarif")).unwrap();
    {
        let conn = crate::history::db::open_at_version(&path, 6).unwrap();
        for (id, head, hash) in [(1, C2.to_string(), "older".to_string()), (2, C3.to_string(), blake3::hash(&report).to_hex().to_string())] {
            conn.execute(
                "INSERT INTO security_snapshots (id, head_sha, scanner_name, scanner_version, report_hash, format, created_at, full_head_sha)
                 VALUES (?1, ?2, 'gitleaks', 'v8.18.4', ?3, 'sarif', 1, ?2)",
                rusqlite::params![id, head, hash],
            )
            .unwrap();
            for (fingerprint, _) in &old {
                conn.execute(
                    "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding, report_path)
                     VALUES (?1, ?2, 'generic-api-key', 'medium', '', 'redacted', 1, 'unmatched', 'x')",
                    rusqlite::params![id, fingerprint],
                )
                .unwrap();
            }
        }
        conn.execute("INSERT INTO security_baseline (id, head_sha, config_hash, created_at) VALUES (1, 'h', 'c', 1)", []).unwrap();
        // Another scanner's snapshot is left as it is; a Gitleaks one is told by
        // its driver name, ASCII case and surrounding space ignored.
        for (id, name) in [(3, "Semgrep OSS"), (4, " Gitleaks ")] {
            conn.execute(
                "INSERT INTO security_snapshots (id, head_sha, scanner_name, scanner_version, report_hash, format, created_at, full_head_sha)
                 VALUES (?1, ?2, ?3, '1', 'other', 'sarif', 1, ?2)",
                rusqlite::params![id, C1, name],
            )
            .unwrap();
        }
        // A pre-7 `gate --save` recorded every per-commit key of a secret: all
        // of them map onto one new key, which the baseline holds once.
        for (fingerprint, _) in &old {
            conn.execute("INSERT INTO security_baseline_findings (fingerprint, scanner_name) VALUES (?1, 'gitleaks')", [fingerprint]).unwrap();
        }
    }

    // Through migration 7 alone: migration 8 marks the same hashes again.
    let migrated = crate::history::db::open_at_version(&path, 7).unwrap();
    let marked: Vec<String> = migrated
        .prepare("SELECT report_hash FROM security_snapshots ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        marked,
        ["pre-7:older".to_string(), format!("pre-7:{}", blake3::hash(&report).to_hex()), "other".to_string(), "pre-7:other".to_string()],
        "every Gitleaks snapshot is marked, and only Gitleaks ones"
    );
    let empty: i64 = migrated
        .query_row("SELECT count(*) FROM security_findings WHERE commits = '' AND legacy_fingerprints = ''", [], |r| r.get(0))
        .unwrap();
    assert_eq!(empty, 16, "every stored row — two snapshots of eight — arrives with both columns empty");
    drop(migrated);

    let summary = ingest_history(&root);
    assert!(!summary.already_ingested, "the marked snapshot's own report re-ingests");
    let conn = Connection::open(&path).unwrap();
    let older: Vec<(String, String)> = conn
        .prepare("SELECT fingerprint, legacy_fingerprints FROM security_findings WHERE snapshot_id = 1 ORDER BY fingerprint")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let current: Vec<SecurityFinding> = gitleaks_findings(&root);
    let mut expected: Vec<(String, String)> = current
        .iter()
        .map(|f| (f.fingerprint.clone(), f.legacy_fingerprints.join(",")))
        .collect();
    expected.sort();
    assert_eq!(older, expected, "the older snapshot's four rows are the two findings, under the new keys");
    let baseline: Vec<String> = conn
        .prepare("SELECT fingerprint FROM security_baseline_findings")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut baseline = baseline;
    baseline.sort();
    let mut new_keys: Vec<String> = old.iter().map(|(_, new)| new.clone()).collect();
    new_keys.sort();
    new_keys.dedup();
    assert_eq!(new_keys.len(), 2, "eight old keys, two secrets");
    assert_eq!(baseline, new_keys, "the baseline names each new key once, and no old one");
}

/// The committed ignore file is never rewritten: an entry naming a key from
/// before migration 7 still suppresses its secret's finding (CR-241: every
/// surface honours an old key), and names the fingerprint it should name.
#[test]
fn an_ignore_entry_under_the_old_identity_still_suppresses_and_names_the_new_fingerprint() {
    let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
    let old = old_keys();
    let (_t, root) = gitleaks_repo(3);
    let text = format!("[[ignore]]\nfingerprint = \"{}\"\nreason = \"fixture\"\n", old[0].0);
    write(&root, super::IGNORE_FILE, &text);
    ingest_history(&root);
    let labelled = label(&gitleaks_findings(&root), &IgnoreFile::load(&root).unwrap(), &root, today);
    let entry = &labelled.entries[0];
    assert!(!entry.orphaned && entry.suppressed == 1, "the old key still names its finding: {entry:?}");
    assert_eq!(entry.should_name.as_deref(), Some(old[0].1.as_str()));
    let suppressed: Vec<&str> =
        labelled.findings.iter().filter(|f| f.is_suppressed()).map(|f| f.finding.fingerprint.as_str()).collect();
    assert_eq!(suppressed, [old[0].1.as_str()], "exactly the secret its old key named");
    assert_eq!(std::fs::read_to_string(root.join(super::IGNORE_FILE)).unwrap(), text, "the file is untouched");
}

/// A row stored before migration 7 inside a scoped run's paths is matched by
/// position, as before: the scoped finding is not stored a second time, and the
/// row keeps its key — a position match is no proof that it is the same secret,
/// so it never re-keys a row or the baseline (a rotated secret on the same
/// columns would otherwise inherit the old one's acceptance). The next full
/// scan re-keys it by its exact report key.
#[test]
fn a_pre_migration_row_matched_by_position_keeps_its_key_and_is_not_duplicated() {
    let (_t, root) = gitleaks_repo(3);
    let path = crate::history::db_path(&root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let conn = crate::history::db::open_at_version(&path, 6).unwrap();
        conn.execute(
            "INSERT INTO security_snapshots (id, head_sha, scanner_name, scanner_version, report_hash, format, created_at, full_head_sha)
             VALUES (1, ?1, 'gitleaks', 'v8.18.4', 'r', 'sarif', 1, ?1)",
            [C3],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding,
                 report_path, path, start_line, start_column, end_line, end_column)
             VALUES (1, 'old-key', 'generic-api-key', 'medium', '', 'redacted', 1, 'bound', ?1, ?1, 13, 2, 13, 42)",
            [SETTINGS],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO security_files (snapshot_id, path, content_hash) VALUES (1, ?1, 'h')",
            [SETTINGS],
        )
        .unwrap();
        conn.execute("INSERT INTO security_baseline (id, head_sha, config_hash, created_at) VALUES (1, 'h', 'c', 1)", []).unwrap();
        conn.execute("INSERT INTO security_baseline_findings (fingerprint, scanner_name) VALUES ('old-key', 'gitleaks')", []).unwrap();
    }
    ingest_scoped_run(&root, "scoped-c3.sarif");
    let settings = settings_finding(&root);
    assert_eq!((settings.fingerprint.as_str(), settings.legacy_fingerprints.len()), ("old-key", 0), "matched, kept, not re-keyed");
    let baseline: String = Connection::open(&path)
        .unwrap()
        .query_row("SELECT fingerprint FROM security_baseline_findings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(baseline, "old-key", "a position match never moves the baseline");
}

/// `tests/fixtures/sarif/gitleaks-8.18.4/neighbours/`: two secrets whose match
/// touches a byte that is no delimiter — `_` before `API_KEY` in `MY_API_KEY`,
/// `.` after `TOKEN`'s closing quote — each written late in C1 and edited
/// alone on its line, its hunk's first, in C2 and C3.
fn neighbours_repo() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("temp root");
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, None, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join(".git/info/exclude"), ".logos/\n").unwrap();
    let prefixed = "MY_API_KEY = \"lgs_pr3fixed_K9wQ2xT7vR4mZ8\"";
    let trailing = "TOKEN = \"lgs_tr4iling_P5nW8qL3kD6sJ2\".strip()";
    for (body, date, message) in [
        (format!("import os\n\n{prefixed}\n{trailing}\n"), "2026-10-01T09:00:00+00:00", "C1: settings"),
        (format!("import os\n\n{prefixed}  # rotate me\n{trailing}\n"), "2026-10-02T09:00:00+00:00", "C2: note the key rotation"),
        (format!("import os\n\n{prefixed}  # rotate me\n{trailing}  # and me\n"), "2026-10-03T09:00:00+00:00", "C3: note the token rotation"),
    ] {
        write(&root, "conf/settings.py", &body);
        git(&root, None, &["add", "-A"]);
        git(&root, Some(date), &["commit", "-q", "-m", message]);
    }
    (tmp, root)
}

/// A secret is one finding whatever bytes stand next to its match: history
/// mode reads each secret late in C1 and as given in the commit that edited
/// its line alone, and a scoped run reads both late — every reading keys each
/// secret alike, and the scoped re-scan adds no copy.
#[test]
fn a_secret_glued_to_its_neighbours_keeps_one_identity() {
    const N1: &str = "7b0550dc003be6886ef8de671aef2e2498b5e8bc";
    const N2: &str = "784c7abbb5f6e5f6f77caa38d5480df6ab0460a7";
    const N3: &str = "3680fcd315352851509e181f950d834aae053803";
    let (_t, root) = neighbours_repo();
    assert_eq!(git(&root, None, &["log", "--format=%H"]).lines().collect::<Vec<_>>(), [N3, N2, N1]);
    let indexed = vec!["conf/settings.py".to_string()];
    ingest(&root, &fixture("neighbours/history.sarif"), &indexed).unwrap();
    let mut full: Vec<(Option<i64>, Vec<String>, String)> =
        gitleaks_findings(&root).into_iter().map(|f| (f.start_line, f.commits, f.fingerprint)).collect();
    full.sort();
    let commits: Vec<(Option<i64>, Vec<String>)> = full.iter().map(|(l, c, _)| (*l, c.clone())).collect();
    assert_eq!(commits, [(Some(3), vec![N2.to_string(), N1.to_string()]), (Some(4), vec![N3.to_string(), N1.to_string()])]);

    let paths = vec!["conf".to_string()];
    ingest_scoped(&root, &fixture("neighbours/scoped.sarif"), &indexed, Some(IngestScope { paths: &paths, keeps_full_findings: true })).unwrap();
    let mut after: Vec<(Option<i64>, Vec<String>, String)> =
        gitleaks_findings(&root).into_iter().map(|f| (f.start_line, f.commits, f.fingerprint)).collect();
    after.sort();
    assert_eq!(after, full, "the scoped run reads both secrets as the recorded ones");

    let (_s, alone) = neighbours_repo();
    ingest_scoped(&alone, &fixture("neighbours/scoped.sarif"), &indexed, Some(IngestScope { paths: &paths, keeps_full_findings: true })).unwrap();
    let mut scoped: Vec<String> = gitleaks_findings(&alone).into_iter().map(|f| f.fingerprint).collect();
    scoped.sort();
    let mut keys: Vec<String> = full.into_iter().map(|(_, _, f)| f).collect();
    keys.sort();
    assert_eq!(scoped, keys, "a scoped run alone keys them as history mode does");
}

/// A report-supplied commit is read only as a commit id: a revision
/// expression or an option-shaped value reads nothing, never handing git
/// something to resolve; the same region at the real commit reads.
#[test]
fn a_commit_that_is_not_an_id_reads_nothing() {
    let (_t, root) = gitleaks_repo(3);
    let mut digests = super::secret::SecretDigests::open(&root).expect("a repository key");
    let region = super::Region { start_line: Some(13), start_column: Some(1), end_line: Some(13), end_column: Some(41) };
    for commit in ["HEAD", "main~1", "-x", "", "906f1389 "] {
        assert_eq!(digests.digest(Some(commit), SETTINGS, &region), None, "{commit:?}");
    }
    assert!(digests.digest(Some(C3), SETTINGS, &region).is_some(), "the commit id itself reads");
}

/// A Gitleaks 8.18.4-shaped report (`--redact`) of `results`, each
/// `(commit, date, uri, line, start column, end column)` of `generic-api-key`;
/// an empty commit is a scoped run's.
fn report(dir: &Path, name: &str, results: &[(&str, &str, &str, i64, i64, i64)]) -> PathBuf {
    let results: Vec<serde_json::Value> = results
        .iter()
        .map(|&(commit, date, uri, line, start, end)| {
            serde_json::json!({
                "ruleId": "generic-api-key",
                "message": { "text": "generic-api-key has detected secret." },
                "locations": [{ "physicalLocation": {
                    "artifactLocation": { "uri": uri },
                    "region": { "startLine": line, "startColumn": start, "endLine": line, "endColumn": end,
                                "snippet": { "text": "REDACTED" } }
                } }],
                "partialFingerprints": { "commitSha": commit, "email": "", "author": "", "date": date, "commitMessage": "" }
            })
        })
        .collect();
    let body = serde_json::json!({
        "version": "2.1.0",
        "runs": [{ "tool": { "driver": { "name": "gitleaks", "semanticVersion": "v8.18.4" } }, "results": results }]
    });
    let path = dir.join(name);
    std::fs::write(&path, body.to_string()).unwrap();
    path
}

/// One secret twice in one commit — on the first line its hunk added and on
/// the next, so read under both column readings — is one finding listing that
/// commit once.
#[test]
fn one_secret_twice_in_one_commit_lists_the_commit_once() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, None, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join(".git/info/exclude"), ".logos/\n").unwrap();
    write(&root, SETTINGS, &format!("{}\n{}\n", key(), key()));
    git(&root, None, &["add", "-A"]);
    git(&root, Some("2026-10-01T09:00:00+00:00"), &["commit", "-q", "-m", "twice"]);
    let head = git(&root, None, &["rev-parse", "HEAD"]).trim().to_string();
    let date = "2026-10-01T09:00:00Z";
    let twice = report(tmp.path(), "twice.sarif", &[(&head, date, SETTINGS, 1, 1, 41), (&head, date, SETTINGS, 2, 2, 42)]);
    ingest(&root, &twice, &[SETTINGS.to_string()]).unwrap();
    let found = gitleaks_findings(&root);
    assert_eq!(found.len(), 1, "one secret, one finding: {found:#?}");
    assert_eq!(found[0].commits, [head], "the commit once");
    assert_eq!(found[0].legacy_fingerprints.len(), 4, "both results' report keys, now and before migration 11");
}

/// A scoped report that names its file otherwise than the index does — here by
/// its absolute path — is read and keyed by the indexed path it binds to, so
/// it keys the secret as history mode does.
#[test]
fn a_scoped_report_naming_its_file_by_an_absolute_path_keys_it_as_history_does() {
    let (_t, root) = gitleaks_repo(3);
    ingest_history(&root);
    let full = settings_finding(&root).fingerprint;

    let (scratch, alone) = gitleaks_repo(3);
    let absolute = alone.join(SETTINGS).display().to_string();
    let text = std::fs::read_to_string(fixture("scoped-c3.sarif")).unwrap();
    assert_eq!(text.matches(&format!("\"uri\": \"{SETTINGS}\"")).count(), 1, "one result to rewrite");
    let rewritten = scratch.path().join("absolute.sarif");
    std::fs::write(&rewritten, text.replace(&format!("\"uri\": \"{SETTINGS}\""), &format!("\"uri\": \"{absolute}\""))).unwrap();
    let paths = scope();
    ingest_scoped(&alone, &rewritten, &indexed(&alone), Some(IngestScope { paths: &paths, keeps_full_findings: true })).unwrap();
    let found = gitleaks_findings(&alone);
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].path.as_deref(), found[0].report_path.as_deref()), (Some(SETTINGS), Some(absolute.as_str())));
    assert_eq!(found[0].fingerprint, full, "keyed by the bound path, read from the bound file");
}

/// The `[security]` ratchet's violations over `root`, through the engine with
/// no scanner on its search path — what `check` reports.
pub(super) fn ratchet_violations(root: &Path) -> Vec<String> {
    use crate::security::{ScanEnvironment, SecurityGateEvidence, VersionProbes};
    let user = root.parent().unwrap().join("user-config");
    std::fs::create_dir_all(&user).unwrap();
    let engine = crate::engine::Engine::open(root).with_scan_environment(ScanEnvironment {
        user_dir: Some(user),
        search_path: Some(std::ffi::OsString::new()),
        tools_dir: None,
        nvd_api_key: None,
    });
    let status = crate::security::status(
        engine.root(),
        &engine.scan_environment(),
        engine.tools_manifest(),
        crate::engine::today_utc(),
        VersionProbes::Never,
        false,
        crate::security::scope::no_test_symbols,
    )
    .unwrap();
    let band = SecurityGateEvidence(&engine).band(&status).unwrap().expect("a [security] table");
    band.violations.into_iter().filter(|v| v.rule == "security:ratchet").map(|v| v.message).collect()
}

/// S-764 × S-765: a checked-in baseline blessed from a store migrated past 7
/// but not yet re-keyed records the old Gitleaks keys. The next Gitleaks
/// ingest re-keys `history.db`, never the checked-in file, and a blessed secret
/// still reads as known — by the key it had.
#[test]
fn a_checked_in_baseline_blessed_before_the_rekey_still_knows_each_secret() {
    let rows: Vec<(String, String, String)> = {
        let (_t, root) = gitleaks_repo(3);
        ingest_history(&root);
        gitleaks_findings(&root)
            .into_iter()
            .flat_map(|f| {
                let (rule, path) = (f.rule_id.clone(), f.report_path.clone().unwrap());
                f.legacy_fingerprints.into_iter().map(move |old| (old, rule.clone(), path.clone()))
            })
            .collect()
    };
    let (_t, root) = gitleaks_repo(3);
    write(&root, ".logos/rules.toml", "[security]\nratchet_min_severity = \"info\"\n");
    let path = crate::history::db_path(&root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let report = std::fs::read(fixture("history.sarif")).unwrap();
    {
        let conn = crate::history::db::open_at_version(&path, 6).unwrap();
        conn.execute(
            "INSERT INTO security_snapshots (id, head_sha, scanner_name, scanner_version, report_hash, format, created_at, full_head_sha)
             VALUES (1, ?1, 'gitleaks', 'v8.18.4', ?2, 'sarif', 1, ?1)",
            rusqlite::params![C3, blake3::hash(&report).to_hex().to_string()],
        )
        .unwrap();
        for (fingerprint, rule, report_path) in &rows {
            conn.execute(
                "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding, report_path)
                 VALUES (1, ?1, ?2, 'medium', '', 'redacted', 1, 'unmatched', ?3)",
                rusqlite::params![fingerprint, rule, report_path],
            )
            .unwrap();
        }
    }
    let user = root.parent().unwrap().join("user-config");
    std::fs::create_dir_all(&user).unwrap();
    let blessed = crate::engine::Engine::open(&root)
        .with_scan_environment(crate::security::ScanEnvironment {
            user_dir: Some(user),
            search_path: Some(std::ffi::OsString::new()),
            tools_dir: None,
            nvd_api_key: None,
        })
        .security_bless()
        .unwrap();
    assert!(blessed.recorded, "{blessed:?}");
    assert_eq!(ratchet_violations(&root), Vec::<String>::new(), "the old keys on both sides");

    assert!(!ingest_history(&root).already_ingested, "the next scan re-keys the rows");
    assert_eq!(ratchet_violations(&root), Vec::<String>::new(), "a blessed secret is known by the key it had");
}

/// A one-commit project at `dir/name` holding a `Cargo.lock` the index does
/// not carry, under a `[security]` table whose ratchet counts every finding.
fn lockfile_project(dir: &Path, name: &str) -> PathBuf {
    let root = dir.join(name);
    std::fs::create_dir_all(&root).unwrap();
    git(&root, None, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join(".git/info/exclude"), ".logos/history.db*\n.logos/logos.db*\n").unwrap();
    write(&root, "Cargo.lock", "# lockfile\n");
    write(&root, "src/main.rs", "fn main() {}\n");
    write(&root, ".logos/rules.toml", "[security]\nratchet_min_severity = \"info\"\n");
    git(&root, None, &["add", "-A"]);
    git(&root, Some("2026-10-01T09:00:00+00:00"), &["commit", "-q", "-m", "c1"]);
    root
}

/// The advisory every OSV-Scanner fixture below reports.
const ADVISORY: &str = "CVE-2026-63127";

/// An osv-scanner 1.9.2 report naming [`ADVISORY`] against each of `paths`
/// under `root`, by absolute `file://` URI as osv-scanner writes it.
fn osv_lockfile_report(dir: &Path, root: &Path, paths: &[&str]) -> PathBuf {
    let uris: Vec<String> = paths.iter().map(|p| format!("file://{}/{p}", root.display())).collect();
    osv_report(dir, &format!("osv-{}.sarif", root.file_name().unwrap().to_string_lossy()), &uris)
}

/// An osv-scanner 1.9.2 report at `dir/name` naming [`ADVISORY`] against each
/// of `uris`, as written.
fn osv_report(dir: &Path, name: &str, uris: &[String]) -> PathBuf {
    let results: Vec<serde_json::Value> = uris
        .iter()
        .map(|uri| {
            serde_json::json!({
                "ruleId": ADVISORY,
                "level": "warning",
                "message": {"text": format!("Package 'rmcp@1.7.0' is vulnerable to '{ADVISORY}'.")},
                "locations": [{"physicalLocation": {"artifactLocation": {"uri": uri}}}]
            })
        })
        .collect();
    let sarif = serde_json::json!({"version": "2.1.0", "runs": [{
        "tool": {"driver": {"name": "osv-scanner", "version": "1.9.2", "rules": []}},
        "results": results
    }]});
    let report = dir.join(name);
    std::fs::write(&report, sarif.to_string()).unwrap();
    report
}

/// The key of a result with no region and no report identity keyed at
/// `path` — what Logos 1.20 gave an OSV-Scanner 1.x lockfile result at its
/// absolute path, and what migration 10 gives it at its project-relative one —
/// spelled out here apart from the code under test.
fn location_key(rule: &str, path: &str) -> String {
    blake3::hash(format!("logos-finding-v1\u{1f}location\u{1f}{rule}\u{1f}{path}\u{1f}:::").as_bytes()).to_hex().to_string()
}

/// The D3 stopgap switched off on this test's thread until dropped
/// ([`crate::security::STOPGAP_OFF`]), so a checked-in baseline knows a
/// finding by its key alone.
struct StopgapOff;

impl StopgapOff {
    fn new() -> Self {
        crate::security::STOPGAP_OFF.with(|off| off.set(true));
        Self
    }
}

impl Drop for StopgapOff {
    fn drop(&mut self) {
        crate::security::STOPGAP_OFF.with(|off| off.set(false));
    }
}

/// The OSV-Scanner findings stored under `root`, as `(report path, fingerprint)`.
fn osv_keys(root: &Path) -> Vec<(String, String)> {
    read_latest(root)
        .unwrap()
        .findings
        .into_iter()
        .filter(|f| f.scanner_name == "osv-scanner")
        .map(|f| (f.report_path.unwrap_or_default(), f.fingerprint))
        .collect()
}

/// `checkout`'s `.logos/security-baseline.json` written by `logos security
/// bless`, which must record `findings`.
fn bless(dir: &Path, checkout: &Path, findings: usize) {
    let user = dir.join("user-config");
    std::fs::create_dir_all(&user).unwrap();
    let blessed = crate::engine::Engine::open(checkout)
        .with_scan_environment(crate::security::ScanEnvironment {
            user_dir: Some(user),
            search_path: Some(std::ffi::OsString::new()),
            tools_dir: None,
            nvd_api_key: None,
        })
        .security_bless()
        .unwrap();
    assert!(blessed.recorded && blessed.findings == findings, "{blessed:?}");
}

/// S-778 (sprint 107 review D3, CR-241): an OSV-Scanner lockfile finding binds
/// to no indexed file and is reported by an absolute path, yet its key is its
/// rule and project-relative path. A baseline blessed in one checkout — the
/// set `gate --save` recorded in `history.db`, and the checked-in file — holds
/// it in a clone at another path by that key alone, with the stopgap's
/// `(rule id, path)` match switched off; the same advisory in `sub/Cargo.lock`
/// keeps a key of its own and reads as new from either.
#[test]
fn a_baseline_blessed_in_one_checkout_holds_in_a_clone_by_its_key_alone() {
    let _off = StopgapOff::new();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let a = lockfile_project(&dir, "checkout-a");
    ingest(&a, &osv_lockfile_report(&dir, &a, &["Cargo.lock"]), &["src/main.rs".to_string()]).unwrap();
    let blessed: Vec<(String, String)> =
        read_latest(&a).unwrap().findings.into_iter().map(|f| (f.scanner_name, f.fingerprint)).collect();
    assert_eq!(blessed.len(), 1);
    assert_eq!(blessed[0].1, location_key(ADVISORY, "Cargo.lock"), "keyed by the project-relative path");
    super::save_baseline(&a, None, "config", &blessed).unwrap();
    bless(&dir, &a, 1);
    assert_eq!(ratchet_violations(&a), Vec::<String>::new());
    git(&a, None, &["add", "-f", ".logos/security-baseline.json"]);
    git(&a, Some("2026-10-02T09:00:00+00:00"), &["commit", "-q", "-m", "bless"]);

    let b = dir.join("checkout-b");
    git(&dir, None, &["clone", "-q", a.to_str().unwrap(), b.to_str().unwrap()]);
    // The store `gate --save` wrote in A, carried to B as a moved checkout
    // carries it.
    for suffix in ["", "-wal", "-shm"] {
        let from = PathBuf::from(format!("{}{suffix}", crate::history::db_path(&a).display()));
        if from.exists() {
            std::fs::copy(&from, format!("{}{suffix}", crate::history::db_path(&b).display())).unwrap();
        }
    }
    ingest(&b, &osv_lockfile_report(&dir, &b, &["Cargo.lock", "sub/Cargo.lock"]), &["src/main.rs".to_string()]).unwrap();
    let keys = osv_keys(&b);
    assert_eq!(keys.len(), 2);
    assert!(keys.iter().any(|(_, key)| *key == blessed[0].1), "one key in both checkouts: {keys:?}");
    assert!(keys.iter().all(|(report, _)| report.starts_with(b.to_str().unwrap())), "both reported under B: {keys:?}");
    assert_ne!(keys[0].1, keys[1].1, "sub/Cargo.lock keeps a key of its own");

    let checked_in = b.join(".logos/security-baseline.json");
    let aside = b.join(".logos/security-baseline.aside");
    std::fs::rename(&checked_in, &aside).unwrap();
    let from_store = ratchet_violations(&b);
    assert_eq!(from_store.len(), 1, "only sub/Cargo.lock is new to history.db's baseline: {from_store:?}");
    assert!(from_store[0].contains("sub/Cargo.lock") && from_store[0].contains("gate --save"), "{from_store:?}");
    std::fs::rename(&aside, &checked_in).unwrap();
    let from_file = ratchet_violations(&b);
    assert_eq!(from_file.len(), 1, "only sub/Cargo.lock is new to the checked-in file: {from_file:?}");
    assert!(from_file[0].contains("sub/Cargo.lock") && from_file[0].contains("security-baseline.json"), "{from_file:?}");
}

/// A checked-in file blessed before migration 10 records the key the finding
/// had then, which carries the blessing checkout's directory: in another
/// checkout the finding is known only through the stopgap's `(rule id,
/// path)` match, which is kept for one release.
#[test]
fn a_checked_in_baseline_blessed_before_migration_10_stays_green_through_the_stopgap() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let b = lockfile_project(&dir, "checkout-b");
    let old = location_key(ADVISORY, "/home/dev/checkout-a/Cargo.lock");
    let file = serde_json::json!({"version": 1, "findings": [{"fingerprint": old, "rule_id": ADVISORY, "path": "Cargo.lock"}]});
    write(&b, ".logos/security-baseline.json", &format!("{file:#}\n"));
    ingest(&b, &osv_lockfile_report(&dir, &b, &["Cargo.lock"]), &["src/main.rs".to_string()]).unwrap();
    assert_eq!(ratchet_violations(&b), Vec::<String>::new(), "held by the stopgap");
    let _off = StopgapOff::new();
    assert_eq!(ratchet_violations(&b).len(), 1, "and by nothing else");
}

/// One fixture of `name` with each location URI pointing at `target`, as the
/// scanner would have reported a lockfile there; nothing else is changed.
fn fixture_at(dir: &Path, name: &str, target: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sarif").join(name);
    let text = std::fs::read_to_string(path).unwrap();
    let mut sarif: serde_json::Value = serde_json::from_str(&text).unwrap();
    for result in sarif["runs"][0]["results"].as_array_mut().unwrap() {
        result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"] = serde_json::json!(target);
    }
    let out = dir.join(format!("at-{name}"));
    std::fs::write(&out, sarif.to_string()).unwrap();
    out
}

/// What a report's OSV-Scanner findings are stored as under `root`:
/// `(rule, report path, fingerprint, legacy fingerprints)`, sorted.
fn osv_rows(root: &Path, report: &Path) -> Vec<(String, String, String, Vec<String>)> {
    ingest(root, report, &["src/main.rs".to_string()]).unwrap();
    let mut rows: Vec<_> = read_latest(root)
        .unwrap()
        .findings
        .into_iter()
        .map(|f| (f.rule_id, f.report_path.unwrap_or_default(), f.fingerprint, f.legacy_fingerprints))
        .collect();
    rows.sort();
    rows
}

/// CR-241 Decision 35: osv-scanner 2.x writes
/// `partialFingerprints.primaryLocationLineHash` where 1.9.2 wrote none. The
/// upstream reports of one scan at each version — two advisories, one of them
/// twice, every result pointed at one lockfile under the root — give one key
/// per finding: the version never re-keys it, though it did before migration
/// 10, whose keys each row lists as its legacy fingerprints.
#[test]
fn osv_scanner_1_9_2_and_2_7_0_reports_of_one_lockfile_give_one_key() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = lockfile_project(&dir, "project");
    let target = format!("file://{}/Cargo.lock", root.display());
    let v1 = osv_rows(&root, &fixture_at(&dir, "osv-scanner-1.9.2.sarif", &target));
    let v2 = osv_rows(&root, &fixture_at(&dir, "osv-scanner-2.7.0.sarif", &target));
    let keys = |rows: &[(String, String, String, Vec<String>)]| rows.iter().map(|r| (r.0.clone(), r.2.clone())).collect::<Vec<_>>();
    assert_eq!(keys(&v1), keys(&v2), "one key per finding at either version");
    let distinct: std::collections::BTreeSet<&str> = v1.iter().map(|r| r.2.as_str()).collect();
    assert_eq!((v1.len(), distinct.len()), (3, 3), "two advisories, one twice: three findings, three keys");
    assert!(v1.iter().any(|r| r.2 == location_key("CVE-2021-3121", "Cargo.lock")), "{v1:?}");
    let legacy = |rows: &[(String, String, String, Vec<String>)]| rows.iter().map(|r| r.3.clone()).collect::<Vec<_>>();
    assert_ne!(legacy(&v1), legacy(&v2), "before migration 10 the version re-keyed them");
    assert!(v1.iter().all(|r| r.3.len() == 1 && r.3[0] != r.2), "a 1.x row lists the one key it had");
    for (one, two) in v1.iter().zip(&v2) {
        assert_eq!(two.3.len(), 2, "a 2.x row lists both keys it could have had: {two:?}");
        assert!(two.3.contains(&one.3[0]), "one of them the key its 1.x report gave it: {one:?} {two:?}");
    }
}

/// CR-241 §3.2 item 2, the upgrade path: a store whose OSV-Scanner rows,
/// baseline and suppression were keyed under osv-scanner 1.9.2 before
/// migration 10, whose first ingest after it is a 2.7.0 report — whose
/// results carry `partialFingerprints`, so the key their own report gave them
/// before migration 10 is not the stored one. The rows of every snapshot and
/// the baseline are re-keyed all the same, and nothing reads as new.
#[test]
fn the_first_ingest_after_migration_10_from_osv_scanner_2_x_rekeys_a_1_x_store() {
    let _off = StopgapOff::new();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = lockfile_project(&dir, "project");
    let lockfile = format!("{}/Cargo.lock", root.display());
    let rules = ["CVE-2021-3121", "CVE-2022-24713"];
    let old: Vec<String> = rules.iter().map(|rule| location_key(rule, &lockfile)).collect();
    let path = crate::history::db_path(&root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let conn = crate::history::db::open_at_version(&path, 9).unwrap();
        conn.execute(
            "INSERT INTO security_snapshots (id, head_sha, scanner_name, scanner_version, report_hash, format, created_at, full_head_sha)
             VALUES (1, ?1, 'osv-scanner', '1.9.2', 'older', 'sarif', 1, ?1)",
            ["1".repeat(40)],
        )
        .unwrap();
        conn.execute("INSERT INTO security_baseline (id, head_sha, config_hash, created_at) VALUES (1, 'h', 'c', 1)", []).unwrap();
        for (key, rule) in old.iter().zip(rules) {
            conn.execute(
                "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding, report_path)
                 VALUES (1, ?1, ?2, 'unrated', '', 'm', 0, 'unmatched', ?3)",
                rusqlite::params![key, rule, lockfile],
            )
            .unwrap();
            conn.execute("INSERT INTO security_baseline_findings (fingerprint, scanner_name) VALUES (?1, 'osv-scanner')", [key]).unwrap();
        }
    }
    write(&root, super::IGNORE_FILE, &format!("[[ignore]]\nfingerprint = \"{}\"\nreason = \"unreachable\"\n", old[0]));

    ingest(&root, &fixture_at(&dir, "osv-scanner-2.7.0.sarif", &format!("file://{lockfile}")), &["src/main.rs".to_string()]).unwrap();
    let new: Vec<String> = rules.iter().map(|rule| location_key(rule, "Cargo.lock")).collect();
    let conn = Connection::open(&path).unwrap();
    let older: Vec<String> = conn
        .prepare("SELECT fingerprint FROM security_findings WHERE snapshot_id = 1 ORDER BY rule_id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(older, new, "the 1.x rows of an older snapshot are re-keyed by a 2.x report");
    let mut baseline: Vec<String> = conn
        .prepare("SELECT fingerprint FROM security_baseline_findings")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    baseline.sort();
    assert_eq!(baseline, new, "and so is the baseline");
    drop(conn);
    // The baseline does not record the 2.x report's second CVE-2022-24713
    // result, so it is the one new finding — and the old-key suppression
    // keeps CVE-2021-3121 out of the ratchet.
    let violations = ratchet_violations(&root);
    assert_eq!(violations.len(), 1, "{violations:?}");
    let today = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
    let labelled = label(&read_latest(&root).unwrap().findings, &IgnoreFile::load(&root).unwrap(), &root, today);
    let entry = &labelled.entries[0];
    assert_eq!((entry.orphaned, entry.suppressed, entry.should_name.as_deref()), (false, 1, Some(new[0].as_str())));
}

/// Where the baseline of [`nothing_stored_from_an_osv_scanner_1_9_2_scan_reads_as_new_after_the_2_7_0_upgrade`]
/// is kept: the set `gate --save` records in `history.db`, or the checked-in
/// file `security bless` writes, which the ratchet reads in its place when it
/// exists. Each is tried on its own, so neither can stand in for the other.
#[derive(Clone, Copy, Debug)]
enum BaselineAt {
    Store,
    CheckedIn,
}

/// S-779, the upgrade itself: a store, a baseline (in `history.db`, or checked
/// in) and a fingerprint suppression built from a 1.9.2 scan under today's
/// keys, then the 2.7.0 report of the same lockfile ingested into it. Each
/// finding keeps its key, the baseline and the suppression still know it, and
/// nothing reads as new — while the finding the 2.7.0 report gives a score now
/// reads that rating, where it read `unrated` before.
fn upgrade_reads_nothing_as_new(at: BaselineAt) {
    let _off = StopgapOff::new();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = lockfile_project(&dir, "project");
    let target = format!("file://{}/Cargo.lock", root.display());
    let paths = ["src/main.rs".to_string()];
    ingest(&root, &fixture_at(&dir, "osv-scanner-1.9.2.sarif", &target), &paths).unwrap();
    let before = read_latest(&root).unwrap().findings;
    assert_eq!(before.len(), 3, "{at:?}: two advisories, one of them twice: {before:#?}");
    assert!(before.iter().all(|f| f.severity.as_str() == "unrated"), "{at:?}: 1.9.2 gives no rating: {before:#?}");
    assert!(before.iter().all(|f| f.severity.gated_as().as_str() == "medium"), "{at:?}: and is gated as medium: {before:#?}");
    let keys = |findings: &[SecurityFinding]| {
        let mut keys: Vec<(String, String)> = findings.iter().map(|f| (f.rule_id.clone(), f.fingerprint.clone())).collect();
        keys.sort();
        keys
    };
    let blessed: Vec<(String, String)> = before.iter().map(|f| (f.scanner_name.clone(), f.fingerprint.clone())).collect();
    super::save_baseline(&root, None, "config", &blessed).unwrap();
    if matches!(at, BaselineAt::CheckedIn) {
        bless(&dir, &root, 3);
    }
    let suppressed = location_key("CVE-2021-3121", "Cargo.lock");
    write(&root, super::IGNORE_FILE, &format!("[[ignore]]\nfingerprint = \"{suppressed}\"\nreason = \"unreachable\"\n"));
    assert_eq!(ratchet_violations(&root), Vec::<String>::new(), "{at:?}: green before the upgrade");

    ingest(&root, &fixture_at(&dir, "osv-scanner-2.7.0.sarif", &target), &paths).unwrap();
    let after = read_latest(&root).unwrap().findings;
    assert_eq!(keys(&after), keys(&before), "{at:?}: every finding keeps its key");
    assert_eq!(ratchet_violations(&root), Vec::<String>::new(), "{at:?}: no entry of the baseline reads new");
    let today = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
    let labelled = label(&after, &IgnoreFile::load(&root).unwrap(), &root, today);
    assert_eq!(labelled.entries.len(), 1);
    let entry = &labelled.entries[0];
    assert_eq!(
        (entry.orphaned, entry.suppressed, entry.should_name.as_deref()),
        (false, 1, None),
        "{at:?}: the suppression still covers it, by its own key: {entry:?}"
    );
    let rated: Vec<(String, String)> = after.iter().map(|f| (f.rule_id.clone(), f.severity.as_str().to_string())).collect();
    assert!(rated.contains(&("CVE-2022-24713".to_string(), "high".to_string())), "{at:?}: the scored advisory reads its rating: {rated:?}");
    assert!(rated.contains(&("CVE-2021-3121".to_string(), "unrated".to_string())), "{at:?}: the unscored one still reads unrated: {rated:?}");
}

#[test]
fn nothing_stored_from_an_osv_scanner_1_9_2_scan_reads_as_new_after_the_2_7_0_upgrade() {
    upgrade_reads_nothing_as_new(BaselineAt::Store);
    upgrade_reads_nothing_as_new(BaselineAt::CheckedIn);
}

/// A report naming one unindexed file both by its absolute path and by its
/// relative one gives the second result, keyed apart by its occurrence suffix,
/// the first's new key as its pre-migration-10 one. That key is no old one:
/// recorded as one, the re-key would merge the first finding into the second.
/// Both findings are stored, in either report order.
#[test]
fn a_report_naming_one_file_two_ways_keeps_both_findings() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    for (n, order) in [[true, false], [false, true]].into_iter().enumerate() {
        let root = lockfile_project(&dir, &format!("project-{n}"));
        let uri = |absolute: bool| if absolute { format!("file://{}/notes.txt", root.display()) } else { "notes.txt".to_string() };
        let results: Vec<serde_json::Value> = order
            .iter()
            .map(|&absolute| {
                serde_json::json!({
                    "ruleId": "R1", "level": "warning", "message": {"text": "m"},
                    "locations": [{"physicalLocation": {"artifactLocation": {"uri": uri(absolute)}, "region": {"startLine": 3}}}]
                })
            })
            .collect();
        let sarif = serde_json::json!({"version": "2.1.0", "runs": [{"tool": {"driver": {"name": "acme", "version": "1", "rules": []}}, "results": results}]});
        let report = dir.join(format!("two-ways-{n}.sarif"));
        std::fs::write(&report, sarif.to_string()).unwrap();
        let summary = ingest(&root, &report, &["src/main.rs".to_string()]).unwrap();
        let stored = read_latest(&root).unwrap().findings;
        assert_eq!((summary.findings, stored.len()), (2, 2), "order {order:?}: {stored:#?}");
        let keys: Vec<&str> = stored.iter().map(|f| f.fingerprint.as_str()).collect();
        assert!(stored.iter().all(|f| f.legacy_fingerprints.iter().all(|old| !keys.contains(&old.as_str()))), "{stored:#?}");
    }
}

/// A path outside the project root keeps its report spelling in the key —
/// the upstream fixtures name `/path/to/…` — so such a key, from a CI report
/// ingested locally, stays root-dependent; the version still never re-keys it.
#[test]
fn a_path_outside_the_root_keeps_its_report_spelling() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = lockfile_project(&dir, "project");
    let fixture = |name: &str| Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sarif").join(name);
    let v1 = osv_rows(&root, &fixture("osv-scanner-1.9.2.sarif"));
    let v2 = osv_rows(&root, &fixture("osv-scanner-2.7.0.sarif"));
    assert_eq!(v1.iter().map(|r| &r.2).collect::<Vec<_>>(), v2.iter().map(|r| &r.2).collect::<Vec<_>>());
    let go = v1.iter().find(|r| r.0 == "CVE-2021-3121").unwrap();
    assert_eq!(go.2, location_key("CVE-2021-3121", "/path/to/go.mod"), "the report's own spelling");
    assert!(v1.iter().all(|r| r.3.is_empty()), "1.9.2's key is unchanged by migration 10: {v1:?}");
}

/// Any scanner: a result at an absolute path under the root that binds to no
/// indexed file is keyed as its project-relative path is, while one outside
/// the root keeps its spelling. Only OSV-Scanner's report identity is left
/// out of its key, by its driver name in any case — not a near miss's — and
/// its region stays in it.
#[test]
fn an_unbound_absolute_path_under_the_root_is_keyed_project_relative() {
    let root = Path::new("/work/project");
    let at = |path: &str, identity: &[&str]| super::ParsedFinding {
        rule_id: "python.lang.security.audit.eval".to_string(),
        severity: super::Severity::High,
        cwes: Vec::new(),
        message: "m".to_string(),
        redacted: false,
        report_path: Some(path.to_string()),
        region: super::Region { start_line: Some(3), start_column: Some(1), end_line: Some(3), end_column: Some(9) },
        identity: identity.iter().map(|s| s.to_string()).collect(),
        commit: None,
        committed_at: None,
    };
    let key = |f: &super::ParsedFinding, scanner: &str| super::fingerprint(f, scanner, root, None);
    let relative = key(&at("scripts/gen.py", &[]), "Semgrep OSS");
    assert_eq!(key(&at("/work/project/scripts/gen.py", &[]), "Semgrep OSS"), relative);
    assert_ne!(key(&at("/work/project-b/scripts/gen.py", &[]), "Semgrep OSS"), relative, "a sibling directory is outside the root");
    assert_ne!(key(&at("/elsewhere/scripts/gen.py", &[]), "Semgrep OSS"), relative, "outside the root, as reported");
    assert_eq!(super::fingerprint(&at("/work/project/scripts/gen.py", &[]), "Semgrep OSS", root, Some("scripts/gen.py")), relative, "and as the path it binds to");
    let hashed = at("Cargo.lock", &["primaryLocationLineHash=abc"]);
    let plain = at("Cargo.lock", &[]);
    assert_eq!(key(&hashed, " OSV-Scanner "), key(&plain, "osv-scanner"), "OSV-Scanner's identity is never hashed");
    assert_ne!(key(&hashed, "osv-scanner-wrapper"), key(&plain, "osv-scanner-wrapper"), "a near miss keeps its identity");
    assert_ne!(key(&hashed, "Semgrep OSS"), key(&plain, "Semgrep OSS"), "every other scanner keeps its identity");
    // OSV-Scanner's key is its rule and location plus its region, when the
    // report gives one.
    let mut elsewhere = plain.clone();
    elsewhere.region.start_line = Some(4);
    assert_ne!(key(&elsewhere, "osv-scanner"), key(&plain, "osv-scanner"), "the region is in the key");
    let spelled = blake3::hash(format!("logos-finding-v1\u{1f}location\u{1f}{}\u{1f}Cargo.lock\u{1f}3:1:3:9", plain.rule_id).as_bytes());
    assert_eq!(key(&hashed, "osv-scanner"), spelled.to_hex().to_string());
}

/// Every security table of the store at `path`, as text, every column but the
/// ones `exclude` names: what two states of one store must agree on.
fn dump(path: &Path, exclude: &[&str]) -> Vec<String> {
    let conn = Connection::open(path).unwrap();
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    let mut out = vec![format!("user_version {version}")];
    for table in ["security_snapshots", "security_findings", "security_files", "security_baseline", "security_baseline_findings"] {
        let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY 1, 2")).unwrap();
        let kept: Vec<usize> =
            stmt.column_names().iter().enumerate().filter(|(_, name)| !exclude.contains(name)).map(|(i, _)| i).collect();
        let rows = stmt
            .query_map([], |r| {
                kept.iter()
                    .map(|&i| r.get::<_, rusqlite::types::Value>(i).map(|v| format!("{v:?}")))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .map(|row| format!("{table}: {}", row.unwrap().join("|")))
            .collect::<Vec<_>>();
        out.extend(rows);
    }
    out
}

/// history.db 9 → 10: every OSV-Scanner snapshot's report hash is marked
/// `pre-10:` and nothing else changes. The next OSV-Scanner ingest — of the
/// very report the current snapshot came from — is no no-op: it re-keys the
/// rows of every snapshot and the ratchet's baseline, each row listing its old
/// key, so the ratchet reads nothing as new; a fingerprint suppression written
/// under the old key still suppresses, and `security status` names the key to
/// write; a path suppression is unaffected, and a checked-in file blessed
/// before the migration still knows each finding by its old key, the stopgap
/// switched off. The re-keyed rows are the ones a fresh store's ingest of the
/// same report writes; migrated step by step or at once, the store is the same.
#[test]
fn migration_10_marks_osv_snapshots_and_the_next_ingest_rekeys_every_surface() {
    let _off = StopgapOff::new();
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = lockfile_project(&dir, "project");
    let report = osv_lockfile_report(&dir, &root, &["Cargo.lock", "sub/Cargo.lock"]);
    let bytes = std::fs::read(&report).unwrap();
    let head = git(&root, None, &["rev-parse", "HEAD"]).trim().to_string();
    let abs = |p: &str| format!("{}/{p}", root.display());
    let old = [location_key(ADVISORY, &abs("Cargo.lock")), location_key(ADVISORY, &abs("sub/Cargo.lock"))];
    let new = [location_key(ADVISORY, "Cargo.lock"), location_key(ADVISORY, "sub/Cargo.lock")];
    let path = crate::history::db_path(&root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let conn = crate::history::db::open_at_version(&path, 9).unwrap();
        let older = "1".repeat(40);
        for (id, head, hash) in [(1, older.as_str(), "older".to_string()), (2, head.as_str(), blake3::hash(&bytes).to_hex().to_string())] {
            conn.execute(
                "INSERT INTO security_snapshots (id, head_sha, scanner_name, scanner_version, report_hash, format, created_at, full_head_sha)
                 VALUES (?1, ?2, 'osv-scanner', '1.9.2', ?3, 'sarif', 1, ?2)",
                rusqlite::params![id, head, hash],
            )
            .unwrap();
            for (key, lockfile) in old.iter().zip(["Cargo.lock", "sub/Cargo.lock"]) {
                conn.execute(
                    "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding, report_path)
                     VALUES (?1, ?2, ?3, 'unrated', '', 'm', 0, 'unmatched', ?4)",
                    rusqlite::params![id, key, ADVISORY, abs(lockfile)],
                )
                .unwrap();
            }
        }
        // Another scanner's snapshot is left as it is; an OSV-Scanner one is
        // told by its driver name, ASCII case and surrounding space ignored.
        for (id, name) in [(3, "Semgrep OSS"), (4, " OSV-Scanner ")] {
            conn.execute(
                "INSERT INTO security_snapshots (id, head_sha, scanner_name, scanner_version, report_hash, format, created_at, full_head_sha)
                 VALUES (?1, ?2, ?3, '1', 'other', 'sarif', 1, ?2)",
                rusqlite::params![id, "2".repeat(40), name],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO security_baseline (id, head_sha, config_hash, created_at) VALUES (1, 'h', 'c', 1)", []).unwrap();
        for (key, scanner) in [(&old[0], "osv-scanner"), (&old[1], "osv-scanner"), (&"f".repeat(64), "Semgrep OSS")] {
            conn.execute("INSERT INTO security_baseline_findings (fingerprint, scanner_name) VALUES (?1, ?2)", [key, scanner]).unwrap();
        }
    }
    let v9 = dir.join("v9.db");
    std::fs::copy(&path, &v9).unwrap();

    // Step by step: migration 10 alone, then each later one in turn.
    let before = dump(&path, &["report_hash"]);
    let migrated = crate::history::db::open_at_version(&path, 10).unwrap();
    let marked: Vec<String> = migrated
        .prepare("SELECT report_hash FROM security_snapshots ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        marked,
        ["pre-10:older".to_string(), format!("pre-10:{}", blake3::hash(&bytes).to_hex()), "other".to_string(), "pre-10:other".to_string()],
        "every OSV-Scanner snapshot is marked, and only OSV-Scanner ones"
    );
    drop(migrated);
    assert_eq!(dump(&path, &["report_hash"])[1..], before[1..], "nothing else changes");
    for version in 11..=crate::history::latest_schema_version() {
        drop(crate::history::db::open_at_version(&path, version).unwrap());
    }

    assert!(!ingest(&root, &report, &["src/main.rs".to_string()]).unwrap().already_ingested, "the marked snapshot's own report re-ingests");
    let conn = Connection::open(&path).unwrap();
    let older: Vec<(String, String)> = conn
        .prepare("SELECT fingerprint, legacy_fingerprints FROM security_findings WHERE snapshot_id = 1 ORDER BY report_path")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let want: Vec<(String, String)> = new.iter().cloned().zip(old.iter().cloned()).collect();
    assert_eq!(older, want, "an older snapshot's rows are re-keyed, each listing its old key");
    let mut current: Vec<(String, Vec<String>)> =
        read_latest(&root).unwrap().findings.into_iter().map(|f| (f.fingerprint, f.legacy_fingerprints)).collect();
    current.sort();
    let mut want: Vec<(String, Vec<String>)> = new.iter().cloned().zip(old.iter().map(|k| vec![k.clone()])).collect();
    want.sort();
    assert_eq!(current, want);
    let mut baseline: Vec<String> = conn
        .prepare("SELECT fingerprint FROM security_baseline_findings")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    baseline.sort();
    let mut want = vec![new[0].clone(), new[1].clone(), "f".repeat(64)];
    want.sort();
    assert_eq!(baseline, want, "the baseline names each new key, and another scanner's key as it was");
    drop(conn);
    let stepwise = dump(&path, &["created_at"]);
    assert_eq!(ratchet_violations(&root), Vec::<String>::new(), "nothing reads as new");
    // A checked-in file blessed before migration 10 holds this checkout's old
    // keys; with no other match it still knows each finding by them.
    let checked_in = |keys: &[String]| {
        let findings: Vec<serde_json::Value> =
            keys.iter().map(|key| serde_json::json!({"fingerprint": key, "rule_id": ADVISORY, "path": null})).collect();
        write(&root, ".logos/security-baseline.json", &format!("{:#}\n", serde_json::json!({"version": 1, "findings": findings})));
    };
    checked_in(&old);
    assert_eq!(ratchet_violations(&root), Vec::<String>::new(), "known by the old keys the file records");
    checked_in(&["e".repeat(64)]);
    assert_eq!(ratchet_violations(&root).len(), 2, "and by nothing else");
    std::fs::remove_file(root.join(".logos/security-baseline.json")).unwrap();

    // At once: the same v9 store opened at the latest version, then ingested.
    // (Today migration 10 is the latest, so the two paths differ only once a
    // later migration exists; the fresh store below is what bites now.)
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    std::fs::copy(&v9, &path).unwrap();
    ingest(&root, &report, &["src/main.rs".to_string()]).unwrap();
    assert_eq!(dump(&path, &["created_at"]), stepwise, "v9 migrated at once equals the stepwise result");
    // Everything but the report's own path spelling, which names its checkout.
    let rows = |root: &Path| -> Vec<(String, String, String, String)> {
        read_latest(root)
            .unwrap()
            .findings
            .into_iter()
            .map(|f| (f.fingerprint, f.rule_id, f.binding.as_str().to_string(), f.severity.as_str().to_string()))
            .collect()
    };
    let fresh = lockfile_project(&dir, "fresh");
    ingest(&fresh, &osv_lockfile_report(&dir, &fresh, &["Cargo.lock", "sub/Cargo.lock"]), &["src/main.rs".to_string()]).unwrap();
    assert_eq!(rows(&root), rows(&fresh), "the re-keyed rows are the ones a fresh store's ingest writes");

    let today = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
    let text = format!(
        "[[ignore]]\nfingerprint = \"{}\"\nreason = \"unreachable\"\n\n[[ignore]]\npath = \"sub\"\nscanner = \"osv-scanner\"\nreason = \"vendored\"\n",
        old[0]
    );
    write(&root, super::IGNORE_FILE, &text);
    let labelled = label(&read_latest(&root).unwrap().findings, &IgnoreFile::load(&root).unwrap(), &root, today);
    let states: Vec<(bool, usize, Option<&str>)> =
        labelled.entries.iter().map(|e| (e.orphaned, e.suppressed, e.should_name.as_deref())).collect();
    assert_eq!(states, [(false, 1, Some(new[0].as_str())), (false, 1, None)], "the old key suppresses and names the new one");
    assert!(labelled.findings.iter().all(|f| f.is_suppressed()));
    let user = dir.join("user-config");
    let status = crate::security::status(
        &root,
        &crate::security::ScanEnvironment { user_dir: Some(user), search_path: Some(std::ffi::OsString::new()), tools_dir: None, nvd_api_key: None },
        crate::engine::Engine::open(&root).tools_manifest(),
        today,
        crate::security::VersionProbes::Never,
        false,
        crate::security::scope::no_test_symbols,
    )
    .unwrap();
    let named: Vec<Option<&str>> = status.suppressions.iter().map(|e| e.should_name.as_deref()).collect();
    assert_eq!(named, [Some(new[0].as_str()), None], "security status names the key to write");
}

/// What [`f`] logged at any level, captured: the returned text holds a control
/// event, so an empty capture can never pass for a clean one.
fn logged<T>(f: impl FnOnce() -> T) -> (T, String) {
    #[derive(Clone, Default)]
    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let out = tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        tracing::trace!("capture control");
        f()
    });
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("capture control"), "the capture works: {log}");
    (out, log)
}

/// Every value that would give a planted secret away, for the secrets read at
/// `reads` — `(commit, git-root path, region)` each: the secrets, the text
/// matched around them and the settings line whole, their plain hashes, their
/// keyed digests and the repository key itself.
fn secret_material(root: &Path, reads: &[(&str, &str, super::Region)]) -> Vec<String> {
    let mut digests = super::secret::SecretDigests::open(root).expect("a repository key");
    let mut forbidden: Vec<String> = Vec::new();
    for value in [SECRET.to_string(), LEGACY_SECRET.to_string(), format!("API_KEY = \"{SECRET}"), format!("CLIENT_TOKEN = \"{LEGACY_SECRET}"), key()] {
        forbidden.push(blake3::hash(value.as_bytes()).to_hex().to_string());
        forbidden.push(value);
    }
    for (commit, path, region) in reads {
        let keyed = digests.digest(Some(commit), path, region).expect("the planted secret is read");
        forbidden.push(blake3::Hash::from(keyed).to_hex().to_string());
    }
    // From a member too: the fixture git's ceiling stops at a project's parent.
    let out = crate::history::miner::git(root, &["rev-list", "--max-parents=0", "HEAD"]).unwrap();
    let root_commit = String::from_utf8(out.stdout).unwrap();
    assert!(!root_commit.trim().is_empty(), "the root commit is read");
    forbidden.push(
        blake3::Hash::from(blake3::derive_key(super::secret::KEY_CONTEXT, root_commit.trim().as_bytes())).to_hex().to_string(),
    );
    forbidden
}

/// No column of any table, no byte of the store file, no payload and no log
/// line under `root` holds any of `forbidden`.
fn assert_holds_none(root: &Path, forbidden: &[String], payloads: &[String]) {
    let rows = super::tests::dump(root);
    assert!(rows.iter().any(|r| r.contains("security_findings")), "the store holds findings: {rows:?}");
    let bytes = std::fs::read(crate::history::db_path(root)).unwrap();
    let raw = String::from_utf8_lossy(&bytes);
    for value in forbidden {
        for row in &rows {
            assert!(!row.contains(value.as_str()), "{row} holds {value:?}");
        }
        assert!(!raw.contains(value.as_str()), "a byte of the store file holds {value:?}");
        for payload in payloads {
            assert!(!payload.contains(value.as_str()), "{payload} holds {value:?}");
        }
    }
}

fn region(line: i64, start: i64, end: i64) -> super::Region {
    super::Region { start_line: Some(line), start_column: Some(start), end_line: Some(line), end_column: Some(end) }
}

/// The project's spelling of a history-mode path: the prefix below the git
/// root off, by whole segments, and a path outside the project led by one `..`
/// per prefix segment — never a project path it is not; an absolute path under
/// the root relative to it.
#[test]
fn a_history_path_is_made_relative_to_the_project() {
    let root = Path::new("/work/repo/services/api");
    let project_relative = |prefix: &str, report: &str| super::project_relative(root, prefix, report);
    assert_eq!(project_relative("", "./legacy\\client.py"), "legacy/client.py");
    assert_eq!(project_relative("services/api/", "services/api/legacy/client.py"), "legacy/client.py");
    assert_eq!(project_relative("services/api/", "services/web/client.py"), "../../services/web/client.py");
    assert_eq!(project_relative("svc/", "svc2/client.py"), "../svc2/client.py", "a prefix strips whole segments only");
    assert_eq!(project_relative("svc/", "client.py"), "../client.py", "a git-root file is outside the member");
    assert_eq!(
        project_relative("services/api/", "/work/repo/services/api/legacy/client.py"),
        "legacy/client.py",
        "an absolute path under the root names no checkout"
    );
    assert_eq!(project_relative("services/api/", "/elsewhere/client.py"), "/elsewhere/client.py", "outside it, its spelling");
}

/// A git repository whose project is `member/`, one directory below the git
/// root, as a workspace member is: one commit adds `member/app/settings.py`
/// and `other/creds.py`, each holding a secret on line 1. Returns the git
/// root, the project root and the commit. Built alike every time, so two
/// copies share their commits and their repository key.
fn member_repo(tmp: &Path, name: &str) -> (PathBuf, PathBuf, String) {
    let top = tmp.join(name);
    std::fs::create_dir_all(&top).unwrap();
    git(&top, None, &["init", "-q", "-b", "main"]);
    std::fs::write(top.join(".git/info/exclude"), ".logos/\n").unwrap();
    write(&top, "member/app/settings.py", &format!("{}\n", key()));
    write(&top, "other/creds.py", &format!("CLIENT_TOKEN = \"{LEGACY_SECRET}\"\n"));
    git(&top, None, &["add", "-A"]);
    git(&top, Some("2026-10-01T09:00:00+00:00"), &["commit", "-q", "-m", "M1: a member and a neighbour"]);
    let commit = git(&top, None, &["rev-parse", "HEAD"]).trim().to_string();
    (top.clone(), top.join("member"), commit)
}

/// S-780 AC 3 (CR-241 CRA-03): in a project below the git root, Gitleaks'
/// history mode names a file relative to the git root. The history-mode key
/// takes the project's prefix off, so it equals the content-mode key of a
/// secret still present (S-765's rule holds), and the secret is bound to its
/// own file. A history path outside the project is historical, never bound to
/// the member's same-named file holding its text. No secret value or digest
/// key reaches a column, a payload or a log.
#[test]
fn in_a_workspace_member_the_history_key_equals_the_content_key() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let (_, member, commit) = member_repo(&dir, "history");
    // The member's own, untracked copy of the neighbour's line.
    write(&member, "creds.py", &format!("CLIENT_TOKEN = \"{LEGACY_SECRET}\"\n"));
    let date = "2026-10-01T09:00:00Z";
    let history = report(
        &dir,
        "member-history.sarif",
        &[(&commit, date, "member/app/settings.py", 1, 1, 41), (&commit, date, "other/creds.py", 1, 1, 42)],
    );
    let indexed = ["app/settings.py".to_string(), "creds.py".to_string()];
    let (summary, log) = logged(|| ingest(&member, &history, &indexed).unwrap());
    let found = gitleaks_findings(&member);
    assert_eq!(found.len(), 2, "{found:#?}");
    let settings = found.iter().find(|f| f.report_path.as_deref() == Some("member/app/settings.py")).unwrap();
    assert_eq!((settings.binding, settings.path.as_deref()), (FindingBinding::Bound, Some("app/settings.py")), "on its line in its own file");
    let outside = found.iter().find(|f| f.report_path.as_deref() == Some("other/creds.py")).unwrap();
    assert_eq!((outside.binding, outside.path.as_deref()), (FindingBinding::Historical, None), "outside the project: never creds.py");

    let (_, content, _) = member_repo(&dir, "content");
    write(&content, "creds.py", &format!("CLIENT_TOKEN = \"{LEGACY_SECRET}\"\n"));
    let scoped = report(&dir, "member-scoped.sarif", &[("", "", "app/settings.py", 1, 1, 41)]);
    let paths = vec!["app".to_string()];
    ingest_scoped(&content, &scoped, &indexed, Some(IngestScope { paths: &paths, keeps_full_findings: true })).unwrap();
    let present = gitleaks_findings(&content);
    assert_eq!(present.len(), 1, "{present:#?}");
    assert_eq!(present[0].fingerprint, settings.fingerprint, "one key in either mode");

    let forbidden = secret_material(
        &member,
        &[(commit.as_str(), "member/app/settings.py", region(1, 1, 41)), (commit.as_str(), "other/creds.py", region(1, 1, 42))],
    );
    let payloads =
        [serde_json::to_string(&summary).unwrap(), serde_json::to_string(&read_latest(&member).unwrap()).unwrap(), format!("{found:?}"), log];
    assert_holds_none(&member, &forbidden, &payloads);
}

/// The v2 secret key of the deleted `legacy/client.py` secret folded with
/// `path` — its own path gives migration 11's key, the file it bound to by
/// suffix the key it had before — spelled here from the identity's
/// definition, not by the ingest.
fn legacy_secret_key(root: &Path, path: &str) -> String {
    let mut digests = super::secret::SecretDigests::open(root).unwrap();
    let digest = digests.digest(Some(C1), "legacy/client.py", &region(1, 1, 42)).unwrap();
    let canonical =
        format!("logos-finding-v2\u{1f}secret\u{1f}generic-api-key\u{1f}{path}\u{1f}{}", blake3::Hash::from(digest).to_hex());
    blake3::hash(canonical.as_bytes()).to_hex().to_string()
}

/// history.db 10 → 11: every Gitleaks snapshot's report hash is marked
/// `pre-11:` and nothing else changes. The next Gitleaks ingest — of the very
/// report the current snapshot came from — is no no-op: it re-keys the
/// deleted `legacy/client.py` secret, stored under the key its suffix binding
/// to a root `client.py` gave it, onto its own path's key in every snapshot
/// and in the ratchet's baseline, each row listing its old key in
/// `legacy_fingerprints`. A suppression and a checked-in baseline written
/// under the old key stay green; no secret value or digest key is stored.
#[test]
fn migration_11_marks_gitleaks_snapshots_and_the_next_ingest_rekeys_a_history_only_secret() {
    let (_t, root) = gitleaks_repo(3);
    write(&root, ".logos/rules.toml", "[security]\nratchet_min_severity = \"info\"\n");
    // The root client.py the old identity bound the deleted file to.
    write(&root, "client.py", "import os\n");
    let (old, new) = (legacy_secret_key(&root, "client.py"), legacy_secret_key(&root, "legacy/client.py"));
    assert_ne!(old, new);
    let settings_key = {
        let (_t2, scratch) = gitleaks_repo(3);
        ingest_history(&scratch);
        settings_finding(&scratch).fingerprint
    };
    let bytes = std::fs::read(fixture("history.sarif")).unwrap();
    let path = crate::history::db_path(&root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let conn = crate::history::db::open_at_version(&path, 10).unwrap();
        for (id, head, hash) in [(1, C2.to_string(), "older".to_string()), (2, C3.to_string(), blake3::hash(&bytes).to_hex().to_string())] {
            conn.execute(
                "INSERT INTO security_snapshots (id, head_sha, scanner_name, scanner_version, report_hash, format, created_at, full_head_sha)
                 VALUES (?1, ?2, 'gitleaks', 'v8.18.4', ?3, 'sarif', 1, ?2)",
                rusqlite::params![id, head, hash],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding,
                     report_path, start_line, start_column, end_line, end_column, commits, anchor_commit)
                 VALUES (?1, ?2, 'generic-api-key', 'unrated', '', 'redacted', 1, 'historical', 'legacy/client.py', 1, 1, 1, 42, ?3, ?3)",
                rusqlite::params![id, old, C1],
            )
            .unwrap();
        }
        for (id, name) in [(3, "Semgrep OSS"), (4, " Gitleaks ")] {
            conn.execute(
                "INSERT INTO security_snapshots (id, head_sha, scanner_name, scanner_version, report_hash, format, created_at, full_head_sha)
                 VALUES (?1, ?2, ?3, '1', 'other', 'sarif', 1, ?2)",
                rusqlite::params![id, C1, name],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO security_baseline (id, head_sha, config_hash, created_at) VALUES (1, 'h', 'c', 1)", []).unwrap();
        for (key, scanner) in [(old.as_str(), "gitleaks"), (settings_key.as_str(), "gitleaks"), (&"f".repeat(64), "Semgrep OSS")] {
            conn.execute("INSERT INTO security_baseline_findings (fingerprint, scanner_name) VALUES (?1, ?2)", [key, scanner]).unwrap();
        }
    }

    let before = dump(&path, &["report_hash"]);
    let migrated = crate::history::db::open_at_version(&path, 11).unwrap();
    let marked: Vec<String> = migrated
        .prepare("SELECT report_hash FROM security_snapshots ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        marked,
        ["pre-11:older".to_string(), format!("pre-11:{}", blake3::hash(&bytes).to_hex()), "other".to_string(), "pre-11:other".to_string()],
        "every Gitleaks snapshot is marked, and only Gitleaks ones"
    );
    drop(migrated);
    assert_eq!(dump(&path, &["report_hash"])[1..], before[1..], "nothing else changes");

    let mut indexed = indexed(&root);
    indexed.push("client.py".to_string());
    let (summary, log) = logged(|| ingest(&root, &fixture("history.sarif"), &indexed).unwrap());
    assert!(!summary.already_ingested, "the marked snapshot's own report re-ingests");
    assert_eq!(
        (summary.historical, summary.unmatched.len()),
        (1, 0),
        "a historical result is no unmatched path, whatever the deleted file would bind to: {:?}",
        summary.unmatched
    );
    let conn = Connection::open(&path).unwrap();
    let older: Vec<(String, String)> = conn
        .prepare("SELECT fingerprint, legacy_fingerprints FROM security_findings WHERE snapshot_id = 1")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(older, [(new.clone(), old.clone())], "an older snapshot's row is re-keyed, listing its old key");
    let legacy: Vec<SecurityFinding> =
        gitleaks_findings(&root).into_iter().filter(|f| f.report_path.as_deref() == Some("legacy/client.py")).collect();
    assert_eq!(legacy.len(), 1, "{legacy:#?}");
    assert_eq!(legacy[0].fingerprint, new, "keyed by its own path");
    assert!(legacy[0].legacy_fingerprints.contains(&old), "the re-key lists its old secret key: {:?}", legacy[0].legacy_fingerprints);
    let mut baseline: Vec<String> = conn
        .prepare("SELECT fingerprint FROM security_baseline_findings")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    baseline.sort();
    let mut want = vec![new.clone(), settings_key.clone(), "f".repeat(64)];
    want.sort();
    assert_eq!(baseline, want, "the history.db baseline names the new key, another scanner's as it was");
    drop(conn);
    assert_eq!(ratchet_violations(&root), Vec::<String>::new(), "the history.db baseline stays green");

    // A checked-in baseline blessed before the migration records the old key.
    let checked_in = |keys: &[&str]| {
        let findings: Vec<serde_json::Value> =
            keys.iter().map(|key| serde_json::json!({"fingerprint": key, "rule_id": "generic-api-key", "path": null})).collect();
        write(&root, ".logos/security-baseline.json", &format!("{:#}\n", serde_json::json!({"version": 1, "findings": findings})));
    };
    checked_in(&[&old, &settings_key]);
    assert_eq!(ratchet_violations(&root), Vec::<String>::new(), "a pre-migration checked-in baseline stays green");
    checked_in(&[&settings_key]);
    assert_eq!(ratchet_violations(&root).len(), 1, "and knows the secret by nothing else");
    std::fs::remove_file(root.join(".logos/security-baseline.json")).unwrap();

    let today = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
    let text = format!("[[ignore]]\nfingerprint = \"{old}\"\nreason = \"rotated\"\n");
    let labelled = label(&gitleaks_findings(&root), &IgnoreFile::parse(&text).unwrap(), &root, today);
    let entry = &labelled.entries[0];
    assert_eq!((entry.orphaned, entry.suppressed, entry.should_name.as_deref()), (false, 1, Some(new.as_str())), "the old key suppresses");

    let forbidden = secret_material(&root, &[(C1, "legacy/client.py", region(1, 1, 42)), (C3, SETTINGS, region(13, 1, 41))]);
    let payloads = [serde_json::to_string(&summary).unwrap(), serde_json::to_string(&read_latest(&root).unwrap()).unwrap(), log];
    assert_holds_none(&root, &forbidden, &payloads);
}

/// Two history results of one commit and rule whose report keys differed only
/// by their occurrence suffix before migration 11: a deleted
/// `old/src/client.py`, listed first, and today's `src/client.py`, which it
/// bound to by suffix. Their secrets cannot be read — the commit is not in
/// this clone — so each is keyed by its report. Before migration 11 the first
/// had the plain key and the second its `#2` suffix; now each is keyed by its
/// own path, in a namespace of its own, so no new key is an old one: the
/// re-key maps each old key onto its own finding, in every snapshot and in
/// the ratchet's baseline, and an ignore entry written for the deleted file's
/// secret still names that finding and no other.
#[test]
fn report_keyed_history_results_sharing_an_old_key_by_occurrence_re_key_one_to_one() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, None, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join(".git/info/exclude"), ".logos/\n").unwrap();
    write(&root, "src/client.py", "import os\n");
    write(&root, ".logos/rules.toml", "[security]\nratchet_min_severity = \"info\"\n");
    git(&root, None, &["add", "-A"]);
    git(&root, Some("2026-10-01T09:00:00+00:00"), &["commit", "-q", "-m", "client"]);
    let head = git(&root, None, &["rev-parse", "HEAD"]).trim().to_string();
    let absent = "dead".repeat(10);
    let date = "2026-09-01T09:00:00Z";
    let sarif = report(tmp.path(), "twins.sarif", &[(&absent, date, "old/src/client.py", 1, 1, 30), (&absent, date, "src/client.py", 1, 1, 30)]);
    let indexed = ["src/client.py".to_string()];

    // The keys the identity before migration 11 gave them: both bound to
    // src/client.py, so the second took the occurrence suffix.
    let text = std::fs::read_to_string(&sarif).unwrap();
    let (_, runs) = super::parse_report(&text).unwrap();
    let mut seen = std::collections::HashMap::new();
    let old: Vec<String> = runs[0]
        .findings
        .iter()
        .map(|f| super::occurrence(&mut seen, super::fingerprint(f, "gitleaks", &root, Some("src/client.py"))))
        .collect();
    assert_ne!(old[0], old[1]);

    let path = crate::history::db_path(&root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let conn = crate::history::db::open_at_version(&path, 10).unwrap();
        let hash = blake3::hash(text.as_bytes()).to_hex().to_string();
        for (id, at, hash) in [(1, "1".repeat(40), "older".to_string()), (2, head.clone(), hash)] {
            conn.execute(
                "INSERT INTO security_snapshots (id, head_sha, scanner_name, scanner_version, report_hash, format, created_at, full_head_sha)
                 VALUES (?1, ?2, 'gitleaks', 'v8.18.4', ?3, 'sarif', 1, ?2)",
                rusqlite::params![id, at, hash],
            )
            .unwrap();
            for (key, report_path) in old.iter().zip(["old/src/client.py", "src/client.py"]) {
                conn.execute(
                    "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding,
                         report_path, start_line, start_column, end_line, end_column, commits, anchor_commit)
                     VALUES (?1, ?2, 'generic-api-key', 'unrated', '', 'redacted', 1, 'historical', ?3, 1, 1, 1, 30, ?4, ?4)",
                    rusqlite::params![id, key, report_path, absent],
                )
                .unwrap();
            }
        }
        conn.execute("INSERT INTO security_baseline (id, head_sha, config_hash, created_at) VALUES (1, 'h', 'c', 1)", []).unwrap();
        for key in &old {
            conn.execute("INSERT INTO security_baseline_findings (fingerprint, scanner_name) VALUES (?1, 'gitleaks')", [key]).unwrap();
        }
    }

    assert!(!ingest(&root, &sarif, &indexed).unwrap().already_ingested);
    let found = gitleaks_findings(&root);
    let by_path = |p: &str| found.iter().find(|f| f.report_path.as_deref() == Some(p)).unwrap().clone();
    let (deleted, today) = (by_path("old/src/client.py"), by_path("src/client.py"));
    assert!(!old.contains(&deleted.fingerprint) && !old.contains(&today.fingerprint), "no new key is an old one");
    assert!(deleted.legacy_fingerprints.contains(&old[0]), "{:?}", deleted.legacy_fingerprints);
    assert!(today.legacy_fingerprints.contains(&old[1]), "{:?}", today.legacy_fingerprints);

    let conn = Connection::open(&path).unwrap();
    let mut older: Vec<(String, String)> = conn
        .prepare("SELECT report_path, fingerprint FROM security_findings WHERE snapshot_id = 1")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    older.sort();
    assert_eq!(
        older,
        [("old/src/client.py".to_string(), deleted.fingerprint.clone()), ("src/client.py".to_string(), today.fingerprint.clone())],
        "each older row re-keyed onto its own finding, none merged away"
    );
    let mut baseline: Vec<String> = conn
        .prepare("SELECT fingerprint FROM security_baseline_findings")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    baseline.sort();
    let mut want = vec![deleted.fingerprint.clone(), today.fingerprint.clone()];
    want.sort();
    assert_eq!(baseline, want);
    drop(conn);
    assert_eq!(ratchet_violations(&root), Vec::<String>::new(), "nothing reads as new");

    let today_date = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
    let entry = format!("[[ignore]]\nfingerprint = \"{}\"\nreason = \"rotated\"\n", old[0]);
    let labelled = label(&found, &IgnoreFile::parse(&entry).unwrap(), &root, today_date);
    let suppressed: Vec<Option<&str>> =
        labelled.findings.iter().filter(|f| f.is_suppressed()).map(|f| f.finding.report_path.as_deref()).collect();
    assert_eq!(suppressed, [Some("old/src/client.py")], "the deleted file's entry names its finding alone");
}

/// A history-mode row stored before migration 11 is re-keyed by its place,
/// not only by the key today's index would recompute: here the deleted
/// `legacy/client.py` secret was keyed while a root `client.py` was indexed,
/// and that file is gone before the first scan after the migration, so no
/// suffix binding gives its old key any more. Its rule, report path, commit
/// and exact region name one match in an immutable blob, and the ingest's
/// result at that place takes the row — in every snapshot and in the ratchet's
/// baseline, listing the old key — so its suppression and a checked-in
/// baseline written under the old key stay green.
#[test]
fn a_pre_migration_11_row_is_re_keyed_by_its_place_once_its_old_binding_is_gone() {
    let (_t, root) = gitleaks_repo(3);
    write(&root, ".logos/rules.toml", "[security]\nratchet_min_severity = \"info\"\n");
    let (old, new) = (legacy_secret_key(&root, "client.py"), legacy_secret_key(&root, "legacy/client.py"));
    let settings_key = {
        let (_t2, scratch) = gitleaks_repo(3);
        ingest_history(&scratch);
        settings_finding(&scratch).fingerprint
    };
    let bytes = std::fs::read(fixture("history.sarif")).unwrap();
    let path = crate::history::db_path(&root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let conn = crate::history::db::open_at_version(&path, 10).unwrap();
        for (id, head, hash) in [(1, C2.to_string(), "older".to_string()), (2, C3.to_string(), blake3::hash(&bytes).to_hex().to_string())] {
            conn.execute(
                "INSERT INTO security_snapshots (id, head_sha, scanner_name, scanner_version, report_hash, format, created_at, full_head_sha)
                 VALUES (?1, ?2, 'gitleaks', 'v8.18.4', ?3, 'sarif', 1, ?2)",
                rusqlite::params![id, head, hash],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding,
                     report_path, start_line, start_column, end_line, end_column, commits, anchor_commit)
                 VALUES (?1, ?2, 'generic-api-key', 'unrated', '', 'redacted', 1, 'historical', 'legacy/client.py', 1, 1, 1, 42, ?3, ?3)",
                rusqlite::params![id, old, C1],
            )
            .unwrap();
        }
        // Near misses of its place in the older snapshot: one column off, and
        // another rule. Neither is this secret's row.
        for (key, rule, end) in [("near-miss-column", "generic-api-key", 41), ("near-miss-rule", "aws-access-token", 42)] {
            conn.execute(
                "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding,
                     report_path, start_line, start_column, end_line, end_column, commits, anchor_commit)
                 VALUES (1, ?1, ?2, 'unrated', '', 'redacted', 1, 'historical', 'legacy/client.py', 1, 1, 1, ?3, ?4, ?4)",
                rusqlite::params![key, rule, end, C1],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO security_baseline (id, head_sha, config_hash, created_at) VALUES (1, 'h', 'c', 1)", []).unwrap();
        for key in [&old, &settings_key] {
            conn.execute("INSERT INTO security_baseline_findings (fingerprint, scanner_name) VALUES (?1, 'gitleaks')", [key]).unwrap();
        }
    }

    // No client.py in the tree or the index: nothing recomputes the old key.
    assert!(!ingest_history(&root).already_ingested);
    let legacy: Vec<SecurityFinding> =
        gitleaks_findings(&root).into_iter().filter(|f| f.report_path.as_deref() == Some("legacy/client.py")).collect();
    assert_eq!(legacy[0].fingerprint, new);
    assert!(legacy[0].legacy_fingerprints.contains(&old), "its place gives its old key: {:?}", legacy[0].legacy_fingerprints);
    let conn = Connection::open(&path).unwrap();
    let mut older: Vec<(String, String)> = conn
        .prepare("SELECT fingerprint, legacy_fingerprints FROM security_findings WHERE snapshot_id = 1")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    older.sort();
    let mut want = vec![(new.clone(), old.clone()), ("near-miss-column".to_string(), String::new()), ("near-miss-rule".to_string(), String::new())];
    want.sort();
    assert_eq!(older, want, "the older snapshot's row is re-keyed by its place, and only it");
    let mut baseline: Vec<String> =
        conn.prepare("SELECT fingerprint FROM security_baseline_findings").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    baseline.sort();
    let mut want = vec![new.clone(), settings_key.clone()];
    want.sort();
    assert_eq!(baseline, want);
    drop(conn);
    assert_eq!(ratchet_violations(&root), Vec::<String>::new(), "nothing reads as new");

    let today = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
    let text = format!("[[ignore]]\nfingerprint = \"{old}\"\nreason = \"rotated\"\n");
    let labelled = label(&gitleaks_findings(&root), &IgnoreFile::parse(&text).unwrap(), &root, today);
    let entry = &labelled.entries[0];
    assert_eq!((entry.orphaned, entry.suppressed, entry.should_name.as_deref()), (false, 1, Some(new.as_str())));
}

/// Sprint 108 review: the place re-key above holds when the only snapshot
/// stored before migration 11 is the one at HEAD — an upgrade rescanned
/// before the next commit. The ingest replaces that snapshot, so its places
/// are read before it goes; and the next ingest, at a new HEAD, keeps the
/// old key the place gave.
#[test]
fn a_pre_migration_11_row_at_head_alone_is_re_keyed_by_its_place() {
    let (_t, root) = gitleaks_repo(3);
    write(&root, ".logos/rules.toml", "[security]\nratchet_min_severity = \"info\"\n");
    let (old, new) = (legacy_secret_key(&root, "client.py"), legacy_secret_key(&root, "legacy/client.py"));
    let settings_key = {
        let (_t2, scratch) = gitleaks_repo(3);
        ingest_history(&scratch);
        settings_finding(&scratch).fingerprint
    };
    let bytes = std::fs::read(fixture("history.sarif")).unwrap();
    let path = crate::history::db_path(&root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let conn = crate::history::db::open_at_version(&path, 10).unwrap();
        conn.execute(
            "INSERT INTO security_snapshots (id, head_sha, scanner_name, scanner_version, report_hash, format, created_at, full_head_sha)
             VALUES (1, ?1, 'gitleaks', 'v8.18.4', ?2, 'sarif', 1, ?1)",
            rusqlite::params![C3, blake3::hash(&bytes).to_hex().to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding,
                 report_path, start_line, start_column, end_line, end_column, commits, anchor_commit)
             VALUES (1, ?1, 'generic-api-key', 'unrated', '', 'redacted', 1, 'historical', 'legacy/client.py', 1, 1, 1, 42, ?2, ?2)",
            rusqlite::params![old, C1],
        )
        .unwrap();
        conn.execute("INSERT INTO security_baseline (id, head_sha, config_hash, created_at) VALUES (1, 'h', 'c', 1)", []).unwrap();
        for key in [&old, &settings_key] {
            conn.execute("INSERT INTO security_baseline_findings (fingerprint, scanner_name) VALUES (?1, 'gitleaks')", [key]).unwrap();
        }
    }

    assert!(!ingest_history(&root).already_ingested);
    let legacy: Vec<SecurityFinding> =
        gitleaks_findings(&root).into_iter().filter(|f| f.report_path.as_deref() == Some("legacy/client.py")).collect();
    assert_eq!(legacy[0].fingerprint, new);
    assert!(legacy[0].legacy_fingerprints.contains(&old), "its place gives its old key: {:?}", legacy[0].legacy_fingerprints);
    assert_eq!(ratchet_violations(&root), Vec::<String>::new(), "nothing reads as new");
    let today = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
    let text = format!("[[ignore]]\nfingerprint = \"{old}\"\nreason = \"rotated\"\n");
    let labelled = label(&gitleaks_findings(&root), &IgnoreFile::parse(&text).unwrap(), &root, today);
    let entry = &labelled.entries[0];
    assert_eq!((entry.orphaned, entry.suppressed, entry.should_name.as_deref()), (false, 1, Some(new.as_str())));

    // The old key outlives its place: at the next HEAD, with no row stored
    // before migration 11 left to recover it from, the finding still carries it.
    write(&root, "app/main.py", "print(\"again\")\n");
    git(&root, None, &["add", "-A"]);
    git(&root, Some("2026-10-04T09:00:00+00:00"), &["commit", "-q", "-m", "C4"]);
    assert!(!ingest_history(&root).already_ingested);
    let legacy: Vec<SecurityFinding> =
        gitleaks_findings(&root).into_iter().filter(|f| f.report_path.as_deref() == Some("legacy/client.py")).collect();
    assert!(legacy[0].legacy_fingerprints.contains(&old), "C4: its old key is kept: {:?}", legacy[0].legacy_fingerprints);
    let labelled = label(&gitleaks_findings(&root), &IgnoreFile::parse(&text).unwrap(), &root, today);
    let entry = &labelled.entries[0];
    assert_eq!((entry.orphaned, entry.suppressed, entry.should_name.as_deref()), (false, 1, Some(new.as_str())), "C4");
    assert_eq!(ratchet_violations(&root), Vec::<String>::new(), "C4: nothing reads as new");
}
