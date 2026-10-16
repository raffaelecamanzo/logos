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
fn dump(root: &Path) -> Vec<String> {
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
    // OSV: an absolute `file://` URI binds by suffix; no level → medium.
    let f = find("osv-scanner", "PYSEC-2018-28");
    assert_eq!((f.binding, f.path.as_deref(), f.severity), (FindingBinding::Bound, Some("requirements.txt"), Severity::Medium));
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
/// band and no level is SARIF's `warning`. The captured Trivy report pins the
/// 4.0 edge on real data.
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
        (None, None, Severity::Medium),
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
    assert_eq!(version, 4);
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
