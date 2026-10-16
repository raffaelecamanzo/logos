//! Tests for the security surfaces' composition (S-667, [FR-SC-06],
//! [FR-SC-07], [FR-SC-08]).
//!
//! No real scanner runs. Each scanner is a stub shell script in a directory that
//! is the whole scanner search path; it records its argv, answers its version
//! command, and either copies a SARIF report to the output its template names or
//! fails with a planted secret on stderr. The project is a git repository in a
//! temporary directory outside every work tree, and the user-level directory is
//! another temporary directory, so neither the process environment nor this
//! machine's own consent record is read.
//!
//! [FR-SC-06]: ../../../docs/specs/requirements/FR-SC-06.md
//! [FR-SC-07]: ../../../docs/specs/requirements/FR-SC-07.md
//! [FR-SC-08]: ../../../docs/specs/requirements/FR-SC-08.md

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::NaiveDate;
use serde_json::{json, Value};
use tempfile::TempDir;

use super::registry::REGISTRY;
use super::surface::{findings, scan, status, FindingsFilter};
use super::{ScanEnvironment, OUTPUT_DIR};
use crate::config::{ConfigError, SCAN_CONSENT_FILE};
use crate::history::security::owasp::SCANNER_IDS;
use crate::history::SecurityIngestSummary;

/// A secret a failing scanner prints to stderr. It must reach the failure log
/// and no payload.
const PLANTED: &str = "LOGOSPLANTEDSECRET0123456789abcdef";

const TODAY: Option<NaiveDate> = NaiveDate::from_ymd_opt(2026, 10, 16);

/// A Bandit report with no results: a clean scan.
const CLEAN_BANDIT: &str = r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Bandit","semanticVersion":"1.7.10","rules":[]}},"results":[]}]}"#;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sarif").join(format!("{name}.sarif"))
}

/// A project (a one-commit git repository), a stub search path, a record
/// directory and a user-level directory.
struct World {
    _tmp: TempDir,
    root: PathBuf,
    bin: PathBuf,
    rec: PathBuf,
    user: PathBuf,
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(root)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0", "-c", "user.email=t@logos", "-c", "user.name=t"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

impl World {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let [root, bin, rec, user] = ["project", "bin", "rec", "user"].map(|d| base.join(d));
        for dir in [&root, &bin, &rec, &user] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::create_dir_all(root.join("app")).unwrap();
        fs::write(root.join("app/db.py"), "import sqlite3\n").unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "seed"]);
        Self { _tmp: tmp, root, bin, rec, user }
    }

    fn env(&self) -> ScanEnvironment {
        ScanEnvironment {
            user_dir: Some(self.user.clone()),
            search_path: Some(self.bin.clone().into_os_string()),
        }
    }

    /// A stub for `name` that writes `report` to its output (when given),
    /// prints `stderr` and exits `exit`.
    fn stub(&self, name: &str, report: Option<&Path>, exit: i32, stderr: &str) {
        let record = self.rec.join(format!("{name}.argv"));
        let copy = report.map_or(String::new(), |r| {
            format!("if [ -n \"$out\" ]; then /bin/cp '{}' \"$out\"; fi", r.display())
        });
        let script = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> '{record}'\n\
             case \"$1\" in --version|version|-version) printf '%s\\n' '{name} 1.2.3'; exit 0;; esac\n\
             out=''; prev=''\n\
             for a in \"$@\"; do case \"$prev\" in --output|--report-path) out=\"$a\";; esac; prev=\"$a\"; done\n\
             {copy}\n\
             printf '%s\\n' '{stderr}' >&2\n\
             exit {exit}\n",
            record = record.display(),
        );
        let path = self.bin.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn clean_bandit(&self) -> PathBuf {
        let path = self.rec.join("clean-bandit.sarif");
        fs::write(&path, CLEAN_BANDIT).unwrap();
        path
    }

    fn repo_config(&self, text: &str) {
        fs::create_dir_all(self.root.join(".logos")).unwrap();
        fs::write(self.root.join(".logos/config.toml"), text).unwrap();
    }

    /// Every recorded invocation of every stub, version probes included.
    fn started(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for entry in fs::read_dir(&self.rec).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "argv") {
                lines.extend(fs::read_to_string(path).unwrap().lines().map(str::to_string));
            }
        }
        lines
    }

    fn ingest(&self) -> impl FnMut(&Path) -> anyhow::Result<SecurityIngestSummary> + '_ {
        let indexed = vec!["app/db.py".to_string()];
        move |report| crate::history::security::ingest(&self.root, report, &indexed)
    }

    fn scan(&self, consent: bool) -> anyhow::Result<super::SecurityScanSummary> {
        scan(&self.root, &self.env(), consent, self.ingest())
    }

    fn status(&self) -> Value {
        serde_json::to_value(status(&self.root, &self.env(), TODAY.unwrap()).unwrap()).unwrap()
    }

    fn findings(&self, filter: &FindingsFilter) -> Value {
        serde_json::to_value(findings(&self.root, &self.env(), filter, TODAY.unwrap()).unwrap()).unwrap()
    }
}

fn scanner_row<'a>(payload: &'a Value, scanner: &str) -> &'a Value {
    payload["scanners"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["scanner"] == scanner)
        .unwrap_or_else(|| panic!("{scanner} is listed: {payload}"))
}

fn consent_refusal(err: &anyhow::Error) -> String {
    let config = err.downcast_ref::<ConfigError>().expect("a consent refusal is a config error");
    assert_eq!(crate::error::exit_code(err), 2);
    config.to_string()
}

/// FR-SC-06 AC 1, with CR-217's no-secret rule: one scanner absent, one failing
/// with a secret on its stderr, two running. The absent one is `not_installed`,
/// the failing one `failed` with its exit code, a log holding the secret and a
/// masked excerpt that does not, and the others are ingested.
#[test]
fn an_absent_and_a_failing_scanner_are_reported_and_the_rest_ingest() {
    let w = World::new();
    w.stub("semgrep", Some(&fixture("semgrep")), 0, "");
    w.stub("bandit", Some(&w.clean_bandit()), 0, "");
    w.stub("gitleaks", None, 7, &format!("fatal: token={PLANTED} in .env"));
    // Trivy is installed nowhere: the search path is the stub directory alone.
    let summary = w.scan(true).expect("the scan runs");
    let payload = serde_json::to_value(&summary).unwrap();

    assert_eq!(scanner_row(&payload, "trivy")["outcome"], "not_installed");
    let failed = scanner_row(&payload, "gitleaks");
    assert_eq!(failed["outcome"], "failed", "{failed}");
    assert_eq!(failed["exit_code"], 7);
    assert_eq!(failed["log"], format!("{OUTPUT_DIR}/gitleaks.failed.log"));
    let excerpt = failed["stderr"].as_str().unwrap();
    assert!(excerpt.contains("fatal") && excerpt.contains(super::mask::MASK), "{excerpt}");
    let log = fs::read_to_string(w.root.join(OUTPUT_DIR).join("gitleaks.failed.log")).unwrap();
    assert!(log.contains(PLANTED), "the log keeps the whole output: {log}");
    assert!(!payload.to_string().contains(PLANTED), "the secret reached the payload: {payload}");

    let semgrep = scanner_row(&payload, "semgrep");
    assert_eq!((semgrep["outcome"].as_str(), semgrep["findings"].as_u64()), (Some("ran"), Some(2)));
    assert_eq!(semgrep["sarif"], json!([format!("{OUTPUT_DIR}/semgrep.sarif")]));
    assert_eq!(semgrep["ingests"][0]["bound"], 2, "bound against the indexed file: {semgrep}");
    let bandit = scanner_row(&payload, "bandit");
    assert_eq!((bandit["outcome"].as_str(), bandit["findings"].as_u64()), (Some("ran"), Some(0)));
    assert_eq!(payload["findings"], 2);
    // Every registered scanner has exactly one outcome, in registry order.
    let names: Vec<&str> = summary.scanners.iter().map(|s| s.scanner).collect();
    assert_eq!(names, REGISTRY.iter().map(|s| s.name).collect::<Vec<_>>());

    // The status marks the failure, and a clean run of that scanner clears it.
    assert_eq!(scanner_row(&w.status(), "gitleaks")["last_failure"], format!("{OUTPUT_DIR}/gitleaks.failed.log"));
    w.stub("gitleaks", Some(&w.clean_bandit()), 0, "");
    w.scan(false).expect("consent is recorded");
    assert_eq!(scanner_row(&w.status(), "gitleaks")["last_failure"], Value::Null);
}

/// FR-SC-06 "one failure never stops the others", for Logos's own I/O too: a
/// failure log that cannot be written or cleared (here a directory a repository
/// committed in its place) degrades to a missing log, and every other scanner
/// still runs.
#[test]
fn a_failure_log_that_cannot_be_written_or_cleared_stops_no_scanner() {
    let w = World::new();
    w.stub("semgrep", Some(&fixture("semgrep")), 0, "");
    w.stub("gitleaks", None, 7, "fatal: no git");
    w.stub("bandit", Some(&w.clean_bandit()), 0, "");
    for blocked in ["semgrep", "gitleaks"] {
        fs::create_dir_all(w.root.join(OUTPUT_DIR).join(format!("{blocked}.failed.log")).join("x")).unwrap();
    }
    let payload = serde_json::to_value(w.scan(true).expect("the scan completes")).unwrap();
    assert_eq!(scanner_row(&payload, "semgrep")["outcome"], "ran", "{payload}");
    let gitleaks = scanner_row(&payload, "gitleaks");
    assert_eq!((gitleaks["outcome"].as_str(), gitleaks["log"].clone()), (Some("failed"), Value::Null), "{gitleaks}");
    assert!(gitleaks["stderr"].as_str().unwrap().contains("fatal"), "{gitleaks}");
    assert_eq!(scanner_row(&payload, "bandit")["outcome"], "ran", "a later scanner still runs: {payload}");
}

/// A scanner that exits cleanly but writes a report that does not parse is a
/// failure too, and its earlier snapshot is kept.
#[test]
fn an_unparseable_report_fails_its_scanner_and_keeps_the_previous_snapshot() {
    let w = World::new();
    w.stub("semgrep", Some(&fixture("semgrep")), 0, "");
    w.scan(true).unwrap();
    let broken = w.rec.join("broken.sarif");
    fs::write(&broken, "{\"version\":\"2.1.0\"").unwrap();
    w.stub("semgrep", Some(&broken), 0, "");
    let payload = serde_json::to_value(w.scan(false).unwrap()).unwrap();
    let semgrep = scanner_row(&payload, "semgrep");
    assert_eq!((semgrep["outcome"].as_str(), semgrep["exit_code"].clone()), (Some("failed"), Value::Null), "{semgrep}");
    let status = w.status();
    let row = scanner_row(&status, "semgrep");
    assert_eq!((row["state"].as_str(), row["findings"].as_u64()), (Some("fresh"), Some(2)), "{row}");
    assert!(row["last_failure"].is_string());
}

/// A CodeQL scan of a project in two languages stores both languages'
/// findings: a snapshot is keyed by HEAD and scanner, so the languages must
/// reach the store as one report, never as reports that replace each other.
/// The suppression of a finding in the first language analysed stays matched.
#[test]
fn every_codeql_language_lands_in_one_snapshot() {
    let w = World::new();
    fs::create_dir_all(w.root.join("web")).unwrap();
    fs::write(w.root.join("web/app.js"), "export {}\n").unwrap();
    // The analysis writes one result per language, its rule named after the
    // database directory, which is the language.
    let script = r#"#!/bin/sh
case "$1" in version) printf '2.19.3\n'; exit 0;; esac
if [ "$2" = analyze ]; then
  lang="${3##*/}"; out=''; prev=''
  for a in "$@"; do [ "$prev" = --output ] && out="$a"; prev="$a"; done
  printf '{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"CodeQL","semanticVersion":"2.19.3","rules":[]}},"results":[{"ruleId":"q/%s","level":"warning","message":{"text":"m"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"app/db.py"},"region":{"startLine":1}}}]}]}]}' "$lang" > "$out"
fi
exit 0
"#;
    let stub = w.bin.join("codeql");
    fs::write(&stub, script).unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();

    let payload = serde_json::to_value(w.scan(true).unwrap()).unwrap();
    let codeql = scanner_row(&payload, "codeql");
    assert_eq!((codeql["outcome"].as_str(), codeql["findings"].as_u64()), (Some("ran"), Some(2)), "{codeql}");
    let listed = w.findings(&FindingsFilter::default());
    let mut rules: Vec<&str> = listed["findings"].as_array().unwrap().iter().map(|f| f["rule_id"].as_str().unwrap()).collect();
    rules.sort_unstable();
    assert_eq!(rules, ["q/javascript-typescript", "q/python"], "both languages are stored: {listed}");
    assert_eq!(scanner_row(&w.status(), "codeql")["findings"], 2);
}

/// FR-SC-11: an expired entry no longer suppresses, on every surface — the
/// finding is listed as active (labelled expired) and counted as unsuppressed
/// by `status`, while a current entry still suppresses.
#[test]
fn an_expired_suppression_counts_as_active_on_status_and_findings() {
    let w = World::new();
    crate::history::security::ingest(&w.root, &fixture("semgrep"), &["app/db.py".to_string()]).unwrap();
    let all = w.findings(&FindingsFilter::default());
    let fp = |i: usize| all["findings"][i]["fingerprint"].as_str().unwrap().to_string();
    let (current, expired) = (fp(0), fp(1));
    fs::write(
        w.root.join(".logos/security-ignore.toml"),
        format!(
            "[[ignore]]\nfingerprint = \"{current}\"\nreason = \"accepted\"\n\n\
             [[ignore]]\nfingerprint = \"{expired}\"\nreason = \"was accepted\"\nexpires = 2000-01-01\n"
        ),
    )
    .unwrap();
    let only = |token: &str| {
        let filter = FindingsFilter::parse(None, None, None, None, Some(token), None).unwrap();
        w.findings(&filter)["findings"].as_array().unwrap().clone()
    };
    let active = only("active");
    assert_eq!(active.len(), 1, "{active:?}");
    assert_eq!((active[0]["fingerprint"].as_str(), active[0]["suppression"]["state"].as_str()), (Some(expired.as_str()), Some("expired")));
    let suppressed = only("suppressed");
    assert_eq!(suppressed.iter().map(|f| f["fingerprint"].as_str().unwrap()).collect::<Vec<_>>(), [current.as_str()]);
    let row = scanner_row(&w.status(), "semgrep").clone();
    assert_eq!((row["findings"].as_u64(), row["suppressed"].as_u64()), (Some(1), Some(1)), "{row}");
}

/// FR-SC-07: refuse without consent (exit 2, naming the flag and the scanners,
/// nothing started); consent records and runs; later scans do not ask; a newly
/// enabled scanner is asked for alone; nothing is written in the repository.
#[test]
fn consent_is_asked_once_per_scanner_and_recorded_outside_the_repository() {
    let w = World::new();
    w.repo_config("[security_scan.trivy]\nenabled = false\n");
    for name in ["semgrep", "bandit", "trivy"] {
        w.stub(name, Some(&w.clean_bandit()), 0, "");
    }

    let refused = w.scan(false).expect_err("the first scan needs consent");
    let message = consent_refusal(&refused);
    assert!(message.contains("--consent") && message.contains("semgrep, bandit"), "{message}");
    assert!(w.started().is_empty(), "nothing starts before consent: {:?}", w.started());
    assert_eq!(w.status()["consent"], json!({"consented": [], "pending": ["semgrep", "bandit"]}));

    let first = w.scan(true).expect("consent given");
    assert_eq!(first.consent_recorded, ["semgrep", "bandit"]);
    let again = w.scan(false).expect("consent is not asked twice");
    assert!(again.consent_recorded.is_empty());

    w.repo_config("[security_scan.trivy]\nenabled = true\n");
    let message = consent_refusal(&w.scan(false).expect_err("a newly enabled scanner is asked for"));
    assert!(message.contains("for trivy —"), "only trivy is asked for: {message}");
    assert!(!message.contains("semgrep"), "{message}");
    assert_eq!(w.status()["consent"]["pending"], json!(["trivy"]));
    assert_eq!(w.scan(true).unwrap().consent_recorded, ["trivy"]);
    assert_eq!(w.status()["consent"], json!({"consented": ["bandit", "semgrep", "trivy"], "pending": []}));

    // The record is user-level state; the repository holds config and output only.
    assert!(w.user.join(SCAN_CONSENT_FILE).is_file());
    let mut in_repo: Vec<String> = fs::read_dir(w.root.join(".logos"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with("history.db"))
        .collect();
    in_repo.sort();
    assert_eq!(in_repo, ["config.toml", "security"]);
}

/// FR-SC-08 AC 1: `never_run` and `fresh` with zero findings are different
/// states, on the status and on the findings list, and not installed and
/// disabled are distinct from both.
#[test]
fn never_run_and_fresh_with_zero_findings_are_distinct() {
    let w = World::new();
    w.repo_config("[security_scan.checkov]\nenabled = false\n");
    w.stub("semgrep", None, 0, "");
    w.stub("bandit", None, 0, "");

    // Nothing stored at all: the findings list says so, rather than reading clean.
    let empty = w.findings(&FindingsFilter::default());
    assert_eq!((empty["total"].as_u64(), empty["scanned"].as_array().map(Vec::len)), (Some(0), Some(0)));
    assert!(empty["notice"].is_string(), "{empty}");
    // `never_run` means what `status` means by it: installed, enabled, nothing
    // stored — never a disabled (checkov) or absent (trivy) scanner.
    assert_eq!(empty["never_run"], json!(["semgrep", "bandit"]));

    crate::history::security::ingest(&w.root, &w.clean_bandit(), &[]).unwrap();
    let status = w.status();
    assert_eq!(scanner_row(&status, "semgrep")["state"], "never_run");
    assert!(scanner_row(&status, "semgrep").get("findings").is_none(), "never run carries no count");
    let bandit = scanner_row(&status, "bandit");
    assert_eq!((bandit["state"].as_str(), bandit["findings"].as_u64()), (Some("fresh"), Some(0)));
    assert_eq!(scanner_row(&status, "trivy")["state"], "not_installed");
    assert_eq!(scanner_row(&status, "checkov")["state"], "disabled");

    let clean = w.findings(&FindingsFilter::default());
    assert_eq!(clean["notice"], Value::Null);
    assert_eq!(clean["scanned"][0]["driver"], "Bandit");
    assert_eq!(clean["scanned"][0]["findings"], 0);
    assert_eq!(clean["never_run"], json!(["semgrep"]));
}

/// UAT-SC-03 for the OWASP view: a scanner installed but never run has looked
/// at nothing, so no category reads covered — a count of 0 would say it looked
/// and found nothing. Its categories become covered once its report is stored.
#[test]
fn an_installed_scanner_that_never_ran_covers_no_category() {
    let w = World::new();
    w.stub("semgrep", Some(&fixture("semgrep")), 0, "");
    let covered = |payload: &Value| -> usize {
        payload["categories"].as_array().unwrap().iter().filter(|c| c["state"]["state"] == "covered").count()
    };
    let before = w.findings(&FindingsFilter::default());
    assert_eq!(covered(&before), 0, "nothing ran, so nothing is covered: {}", before["categories"]);
    w.scan(true).unwrap();
    assert!(covered(&w.findings(&FindingsFilter::default())) > 0, "semgrep's report covers its categories");
}

/// The status states the S-667 review's mutations found unasserted: a changed
/// file is stale; an unregistered tool's report is listed apart; disabled
/// outranks a stored report; suppressed findings are counted apart; a symlink
/// in place of a failure log is not reported; and neither read starts a scanner.
#[test]
fn status_states_each_case_and_neither_read_starts_a_scanner() {
    let w = World::new();
    w.stub("semgrep", Some(&fixture("semgrep")), 0, "");
    w.repo_config("[security_scan.bandit]\nenabled = false\n");
    let indexed = vec!["app/db.py".to_string()];
    crate::history::security::ingest(&w.root, &fixture("semgrep"), &indexed).unwrap();
    crate::history::security::ingest(&w.root, &w.clean_bandit(), &[]).unwrap();
    let other = w.rec.join("other.sarif");
    fs::write(&other, CLEAN_BANDIT.replace("\"Bandit\"", "\"ESLint\"")).unwrap();
    crate::history::security::ingest(&w.root, &other, &[]).unwrap();

    let semgrep_findings = w.findings(&FindingsFilter::default());
    let first = semgrep_findings["findings"][0]["fingerprint"].as_str().unwrap().to_string();
    fs::write(
        w.root.join(".logos/security-ignore.toml"),
        format!("[[ignore]]\nfingerprint = \"{first}\"\nreason = \"accepted\"\n"),
    )
    .unwrap();
    fs::create_dir_all(w.root.join(OUTPUT_DIR)).unwrap();
    std::os::unix::fs::symlink(w.rec.join("elsewhere.log"), w.root.join(OUTPUT_DIR).join("semgrep.failed.log")).unwrap();
    fs::write(w.rec.join("elsewhere.log"), "a log outside the project").unwrap();

    let status = w.status();
    let semgrep = scanner_row(&status, "semgrep");
    assert_eq!((semgrep["state"].as_str(), semgrep["findings"].as_u64(), semgrep["suppressed"].as_u64()), (Some("fresh"), Some(1), Some(1)), "{semgrep}");
    assert_eq!(semgrep["last_failure"], Value::Null, "a symlinked log is not reported");
    assert_eq!(scanner_row(&status, "bandit")["state"], "disabled", "disabled outranks a stored report");
    assert_eq!(status["other_reports"], json!([{"scanner": "ESLint", "driver": "ESLint", "state": "fresh", "snapshot_head": semgrep["snapshot_head"], "version": "1.7.10", "findings": 0, "suppressed": 0, "last_failure": null}]));
    assert!(w.started().is_empty(), "status and findings start no scanner: {:?}", w.started());

    fs::write(w.root.join("app/db.py"), "import sqlite3  # edited\n").unwrap();
    let semgrep = w.status();
    let semgrep = scanner_row(&semgrep, "semgrep");
    assert_eq!((semgrep["state"].as_str(), semgrep["because"].as_str()), (Some("stale"), Some("files_changed")), "{semgrep}");
}

/// A report stored at an older HEAD reads stale, and says why.
#[test]
fn a_report_from_an_older_head_is_stale() {
    let w = World::new();
    crate::history::security::ingest(&w.root, &w.clean_bandit(), &[]).unwrap();
    fs::write(w.root.join("app/more.py"), "x = 1\n").unwrap();
    git(&w.root, &["add", "."]);
    git(&w.root, &["commit", "-q", "-m", "more"]);
    let bandit = w.status();
    let bandit = scanner_row(&bandit, "bandit");
    assert_eq!((bandit["state"].as_str(), bandit["because"].as_str()), (Some("stale"), Some("head_moved")));
}

/// S-664 hand-off (a): the ids `findings` passes to `classify` are the
/// registry's, and they spell exactly the table's.
#[test]
fn the_registry_ids_are_the_owasp_table_s_scanner_ids() {
    let registry: Vec<&str> = REGISTRY.iter().map(|s| s.name).collect();
    assert_eq!(registry, SCANNER_IDS);
}

/// FR-SC-08 AC 3: each filter narrows deterministically and combined filters
/// intersect; suppression is labelled, and a category no scanner here can see
/// is `not_covered` while its findings are still listed (S-664 hand-off (b)).
#[test]
fn findings_are_classified_labelled_and_the_filters_intersect() {
    let w = World::new();
    let indexed = vec!["app/db.py".to_string()];
    for name in ["semgrep", "bandit", "gitleaks"] {
        crate::history::security::ingest(&w.root, &fixture(name), &indexed).unwrap();
    }
    let all = w.findings(&FindingsFilter::default());
    let total = all["total"].as_u64().unwrap() as usize;
    assert_eq!(all["findings"].as_array().unwrap().len(), total);

    // Suppress one finding; the list still shows it, labelled.
    let target = all["findings"][0]["fingerprint"].as_str().unwrap().to_string();
    fs::write(
        w.root.join(".logos/security-ignore.toml"),
        format!("[[ignore]]\nfingerprint = \"{target}\"\nreason = \"accepted\"\n"),
    )
    .unwrap();
    let parse = |args: [Option<&str>; 6]| FindingsFilter::parse(args[0], args[1], args[2], args[3], args[4], args[5]).unwrap();
    let count = |filter: &FindingsFilter| w.findings(filter)["findings"].as_array().unwrap().len();
    let fingerprints = |filter: &FindingsFilter| -> Vec<String> {
        w.findings(filter)["findings"].as_array().unwrap().iter().map(|f| f["fingerprint"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(fingerprints(&parse([None, None, None, None, Some("suppressed"), None])), std::slice::from_ref(&target));
    assert_eq!(count(&parse([None, None, None, None, Some("active"), None])), total - 1);

    let semgrep = parse([None, Some("semgrep"), None, None, None, None]);
    let by_driver = parse([None, Some("Semgrep OSS"), None, None, None, None]);
    assert_eq!(fingerprints(&semgrep), fingerprints(&by_driver));
    assert_eq!(count(&semgrep), 2);
    for spelling in ["SEMGREP", "semgrep oss"] {
        assert_eq!(fingerprints(&parse([None, Some(spelling), None, None, None, None])), fingerprints(&semgrep), "{spelling}");
    }
    // Each filter keeps exactly what an oracle over the unfiltered payload keeps.
    let oracle = |keep: &dyn Fn(&Value) -> bool| -> Vec<String> {
        all["findings"].as_array().unwrap().iter().filter(|f| keep(f)).map(|f| f["fingerprint"].as_str().unwrap().to_string()).collect()
    };
    let high = parse([Some("high"), None, None, None, None, None]);
    let high_or_worse = oracle(&|f| ["high", "critical"].contains(&f["severity"].as_str().unwrap()));
    assert!(!high_or_worse.is_empty() && high_or_worse.len() < total);
    assert_eq!(fingerprints(&high), high_or_worse);
    assert_eq!(fingerprints(&parse([Some("HIGH"), None, None, None, None, None])), high_or_worse, "tokens ignore case");
    let injection = oracle(&|f| f["owasp"].as_array().unwrap().iter().any(|c| c == "A05:2025"));
    assert!(!injection.is_empty() && injection.len() < total);
    for spelling in ["A05", "a05", "A05:2025", "a05:2025"] {
        assert_eq!(fingerprints(&parse([None, None, Some(spelling), None, None, None])), injection, "{spelling}");
    }
    let under_app = oracle(&|f| {
        let p = f["path"].as_str().or(f["report_path"].as_str()).unwrap_or("");
        p.starts_with("app/")
    });
    assert!(!under_app.is_empty(), "{under_app:?}");
    for spelling in ["app", "app/", "./app"] {
        assert_eq!(fingerprints(&parse([None, None, None, Some(spelling), None, None])), under_app, "{spelling}");
    }
    let db = oracle(&|f| f["path"] == "app/db.py" || f["report_path"] == "app/db.py");
    assert!(!db.is_empty() && db.len() < total, "{db:?}");
    assert_eq!(fingerprints(&parse([None, None, None, Some("app/db.py"), None, None])), db, "a file names itself");
    // An unmatched finding (gitleaks' app/settings.py is not indexed here) is
    // found by the path its report gave.
    let settings = parse([None, None, None, Some("app/settings.py"), None, None]);
    let in_settings = oracle(&|f| f["path"].is_null() && f["report_path"] == "app/settings.py");
    assert!(!in_settings.is_empty());
    assert_eq!(fingerprints(&settings), in_settings);
    // The edition reclassifies the same stored findings: CWE-89 is A05:2025 and A03:2021.
    let edition_2021 = w.findings(&parse([None, Some("semgrep"), None, None, None, Some("2021")]));
    assert_eq!(edition_2021["edition"], "2021");
    for f in edition_2021["findings"].as_array().unwrap() {
        assert_eq!(f["owasp"], json!(["A03:2021"]), "{f}");
    }
    assert!(edition_2021["categories"].as_array().unwrap().iter().all(|c| c["id"].as_str().unwrap().ends_with(":2021")));
    for f in w.findings(&semgrep)["findings"].as_array().unwrap() {
        assert_eq!(f["owasp"], json!(["A05:2025"]), "{f}");
    }
    // Combined filters intersect: the result is the intersection of the parts.
    let both = parse([Some("high"), Some("semgrep"), None, None, None, None]);
    let expected: Vec<String> = fingerprints(&semgrep).into_iter().filter(|f| fingerprints(&high).contains(f)).collect();
    assert_eq!(fingerprints(&both), expected);
    assert_eq!(fingerprints(&both), fingerprints(&both), "deterministic");
    assert_eq!(count(&parse([None, None, None, Some("app/db"), None, None])), 0, "a prefix of a name is not a directory");

    // No scanner is installed here and only three reports are stored, so a
    // category only an absent scanner covers reads not covered — never 0.
    let categories = all["categories"].as_array().unwrap();
    assert!(categories.iter().any(|c| c["state"]["state"] == "not_covered" || c["state"] == "not_covered"), "{categories:?}");
    for c in categories {
        if c.to_string().contains("not_covered") {
            assert!(c["state"].get("findings").is_none(), "not covered never carries a count: {c}");
        }
    }
    let bad = FindingsFilter::parse(Some("severe"), None, None, None, None, None).unwrap_err();
    assert!(bad.to_string().contains("severe"), "{bad}");
}
