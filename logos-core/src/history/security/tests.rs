//! Integration tests for the security ingest over **real** git fixtures and the
//! committed per-scanner SARIF reports ([S-663], [FR-SC-01]..[FR-SC-03],
//! [ADR-79]).
//!
//! Every fixture repository lives in a temporary directory outside the work
//! tree, with `GIT_CEILING_DIRECTORIES` set and git auto-maintenance off, so no
//! git call can climb into this checkout and no background gc races a test.
//!
//! [S-663]: ../../../../docs/planning/journal.md#s-663-sarif-findings-are-ingested-into-the-evidence-store-anchored-and-redacted
//! [FR-SC-01]: ../../../../docs/specs/requirements/FR-SC-01.md
//! [FR-SC-03]: ../../../../docs/specs/requirements/FR-SC-03.md
//! [ADR-79]: ../../../../docs/specs/architecture/decisions/ADR-79.md

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::Connection;
use tempfile::TempDir;

use super::{ingest, read_latest, FindingBinding, SecurityIngestSummary, Severity};
use crate::history::{FRESHNESS_FRESH, FRESHNESS_NA, FRESHNESS_STALE};

/// The secret planted in the Gitleaks, Bandit and gosec fixtures (see the
/// fixtures' README).
const PLANTED: &str = "lgs_pl4nted_S3cr3t_q7Vx9Rz2Kw";

/// The fixture project every committed report targets.
const PROJECT: &[(&str, &str)] = &[
    (
        "app/db.py",
        "import sqlite3\n\n\ndef conn():\n    return sqlite3.connect('app.db')\n\n\n\n\
         def find(user_id):\n    c = conn()\n    cursor = c.cursor()\n    \
         cursor.execute(\"SELECT * FROM users WHERE id = %s\" % user_id)\n    return cursor.fetchone()\n",
    ),
    ("app/views.py", "from app import db\n\n\ndef show(request):\n    return db.find(request.args['id'])\n"),
    // Written at test time from `PLANTED`, never committed as a file.
    ("app/settings.py", ""),
    ("app/config.py", "import os\n"),
    ("requirements.txt", "Django==2.2.0\nrequests==2.19.0\n"),
    ("Dockerfile", "FROM ubuntu:latest\nRUN apt-get update\nCOPY . /app\nCMD [\"python\",\"/app/main.py\"]\n"),
    ("cmd/server/main.go", "package main\n\nfunc main() {}\n"),
    ("infra/main.tf", "resource \"aws_s3_bucket\" \"data\" {\n  bucket = \"fixture-data\"\n}\n"),
];

/// The nine registered scanners' committed reports, with the driver name and
/// version each carries and the number of results it holds.
const SCANNERS: &[(&str, &str, &str, usize)] = &[
    ("semgrep.sarif", "Semgrep OSS", "1.85.0", 2),
    ("trivy.sarif", "Trivy", "0.74.0", 46),
    ("gitleaks.sarif", "gitleaks", "v8.18.4", 2),
    ("osv-scanner.sarif", "osv-scanner", "1.9.1", 3),
    ("bandit.sarif", "Bandit", "1.7.10", 2),
    ("gosec.sarif", "gosec", "2.21.4", 3),
    ("checkov.sarif", "Checkov", "3.2.255", 4),
    ("codeql.sarif", "CodeQL", "2.19.3", 2),
    ("dependency-check.sarif", "dependency-check", "10.0.4", 2),
];

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sarif")
        .join(name)
}

/// Run git in `cwd` with a fixed identity, the ceiling set above the fixture
/// and auto-maintenance off.
fn sh_git(cwd: &Path, args: &[&str]) {
    let ceiling = cwd.parent().unwrap_or(cwd);
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .env("GIT_CEILING_DIRECTORIES", ceiling)
        .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn settings_py() -> String {
    format!("DEBUG = False\nAPI_TIMEOUT = 30\nDB_PASSWORD = '{PLANTED}'\nAPI_KEY = '{PLANTED}'\n")
}

/// A git repository at `<tmp>/repo` holding the fixture project, committed.
fn repo_with(extra: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("temp root");
    let r = tmp.path().join("repo");
    std::fs::create_dir_all(&r).unwrap();
    sh_git(&r, &["init", "-q", "-b", "main"]);
    let settings = settings_py();
    for &(path, body) in PROJECT.iter().chain(extra) {
        let body = if path == "app/settings.py" { settings.as_str() } else { body };
        let full = r.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, body).unwrap();
    }
    sh_git(&r, &["add", "-A"]);
    sh_git(&r, &["commit", "-q", "-m", "fixture project"]);
    (tmp, r)
}

fn repo() -> (TempDir, PathBuf) {
    repo_with(&[])
}

fn indexed() -> Vec<String> {
    PROJECT.iter().map(|(p, _)| p.to_string()).collect()
}

fn write(root: &Path, name: &str, body: &str) -> PathBuf {
    let p = root.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

fn ingest_fixture(root: &Path, name: &str) -> SecurityIngestSummary {
    ingest(root, &fixture(name), &indexed()).unwrap_or_else(|e| panic!("{name}: {e:#}"))
}

/// Every row of every table in `history.db`, each rendered as one line — the
/// full-store view the byte-identity and planted-secret checks read.
pub(super) fn dump(root: &Path) -> Vec<String> {
    let conn = Connection::open(crate::history::db_path(root)).unwrap();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut lines = Vec::new();
    for table in tables {
        let mut stmt = conn.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
        let cols = stmt.column_count();
        let rows = stmt
            .query_map([], |row| {
                let mut cells = Vec::with_capacity(cols);
                for i in 0..cols {
                    let cell = match row.get_ref(i)? {
                        rusqlite::types::ValueRef::Null => "NULL".to_string(),
                        rusqlite::types::ValueRef::Integer(n) => n.to_string(),
                        rusqlite::types::ValueRef::Real(f) => f.to_string(),
                        rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                        rusqlite::types::ValueRef::Blob(b) => String::from_utf8_lossy(b).into_owned(),
                    };
                    cells.push(cell);
                }
                Ok(format!("{table}|{}", cells.join("|")))
            })
            .unwrap();
        lines.extend(rows.map(Result::unwrap));
    }
    lines.sort();
    lines
}

fn count(root: &Path, table: &str) -> i64 {
    Connection::open(crate::history::db_path(root))
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

// ── FR-SC-01: every registered scanner's report parses ──────────────────────

/// [FR-SC-01] acceptance 3: the committed report of each of the nine
/// registered scanners parses and ingests, carrying its scanner identity and
/// every result it holds.
#[test]
fn every_registered_scanner_report_parses_and_ingests() {
    assert_eq!(SCANNERS.len(), 9, "one fixture per registered scanner");
    let (_t, r) = repo();
    for &(file, name, version, results) in SCANNERS {
        let s = ingest_fixture(&r, file);
        assert!(!s.already_ingested, "{file}: a first ingest writes");
        assert_eq!(s.scanners.len(), 1, "{file}: one scanner per report");
        assert_eq!(s.scanners[0].name, name, "{file}");
        assert_eq!(s.scanners[0].version, version, "{file}");
        assert_eq!(s.findings, results, "{file}: every result is kept");
        assert_eq!(s.scanners[0].findings, results, "{file}");
        assert!(!s.scanners[0].replaced_existing, "{file}: nothing to replace yet");
    }
    let view = read_latest(&r).unwrap();
    assert_eq!(view.snapshots.len(), 9, "one snapshot per scanner at this HEAD");
    assert_eq!(
        view.findings.len(),
        SCANNERS.iter().map(|s| s.3).sum::<usize>(),
        "every finding reads back"
    );
}

/// The parser on its own — no repository, no store — reads every registered
/// scanner's report: the entry a scanner run checks its output with.
#[test]
fn every_registered_scanner_report_parses_without_a_store() {
    for &(file, name, version, results) in SCANNERS {
        let text = std::fs::read_to_string(fixture(file)).unwrap();
        let (format, runs) = super::parse_report(&text).unwrap_or_else(|e| panic!("{file}: {e:#}"));
        assert_eq!(format, super::SecurityFormat::Sarif);
        assert_eq!(runs.len(), 1, "{file}");
        assert_eq!((runs[0].scanner_name.as_str(), runs[0].scanner_version.as_str()), (name, version));
        assert_eq!(runs[0].findings.len(), results, "{file}");
    }
    let err = format!("{:#}", super::parse_report("TN:\nSF:a.rs\n").unwrap_err());
    assert!(err.contains("not a SARIF 2.1.0 report"), "{err}");
}

/// What each scanner's report means once parsed: the severity, CWE and binding
/// a finding of each shape carries. One row per scanner, each pinned to a
/// shape only that scanner produces.
#[test]
fn each_scanner_shape_reads_back_with_its_severity_cwes_and_binding() {
    let (_t, r) = repo();
    for &(file, ..) in SCANNERS {
        ingest_fixture(&r, file);
    }
    let view = read_latest(&r).unwrap();
    let find = |scanner: &str, rule: &str| {
        view.findings
            .iter()
            .find(|f| f.scanner_name == scanner && f.rule_id == rule)
            .unwrap_or_else(|| panic!("{scanner} {rule} is stored"))
    };

    // Semgrep: CWE from a `CWE-89: …` tag; level from the rule's default.
    let f = find("Semgrep OSS", "python.lang.security.audit.formatted-sql-query.formatted-sql-query");
    assert_eq!((f.severity, f.cwes.as_slice()), (Severity::Medium, &["CWE-89".to_string()][..]));
    assert_eq!((f.binding, f.path.as_deref(), f.start_line), (FindingBinding::Bound, Some("app/db.py"), Some(12)));
    // Trivy: `security-severity` 9.5 on a secret rule → critical, redacted.
    let f = find("Trivy", "aws-access-key-id");
    assert_eq!(f.severity, Severity::Critical);
    assert!(f.redacted, "Trivy secret scanning is secret-class");
    assert_eq!(f.path.as_deref(), Some("app/config.py"));
    // Trivy misconfiguration: a level-only finding, kept verbatim.
    let f = find("Trivy", "DS-0002");
    assert!(!f.redacted && f.message.contains("Specify at least 1 USER command"));
    assert_eq!(f.path.as_deref(), Some("Dockerfile"));
    // Gitleaks: every finding is secret-class.
    assert!(view.findings.iter().filter(|f| f.scanner_name == "gitleaks").all(|f| f.redacted));
    // OSV: an absolute `file://` URI binds by suffix; no level and no score → unrated.
    let f = find("osv-scanner", "PYSEC-2018-28");
    assert_eq!((f.binding, f.path.as_deref(), f.severity), (FindingBinding::Bound, Some("requirements.txt"), Severity::Unrated));
    // Bandit: `external/cwe/cwe-259` → CWE-259 → secret-class; B608 keeps its message.
    let f = find("Bandit", "B105");
    assert_eq!((f.cwes.as_slice(), f.redacted, f.severity), (&["CWE-259".to_string()][..], true, Severity::Low));
    let f = find("Bandit", "B608");
    assert_eq!(f.message, "Possible SQL injection vector through string-based query construction.");
    // gosec: CWE from a relationship into the CWE taxonomy.
    let f = find("gosec", "G304");
    assert_eq!((f.cwes.as_slice(), f.severity), (&["CWE-22".to_string()][..], Severity::Medium));
    assert!(find("gosec", "G101").redacted, "CWE-798 is a hard-coded credential");
    // Checkov: the `CKV_SECRET_*` rule is redacted, the IaC rule is not.
    assert!(find("Checkov", "CKV_SECRET_2").redacted);
    let f = find("Checkov", "CKV_AWS_20");
    assert_eq!((f.redacted, f.severity, f.path.as_deref()), (false, Severity::High, Some("infra/main.tf")));
    // CodeQL: a rule found through `tool.extensions`, severity from its score.
    let f = find("CodeQL", "py/sql-injection");
    assert_eq!((f.severity, f.cwes.as_slice()), (Severity::High, &["CWE-89".to_string()][..]));
    let f = find("CodeQL", "py/clear-text-logging-sensitive-data");
    assert_eq!(f.cwes, ["CWE-312", "CWE-359", "CWE-532"]);
    assert_eq!(f.path.as_deref(), Some("app/views.py"));
    // Dependency-Check: a jar that is not an indexed file is unmatched, never dropped.
    let f = find("dependency-check", "CVE-2021-44228");
    assert_eq!((f.binding, f.path.as_deref(), f.freshness), (FindingBinding::Unmatched, None, FRESHNESS_NA));
    assert_eq!(f.report_path.as_deref(), Some("/home/ci/project/lib/log4j-core-2.14.1.jar"));
}

/// Severity bands at their edges: a CVSS score of exactly 9.0, 7.0 or 4.0
/// opens the higher band, 0 is info; without a score each level maps to its
/// band, and no rating level is `unrated`, never SARIF's default `warning`
/// (S-768). The captured Trivy report pins the 4.0 edge on real data.
#[test]
fn severity_bands_hold_at_their_edges() {
    let cases: &[(Option<f64>, Option<&str>, Severity)] = &[
        (Some(10.0), None, Severity::Critical),
        (Some(9.0), None, Severity::Critical),
        (Some(8.99), None, Severity::High),
        (Some(7.0), None, Severity::High),
        (Some(6.99), None, Severity::Medium),
        (Some(4.0), Some("error"), Severity::Medium),
        (Some(3.99), None, Severity::Low),
        (Some(0.1), None, Severity::Low),
        (Some(0.0), Some("error"), Severity::Info),
        (None, Some("error"), Severity::High),
        (None, Some("warning"), Severity::Medium),
        (None, Some("note"), Severity::Low),
        (None, Some("none"), Severity::Info),
        (None, None, Severity::Unrated),
    ];
    for &(score, level, want) in cases {
        assert_eq!(Severity::from_sarif(score, level), want, "{score:?} / {level:?}");
    }

    let (_t, r) = repo();
    ingest_fixture(&r, "trivy.sarif");
    let view = read_latest(&r).unwrap();
    let edge = view.findings.iter().find(|f| f.rule_id == "CVE-2025-48432").unwrap();
    assert_eq!(edge.severity, Severity::Medium, "Trivy's security-severity \"4.0\" is medium");
}

/// S-768: OSV-Scanner's severities are never made up. Upstream's own SARIF
/// test output for one scan at 1.9.2 and at 2.7.0 (the fixtures' README): at
/// 1.9.2 no rule carries a score and every result reads `warning`, so both
/// advisories read `unrated` with their reason; at 2.7.0 the rule whose
/// advisory has a CVSS score carries it as `security-severity` and reads its
/// band, and the one without still reads `unrated`. No finding reads `medium`.
/// Upstream's sample reports CVE-2022-24713 twice in each version, and both
/// results are kept.
#[test]
fn osv_scanner_reads_the_advisorys_rating_or_unrated_never_a_made_up_medium() {
    let read = |name: &str| {
        let (_t, r) = repo();
        ingest_fixture(&r, name);
        let mut got: Vec<(String, Severity, Option<&str>)> = read_latest(&r)
            .unwrap()
            .findings
            .into_iter()
            .map(|f| (f.rule_id, f.severity, f.severity.reason()))
            .collect();
        got.sort();
        got
    };
    let unrated = Some("the scanner gave none");
    assert_eq!(
        read("osv-scanner-1.9.2.sarif"),
        [
            ("CVE-2021-3121".to_string(), Severity::Unrated, unrated),
            ("CVE-2022-24713".to_string(), Severity::Unrated, unrated),
            ("CVE-2022-24713".to_string(), Severity::Unrated, unrated),
        ],
        "1.9.2 gives no rating: every result reads `warning`, no rule a score"
    );
    assert_eq!(
        read("osv-scanner-2.7.0.sarif"),
        [
            ("CVE-2021-3121".to_string(), Severity::Unrated, unrated),
            ("CVE-2022-24713".to_string(), Severity::High, None),
            ("CVE-2022-24713".to_string(), Severity::High, None),
        ],
        "2.7.0's rule `security-severity` 7.5 is the advisory's rating"
    );
}

/// history.db 8 → 9: `unrated` joins the severities. A `medium` row of an
/// OSV-Scanner 1.x snapshot or of a Gitleaks one reads `unrated` — neither
/// report ever carries a score or a level of its own, so its `medium` came
/// from a default level — and each near miss keeps its severity: a 2.x
/// snapshot, a `10.x` version, another severity, another scanner, a driver
/// one word from Gitleaks. Every other column is copied, every snapshot's
/// report hash is marked so its own report re-ingests, and the CHECK still
/// refuses a token Logos never writes.
#[test]
fn migration_9_reads_a_stored_osv_scanner_1x_or_gitleaks_medium_as_unrated_and_nothing_else() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("history.db");
    let snapshots = [
        (1, "osv-scanner", "1.9.2"),
        (2, " OSV-Scanner ", "v1.8.5"),
        (3, "osv-scanner", "2.7.0"),
        (4, "osv-scanner", "10.1.0"),
        (5, "Semgrep OSS", "1.85.0"),
        (6, " Gitleaks ", "v8.18.4"),
        (7, "gitleaks-wrapper", "1.0.0"),
    ];
    {
        let conn = crate::history::db::open_at_version(&path, 8).unwrap();
        for (id, name, version) in snapshots {
            conn.execute(
                "INSERT INTO security_snapshots (id, head_sha, scanner_name, scanner_version, report_hash, format, created_at, full_head_sha)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'sarif', 1, ?2)",
                rusqlite::params![id, format!("head{id}"), name, version, format!("hash{id}")],
            )
            .unwrap();
        }
        // Every row differs from the others in every column a copy could drop
        // or swap, none of them NULL or at its default.
        let rows = [(1, "a", "medium"), (1, "b", "high"), (2, "c", "medium"), (3, "d", "medium"), (4, "e", "medium"), (5, "f", "medium"),
                    (6, "g", "medium"), (6, "h", "low"), (7, "i", "medium")];
        for (n, (id, fingerprint, severity)) in (0_i64..).zip(rows) {
            conn.execute(
                "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding,
                     report_path, path, start_line, start_column, end_line, end_column, scoped, commits, legacy_fingerprints,
                     anchor_commit)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'bound', ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                rusqlite::params![
                    id, fingerprint, format!("rule-{fingerprint}"), severity, format!("CWE-{n}"), format!("msg {fingerprint}"),
                    n % 2, format!("lib/{fingerprint}.lock"), format!("src/{fingerprint}.lock"), 10 + n, 20 + n, 30 + n, 40 + n,
                    (n + 1) % 2, format!("c-{fingerprint}"), format!("old-{fingerprint}"), format!("anchor-{fingerprint}")
                ],
            )
            .unwrap();
        }
    }
    // Every column but `severity`, quoted, row by row in key order.
    let columns = |conn: &Connection| -> Vec<String> {
        const COLUMNS: [&str; 17] = [
            "snapshot_id", "fingerprint", "rule_id", "cwes", "message", "redacted", "binding", "report_path", "path",
            "start_line", "start_column", "end_line", "end_column", "scoped", "commits", "legacy_fingerprints", "anchor_commit",
        ];
        let row = COLUMNS.map(|c| format!("quote({c})")).join(" || '|' || ");
        conn.prepare(&format!("SELECT {row} FROM security_findings ORDER BY snapshot_id, fingerprint"))
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let before = columns(&Connection::open(&path).unwrap());

    let migrated = crate::history::db::open(&path).unwrap();
    assert_eq!(crate::history::db::current_version(&migrated).unwrap(), 9);
    assert_eq!(columns(&migrated), before, "every other column of every row is copied as it was");
    let severities: Vec<(String, String)> = migrated
        .prepare("SELECT fingerprint, severity FROM security_findings ORDER BY fingerprint")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let want = [
        ("a", "unrated"), ("b", "high"), ("c", "unrated"), ("d", "medium"), ("e", "medium"), ("f", "medium"),
        ("g", "unrated"), ("h", "low"), ("i", "medium"),
    ];
    assert_eq!(severities, want.map(|(f, s)| (f.to_string(), s.to_string())));
    let hashes: Vec<String> = migrated
        .prepare("SELECT report_hash FROM security_snapshots ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(hashes, (1..=7).map(|id| format!("pre-9:hash{id}")).collect::<Vec<_>>(), "every snapshot is marked");
    for (severity, accepted) in [("unrated", true), ("bogus", false)] {
        let inserted = migrated.execute(
            "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding)
             VALUES (5, ?1, 'r', ?1, '', 'm', 0, 'unbound')",
            [severity],
        );
        assert_eq!(inserted.is_ok(), accepted, "{severity}: {inserted:?}");
    }
    let index: i64 = migrated
        .query_row("SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'security_findings_path'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(index, 1, "the path index is rebuilt with the table");
}

// ── FR-SC-01: atomic rejection that names the defect ───────────────────────

/// [FR-SC-01] acceptance 1: a malformed report writes nothing — the store is
/// row-for-row identical, and with no prior store none is created — and the
/// error names the defect.
#[test]
fn a_malformed_report_writes_nothing_and_names_the_defect() {
    let semgrep = std::fs::read_to_string(fixture("semgrep.sarif")).unwrap();
    let cases: Vec<(&str, String, &str)> = vec![
        ("truncated", semgrep[..semgrep.len() / 2].to_string(), "EOF while parsing"),
        ("not json", "this is not a report\n".to_string(), "not a SARIF 2.1.0 report"),
        ("bad json", "{\"version\": \"2.1.0\", \"runs\": [}".to_string(), "line 1 column"),
        ("old version", r#"{"version":"2.0.0","runs":[]}"#.to_string(), "only \"2.1.0\""),
        ("no version", r#"{"runs":[]}"#.to_string(), "`version` is missing"),
        ("no runs", r#"{"version":"2.1.0"}"#.to_string(), "`runs` array is missing"),
        (
            "nameless driver",
            r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"rules":[]}},"results":[]}]}"#.to_string(),
            "runs[0].tool.driver.name is missing",
        ),
        (
            "absent results",
            r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Bandit"}}}]}"#.to_string(),
            "runs[0].results is absent",
        ),
        (
            "failed execution",
            r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Bandit"}},"invocations":[{"executionSuccessful":false}],"results":[]}]}"#
                .to_string(),
            "runs[0].invocations[0].executionSuccessful is false",
        ),
        (
            "result without message",
            r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"t"}},"results":[{"ruleId":"r"},{"ruleId":"r","message":{"text":"m"}}]}]}"#
                .to_string(),
            "runs[0].results[0].message has no `text`",
        ),
        (
            "wrong type",
            r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"t"}},"results":[{"ruleId":"r","message":{"text":"m"},"locations":[{"physicalLocation":{"region":{"startLine":"twelve"}}}]}]}]}"#
                .to_string(),
            "invalid type",
        ),
    ];

    // With no prior store, a rejected report does not even create one.
    let (_t, fresh) = repo();
    for (label, body, _) in &cases {
        let report = write(&fresh, "report.sarif", body);
        assert!(ingest(&fresh, &report, &indexed()).is_err(), "{label} is rejected");
    }
    assert!(!crate::history::db_path(&fresh).exists(), "a rejected report never opens the store");

    // With a populated store, every rejection leaves it row-for-row identical.
    let (_t, r) = repo();
    ingest_fixture(&r, "bandit.sarif");
    let before = dump(&r);
    for (label, body, defect) in &cases {
        let report = write(&r, "report.sarif", body);
        let err = format!("{:#}", ingest(&r, &report, &indexed()).unwrap_err());
        assert!(err.contains(defect), "{label}: the error names the defect ({defect}): {err}");
        assert_eq!(dump(&r), before, "{label}: nothing was written");
    }
}

/// [FR-SC-03] for a rejected report: the error names the defect and its
/// position but quotes no value from the report, because it reaches the CLI,
/// the MCP error and the watcher's log, and a report's strings can hold a
/// secret. One planted value per slot an error could echo.
#[test]
fn a_rejected_report_s_error_quotes_none_of_its_values() {
    const SECRET: &str = "AKIAPLANTEDREJECT123 password=hunter2";
    let run = |result: &str| {
        format!(r#"{{"version":"2.1.0","runs":[{{"tool":{{"driver":{{"name":"t"}}}},"results":[{result}]}}]}}"#)
    };
    let cases = [
        ("a string where an object goes", run(&format!(r#"{{"ruleId":"r","message":"{SECRET}"}}"#)), "invalid type"),
        ("a string where a number goes", run(&format!(r#"{{"ruleId":"r","message":{{"text":"m"}},"locations":[{{"physicalLocation":{{"region":{{"startLine":"{SECRET}"}}}}}}]}}"#)), "invalid type"),
        ("the version", format!(r#"{{"version":"{SECRET}","runs":[]}}"#), "only \"2.1.0\""),
        ("the kind", run(&format!(r#"{{"ruleId":"r","kind":"{SECRET}","message":{{"text":"m"}}}}"#)), "runs[0].results[0].kind"),
        ("the level", run(&format!(r#"{{"ruleId":"r","level":"{SECRET}","message":{{"text":"m"}}}}"#)), "runs[0].results[0].level"),
    ];
    let (_t, r) = repo();
    for (label, body, defect) in cases {
        let report = write(&r, "report.sarif", &body);
        let err = format!("{:#}", ingest(&r, &report, &indexed()).unwrap_err());
        assert!(err.contains(defect), "{label}: the defect is named ({defect}): {err}");
        for word in SECRET.split(' ') {
            assert!(!err.contains(word), "{label}: the error quotes the report: {err}");
        }
    }
}

/// The rejections the string table above cannot carry: a rule's invalid default
/// level, a report that is not UTF-8, and a root with no HEAD to anchor to. Each
/// names its defect and writes nothing.
#[test]
fn byte_level_and_anchor_rejections_name_the_defect_and_write_nothing() {
    let (_t, r) = repo();
    ingest_fixture(&r, "bandit.sarif");
    let before = dump(&r);

    let rule_level = write(
        &r,
        "level.sarif",
        r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Bandit","rules":[{"id":"B1","defaultConfiguration":{"level":"fatal"}}]}},"results":[{"ruleId":"B1","message":{"text":"m"}}]}]}"#,
    );
    let err = format!("{:#}", ingest(&r, &rule_level, &indexed()).unwrap_err());
    assert!(err.contains("rule \"B1\" defaultConfiguration.level") && !err.contains("fatal"), "named, never quoted: {err}");
    assert_eq!(dump(&r), before, "an invalid rule level writes nothing");

    let latin1 = r.join("latin1.sarif");
    std::fs::write(
        &latin1,
        b"{\"version\":\"2.1.0\",\"runs\":[{\"tool\":{\"driver\":{\"name\":\"Bandit\"}},\"results\":[{\"ruleId\":\"r\",\"message\":{\"text\":\"caf\xe9\"}}]}]}",
    )
    .unwrap();
    let err = format!("{:#}", ingest(&r, &latin1, &indexed()).unwrap_err());
    assert!(err.contains("is not UTF-8"), "{err}");
    assert_eq!(dump(&r), before, "a non-UTF-8 report writes nothing");

    // No git repository: nothing to anchor to, and no store is created.
    let plain = TempDir::new().unwrap();
    let report = plain.path().join("semgrep.sarif");
    std::fs::copy(fixture("semgrep.sarif"), &report).unwrap();
    let err = format!("{:#}", ingest(plain.path(), &report, &indexed()).unwrap_err());
    assert!(err.contains("resolvable HEAD"), "{err}");
    assert!(!crate::history::db_path(plain.path()).exists(), "no anchor, no store");
}

// ── FR-SC-01: byte-identical re-ingest is a no-op ───────────────────────────

/// [FR-SC-01] acceptance 2: re-ingesting each byte-identical report is a no-op
/// reported `already_ingested`; every table's rows and counts are unchanged.
#[test]
fn a_byte_identical_reingest_is_a_no_op_reported_already_ingested() {
    let (_t, r) = repo();
    for &(file, ..) in SCANNERS {
        ingest_fixture(&r, file);
    }
    let counts = |r: &Path| {
        ["security_snapshots", "security_files", "security_findings"].map(|t| count(r, t))
    };
    let (rows, before) = (dump(&r), counts(&r));
    assert!(before.iter().all(|&n| n > 0));
    for &(file, ..) in SCANNERS {
        let again = ingest_fixture(&r, file);
        assert!(again.already_ingested, "{file}: the second ingest is a no-op");
    }
    assert_eq!(counts(&r), before, "row counts are unchanged");
    assert_eq!(dump(&r), rows, "every row is unchanged");
}

/// [FR-SC-02] acceptance 3: two scanners at one HEAD keep separate snapshots,
/// and re-running one (different bytes) replaces only its own; a new HEAD opens
/// a new snapshot and keeps the old one as provenance.
#[test]
fn a_rescan_replaces_only_its_own_scanner_and_a_new_head_keeps_the_old() {
    let (_t, r) = repo();
    let semgrep = ingest_fixture(&r, "semgrep.sarif");
    ingest_fixture(&r, "bandit.sarif");
    let semgrep_rows: Vec<String> = dump(&r)
        .into_iter()
        .filter(|l| l.starts_with(&format!("security_findings|{}|", semgrep.scanners[0].snapshot_id)))
        .collect();
    assert_eq!(semgrep_rows.len(), 2);

    // A rescan of Bandit at the same HEAD with one result fewer.
    let bandit = std::fs::read_to_string(fixture("bandit.sarif")).unwrap();
    let mut doc: serde_json::Value = serde_json::from_str(&bandit).unwrap();
    doc["runs"][0]["results"].as_array_mut().unwrap().pop();
    let rescan = write(&r, "bandit-rescan.sarif", &doc.to_string());
    let s = ingest(&r, &rescan, &indexed()).unwrap();
    assert!(s.scanners[0].replaced_existing && !s.already_ingested);

    let view = read_latest(&r).unwrap();
    assert_eq!(view.snapshots.len(), 2, "still one snapshot per scanner");
    assert_eq!(view.findings.iter().filter(|f| f.scanner_name == "Bandit").count(), 1);
    let still: Vec<String> = dump(&r)
        .into_iter()
        .filter(|l| l.starts_with(&format!("security_findings|{}|", semgrep.scanners[0].snapshot_id)))
        .collect();
    assert_eq!(still, semgrep_rows, "Semgrep's snapshot is untouched");

    // A new HEAD: the same report opens a new snapshot; the old one stays.
    std::fs::write(r.join("app/views.py"), "# moved\n").unwrap();
    sh_git(&r, &["commit", "-q", "-am", "next"]);
    let next = ingest_fixture(&r, "semgrep.sarif");
    assert!(!next.already_ingested, "the same bytes at a new HEAD are new evidence");
    assert_ne!(next.head_sha, semgrep.head_sha);
    assert_eq!(count(&r, "security_snapshots"), 3, "the old HEAD's snapshot is provenance");
    let view = read_latest(&r).unwrap();
    let latest = view.snapshots.iter().find(|s| s.scanner_name == "Semgrep OSS").unwrap();
    assert_eq!(latest.head_sha, next.head_sha, "the reader takes the latest snapshot");
}

/// Going back to an older HEAD and re-ingesting its byte-identical report makes
/// that snapshot current again: it is not a no-op that leaves the newer HEAD's
/// (clean) snapshot served while the finding is back in the tree.
#[test]
fn reingesting_an_older_heads_report_makes_it_current_again() {
    let (_t, r) = repo();
    let first = ingest_fixture(&r, "semgrep.sarif");
    let h1 = first.head_sha.clone();

    std::fs::write(r.join("app/views.py"), "# fixed\n").unwrap();
    sh_git(&r, &["commit", "-q", "-am", "fix"]);
    let clean = write(
        &r,
        "clean.sarif",
        r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Semgrep OSS","semanticVersion":"1.85.0"}},"results":[]}]}"#,
    );
    ingest(&r, &clean, &indexed()).unwrap();
    assert_eq!(read_latest(&r).unwrap().snapshots[0].finding_count, 0);

    sh_git(&r, &["checkout", "-q", &h1]);
    let again = ingest_fixture(&r, "semgrep.sarif");
    assert!(!again.already_ingested, "an older HEAD's report made current again is a rewrite");
    let view = read_latest(&r).unwrap();
    assert_eq!(view.snapshots[0].head_sha, h1, "the checked-out HEAD's snapshot is served");
    assert_eq!(view.snapshots[0].finding_count, 2);

    // Now it is current, so the same bytes again are the no-op.
    assert!(ingest_fixture(&r, "semgrep.sarif").already_ingested);
}

/// One report can carry several scanners, and one scanner several runs: runs of
/// one tool merge into its one snapshot; each tool keeps its own. A byte-identical
/// re-ingest is a no-op only when **every** scanner still holds this report — once
/// one of them was replaced by another report, the original is new again.
#[test]
fn a_multi_scanner_report_is_one_snapshot_per_scanner_and_a_no_op_only_whole() {
    let (_t, r) = repo();
    let run = |name: &str, rule: &str| {
        format!(
            r#"{{"tool":{{"driver":{{"name":"{name}"}}}},"results":[{{"ruleId":"{rule}","message":{{"text":"m"}}}}]}}"#
        )
    };
    let both = write(
        &r,
        "both.sarif",
        &format!(
            r#"{{"version":"2.1.0","runs":[{},{},{}]}}"#,
            run("CodeQL", "py/a"),
            run("Bandit", "B1"),
            run("CodeQL", "go/b")
        ),
    );
    let first = ingest(&r, &both, &indexed()).unwrap();
    let per: Vec<(&str, usize, bool)> = first
        .scanners
        .iter()
        .map(|s| (s.name.as_str(), s.findings, s.replaced_existing))
        .collect();
    assert_eq!(per, [("Bandit", 1, false), ("CodeQL", 2, false)], "two CodeQL runs merge");
    assert_eq!(count(&r, "security_snapshots"), 2);

    // CodeQL alone, rescanned: only CodeQL's snapshot is replaced.
    let codeql = write(&r, "codeql-only.sarif", &format!(r#"{{"version":"2.1.0","runs":[{}]}}"#, run("CodeQL", "py/c")));
    let s = ingest(&r, &codeql, &indexed()).unwrap();
    assert!(s.scanners[0].replaced_existing);

    // The two-scanner report again: Bandit still holds it, CodeQL does not.
    let again = ingest(&r, &both, &indexed()).unwrap();
    assert!(!again.already_ingested, "one scanner moved on, so this is not a no-op");
    let view = read_latest(&r).unwrap();
    assert!(view.snapshots.iter().all(|s| s.report_hash == first.report_hash));
    let rules: Vec<&str> = view.findings.iter().filter(|f| f.scanner_name == "CodeQL").map(|f| f.rule_id.as_str()).collect();
    assert_eq!(rules.len(), 2);
    assert!(rules.contains(&"py/a") && rules.contains(&"go/b"));
    assert!(ingest(&r, &both, &indexed()).unwrap().already_ingested, "now whole again");
}

// ── FR-SC-02: freshness and binding ─────────────────────────────────────────

/// [FR-SC-02] acceptance 1: a file edited after ingest reads its findings as
/// `stale`; reverting it restores `fresh`; other files stay fresh.
#[test]
fn an_edit_after_ingest_reads_stale_and_a_revert_reads_fresh() {
    let (_t, r) = repo();
    ingest_fixture(&r, "semgrep.sarif");
    ingest_fixture(&r, "checkov.sarif");
    let freshness = |path: &str| -> Vec<&'static str> {
        read_latest(&r)
            .unwrap()
            .findings
            .iter()
            .filter(|f| f.path.as_deref() == Some(path))
            .map(|f| f.freshness)
            .collect()
    };
    assert_eq!(freshness("app/db.py"), [FRESHNESS_FRESH; 2]);

    let original = std::fs::read_to_string(r.join("app/db.py")).unwrap();
    std::fs::write(r.join("app/db.py"), format!("# edited\n{original}")).unwrap();
    assert_eq!(freshness("app/db.py"), [FRESHNESS_STALE; 2], "an edited file reads stale");
    assert!(
        freshness("Dockerfile").iter().all(|&f| f == FRESHNESS_FRESH),
        "an untouched file stays fresh"
    );

    std::fs::write(r.join("app/db.py"), original).unwrap();
    assert_eq!(freshness("app/db.py"), [FRESHNESS_FRESH; 2], "a revert restores fresh");

    std::fs::remove_file(r.join("app/db.py")).unwrap();
    assert_eq!(freshness("app/db.py"), [FRESHNESS_STALE; 2], "a deleted file is never fresh");
}

/// [FR-SC-02] acceptance 2: a report path two indexed files share is listed
/// unmatched with its candidates count, and its finding is attached to neither.
#[test]
fn an_ambiguous_path_is_listed_unmatched_with_its_candidates_never_guessed() {
    let (_t, r) = repo_with(&[
        ("services/a/util.py", "def helper(): pass\n"),
        ("services/b/util.py", "def helper(): pass\n"),
    ]);
    let mut idx = indexed();
    idx.extend(["services/a/util.py".to_string(), "services/b/util.py".to_string()]);
    let report = write(
        &r,
        "report.sarif",
        r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Semgrep OSS","semanticVersion":"1.85.0"}},
            "results":[
              {"ruleId":"r.ambiguous","message":{"text":"m"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"util.py"},"region":{"startLine":1}}}]},
              {"ruleId":"r.unique","message":{"text":"m"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"a/util.py"},"region":{"startLine":1}}}]}
            ]}]}"#,
    );
    let s = ingest(&r, &report, &idx).unwrap();
    assert_eq!(
        s.unmatched,
        [super::UnmatchedPath { path: "util.py".to_string(), candidates: 2 }],
        "listed unmatched with its candidates count"
    );
    assert_eq!(s.bound, 1);

    let view = read_latest(&r).unwrap();
    let ambiguous = view.findings.iter().find(|f| f.rule_id == "r.ambiguous").unwrap();
    assert_eq!((ambiguous.binding, ambiguous.path.as_deref()), (FindingBinding::Ambiguous, None));
    assert_eq!(ambiguous.freshness, FRESHNESS_NA, "no file is guessed, so none is compared");
    assert_eq!(count(&r, "security_files"), 1, "only the unique path is anchored");
    let unique = view.findings.iter().find(|f| f.rule_id == "r.unique").unwrap();
    assert_eq!(unique.path.as_deref(), Some("services/a/util.py"), "a longer suffix still binds");
}

/// A report path that binds to exactly one indexed file which cannot be read on
/// disk is not anchored to a guess: it is listed unmatched with one candidate,
/// no file row is written, and it reads `n/a`.
#[test]
fn a_bound_but_unreadable_file_is_unmatched_with_one_candidate() {
    let (_t, r) = repo();
    let mut idx = indexed();
    idx.push("app/gone.py".to_string()); // indexed, but not on disk
    let report = write(
        &r,
        "gone.sarif",
        r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"t"}},"results":[
            {"ruleId":"r","message":{"text":"m"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"app/gone.py"},"region":{"startLine":1}}}]}]}]}"#,
    );
    let s = ingest(&r, &report, &idx).unwrap();
    assert_eq!(s.unmatched, [super::UnmatchedPath { path: "app/gone.py".to_string(), candidates: 1 }]);
    assert_eq!((s.bound, count(&r, "security_files")), (0, 0));
    let view = read_latest(&r).unwrap();
    let f = &view.findings[0];
    assert_eq!((f.binding, f.path.as_deref(), f.freshness), (FindingBinding::Unmatched, None, FRESHNESS_NA));
}

/// [FR-SC-01] acceptance 4: a result with no physical location is kept and
/// listed as unbound, never dropped.
#[test]
fn a_result_without_a_location_is_kept_as_unbound() {
    let (_t, r) = repo();
    let report = write(
        &r,
        "report.sarif",
        r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"t"}},"results":[
            {"ruleId":"r","message":{"text":"repository-wide"}},
            {"ruleId":"r","message":{"text":"logical only"},"locations":[{"logicalLocations":[{"fullyQualifiedName":"pkg"}]}]}]}]}"#,
    );
    let s = ingest(&r, &report, &indexed()).unwrap();
    assert_eq!((s.findings, s.unbound, s.bound), (2, 2, 0));
    assert!(s.unmatched.is_empty(), "no path, so nothing is unmatched");
    let view = read_latest(&r).unwrap();
    assert!(view
        .findings
        .iter()
        .all(|f| f.binding == FindingBinding::Unbound && f.report_path.is_none() && f.freshness == FRESHNESS_NA));
}

/// A zero-result run is evidence too: its scanner reads *fresh, 0 findings*,
/// distinct from a scanner that never ran (absent from the view).
#[test]
fn a_clean_scan_is_a_snapshot_with_zero_findings_not_never_run() {
    let (_t, r) = repo();
    let report = write(
        &r,
        "clean.sarif",
        r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"gosec","version":"2.21.4"}},"results":[]}]}"#,
    );
    ingest(&r, &report, &indexed()).unwrap();
    let view = read_latest(&r).unwrap();
    assert_eq!(view.snapshots.len(), 1);
    assert_eq!((view.snapshots[0].scanner_name.as_str(), view.snapshots[0].finding_count), ("gosec", 0));
    assert!(view.snapshots.iter().all(|s| s.scanner_name != "Bandit"), "Bandit never ran");
}

// ── FR-SC-02: identity ──────────────────────────────────────────────────────

/// The identity rule: the report's own fingerprint keys a finding wherever it
/// moves; without one, the bound path keys it, so a CI report's absolute path
/// and a local relative one name the same finding; two results sharing one
/// identity in one report are both kept.
#[test]
fn fingerprints_follow_the_report_identity_then_the_bound_location() {
    let (_t, r) = repo();
    let run = |uri: &str, line: u32, fp: &str| {
        format!(
            r#"{{"ruleId":"r","message":{{"text":"m"}},{fp}"locations":[{{"physicalLocation":{{"artifactLocation":{{"uri":"{uri}"}},"region":{{"startLine":{line}}}}}}}]}}"#
        )
    };
    let fingerprints = |results: &[String]| -> Vec<String> {
        let body = format!(
            r#"{{"version":"2.1.0","runs":[{{"tool":{{"driver":{{"name":"t"}}}},"results":[{}]}}]}}"#,
            results.join(",")
        );
        let report = write(&r, "fp.sarif", &body);
        ingest(&r, &report, &indexed()).unwrap();
        read_latest(&r)
            .unwrap()
            .findings
            .into_iter()
            .map(|f| f.fingerprint)
            .collect()
    };
    let local = fingerprints(&[run("app/db.py", 12, "")]);
    let ci = fingerprints(&[run("file:///home/ci/build/app/db.py", 12, "")]);
    assert_eq!(local, ci, "the bound path keys the location fallback");
    let moved = fingerprints(&[run("app/db.py", 13, "")]);
    assert_ne!(local, moved, "without a report identity, the location is the identity");

    let pfp = r#""partialFingerprints":{"primaryLocationLineHash":"abc:1"},"#;
    let a = fingerprints(&[run("app/db.py", 12, pfp)]);
    let b = fingerprints(&[run("app/db.py", 40, pfp)]);
    assert_eq!(a, b, "a report fingerprint survives a line shift");

    let twins = fingerprints(&[run("app/db.py", 12, pfp), run("app/db.py", 30, pfp)]);
    assert_eq!(twins.len(), 2, "two results with one identity are two findings");
    assert_ne!(twins[0], twins[1]);
    assert!(twins.iter().all(|f| f.len() == 64 && f.bytes().all(|b| b.is_ascii_hexdigit())));
}

/// Every later occurrence of one identity in one report takes a suffix of its
/// own, the third as well as the second: three results are three findings with
/// three keys, never a key collision that rejects the ingest.
#[test]
fn three_results_with_one_identity_are_three_findings() {
    let (_t, r) = repo();
    let result = |line: u32| {
        format!(
            r#"{{"ruleId":"r","message":{{"text":"m"}},"partialFingerprints":{{"primaryLocationLineHash":"abc:1"}},"locations":[{{"physicalLocation":{{"artifactLocation":{{"uri":"app/db.py"}},"region":{{"startLine":{line}}}}}}}]}}"#
        )
    };
    let body = format!(
        r#"{{"version":"2.1.0","runs":[{{"tool":{{"driver":{{"name":"t"}}}},"results":[{},{},{}]}}]}}"#,
        result(12),
        result(30),
        result(48)
    );
    let report = write(&r, "triplets.sarif", &body);
    ingest(&r, &report, &indexed()).expect("three results with one identity ingest without a key collision");
    let keys: BTreeSet<String> = read_latest(&r).unwrap().findings.into_iter().map(|f| f.fingerprint).collect();
    assert_eq!(keys.len(), 3, "three results with one identity are three findings with three keys");
}

/// The path is part of every identity, the report's own fingerprint included:
/// CodeQL gives two files carrying one identical line the same
/// `primaryLocationLineHash`, and those are two findings with two keys — a
/// suppression or a fix of one never lands on the other. An unmatched path keys
/// by the report's spelling, separator-normalised.
#[test]
fn the_same_report_fingerprint_in_two_files_is_two_identities() {
    let (_t, r) = repo();
    let result = |uri: &str| {
        format!(
            r#"{{"ruleId":"py/sql-injection","message":{{"text":"m"}},
                "partialFingerprints":{{"primaryLocationLineHash":"5b3a9c0e7d2f1a48:1"}},
                "locations":[{{"physicalLocation":{{"artifactLocation":{{"uri":"{uri}","uriBaseId":"%SRCROOT%"}},"region":{{"startLine":12}}}}}}]}}"#
        )
    };
    let report = |results: &[String]| {
        write(
            &r,
            "codeql.sarif",
            &format!(
                r#"{{"version":"2.1.0","runs":[{{"tool":{{"driver":{{"name":"CodeQL"}}}},"results":[{}]}}]}}"#,
                results.join(",")
            ),
        )
    };
    let keys = |path: &PathBuf| -> Vec<(Option<String>, String)> {
        ingest(&r, path, &indexed()).unwrap();
        read_latest(&r)
            .unwrap()
            .findings
            .into_iter()
            .map(|f| (f.path, f.fingerprint))
            .collect()
    };

    let both = keys(&report(&[result("app/db.py"), result("app/views.py")]));
    assert_eq!(both.len(), 2);
    assert_ne!(both[0].1, both[1].1, "one line hash in two files is two identities");

    // Fix the first file: the second keeps its own key, not the first one's.
    let only_views = keys(&report(&[result("app/views.py")]));
    let views_before = both.iter().find(|(p, _)| p.as_deref() == Some("app/views.py")).unwrap();
    assert_eq!(only_views.as_slice(), std::slice::from_ref(views_before), "the surviving finding keeps its key");

    // An unmatched path keys by the report's spelling, `\\` and `./` normalised.
    let a = keys(&report(&[result("lib/vendored.py")]));
    let b = keys(&report(&[result(".\\\\lib\\\\vendored.py")]));
    assert_eq!(a[0].0, None, "not an indexed file");
    assert_eq!(a[0].1, b[0].1, "one unmatched location spelled two ways is one key");
}

// ── FR-SC-03: no secret, no snippet ─────────────────────────────────────────

/// [FR-SC-03] acceptance 1 / [UAT-SC-01]: the planted secret — present in the
/// Gitleaks snippet and commit message, the Bandit message and snippets, and the
/// gosec snippet — appears in no `history.db` row and in no byte of the store's
/// files.
///
/// [UAT-SC-01]: ../../../../docs/specs/requirements/UAT-SC-01.md
#[test]
fn a_planted_secret_reaches_no_history_db_row_or_byte() {
    // Control: the secret really is in each fixture, in the fields named above.
    for file in ["gitleaks.sarif", "bandit.sarif", "gosec.sarif"] {
        let text = std::fs::read_to_string(fixture(file)).unwrap();
        assert!(text.contains(PLANTED), "{file} carries the planted secret");
    }
    let bandit = std::fs::read_to_string(fixture("bandit.sarif")).unwrap();
    assert!(bandit.contains(&format!("Possible hardcoded password: '{PLANTED}'")), "in a message");

    let (_t, r) = repo();
    let mut redacted = 0;
    for &(file, ..) in SCANNERS {
        redacted += ingest_fixture(&r, file).redacted;
    }
    // Trivy 2, Gitleaks 2, Bandit B105, gosec G101, Checkov CKV_SECRET_2.
    assert_eq!(redacted, 7, "every secret-class finding was redacted");

    let rows = dump(&r);
    assert!(rows.len() > 60, "the dump read the store ({} rows)", rows.len());
    let hits: Vec<&String> = rows.iter().filter(|l| l.contains(PLANTED)).collect();
    assert!(hits.is_empty(), "no history.db row holds the secret: {hits:?}");

    // The raw store bytes, per secret-bearing report, each ingested alone into
    // its own store and read straight after: a later ingest reuses freed pages,
    // so a secret written and then deleted or overwritten would be erased before
    // the grep if the check ran only once at the end.
    for file in ["gitleaks.sarif", "bandit.sarif", "gosec.sarif"] {
        let (_t, alone) = repo();
        ingest_fixture(&alone, file);
        let db = crate::history::db_path(&alone);
        let main = std::fs::read(&db).expect("the store file exists");
        assert!(!main.is_empty());
        assert!(
            !main.windows(PLANTED.len()).any(|w| w == PLANTED.as_bytes()),
            "{file}: history.db holds the secret in some page"
        );
        // A commit can still sit in the WAL; when there is one, it is clean too.
        let wal = db.with_extension("db-wal");
        let wal_clean = match std::fs::read(&wal) {
            Ok(bytes) => !bytes.windows(PLANTED.len()).any(|w| w == PLANTED.as_bytes()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            Err(e) => panic!("{}: {e}", wal.display()),
        };
        assert!(wal_clean, "{file}: the WAL holds the secret");
    }

    // The redacted message names the rule and location, nothing else.
    let view = read_latest(&r).unwrap();
    let b105 = view.findings.iter().find(|f| f.rule_id == "B105").unwrap();
    assert_eq!(b105.message, "secret-class finding redacted: rule B105 at app/settings.py:3");
}

/// [FR-SC-03] acceptance 3: no stored finding has a snippet field — not in the
/// schema, not in the read model — while every fixture does carry snippets
/// somewhere, and a non-secret finding keeps its rule message.
#[test]
fn no_stored_finding_has_a_snippet_field() {
    let with_snippets: Vec<&str> = SCANNERS
        .iter()
        .map(|s| s.0)
        .filter(|file| std::fs::read_to_string(fixture(file)).unwrap().contains("\"snippet\""))
        .collect();
    assert_eq!(
        with_snippets,
        ["semgrep.sarif", "gitleaks.sarif", "bandit.sarif", "gosec.sarif", "checkov.sarif"],
        "the fixtures do carry snippets"
    );

    let (_t, r) = repo();
    for &(file, ..) in SCANNERS {
        ingest_fixture(&r, file);
    }
    let conn = Connection::open(crate::history::db_path(&r)).unwrap();
    for table in ["security_snapshots", "security_files", "security_findings"] {
        let columns: Vec<String> = conn
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(!columns.is_empty(), "{table} exists");
        for banned in ["snippet", "context", "code", "region_text", "description", "help"] {
            assert!(
                columns.iter().all(|c| !c.contains(banned)),
                "{table} has no {banned} column: {columns:?}"
            );
        }
    }

    let view = read_latest(&r).unwrap();
    let json = serde_json::to_string(&view).unwrap();
    assert!(!json.contains("snippet"), "the read model carries no snippet field");
    assert!(!json.contains("cursor.execute"), "no source line survives into any field");
    let semgrep = view
        .findings
        .iter()
        .find(|f| f.scanner_name == "Semgrep OSS")
        .unwrap();
    assert_eq!(
        semgrep.message,
        "Detected possible formatted SQL query. Use parameterized queries instead.",
        "a non-secret finding keeps its rule message"
    );
}

// ── Migration and survival ──────────────────────────────────────────────────

/// The security tables arrive by a forward-only migration from the pre-change
/// schema (version 3): an existing store with coverage evidence upgrades in
/// place, its coverage rows untouched, and the new tables start empty.
#[test]
fn the_security_migration_upgrades_a_pre_change_store_in_place() {
    let (_t, r) = repo();
    let path = crate::history::db_path(&r);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let conn = crate::history::db::open_at_version(&path, 3).unwrap();
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, 3, "the pre-change schema");
        let has_security: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name LIKE 'security_%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_security, 0);
        conn.execute(
            "INSERT INTO coverage_snapshots (head_sha, config_hash, created_at) VALUES ('h', 'c', 1)",
            [],
        )
        .unwrap();
    }

    let summary = ingest_fixture(&r, "semgrep.sarif");
    assert_eq!(summary.findings, 2);
    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, crate::history::latest_schema_version());
    assert!(version >= 4, "migration 4 applied, and every later one");
    let coverage: (String, String) = conn
        .query_row("SELECT head_sha, config_hash FROM coverage_snapshots", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(coverage, ("h".to_string(), "c".to_string()), "coverage evidence survives");
}

/// [FR-SC-02] acceptance 4, through the engine: `Engine::security_ingest` binds
/// against the indexed graph, and a full `index` does not delete the evidence.
#[test]
fn the_engine_binds_against_the_index_and_evidence_survives_a_full_index() {
    let (_t, r) = repo();
    std::fs::create_dir_all(r.join(".logos")).unwrap();
    std::fs::write(r.join(".logos/config.toml"), "[project]\nid = \"sarif-fixture\"\n").unwrap();
    let engine = crate::Engine::start(&r).expect("engine starts");
    let indexed = engine.index();
    assert!(indexed.files_indexed > 0, "the fixture indexes: {:?}", indexed.warnings);

    let s = engine.security_ingest(&fixture("semgrep.sarif")).unwrap();
    assert_eq!((s.findings, s.bound), (2, 2), "both findings bind through the graph's file set");
    let before = dump(&r);

    let _ = engine.index();
    assert_eq!(dump(&r), before, "security evidence survives a full logos.db rebuild");
    assert!(engine.security_ingest(&fixture("semgrep.sarif")).unwrap().already_ingested);
}

/// [S-668] migration 5: a store at schema 4 holding a security snapshot
/// migrates to 5 on its next open, the snapshot survives, no baseline reads as
/// `None` (not an empty set), and a saved baseline replaces the last one whole.
///
/// [S-668]: ../../../../docs/planning/journal.md#s-668-a-security-rule-table-gates-on-new-findings-and-caps-and-a-missing-scan-exits-5
#[test]
fn the_baseline_migration_upgrades_a_schema_4_store_and_the_baseline_round_trips() {
    let (_t, r) = repo();
    let path = crate::history::db_path(&r);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let conn = crate::history::db::open_at_version(&path, 4).unwrap();
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, 4, "the pre-change schema");
        let has_baseline: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master WHERE name LIKE 'security_baseline%'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(has_baseline, 0);
        conn.execute(
            "INSERT INTO security_snapshots (head_sha, scanner_name, scanner_version, report_hash, format, created_at)
             VALUES ('h', 'Semgrep OSS', '1.85.0', 'r', 'sarif', 1)",
            [],
        )
        .unwrap();
    }

    assert_eq!(super::read_baseline(&r).unwrap(), None, "never saved is not an empty baseline");
    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, crate::history::latest_schema_version());
    assert!(version >= 5, "migration 5 applied, and every later one");
    let kept: String = conn.query_row("SELECT scanner_name FROM security_snapshots", [], |r| r.get(0)).unwrap();
    assert_eq!(kept, "Semgrep OSS", "security evidence survives the migration");

    let pair = |s: &str, f: &str| (s.to_string(), f.to_string());
    let saved = super::save_baseline(&r, Some("h"), "c1", &[pair("A", "f2"), pair("A", "f1"), pair("B", "f1")]).unwrap();
    assert_eq!(saved, 2, "a fingerprint two scanners share is recorded once");
    let baseline = super::read_baseline(&r).unwrap().unwrap();
    assert_eq!((baseline.head_sha.as_deref(), baseline.config_hash.as_str()), (Some("h"), "c1"));
    assert_eq!(baseline.fingerprints.iter().map(String::as_str).collect::<Vec<_>>(), ["f1", "f2"]);

    super::save_baseline(&r, None, "c2", &[pair("A", "f3")]).unwrap();
    let baseline = super::read_baseline(&r).unwrap().unwrap();
    assert_eq!((baseline.head_sha, baseline.config_hash.as_str()), (None, "c2"));
    assert_eq!(baseline.fingerprints.into_iter().collect::<Vec<_>>(), ["f3"], "replaced whole");
}

/// S-674: a read over a project that never stored a report is empty and
/// creates no store — the workspace roll-up reads never-scanned members, and a
/// GET writes nothing ([ADR-28]).
///
/// [ADR-28]: ../../../../docs/specs/architecture/decisions/ADR-28.md
#[test]
fn a_read_with_no_store_is_empty_and_creates_none() {
    let tmp = TempDir::new().unwrap();
    let view = read_latest(tmp.path()).unwrap();
    assert!(view.snapshots.is_empty() && view.findings.is_empty(), "{view:?}");
    assert!(!crate::history::db_path(tmp.path()).exists(), "the read created history.db");
    assert!(!tmp.path().join(".logos").exists(), "the read created .logos/");
    // The gate band's baseline read is a read too.
    assert_eq!(super::read_baseline(tmp.path()).unwrap(), None, "an absent store reads as None");
    assert!(!tmp.path().join(".logos").exists(), "the baseline read created .logos/");
}

// ── FR-SC-17: a scoped report replaces only what it scanned ─────────────────

/// A Semgrep report holding `results` — `(uri, rule, start line)`, an empty
/// uri meaning no location.
fn semgrep_report(results: &[(&str, &str, i64)]) -> String {
    let results: Vec<serde_json::Value> = results
        .iter()
        .map(|&(uri, rule, line)| {
            let mut result = serde_json::json!({ "ruleId": rule, "message": { "text": format!("{rule} fired") } });
            if !uri.is_empty() {
                result["locations"] = serde_json::json!([{
                    "physicalLocation": { "artifactLocation": { "uri": uri }, "region": { "startLine": line } }
                }]);
            }
            result
        })
        .collect();
    serde_json::json!({
        "version": "2.1.0",
        "runs": [{ "tool": { "driver": { "name": "Semgrep OSS", "semanticVersion": "1.85.0" } }, "results": results }]
    })
    .to_string()
}

/// The latest snapshot's rows of `scanner` whose location is not `path`, every
/// column but the snapshot id: what must stay byte-identical across a scoped
/// ingest of `path` ([FR-SC-17]).
///
/// [FR-SC-17]: ../../../../docs/specs/requirements/FR-SC-17.md
fn rows_outside(root: &Path, scanner: &str, path: &str) -> (Vec<String>, Vec<String>) {
    let conn = Connection::open(crate::history::db_path(root)).unwrap();
    let id: i64 = conn
        .query_row("SELECT MAX(id) FROM security_snapshots WHERE scanner_name = ?1", [scanner], |r| r.get(0))
        .unwrap();
    let text = |v: rusqlite::types::ValueRef<'_>| match v {
        rusqlite::types::ValueRef::Null => "NULL".to_string(),
        rusqlite::types::ValueRef::Integer(n) => n.to_string(),
        rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
        other => format!("{other:?}"),
    };
    let rows = |sql: &str| -> Vec<String> {
        let mut stmt = conn.prepare(sql).unwrap();
        let cols = stmt.column_count();
        let mut out: Vec<String> = stmt
            .query_map(rusqlite::params![id, path], |row| {
                Ok((0..cols).map(|i| text(row.get_ref(i).unwrap())).collect::<Vec<_>>().join("|"))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        out.sort();
        out
    };
    (
        rows(
            "SELECT fingerprint, rule_id, severity, cwes, message, redacted, binding, report_path, path,
                    start_line, start_column, end_line, end_column
               FROM security_findings
              WHERE snapshot_id = ?1 AND COALESCE(path, report_path, '') <> ?2",
        ),
        rows("SELECT path, content_hash FROM security_files WHERE snapshot_id = ?1 AND path <> ?2"),
    )
}

/// [FR-SC-17] acceptance 1, at the store: a scoped Semgrep report on one file
/// replaces only that file's Semgrep findings and anchor. Every other file's
/// findings — an unbound and an unmatched one included — and freshness rows
/// are byte-identical, another scanner's snapshot is untouched, and a stray
/// result outside the scope is dropped, never stored.
///
/// [FR-SC-17]: ../../../../docs/specs/requirements/FR-SC-17.md
#[test]
fn a_scoped_ingest_replaces_only_the_scanned_files_findings_and_every_other_row_is_byte_identical() {
    let (_t, r) = repo();
    let full = write(
        &r,
        "semgrep-full.sarif",
        &semgrep_report(&[
            ("app/db.py", "r.one", 12),
            ("app/db.py", "r.two", 18),
            ("app/views.py", "r.three", 5),
            ("", "r.nowhere", 0),
            ("vendor/lib.py", "r.vendored", 3),
        ]),
    );
    ingest(&r, &full, &indexed()).unwrap();
    let bandit = ingest_fixture(&r, "bandit.sarif");
    let bandit_rows = |r: &Path| -> Vec<String> {
        let prefix = |t: &str| format!("{t}|{}|", bandit.scanners[0].snapshot_id);
        dump(r).into_iter().filter(|l| l.starts_with(&prefix("security_findings")) || l.starts_with(&prefix("security_files"))).collect()
    };
    let before = rows_outside(&r, "Semgrep OSS", "app/db.py");
    let bandit_before = bandit_rows(&r);
    let others = |r: &Path| -> String {
        let view = read_latest(r).unwrap();
        serde_json::to_string(&view.findings.iter().filter(|f| f.path.as_deref() != Some("app/db.py")).collect::<Vec<_>>()).unwrap()
    };
    let others_before = others(&r);
    assert_eq!(before.0.len(), 3, "views.py, the unbound and the unmatched finding: {:?}", before.0);
    assert_eq!(before.1.len(), 1, "views.py's anchor: {:?}", before.1);

    // The fix: db.py changes, and a scoped run finds one result there (and,
    // misbehaving, one outside its scope).
    std::fs::write(r.join("app/db.py"), "import sqlite3\n# fixed\n").unwrap();
    let scoped = write(&r, "semgrep-scoped.sarif", &semgrep_report(&[("app/db.py", "r.two", 2), ("app/config.py", "r.stray", 1)]));
    let summary = super::ingest_scoped(&r, &scoped, &indexed(), Some(super::IngestScope::replacing(&["app/db.py".to_string()]))).unwrap();
    assert_eq!(summary.findings, 1, "the stray result outside the scope is dropped");
    assert!(!summary.already_ingested);

    assert_eq!(rows_outside(&r, "Semgrep OSS", "app/db.py"), before, "every other file's rows are byte-identical");
    assert_eq!(bandit_rows(&r), bandit_before, "another scanner's snapshot is untouched");
    assert_eq!(others(&r), others_before, "every other finding reads back identically, freshness included");

    let view = read_latest(&r).unwrap();
    let db: Vec<(&str, Option<i64>, &str)> = view
        .findings
        .iter()
        .filter(|f| f.scanner_name == "Semgrep OSS" && f.path.as_deref() == Some("app/db.py"))
        .map(|f| (f.rule_id.as_str(), f.start_line, f.freshness))
        .collect();
    assert_eq!(db, [("r.two", Some(2), FRESHNESS_FRESH)], "db.py holds only the scoped run's finding, anchored anew");
    assert!(view.findings.iter().all(|f| f.rule_id != "r.stray" && f.rule_id != "r.one"));
    let semgrep = view.snapshots.iter().find(|s| s.scanner_name == "Semgrep OSS").unwrap();
    assert_eq!(semgrep.full_head_sha.as_deref(), Some(summary.head_sha.as_str()), "the full scan stands behind it");
    assert_eq!(semgrep.scope, ["app/db.py"]);
    assert_eq!(semgrep.finding_count, 4);
}

/// [FR-SC-14] and [S-760]: a store holding only `--path` scans still has its
/// findings column. Freshness is the finding's own, so a scoped report's finding
/// in an untouched file counts exactly as on the Security page and in the
/// workspace roll-up; only a store with no report at all has no column.
///
/// [FR-SC-14]: ../../../../docs/specs/requirements/FR-SC-14.md
/// [S-760]: ../../../../docs/planning/journal.md#s-760-one-freshness-definition-across-the-security-surfaces
#[test]
fn a_store_of_scoped_scans_alone_counts_its_findings_per_file() {
    let (_t, r) = repo();
    let one = |name: &str, uri: &str| write(&r, name, &semgrep_report(&[(uri, "r.one", 1)]));
    let today = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    assert_eq!(super::path_findings(&r, today).unwrap(), None, "nothing stored: no column, not a column of 0");

    let db = vec!["app/db.py".to_string()];
    super::ingest_scoped(&r, &one("a.sarif", "app/db.py"), &indexed(), Some(super::IngestScope::replacing(&db))).unwrap();
    let cells = super::path_findings(&r, today).unwrap().expect("a stored report has a column");
    assert_eq!(cells["app/db.py"].count, 1, "its finding is in an untouched file: fresh");
    assert!(!cells.contains_key("app/views.py"), "a file with no finding has no cell");

    ingest(&r, &one("full.sarif", "app/views.py"), &indexed()).unwrap();
    let cells = super::path_findings(&r, today).unwrap().expect("a full scan stands behind the report");
    assert_eq!(cells["app/views.py"].count, 1);
}

/// A scoped snapshot records the full scan behind its carried rows: none when
/// only scoped runs exist, the earlier HEAD's when a scoped run at a new HEAD
/// carries them — whose snapshot is kept as provenance — and scopes accumulate
/// until a full scan clears them ([FR-SC-17], migration 6).
///
/// [FR-SC-17]: ../../../../docs/specs/requirements/FR-SC-17.md
#[test]
fn a_scoped_snapshot_names_the_full_scan_behind_it_or_none() {
    let (_t, r) = repo();
    let one = |name: &str, uri: &str| write(&r, name, &semgrep_report(&[(uri, "r.one", 1)]));
    let scope = |paths: &[&str]| paths.iter().map(|p| p.to_string()).collect::<Vec<_>>();
    let latest = |r: &Path| read_latest(r).unwrap().snapshots.into_iter().find(|s| s.scanner_name == "Semgrep OSS").unwrap();

    super::ingest_scoped(&r, &one("a.sarif", "app/db.py"), &indexed(), Some(super::IngestScope::replacing(&scope(&["app/db.py"])))).unwrap();
    let only = latest(&r);
    assert_eq!((only.full_head_sha, only.scope), (None, scope(&["app/db.py"])), "no full scan stands behind it");

    let head_a = ingest(&r, &one("full.sarif", "app/views.py"), &indexed()).unwrap().head_sha;
    let full = latest(&r);
    assert_eq!((full.full_head_sha.as_deref(), full.scope.len()), (Some(head_a.as_str()), 0), "a full scan clears the scope");

    std::fs::write(r.join("app/config.py"), "import sys\n").unwrap();
    sh_git(&r, &["commit", "-q", "-am", "next"]);
    let at_b = super::ingest_scoped(&r, &one("b.sarif", "app/db.py"), &indexed(), Some(super::IngestScope::replacing(&scope(&["app/db.py"])))).unwrap();
    assert_ne!(at_b.head_sha, head_a);
    let carried = latest(&r);
    assert_eq!(carried.head_sha, at_b.head_sha);
    assert_eq!(carried.full_head_sha.as_deref(), Some(head_a.as_str()), "the carried rows came from HEAD A's full scan");
    assert_eq!(carried.finding_count, 2, "views.py's finding was carried, db.py's added");
    assert_eq!(count(&r, "security_snapshots"), 2, "HEAD A's snapshot is kept as provenance");

    let c = one("c.sarif", "app/views.py");
    super::ingest_scoped(&r, &c, &indexed(), Some(super::IngestScope::replacing(&scope(&["app/views.py"])))).unwrap();
    let before = dump(&r);
    let rerun = super::ingest_scoped(&r, &c, &indexed(), Some(super::IngestScope::replacing(&scope(&["app/views.py"])))).unwrap();
    assert!(rerun.already_ingested, "the same report over the same scope is a no-op");
    assert_eq!(dump(&r), before, "and writes nothing");
    let other = super::ingest_scoped(&r, &c, &indexed(), Some(super::IngestScope::replacing(&scope(&["app/db.py"])))).unwrap();
    assert!(!other.already_ingested, "the same bytes over another scope are new evidence");
    assert_ne!(other.report_hash, rerun.report_hash);
    super::ingest_scoped(&r, &c, &indexed(), Some(super::IngestScope::replacing(&scope(&["app/views.py"])))).unwrap();
    let again = latest(&r);
    assert_eq!(again.scope, scope(&["app/db.py", "app/views.py"]), "scopes accumulate, sorted");
    assert_eq!(again.full_head_sha.as_deref(), Some(head_a.as_str()));
    assert_eq!(count(&r, "security_snapshots"), 2, "a scoped run at the same HEAD replaces, never adds");
}

/// [S-685] migration 6: a schema-5 store's snapshot reads as the full scan it
/// was — its own HEAD behind it, no scope.
///
/// [S-685]: ../../../../docs/planning/journal.md#s-685-a-security-re-scan-can-target-one-scanner-and-a-set-of-paths
#[test]
fn the_scope_migration_reads_a_schema_5_snapshot_as_a_full_scan() {
    let (_t, r) = repo();
    let path = crate::history::db_path(&r);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let conn = crate::history::db::open_at_version(&path, 5).unwrap();
        conn.execute(
            "INSERT INTO security_snapshots (head_sha, scanner_name, scanner_version, report_hash, format, created_at)
             VALUES ('h', 'Semgrep OSS', '1.85.0', 'r', 'sarif', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO security_findings (snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted, binding)
             VALUES (1, 'f', 'r.one', 'high', '', 'm', 0, 'unbound')",
            [],
        )
        .unwrap();
    }
    let view = read_latest(&r).unwrap();
    assert_eq!(view.snapshots.len(), 1);
    let scoped: i64 = Connection::open(&path).unwrap().query_row("SELECT scoped FROM security_findings", [], |r| r.get(0)).unwrap();
    assert_eq!(scoped, 0, "a row written before migration 6 is a full scan's");
    assert_eq!((view.snapshots[0].full_head_sha.as_deref(), view.snapshots[0].scope.len()), (Some("h"), 0));
    let version: i64 =
        Connection::open(&path).unwrap().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, crate::history::latest_schema_version());
    assert!(version >= 6, "migration 6 applied, and every later one");
}

/// A scope holds a path by whole segments: its near misses are outside it.
#[test]
fn a_scope_holds_its_paths_and_what_lies_under_them_by_whole_segments() {
    let scope = |s: &[&str]| s.iter().map(|p| p.to_string()).collect::<Vec<_>>();
    assert!(super::in_scope(Some("app/db.py"), &scope(&["app/db.py"])));
    assert!(super::in_scope(Some("app/db.py"), &scope(&["app"])));
    assert!(!super::in_scope(Some("app/db.py"), &scope(&["app/db"])), "a prefix that is not a segment");
    assert!(!super::in_scope(Some("app/db.pyc"), &scope(&["app/db.py"])));
    assert!(!super::in_scope(Some("application/x.py"), &scope(&["app"])));
    assert!(!super::in_scope(None, &scope(&["app"])), "no location is in no scope");
}

/// A finding in scope on a file the index does not hold is matched by the path
/// its report named: a scoped run stores the new one ([FR-SC-17]).
///
/// [FR-SC-17]: ../../../../docs/specs/requirements/FR-SC-17.md
#[test]
fn a_scoped_run_stores_a_finding_on_a_file_outside_the_index_in_scope() {
    let (_t, r) = repo();
    let scoped = write(&r, "new.sarif", &semgrep_report(&[("app/new.py", "r.new", 1)]));
    let summary = super::ingest_scoped(&r, &scoped, &indexed(), Some(super::IngestScope::replacing(&["app/new.py".to_string()]))).unwrap();
    assert_eq!(summary.findings, 1, "the finding on a file outside the index is in scope");
    let view = read_latest(&r).unwrap();
    let stored: Vec<(&str, Option<&str>, FindingBinding)> =
        view.findings.iter().map(|f| (f.rule_id.as_str(), f.report_path.as_deref(), f.binding)).collect();
    assert_eq!(stored, [("r.new", Some("app/new.py"), FindingBinding::Unmatched)]);
}

/// A carried finding unbound to the index is inside the scope by the path its
/// report named: a scoped run of that path that finds nothing removes it, so a
/// fixed finding never survives its own re-scan ([FR-SC-17]).
///
/// [FR-SC-17]: ../../../../docs/specs/requirements/FR-SC-17.md
#[test]
fn a_scoped_run_replaces_a_carried_finding_on_a_file_outside_the_index() {
    let (_t, r) = repo();
    let full = write(&r, "full.sarif", &semgrep_report(&[("vendor/lib.py", "r.vendored", 3), ("app/db.py", "r.one", 1)]));
    ingest(&r, &full, &indexed()).unwrap();
    let clean = write(&r, "clean.sarif", &semgrep_report(&[]));
    super::ingest_scoped(&r, &clean, &indexed(), Some(super::IngestScope::replacing(&["vendor/lib.py".to_string()]))).unwrap();
    let rules: Vec<String> = read_latest(&r).unwrap().findings.into_iter().map(|f| f.rule_id).collect();
    assert_eq!(rules, ["r.one"], "vendor/lib.py's finding is gone, db.py's carried");
}

/// A secret finding at `line:start-end` in `app/settings.py`, as a stored row.
fn secret_row(fingerprint: &str, line: i64, start: i64, end: i64) -> super::store::FindingRow {
    super::store::FindingRow {
        fingerprint: fingerprint.to_string(),
        rule_id: "generic-api-key".to_string(),
        severity: Severity::Medium,
        cwes: Vec::new(),
        message: "redacted".to_string(),
        redacted: true,
        binding: FindingBinding::Unmatched,
        report_path: Some("app/settings.py".to_string()),
        path: None,
        region: super::Region { start_line: Some(line), start_column: Some(start), end_line: Some(line), end_column: Some(end) },
        scoped: false,
        anchor_commit: None,
        commits: Vec::new(),
        legacy_fingerprints: Vec::new(),
    }
}

/// How a scoped Gitleaks finding is told to be a kept history finding stored
/// before migration 7, still keyed by its report (the coordinator's S-685
/// decision): the same file, rule and whole position;
/// else, for a secret whose lines moved, the same column span when exactly one
/// finding on each side has it. Ties go to the lowest fingerprint, and a
/// different rule or file never matches.
#[test]
fn a_scoped_secret_matches_a_kept_one_by_position_then_by_a_unique_span() {
    use super::store::matches_kept;
    let kept = [secret_row("b-hist", 4, 14, 45), secret_row("a-hist", 4, 14, 45), secret_row("c-hist", 9, 3, 20)];
    // Exact position: one fresh finding takes one kept one; a second fresh copy
    // at that position takes the other.
    assert_eq!(matches_kept(&[secret_row("x", 4, 14, 45)], &kept), [0]);
    assert_eq!(matches_kept(&[secret_row("x", 4, 14, 45), secret_row("y", 4, 14, 45)], &kept), [0, 1]);
    // Lines moved by two: the 3-20 span is unique on both sides, so it matches.
    assert_eq!(matches_kept(&[secret_row("x", 11, 3, 20)], &kept), [0]);
    // Two fresh findings share that span: neither is matched by span alone.
    assert!(matches_kept(&[secret_row("x", 11, 3, 20), secret_row("y", 12, 3, 20)], &kept).is_empty());
    // A new secret at a new span is new.
    assert!(matches_kept(&[secret_row("x", 6, 10, 40)], &kept).is_empty());
    // Another rule or another file never matches.
    let mut other_rule = secret_row("x", 4, 14, 45);
    other_rule.rule_id = "aws-access-token".to_string();
    let mut other_file = secret_row("y", 4, 14, 45);
    other_file.report_path = Some("app/config.py".to_string());
    assert!(matches_kept(&[other_rule, other_file], &kept).is_empty());
    // A kept row keyed by its secret (migration 7) is never matched by position:
    // a scoped finding of that secret has its very fingerprint.
    let mut secret_keyed = secret_row("d-hist", 4, 14, 45);
    secret_keyed.legacy_fingerprints = vec!["old".to_string()];
    assert!(matches_kept(&[secret_row("x", 4, 14, 45)], &[secret_keyed]).is_empty());
}
