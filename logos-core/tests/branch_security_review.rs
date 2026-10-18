//! The branch security delta (S-686, [FR-SC-18], [UAT-SC-06]), end-to-end
//! through `Engine::review` over real git fixtures and ingested SARIF reports.
//!
//! Coverage by acceptance criterion:
//! - an added secret is `introduced` via `hunk` with no merge-base report, and
//!   via `fingerprint` with one;
//! - a pre-existing finding inside an edited hunk is `touched`;
//! - a removed finding is `resolved` only once a re-scan saw its file as it
//!   stands; with no re-scan, or an edit after it, it is never resolved;
//! - suppressed findings are listed apart, and the planted secret appears
//!   nowhere in the output.
//!
//! No scanner is run: each report is a SARIF document ingested at the commit
//! it describes, which is what a scan stores. Every fixture is a throw-away
//! repository with `GIT_CEILING_DIRECTORIES` set and git maintenance off.
//!
//! [FR-SC-18]: ../../docs/specs/requirements/FR-SC-18.md
//! [UAT-SC-06]: ../../docs/specs/requirements/UAT-SC-06.md

#![cfg(feature = "lang-python")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use logos_core::models::quality::{
    BranchReview, SecurityDelta, SecurityDeltaEntry, SecurityDeltaStatus,
};
use logos_core::history::security::IngestScope;
use logos_core::security::{FindingsFilter, ScanEnvironment};
use logos_core::Engine;
use serde_json::Value;
use tempfile::TempDir;

/// The secret the fixture plants. It must never reach the review's output.
const PLANTED: &str = "planted-secret-7f3a9c";

/// A query built by concatenation: the SQL-injection finding sits on line 2.
const DB_PY: &str = "\
def query(conn, name):
    return conn.execute(\"SELECT * FROM t WHERE n = '\" + name + \"'\")


def other():
    return 1
";

/// A weak hash on line 2, never edited by any branch.
const UTIL_PY: &str = "\
def digest(data):
    return hashlib.md5(data).hexdigest()
";

struct Project {
    _tmp: TempDir,
    root: PathBuf,
    reports: PathBuf,
    env: ScanEnvironment,
}

/// One SARIF result: rule, path, line and the report's own fingerprint.
type Hit<'a> = (&'a str, &'a str, i64, &'a str);

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c", "user.email=dev@logos", "-c", "user.name=Logos Dev",
            "-c", "commit.gpgsign=false", "-c", "core.hooksPath=",
            "-c", "maintenance.auto=false", "-c", "gc.auto=0",
        ])
        .args(args)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn head(root: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", message]);
}

/// Set `rel`'s modification time `secs` seconds from now (negative: earlier).
fn touch(root: &Path, rel: &str, secs: i64) {
    let now = SystemTime::now();
    let when = if secs < 0 {
        now - Duration::from_secs(secs.unsigned_abs())
    } else {
        now + Duration::from_secs(secs.unsigned_abs())
    };
    fs::File::options().write(true).open(root.join(rel)).unwrap().set_modified(when).unwrap();
}

/// A repository whose `main` holds `app/db.py` and `app/util.py`.
fn project() -> Project {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (root, user, bin, reports) = (base.join("project"), base.join("user"), base.join("bin"), base.join("reports"));
    for dir in [&root.join(".logos"), &user, &bin, &reports] {
        fs::create_dir_all(dir).unwrap();
    }
    fs::write(root.join(".logos/config.toml"), "[project]\nid = \"branch-security-review\"\n").unwrap();
    write(&root, ".gitignore", ".logos/\n");
    write(&root, "app/db.py", DB_PY);
    write(&root, "app/util.py", UTIL_PY);
    git(&root, &["init", "-q", "-b", "main"]);
    commit(&root, "base");
    let env = ScanEnvironment {
        user_dir: Some(user),
        search_path: Some(bin.into_os_string()),
        tools_dir: None,
    };
    Project { _tmp: tmp, root, reports, env }
}

impl Project {
    fn engine(&self) -> Engine {
        let engine = Engine::start(&self.root).expect("engine").with_scan_environment(self.env.clone());
        engine.index();
        engine
    }

    /// Ingest `driver`'s report of `hits` at the current HEAD.
    fn ingest(&self, driver: &str, hits: &[Hit]) {
        let report = self.report(driver, hits);
        self.engine().security_ingest(&report).expect("the report ingests");
    }

    /// Ingest `driver`'s report of `hits` as a `--path` re-scan of `paths` at
    /// the current HEAD, which is what a scoped scan stores.
    fn ingest_scoped(&self, driver: &str, hits: &[Hit], paths: &[String]) {
        let report = self.report(driver, hits);
        let indexed = ["app/db.py", "app/util.py"].map(String::from);
        logos_core::history::security::ingest_scoped(&self.root, &report, &indexed, Some(IngestScope::replacing(paths)))
            .expect("the scoped report ingests");
    }

    /// Write `driver`'s SARIF report of `hits`.
    fn report(&self, driver: &str, hits: &[Hit]) -> PathBuf {
        let results: Vec<Value> = hits
            .iter()
            .map(|(rule, path, line, id)| {
                serde_json::json!({
                    "ruleId": rule,
                    // A secret scanner's message carries the secret itself.
                    "message": {"text": format!("{rule} matched {PLANTED}")},
                    "partialFingerprints": {"primaryLocationLineHash": id},
                    // The injection carries its CWE, so it falls under an OWASP category.
                    "properties": {"tags": if *rule == "python.sql-injection" { vec!["CWE-89"] } else { Vec::new() }},
                    "locations": [{"physicalLocation": {
                        "artifactLocation": {"uri": path},
                        "region": {"startLine": line, "endLine": line}
                    }}]
                })
            })
            .collect();
        let report = self.reports.join(format!("{}.sarif", driver.replace(' ', "-")));
        let sarif = serde_json::json!({"version": "2.1.0", "runs": [{
            "tool": {"driver": {"name": driver, "semanticVersion": "1.0.0"}},
            "results": results
        }]});
        fs::write(&report, sarif.to_string()).unwrap();
        report
    }

    /// The `security findings` listing's entry for `fingerprint`.
    fn listed(&self, fingerprint: &str) -> Value {
        let listing = serde_json::to_value(self.engine().security_findings(&FindingsFilter::default()).unwrap()).unwrap();
        listing["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["fingerprint"] == fingerprint)
            .unwrap_or_else(|| panic!("{fingerprint} is listed: {listing}"))
            .clone()
    }

    fn review(&self) -> (BranchReview, SecurityDelta, String) {
        let review = self.engine().review(Some("main")).expect("review runs");
        assert!(review.measured, "measured: {:?}", review.not_measured_because);
        let json = serde_json::to_string(&review).unwrap();
        let delta = review.sections.security.clone().expect("a measured review carries the security section");
        assert!(delta.measured, "security measured: {:?}", delta.not_measured_because);
        (review, delta, json)
    }
}

/// The semgrep findings `main` starts with: the injection and the weak hash.
const SEMGREP_BASE: &[Hit] = &[
    ("python.sql-injection", "app/db.py", 2, "sql-1"),
    ("python.weak-hash", "app/util.py", 2, "md5-1"),
];

fn entry<'a>(delta: &'a SecurityDelta, rule: &str) -> &'a SecurityDeltaEntry {
    delta
        .findings
        .iter()
        .find(|e| e.rule_id == rule)
        .unwrap_or_else(|| panic!("{rule} is listed: {:#?}", delta.findings))
}

/// `(status, method)` of the entry for `rule`.
fn class(delta: &SecurityDelta, rule: &str) -> (SecurityDeltaStatus, &'static str) {
    let e = entry(delta, rule);
    (e.status, e.method)
}

/// The feature branch adds a module holding the planted secret.
fn add_secret(p: &Project) {
    git(&p.root, &["checkout", "-q", "-b", "feature"]);
    write(&p.root, "app/config.py", &format!("API_TOKEN = \"{PLANTED}\"\n"));
    commit(&p.root, "add a config module");
}

#[test]
fn an_added_secret_is_introduced_via_hunk_without_a_merge_base_report_and_via_fingerprint_with_one() {
    // No report recorded at the merge-base: the hunk decides.
    let p = project();
    add_secret(&p);
    p.ingest("gitleaks", &[("generic-api-key", "app/config.py", 1, "secret-1")]);
    p.ingest("Semgrep OSS", SEMGREP_BASE);
    let (_, delta, json) = p.review();
    assert_eq!(class(&delta, "generic-api-key"), (SecurityDeltaStatus::Introduced, SecurityDelta::METHOD_HUNK));
    assert_eq!(class(&delta, "python.sql-injection"), (SecurityDeltaStatus::PreExisting, SecurityDelta::METHOD_HUNK));
    let secret = entry(&delta, "generic-api-key");
    assert!(secret.redacted, "{secret:?}");
    assert_eq!(secret.freshness, "fresh");
    for e in [secret, entry(&delta, "python.sql-injection")] {
        let reach = &p.listed(&e.fingerprint)["context"]["reachability"];
        assert_eq!(
            (Value::from(e.reachability.as_str()), Value::from(e.confidence.as_str())),
            (reach["verdict"].clone(), reach["confidence"].clone()),
            "S-669's reachability claim, field for field: {e:?}"
        );
    }
    assert_ne!(secret.reachability, secret.confidence, "the verdict and its confidence are distinct tokens here");
    let at = head(&p.root);
    assert_eq!(
        delta.scanners.iter().map(|s| (s.scanner.as_str(), s.method, s.latest_head.as_str())).collect::<Vec<_>>(),
        vec![("Semgrep OSS", SecurityDelta::METHOD_HUNK, at.as_str()), ("gitleaks", SecurityDelta::METHOD_HUNK, at.as_str())],
        "every scanner with a report, by name, each with how it was compared"
    );
    assert_eq!((delta.counts.introduced, delta.counts.pre_existing), (1, 2), "{:?}", delta.counts);
    assert_eq!(
        delta.findings.iter().map(|e| (e.status, e.path.as_deref().unwrap_or(""))).collect::<Vec<_>>(),
        vec![
            (SecurityDeltaStatus::Introduced, "app/config.py"),
            (SecurityDeltaStatus::PreExisting, "app/db.py"),
            (SecurityDeltaStatus::PreExisting, "app/util.py"),
        ],
        "ordered by status, then path"
    );
    assert!(!json.contains(PLANTED), "the planted secret leaked: {json}");
    let (_, _, again) = p.review();
    assert_eq!(json, again, "a rerun is byte-identical");

    // A report recorded at the merge-base: the fingerprint decides.
    let p = project();
    p.ingest("gitleaks", &[]);
    p.ingest("Semgrep OSS", SEMGREP_BASE);
    add_secret(&p);
    p.ingest("gitleaks", &[("generic-api-key", "app/config.py", 1, "secret-1")]);
    p.ingest("Semgrep OSS", SEMGREP_BASE);
    let (_, delta, json) = p.review();
    assert_eq!(
        class(&delta, "generic-api-key"),
        (SecurityDeltaStatus::Introduced, SecurityDelta::METHOD_FINGERPRINT)
    );
    assert_eq!(
        class(&delta, "python.sql-injection"),
        (SecurityDeltaStatus::PreExisting, SecurityDelta::METHOD_FINGERPRINT)
    );
    assert_eq!(
        delta.scanners.iter().map(|s| (s.scanner.as_str(), s.method)).collect::<Vec<_>>(),
        vec![("Semgrep OSS", SecurityDelta::METHOD_FINGERPRINT), ("gitleaks", SecurityDelta::METHOD_FINGERPRINT)]
    );
    assert!(!json.contains(PLANTED), "the planted secret leaked: {json}");
}

#[test]
fn a_fingerprint_absent_at_the_merge_base_is_introduced_even_outside_any_hunk() {
    // The hunk alone would call it pre-existing: the record wins.
    let p = project();
    p.ingest("Semgrep OSS", &SEMGREP_BASE[..1]);
    git(&p.root, &["checkout", "-q", "-b", "feature"]);
    // A commit of its own, so the branch's report is not stored at the
    // merge-base commit (one report per commit and scanner).
    write(&p.root, "README.md", "notes\n");
    commit(&p.root, "notes");
    p.ingest("Semgrep OSS", SEMGREP_BASE);
    let (_, delta, _) = p.review();
    assert_eq!(
        class(&delta, "python.weak-hash"),
        (SecurityDeltaStatus::Introduced, SecurityDelta::METHOD_FINGERPRINT)
    );
}

#[test]
fn a_pre_existing_finding_inside_an_edited_hunk_is_touched() {
    let p = project();
    p.ingest("Semgrep OSS", SEMGREP_BASE);
    git(&p.root, &["checkout", "-q", "-b", "feature"]);
    write(&p.root, "app/db.py", &DB_PY.replace("SELECT *", "SELECT id"));
    commit(&p.root, "narrow the query");
    p.ingest("Semgrep OSS", SEMGREP_BASE);
    let (_, delta, _) = p.review();
    assert_eq!(
        class(&delta, "python.sql-injection"),
        (SecurityDeltaStatus::Touched, SecurityDelta::METHOD_FINGERPRINT)
    );
    assert_eq!(
        class(&delta, "python.weak-hash"),
        (SecurityDeltaStatus::PreExisting, SecurityDelta::METHOD_FINGERPRINT),
        "a finding outside every hunk is not touched"
    );
    assert_eq!(delta.counts.touched, 1, "{:?}", delta.counts);
}

#[test]
fn a_removed_finding_is_resolved_only_on_a_rescanned_file_and_stale_otherwise() {
    let p = project();
    p.ingest("Semgrep OSS", SEMGREP_BASE);
    git(&p.root, &["checkout", "-q", "-b", "fix"]);
    write(&p.root, "app/db.py", "def query(conn, name):\n    return conn.execute(\"SELECT * FROM t WHERE n = ?\", (name,))\n");
    commit(&p.root, "bind the parameter");

    // Edited, not re-scanned: the merge-base report is still the latest.
    let (_, delta, _) = p.review();
    let sql = entry(&delta, "python.sql-injection");
    let owasp = p.listed(&sql.fingerprint)["owasp"].clone();
    assert!(owasp.as_array().is_some_and(|c| !c.is_empty()), "the injection has a category: {owasp}");
    assert_eq!(serde_json::to_value(&sql.owasp).unwrap(), owasp, "the categories the findings listing gives");
    // Still carried by the latest report, so it keeps its class — its line
    // was edited — and its freshness says the report predates the edit.
    assert_eq!((sql.status, sql.freshness.as_str()), (SecurityDeltaStatus::Touched, "stale"), "{sql:?}");
    assert_eq!(delta.counts.resolved, 0, "{:?}", delta.counts);
    assert!(delta.notes.iter().any(|n| n.contains("has not re-scanned")), "{:?}", delta.notes);

    // Re-scanned after the edit: resolved.
    touch(&p.root, "app/db.py", -120);
    p.ingest("Semgrep OSS", &SEMGREP_BASE[1..]);
    let (_, delta, _) = p.review();
    let sql = entry(&delta, "python.sql-injection");
    assert_eq!((sql.status, sql.method, sql.freshness.as_str()), (SecurityDeltaStatus::Resolved, SecurityDelta::METHOD_FINGERPRINT, "fresh"));
    assert_eq!(serde_json::to_value(&sql.owasp).unwrap(), owasp, "a resolved finding keeps its merge-base categories");
    assert_eq!(class(&delta, "python.weak-hash").0, SecurityDeltaStatus::PreExisting);

    // Edited again after that re-scan: never claimed resolved.
    write(&p.root, "app/db.py", "def query(conn, name):\n    return None\n");
    touch(&p.root, "app/db.py", 120);
    let (_, delta, _) = p.review();
    let sql = entry(&delta, "python.sql-injection");
    assert_eq!((sql.status, sql.freshness.as_str()), (SecurityDeltaStatus::Stale, "stale"), "{sql:?}");
    assert_eq!((delta.counts.resolved, delta.counts.stale), (0, 1), "{:?}", delta.counts);
}

#[test]
fn a_file_that_still_carries_a_finding_of_the_rescan_is_judged_by_its_anchor() {
    let p = project();
    p.ingest("Semgrep OSS", SEMGREP_BASE);
    git(&p.root, &["checkout", "-q", "-b", "fix"]);
    // A second finding stays on util.py after the weak hash is fixed.
    write(&p.root, "app/util.py", "def digest(data):\n    return hashlib.sha256(data).hexdigest()\n    assert data\n");
    commit(&p.root, "swap the hash");
    // The file is newer than the ingest, but the re-scan anchors it fresh.
    touch(&p.root, "app/util.py", 120);
    p.ingest("Semgrep OSS", &[SEMGREP_BASE[0], ("python.assert-used", "app/util.py", 3, "assert-1")]);
    let (_, delta, _) = p.review();
    let weak = entry(&delta, "python.weak-hash");
    assert_eq!((weak.status, weak.freshness.as_str()), (SecurityDeltaStatus::Resolved, "fresh"), "{weak:?}");
    assert_eq!(class(&delta, "python.assert-used").0, SecurityDeltaStatus::Introduced);
}

#[test]
fn suppressed_findings_are_listed_apart_and_no_secret_value_is_emitted() {
    let p = project();
    add_secret(&p);
    p.ingest("gitleaks", &[("generic-api-key", "app/config.py", 1, "secret-1")]);
    p.ingest("Semgrep OSS", SEMGREP_BASE);
    let (_, delta, _) = p.review();
    let fingerprint = entry(&delta, "generic-api-key").fingerprint.clone();
    write(
        &p.root,
        ".logos/security-ignore.toml",
        &format!("[[ignore]]\nfingerprint = \"{fingerprint}\"\nreason = \"fixture token, never deployed\"\nexpires = 2999-12-31\n"),
    );

    let (_, delta, json) = p.review();
    assert!(delta.findings.iter().all(|e| e.fingerprint != fingerprint), "{:#?}", delta.findings);
    assert_eq!(delta.suppressed.len(), 1, "{:#?}", delta.suppressed);
    let suppressed = &delta.suppressed[0];
    assert_eq!(suppressed.entry.status, SecurityDeltaStatus::Introduced);
    assert_eq!(suppressed.reason, "fixture token, never deployed");
    assert_eq!(suppressed.expires.as_deref(), Some("2999-12-31"));
    assert_eq!((delta.counts.suppressed, delta.counts.introduced), (1, 0), "{:?}", delta.counts);
    assert_eq!(json.matches(PLANTED).count(), 0, "the planted secret leaked: {json}");
}

#[test]
fn a_project_never_scanned_carries_an_unmeasured_section() {
    let p = project();
    git(&p.root, &["checkout", "-q", "-b", "feature"]);
    let review = p.engine().review(Some("main")).expect("review runs");
    let delta = review.sections.security.expect("the section is present");
    assert!(!delta.measured);
    assert!(delta.findings.is_empty() && delta.suppressed.is_empty());
    assert!(
        delta.not_measured_because.as_deref().is_some_and(|why| why.contains("no security report")),
        "{delta:?}"
    );
}

#[test]
fn a_secret_in_an_untracked_file_is_introduced_via_hunk() {
    // Never committed: git's diff does not list it, the branch added it all the same.
    let p = project();
    git(&p.root, &["checkout", "-q", "-b", "feature"]);
    write(&p.root, "README.md", "notes\n");
    commit(&p.root, "notes");
    write(&p.root, "app/local.py", &format!("\n\nTOKEN = \"{PLANTED}\"\n"));
    p.ingest("gitleaks", &[("generic-api-key", "app/local.py", 3, "secret-2")]);
    let (_, delta, json) = p.review();
    assert_eq!(class(&delta, "generic-api-key"), (SecurityDeltaStatus::Introduced, SecurityDelta::METHOD_HUNK));
    assert!(!json.contains(PLANTED), "the planted secret leaked: {json}");
}

/// A `--path` re-scan at HEAD = merge-base — the Stop hook's, on a branch with
/// no commit yet — replaces the merge-base report with what the working tree
/// held then. That report is no before-state: a finding the branch then commits
/// is `introduced` via `hunk`, never `touched` against its own uncommitted copy.
#[test]
fn a_scoped_scan_at_the_merge_base_is_no_before_state() {
    let p = project();
    p.ingest("Semgrep OSS", SEMGREP_BASE);
    git(&p.root, &["checkout", "-q", "-b", "feature"]);
    // Uncommitted, at the merge-base commit: the scoped run sees the new code.
    write(&p.root, "app/db.py", &format!("{DB_PY}\n\ndef raw(conn, q):\n    return conn.execute(q)\n"));
    let added: Hit = ("python.raw-query", "app/db.py", 10, "raw-1");
    p.ingest_scoped("Semgrep OSS", &[SEMGREP_BASE[0], added], &["app/db.py".to_string()]);
    commit(&p.root, "a raw query helper");

    let (_, delta, _) = p.review();
    assert_eq!(class(&delta, "python.raw-query"), (SecurityDeltaStatus::Introduced, SecurityDelta::METHOD_HUNK));
    assert_eq!(class(&delta, "python.weak-hash"), (SecurityDeltaStatus::PreExisting, SecurityDelta::METHOD_HUNK));
    assert!(
        delta.notes.iter().any(|n| n.starts_with("Semgrep OSS: the merge-base report was rewritten")),
        "{:?}",
        delta.notes
    );
}
