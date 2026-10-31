//! One advisory on one package is one finding, whichever scanners reported it
//! ([S-809], [FR-SC-29], [UAT-SC-14]).
//!
//! Every test ingests SARIF shaped on each scanner's own writer
//! (`tests/fixtures/sarif/README.md`): OSV-Scanner's absolute `file://` lockfile
//! path, `Package '<name>@<version>' is vulnerable to '<id>' (also known as …)`
//! message and fixed `warning` level, so it reads `unrated` without a score;
//! Trivy's `ROOTPATH`-relative path, `Package:` / `Installed Version:` lines and
//! Go `v`-prefixed version; Dependency-Check's purl in `logicalLocations`.
//!
//! [S-809]: ../../../../docs/planning/journal.md#s-809-one-advisory-on-one-package-is-one-finding-whichever-scanners-reported-it
//! [FR-SC-29]: ../../../../docs/specs/requirements/FR-SC-29.md
//! [UAT-SC-14]: ../../../../docs/specs/requirements/UAT-SC-14.md

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde_json::{json, Value};
use tempfile::TempDir;

use super::identity_tests::{git, write};
use super::{ingest, label, read_latest, stored_finding, IgnoreFile, SecurityFinding, Severity};

/// The advisory desk-picker's `go.mod` excelize result reads under: its CVE,
/// with the Go and GitHub records OSV-Scanner lists as aliases.
const CVE: &str = "CVE-2026-107214";
const GO_ID: &str = "GO-2026-6684";
const GHSA: &str = "GHSA-2j4c-ffch-9f23";
const EXCELIZE: &str = "github.com/xuri/excelize/v2";

/// A git project with a `go.mod` and an npm lockfile, neither indexed.
fn project(dir: &Path) -> PathBuf {
    let root = dir.join("project");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, None, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join(".git/info/exclude"), ".logos/history.db*\n.logos/logos.db*\n").unwrap();
    write(&root, "go.mod", "module example.test/app\n");
    write(&root, "frontend/package-lock.json", "{}\n");
    write(&root, "src/main.rs", "fn main() {}\n");
    git(&root, None, &["add", "-A"]);
    git(&root, Some("2026-10-01T09:00:00+00:00"), &["commit", "-q", "-m", "c1"]);
    root
}

/// An OSV-Scanner 2.7.0 result: `package` (`name@version`) vulnerable to
/// `rule`, also known as `aliases`, at `path` under `root` by absolute URI.
fn osv(root: &Path, path: &str, package: &str, rule: &str, aliases: &[&str]) -> Value {
    let aka: Vec<String> = aliases.iter().map(|a| format!("'{a}'")).collect();
    json!({
        "ruleId": rule, "ruleIndex": 0, "level": "warning", "kind": "fail",
        "message": {"text": format!("Package '{package}' is vulnerable to '{rule}' (also known as {}).", aka.join(", "))},
        "locations": [{"physicalLocation": {"artifactLocation": {"index": -1, "uri": format!("file://{}/{path}", root.display())}}}]
    })
}

/// A Trivy 0.76.0 result: `name` at `version` vulnerable to `rule`, at
/// `path`, rated by `score` (a CVSS `security-severity`).
fn trivy(path: &str, name: &str, version: &str, rule: &str, score: &str) -> Value {
    json!({
        "ruleId": rule, "level": "error", "properties": {"security-severity": score},
        "message": {"text": format!("Package: {name}\nInstalled Version: {version}\nVulnerability {rule}\nSeverity: HIGH")},
        "locations": [{
            "physicalLocation": {"artifactLocation": {"uri": path, "uriBaseId": "ROOTPATH"}, "region": {"startLine": 1, "startColumn": 1, "endLine": 1, "endColumn": 1}},
            "message": {"text": format!("{path}: {name}@{version}")}
        }]
    })
}

/// A Dependency-Check 10.0.4 result: the dependency `purl` vulnerable to
/// `rule`, at `path` under `root` by absolute URI.
fn dependency_check(root: &Path, path: &str, purl: &str, rule: &str) -> Value {
    json!({
        "ruleId": rule, "level": "error",
        "message": {"text": format!("{rule} - a description")},
        "locations": [{
            "physicalLocation": {"artifactLocation": {"uri": format!("file://{}/{path}", root.display())}},
            "logicalLocations": [{"fullyQualifiedName": purl}]
        }]
    })
}

/// One run of `driver` at `version` holding `results`.
fn run(driver: &str, version: &str, results: Vec<Value>) -> Value {
    json!({"tool": {"driver": {"name": driver, "version": version, "rules": []}}, "results": results})
}

/// Ingest `runs` as one report under `root`.
fn ingest_runs(dir: &Path, root: &Path, name: &str, runs: Vec<Value>) {
    let path = dir.join(name);
    std::fs::write(&path, json!({"version": "2.1.0", "runs": runs}).to_string()).unwrap();
    ingest(root, &path, &["src/main.rs".to_string()]).unwrap();
}

/// desk-picker's `go.mod` advisory as both scanners write it: OSV-Scanner's
/// result twice, byte for byte, unrated; Trivy's under its CVE at `score`.
fn excelize_runs(root: &Path, score: &str) -> Vec<Value> {
    let repeated = osv(root, "go.mod", &format!("{EXCELIZE}@2.11.0"), CVE, &[GO_ID, GHSA]);
    vec![
        run("osv-scanner", "2.7.0", vec![repeated.clone(), repeated]),
        run("Trivy", "0.76.0", vec![trivy("go.mod", EXCELIZE, "v2.11.0", CVE, score)]),
    ]
}

/// The findings' `(scanner, rule)` reporters, each finding's in order.
fn reporters(findings: &[SecurityFinding]) -> Vec<Vec<(String, String)>> {
    findings
        .iter()
        .map(|f| f.reported_by().into_iter().map(|(s, r)| (s.to_string(), r.to_string())).collect())
        .collect()
}

/// FR-SC-29 AC 1–2, UAT-SC-14: OSV-Scanner's repeated `go.mod` result and
/// Trivy's on the same package and version — `v2.11.0` against `2.11.0` —
/// under one CVE are one finding naming both scanners, each by its own key,
/// at the higher severity: Trivy's `high`, which OSV-Scanner's `unrated`
/// never lowers.
#[test]
fn an_advisory_osv_scanner_and_trivy_report_on_one_lockfile_is_one_finding_naming_both() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = project(&dir);
    ingest_runs(&dir, &root, "go.sarif", excelize_runs(&root, "7.5"));

    let findings = read_latest(&root).unwrap().findings;
    assert_eq!(findings.len(), 1, "{findings:#?}");
    let merged = &findings[0];
    assert_eq!(
        reporters(&findings),
        [vec![("Trivy".to_string(), CVE.to_string()), ("osv-scanner".to_string(), CVE.to_string())]],
        "both scanners are named, in read order"
    );
    assert_eq!(merged.severity, Severity::High);
    assert_eq!(merged.reporters[1].severity, Severity::Unrated, "OSV-Scanner's own rating is kept on its reporter");
    let keys: Vec<&str> = merged.reporters.iter().map(|r| r.fingerprint.as_str()).collect();
    assert_ne!(keys[0], keys[1], "each scanner keeps its own fingerprint");
    for key in &keys {
        assert!(merged.known_by(key), "the merged finding is known by every reporter's key");
        assert_eq!(stored_finding(&root, key).unwrap().as_ref(), Some(merged), "and found by it");
    }
    let payload = serde_json::to_value(merged).unwrap();
    assert_eq!(payload["reporters"].as_array().map(Vec::len), Some(2), "the payload names its reporters: {payload}");
}

/// FR-SC-29: an `unrated` member never lowers a rated one, even a rating that
/// sorts below `unrated` — OSV-Scanner unrated beside Trivy's `low` reads `low`.
#[test]
fn an_unrated_reporter_never_lowers_a_low_rating() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = project(&dir);
    ingest_runs(&dir, &root, "go.sarif", excelize_runs(&root, "3.1"));

    let findings = read_latest(&root).unwrap().findings;
    assert_eq!(findings.len(), 1, "{findings:#?}");
    assert_eq!(findings[0].severity, Severity::Low, "not `unrated`, where an unrated finding sorts above `low`");
}

/// FR-SC-29 AC 3, and the near misses one field off a merge: two advisories
/// on one package whose aliases do not meet stay two; one advisory on another
/// version or in another lockfile stays apart; and a GHSA id one scanner
/// reports under joins the CVE another lists it beside.
#[test]
fn only_one_lockfile_package_version_and_a_shared_alias_merge() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = project(&dir);
    let lockfile = "frontend/package-lock.json";
    ingest_runs(
        &dir,
        &root,
        "npm.sarif",
        vec![
            run(
                "osv-scanner",
                "2.7.0",
                vec![
                    osv(&root, lockfile, "vitest@3.2.7", "CVE-2026-1", &["GHSA-aaaa-bbbb-cccc"]),
                    osv(&root, lockfile, "vitest@3.2.7", "CVE-2026-2", &["GHSA-dddd-eeee-ffff"]),
                    osv(&root, lockfile, "lodash@4.17.20", "CVE-2026-3", &[]),
                    osv(&root, "go.mod", "lodash@4.17.20", "CVE-2026-4", &[]),
                ],
            ),
            run(
                "Trivy",
                "0.76.0",
                vec![
                    // Under the GHSA OSV-Scanner lists as CVE-2026-1's alias.
                    trivy(lockfile, "vitest", "3.2.7", "GHSA-aaaa-bbbb-cccc", "7.5"),
                    // CVE-2026-3, on another version.
                    trivy(lockfile, "lodash", "4.17.21", "CVE-2026-3", "7.5"),
                    // CVE-2026-4, in another lockfile.
                    trivy(lockfile, "lodash", "4.17.20", "CVE-2026-4", "7.5"),
                ],
            ),
        ],
    );

    let findings = read_latest(&root).unwrap().findings;
    let mut merged: Vec<Vec<(String, String)>> = reporters(&findings).into_iter().filter(|r| r.len() > 1).collect();
    merged.sort();
    assert_eq!(
        merged,
        [vec![("Trivy".to_string(), "GHSA-aaaa-bbbb-cccc".to_string()), ("osv-scanner".to_string(), "CVE-2026-1".to_string())]],
        "only the shared alias on one lockfile, package and version merges"
    );
    assert_eq!(findings.len(), 6, "seven results, two of them one advisory: {findings:#?}");
}

/// FR-SC-29 for Dependency-Check: its purl `pkg:npm/%40vitest/mocker@3.2.7`
/// names OSV-Scanner's `@vitest/mocker@3.2.7`, read from the package the row
/// stores (history.db migration 14) — its message names none.
#[test]
fn a_dependency_check_purl_merges_with_the_package_another_scanner_names() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = project(&dir);
    let lockfile = "frontend/package-lock.json";
    ingest_runs(
        &dir,
        &root,
        "npm.sarif",
        vec![
            run("dependency-check", "10.0.4", vec![dependency_check(&root, lockfile, "pkg:npm/%40vitest/mocker@3.2.7", "CVE-2026-9")]),
            run("osv-scanner", "2.7.0", vec![osv(&root, lockfile, "@vitest/mocker@3.2.7", "CVE-2026-9", &[])]),
        ],
    );

    let findings = read_latest(&root).unwrap().findings;
    // OSV-Scanner's message names the package and Dependency-Check's does not,
    // so OSV-Scanner's finding stands for it, though Dependency-Check sorts first.
    assert_eq!(
        reporters(&findings),
        [vec![("osv-scanner".to_string(), "CVE-2026-9".to_string()), ("dependency-check".to_string(), "CVE-2026-9".to_string())]]
    );
    assert_eq!(findings[0].severity, Severity::High, "Dependency-Check's `error` outranks OSV-Scanner's unrated");
}

/// A row stored before history.db migration 14 has no package column: an
/// OSV-Scanner or Trivy row is still merged by the package its message names,
/// and a Dependency-Check row, whose message names none, is read on its own.
#[test]
fn a_row_stored_before_migration_14_merges_by_its_message() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = project(&dir);
    let lockfile = "frontend/package-lock.json";
    let mut runs = excelize_runs(&root, "7.5");
    runs.push(run("dependency-check", "10.0.4", vec![dependency_check(&root, lockfile, "pkg:npm/vitest@3.2.7", "CVE-2026-9")]));
    runs.push(run("osv-scanner", "2.7.0", vec![osv(&root, lockfile, "vitest@3.2.7", "CVE-2026-9", &[])]));
    // One run per driver in one report: the second OSV-Scanner run joins the first.
    let osv_results: Vec<Value> = [&runs[0], &runs[3]].iter().flat_map(|r| r["results"].as_array().unwrap().clone()).collect();
    let runs = vec![run("osv-scanner", "2.7.0", osv_results), runs[1].clone(), runs[2].clone()];
    ingest_runs(&dir, &root, "all.sarif", runs);
    let conn = crate::history::open(&root).unwrap();
    conn.execute("UPDATE security_findings SET package_name = NULL, package_version = NULL", []).unwrap();
    drop(conn);

    let findings = read_latest(&root).unwrap().findings;
    let mut merged: Vec<Vec<(String, String)>> = reporters(&findings);
    merged.sort();
    assert_eq!(
        merged,
        [
            vec![("Trivy".to_string(), CVE.to_string()), ("osv-scanner".to_string(), CVE.to_string())],
            vec![("dependency-check".to_string(), "CVE-2026-9".to_string())],
            vec![("osv-scanner".to_string(), "CVE-2026-9".to_string())],
        ]
    );
}

/// history.db 13 → 14 marks each Dependency-Check snapshot's report hash, and
/// no other's, so re-ingesting its very report stores the package.
#[test]
fn migration_14_marks_dependency_check_snapshots_alone() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("history.db");
    let conn = crate::history::db::open_at_version(&path, 13).unwrap();
    for (head, scanner, hash) in
        [("h1", "dependency-check", "a"), ("h2", "Dependency-Check", "b"), ("h1", "osv-scanner", "c"), ("h3", "dependency-check", "pre-13:d")]
    {
        conn.execute(
            "INSERT INTO security_snapshots (head_sha, scanner_name, scanner_version, report_hash, format, created_at)
             VALUES (?1, ?2, '1', ?3, 'sarif', 0)",
            [head, scanner, hash],
        )
        .unwrap();
    }
    drop(conn);
    let conn = crate::history::db::open_at_version(&path, 14).unwrap();
    let mut stmt = conn.prepare("SELECT report_hash FROM security_snapshots ORDER BY id").unwrap();
    let hashes: Vec<String> = stmt.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(hashes, ["pre-14:a", "pre-14:b", "c", "pre-13:d"]);
}

/// FR-SC-29 suppression: an entry naming either reporter's fingerprint
/// suppresses the merged finding; a rule-wide entry narrowed to one scanner
/// names that reporter alone, so the finding stays unsuppressed until the
/// other reporter is suppressed too.
#[test]
fn a_merged_finding_is_suppressed_only_when_every_reporter_is() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = project(&dir);
    ingest_runs(&dir, &root, "go.sarif", excelize_runs(&root, "7.5"));
    let findings = read_latest(&root).unwrap().findings;
    let today = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();
    let suppressed = |ignore: &str| {
        let labelled = label(&findings, &IgnoreFile::parse(ignore).unwrap(), &root, today);
        (labelled.findings[0].is_suppressed(), labelled.entries.iter().map(|e| (e.orphaned, e.suppressed)).collect::<Vec<_>>())
    };

    for reporter in &findings[0].reporters {
        let entry = format!("[[ignore]]\nfingerprint = \"{}\"\nreason = \"tracked upstream\"\n", reporter.fingerprint);
        assert_eq!(suppressed(&entry), (true, vec![(false, 1)]), "{}'s key suppresses the finding", reporter.scanner_name);
    }
    let trivy_rule = format!("[[ignore]]\nrule = \"{CVE}\"\nscanner = \"Trivy\"\nreason = \"not reachable\"\n");
    assert_eq!(suppressed(&trivy_rule), (false, vec![(false, 0)]), "one reporter suppressed is not the finding");
    let both = format!("{trivy_rule}\n[[ignore]]\nrule = \"{CVE}\"\nscanner = \"osv-scanner\"\nreason = \"not reachable\"\n");
    assert_eq!(suppressed(&both), (true, vec![(false, 1), (false, 0)]), "every reporter suppressed is; the first labels it");
}

/// S-809 review: a merged finding reads the dependency scope of its package
/// (FR-SC-30) whichever scanner sorts first. Dependency-Check, whose message
/// names no package, sorts before OSV-Scanner; the merged finding is one a
/// package-naming reporter stands for, so its `dev` scope is not lost and it
/// stays out of the headline.
#[test]
fn a_merged_finding_on_a_dev_only_package_reads_dev_whichever_scanner_sorts_first() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = project(&dir);
    let lockfile = "frontend/package-lock.json";
    write(
        &root,
        lockfile,
        r#"{"lockfileVersion":3,"packages":{"node_modules/@vitest/mocker":{"version":"3.2.7","dev":true}}}"#,
    );
    ingest_runs(
        &dir,
        &root,
        "npm.sarif",
        vec![
            run("dependency-check", "10.0.4", vec![dependency_check(&root, lockfile, "pkg:npm/%40vitest/mocker@3.2.7", "CVE-2026-9")]),
            run("osv-scanner", "2.7.0", vec![osv(&root, lockfile, "@vitest/mocker@3.2.7", "CVE-2026-9", &[])]),
        ],
    );

    let findings = read_latest(&root).unwrap().findings;
    assert_eq!(findings.len(), 1, "{findings:#?}");
    let scoped = crate::security::scope::scopes(&root, &findings.iter().collect::<Vec<_>>(), |f| Ok(vec![false; f.len()])).unwrap();
    assert_eq!(scoped[0].code_scope, super::CodeScope::Dev, "{:?}", findings[0]);
    assert_eq!(findings[0].reported_by()[0].0, findings[0].scanner_name, "the finding is its first reporter");
}

/// S-809 review: a merged finding none of whose reporters is suppressed is
/// labelled by the first entry naming any of them — an expired entry naming
/// OSV-Scanner alone still says it expired, though Trivy, which sorts first,
/// is named by none.
#[test]
fn an_expired_entry_naming_any_reporter_labels_the_merged_finding() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = project(&dir);
    ingest_runs(&dir, &root, "go.sarif", excelize_runs(&root, "7.5"));
    let findings = read_latest(&root).unwrap().findings;
    let today = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();
    for scanner in ["Trivy", "osv-scanner"] {
        let ignore = format!("[[ignore]]\nrule = \"{CVE}\"\nscanner = \"{scanner}\"\nreason = \"lapsed\"\nexpires = 2026-01-01\n");
        let labelled = label(&findings, &IgnoreFile::parse(&ignore).unwrap(), &root, today);
        assert!(
            matches!(labelled.findings[0].suppression, Some(super::Suppression::Expired { .. })),
            "an expired entry naming {scanner} labels it: {:?}",
            labelled.findings[0].suppression
        );
    }
}

/// S-809 review: a merged finding is fresh while any reporter's own scan of
/// its file is: Trivy scanned `go.mod` before an edit and OSV-Scanner after
/// it, so the advisory is current although its first reporter's anchor moved.
#[test]
fn a_merged_finding_is_fresh_while_any_reporter_scanned_its_file_as_it_is() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = project(&dir);
    let runs = excelize_runs(&root, "7.5");
    let indexed = ["go.mod".to_string()];
    let ingest_one = |name: &str, run: &Value| {
        let path = dir.join(name);
        std::fs::write(&path, json!({"version": "2.1.0", "runs": [run]}).to_string()).unwrap();
        ingest(&root, &path, &indexed).unwrap();
    };
    ingest_one("trivy.sarif", &runs[1]);
    write(&root, "go.mod", "module example.test/app\n\ngo 1.23\n");
    ingest_one("osv.sarif", &runs[0]);

    let findings = read_latest(&root).unwrap().findings;
    assert_eq!(findings.len(), 1, "{findings:#?}");
    let own: Vec<(&str, &str)> = findings[0].reporters.iter().map(|r| (r.scanner_name.as_str(), r.freshness)).collect();
    assert_eq!(own, [("Trivy", crate::history::FRESHNESS_STALE), ("osv-scanner", crate::history::FRESHNESS_FRESH)]);
    assert_eq!(findings[0].freshness, crate::history::FRESHNESS_FRESH);
}

/// S-809 review: an ignore entry written under a key a later reporter had
/// before a re-key still names the merged finding, and is told that
/// reporter's key, not the first reporter's, as the one to write.
#[test]
fn an_entry_under_a_later_reporters_old_key_is_told_that_reporters_key() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = project(&dir);
    ingest_runs(&dir, &root, "go.sarif", excelize_runs(&root, "7.5"));
    let mut merged = read_latest(&root).unwrap().findings.remove(0);
    let old = "0".repeat(64);
    merged.reporters[1].legacy_fingerprints = vec![old.clone()];
    let ignore = IgnoreFile::parse(&format!("[[ignore]]\nfingerprint = \"{old}\"\nreason = \"tracked upstream\"\n")).unwrap();
    let labelled = label(std::slice::from_ref(&merged), &ignore, &root, NaiveDate::from_ymd_opt(2026, 10, 28).unwrap());
    assert!(labelled.findings[0].is_suppressed(), "the old key still names it");
    assert_eq!(labelled.entries[0].should_name.as_deref(), Some(merged.reporters[1].fingerprint.as_str()));
}

/// S-809 review: one reporter suppressed and the other named by an expired
/// entry is not suppressed, and says why: the expired entry labels it.
#[test]
fn one_reporter_suppressed_and_one_lapsed_reads_expired() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = project(&dir);
    ingest_runs(&dir, &root, "go.sarif", excelize_runs(&root, "7.5"));
    let findings = read_latest(&root).unwrap().findings;
    for (live, lapsed) in [("Trivy", "osv-scanner"), ("osv-scanner", "Trivy")] {
        let ignore = format!(
            "[[ignore]]\nrule = \"{CVE}\"\nscanner = \"{live}\"\nreason = \"not reachable\"\n\n\
             [[ignore]]\nrule = \"{CVE}\"\nscanner = \"{lapsed}\"\nreason = \"lapsed\"\nexpires = 2026-01-01\n"
        );
        let labelled = label(&findings, &IgnoreFile::parse(&ignore).unwrap(), &root, NaiveDate::from_ymd_opt(2026, 10, 28).unwrap());
        assert!(
            matches!(&labelled.findings[0].suppression, Some(super::Suppression::Expired { reason, .. }) if reason == "lapsed"),
            "{live} suppressed, {lapsed} lapsed: {:?}",
            labelled.findings[0].suppression
        );
    }
}

/// S-809 review: a merged finding carries every reporter's CWEs, so an OWASP
/// category or `--category` filter one reporter's rule reads is the
/// finding's: here only OSV-Scanner's rule, the later reporter's, has one.
#[test]
fn a_merged_finding_carries_every_reporters_cwes() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = project(&dir);
    let mut runs = excelize_runs(&root, "7.5");
    runs[0]["tool"]["driver"]["rules"] = json!([{"id": CVE, "properties": {"tags": ["CWE-1333"]}}]);
    ingest_runs(&dir, &root, "go.sarif", runs);

    let findings = read_latest(&root).unwrap().findings;
    assert_eq!((findings.len(), findings[0].scanner_name.as_str()), (1, "Trivy"), "{findings:#?}");
    assert_eq!(findings[0].cwes, ["CWE-1333"]);
}

/// S-809 review: advisory ids meet ASCII case-insensitively — OSV-Scanner's
/// `ghsa-…` alias is Trivy's `GHSA-…` rule.
#[test]
fn advisory_ids_meet_whatever_their_letter_case() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = project(&dir);
    let lockfile = "frontend/package-lock.json";
    ingest_runs(
        &dir,
        &root,
        "npm.sarif",
        vec![
            run("osv-scanner", "2.7.0", vec![osv(&root, lockfile, "vitest@3.2.7", "CVE-2026-1", &["ghsa-aaaa-bbbb-cccc"])]),
            run("Trivy", "0.76.0", vec![trivy(lockfile, "vitest", "3.2.7", "GHSA-AAAA-BBBB-CCCC", "7.5")]),
        ],
    );
    assert_eq!(read_latest(&root).unwrap().findings.len(), 1);
}

/// The closure is transitive whatever the read order: Trivy's CVE and
/// Dependency-Check's GHSA share no id, and are read first, as two findings;
/// OSV-Scanner, read last, lists both as one advisory, so the three are one.
#[test]
fn a_reporter_read_last_joins_two_findings_its_aliases_bridge() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let root = project(&dir);
    let lockfile = "frontend/package-lock.json";
    ingest_runs(
        &dir,
        &root,
        "npm.sarif",
        vec![
            run("Trivy", "0.76.0", vec![trivy(lockfile, "vitest", "3.2.7", "CVE-2026-1", "7.5")]),
            run("dependency-check", "10.0.4", vec![dependency_check(&root, lockfile, "pkg:npm/vitest@3.2.7", "GHSA-aaaa-bbbb-cccc")]),
            run("osv-scanner", "2.7.0", vec![osv(&root, lockfile, "vitest@3.2.7", "PYSEC-2026-1", &["CVE-2026-1", "GHSA-aaaa-bbbb-cccc"])]),
        ],
    );
    let findings = read_latest(&root).unwrap().findings;
    let mut names: Vec<String> = findings.iter().flat_map(|f| f.reported_by()).map(|(scanner, _)| scanner.to_string()).collect();
    names.sort();
    assert_eq!((findings.len(), names), (1, vec!["Trivy".to_string(), "dependency-check".to_string(), "osv-scanner".to_string()]), "{findings:#?}");
}
