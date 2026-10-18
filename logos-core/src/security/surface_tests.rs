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

use super::context::{FindingContext, UnboundBecause};
use super::registry::REGISTRY;
use super::surface::{consent_pending, findings, scan, status, FindingsFilter, ScanProgress};
use super::{ScanEnvironment, ScanRequest, OUTPUT_DIR};
use crate::config::{ConfigError, SCAN_CONSENT_FILE};
use crate::history::security::owasp::SCANNER_IDS;
use crate::history::{SecurityFinding, SecurityIngestSummary};

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
            tools_dir: None,
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

    fn ingest(&self) -> impl FnMut(&Path, Option<crate::history::security::IngestScope<'_>>) -> anyhow::Result<SecurityIngestSummary> + '_ {
        let indexed = vec!["app/db.py".to_string()];
        move |report, scope| crate::history::security::ingest_scoped(&self.root, report, &indexed, scope)
    }

    fn scan(&self, consent: bool) -> anyhow::Result<super::SecurityScanSummary> {
        self.scan_reporting(consent, &mut |_| {})
    }

    fn scan_reporting(
        &self,
        consent: bool,
        progress: &mut dyn FnMut(&ScanProgress),
    ) -> anyhow::Result<super::SecurityScanSummary> {
        self.scan_request(consent, &ScanRequest::full(), progress)
    }

    fn scan_request(
        &self,
        consent: bool,
        request: &ScanRequest,
        progress: &mut dyn FnMut(&ScanProgress),
    ) -> anyhow::Result<super::SecurityScanSummary> {
        scan(&self.root, &self.env(), crate::tools::Manifest::builtin(), consent, request, progress, self.ingest())
    }

    fn status(&self) -> Value {
        serde_json::to_value(status(&self.root, &self.env(), crate::tools::Manifest::builtin(), TODAY.unwrap(), super::VersionProbes::Every).unwrap()).unwrap()
    }

    /// The listing with no graph behind it: every finding unbound. The graph
    /// context itself is `tests/security_context.rs`'s.
    fn findings(&self, filter: &FindingsFilter) -> Value {
        let no_graph = |listed: &[&SecurityFinding]| {
            Ok(listed.iter().map(|f| FindingContext::unbound(f.start_line, UnboundBecause::NoSymbol)).collect())
        };
        serde_json::to_value(findings(&self.root, &self.env(), crate::tools::Manifest::builtin(), filter, TODAY.unwrap(), no_graph).unwrap()).unwrap()
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
/// in place of a failure log is not reported; and neither read starts a scan —
/// status runs only the installed scanner's version probe (S-744).
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
    assert_eq!(status["other_reports"], json!([{"scanner": "ESLint", "driver": "ESLint", "state": "fresh", "snapshot_head": semgrep["snapshot_head"], "version": "1.7.10", "findings": 0, "suppressed": 0, "installed": null, "install": null, "last_failure": null}]));
    // Status reads each installed copy's version (S-744, FR-SC-08): the
    // version probe is the only process it starts, and findings starts none.
    assert_eq!(w.started(), ["--version"], "status starts no scan, only semgrep's version probe");
    fs::remove_file(w.rec.join("semgrep.argv")).unwrap();
    w.findings(&FindingsFilter::default());
    assert!(w.started().is_empty(), "findings starts no scanner: {:?}", w.started());

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

/// S-673: each installed scanner's start, then every registered scanner's
/// settled outcome, in registry order and as the summary lists it.
#[test]
fn a_scan_reports_each_scanner_as_it_happens() {
    let w = World::new();
    w.stub("semgrep", Some(&fixture("semgrep")), 0, "");
    w.stub("bandit", Some(&w.clean_bandit()), 0, "");
    let mut heard = Vec::new();
    let summary = w.scan_reporting(true, &mut |step| heard.push(step.clone())).unwrap();

    let started: Vec<&str> = heard
        .iter()
        .filter_map(|step| match step {
            ScanProgress::Started { scanner } => Some(*scanner),
            ScanProgress::Finished(_) => None,
        })
        .collect();
    assert_eq!(started, ["semgrep", "bandit"], "only what is installed starts");
    let finished: Vec<_> = heard
        .iter()
        .filter_map(|step| match step {
            ScanProgress::Finished(run) => Some(run.clone()),
            ScanProgress::Started { .. } => None,
        })
        .collect();
    assert_eq!(finished, summary.scanners, "every outcome, as the summary lists it");
    // Each start comes right before its own outcome.
    let semgrep_start = heard.iter().position(|s| *s == ScanProgress::Started { scanner: "semgrep" }).unwrap();
    assert!(matches!(&heard[semgrep_start + 1], ScanProgress::Finished(run) if run.scanner == "semgrep"));

    let wire = serde_json::to_value(&heard[semgrep_start + 1]).unwrap();
    assert_eq!((wire["phase"].as_str(), wire["scanner"].as_str(), wire["outcome"].as_str()), (Some("finished"), Some("semgrep"), Some("ran")), "{wire}");
    assert_eq!(serde_json::to_value(&heard[semgrep_start]).unwrap(), json!({ "phase": "started", "scanner": "semgrep" }));
}

/// S-673: what a scan without consent would refuse over, asked without
/// starting a process — the stubs record every invocation, and none is made.
#[test]
fn consent_pending_names_the_unconsented_scanners_and_starts_nothing() {
    let w = World::new();
    w.stub("semgrep", None, 0, "");
    w.stub("bandit", None, 0, "");
    let pending = consent_pending(&w.root, &w.env(), crate::tools::Manifest::builtin()).unwrap();
    assert_eq!(pending, ["semgrep", "bandit"]);
    assert!(w.started().is_empty(), "a process started: {:?}", w.started());
    crate::config::record_scan_consent(Some(&w.user), &w.root, &["semgrep"]).unwrap();
    let pending = consent_pending(&w.root, &w.env(), crate::tools::Manifest::builtin()).unwrap();
    assert_eq!(pending, ["bandit"], "only the scanner the record lacks");
}

/// S-673: a scan on a job thread stops at the job's cancel. Semgrep — first in
/// the registry — is killed with the grandchild it left, no failure log is
/// written for it, Bandit after it never starts (not even its version probe),
/// the earlier snapshot is kept, and the job ends cancelled.
#[test]
fn a_cancelled_scan_kills_its_scanner_writes_no_failure_log_and_starts_nothing_after() {
    use crate::jobs::{JobKind, JobState, ScanJobs};
    use std::time::{Duration, Instant};

    let w = World::new();
    w.stub("semgrep", Some(&fixture("semgrep")), 0, "");
    w.scan(true).expect("a first scan stores a snapshot");
    let pids = w.rec.join("semgrep.pids");
    let sleeper = format!(
        "#!/bin/sh\ncase \"$1\" in --version) printf '%s\\n' 'semgrep 1.2.3'; exit 0;; esac\n\
         /bin/sleep 60 & echo $$ $! > '{}'\nwait\n",
        pids.display()
    );
    fs::write(w.bin.join("semgrep"), sleeper).unwrap();
    w.stub("bandit", Some(&w.clean_bandit()), 0, "");
    crate::config::record_scan_consent(Some(&w.user), &w.root, &["bandit"]).unwrap();
    fs::remove_dir_all(&w.rec).unwrap();
    fs::create_dir_all(&w.rec).unwrap();

    let jobs = ScanJobs::new();
    let (root, env) = (w.root.clone(), w.env());
    jobs.start(JobKind::SecurityScan, &w.root, move |_| {
        let indexed = vec!["app/db.py".to_string()];
        let summary = scan(&root, &env, crate::tools::Manifest::builtin(), false, &ScanRequest::full(), &mut |_| {}, |report, _| {
            crate::history::security::ingest(&root, report, &indexed)
        })?;
        Ok(serde_json::to_value(summary)?)
    })
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    while !fs::read_to_string(&pids).is_ok_and(|s| s.ends_with('\n')) {
        assert!(Instant::now() < deadline, "the sleeping scanner never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    let recorded: Vec<i32> = fs::read_to_string(&pids).unwrap().split_whitespace().map(|p| p.parse().unwrap()).collect();
    jobs.cancel(JobKind::SecurityScan, &w.root);
    let job = loop {
        let job = jobs.snapshot(JobKind::SecurityScan, &w.root).unwrap();
        if job.state != JobState::Running {
            break job;
        }
        assert!(Instant::now() < deadline, "the cancelled scan is still running");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(job.state, JobState::Cancelled, "{job:?}");
    for pid in recorded {
        let gone_by = Instant::now() + Duration::from_secs(10);
        // SAFETY: signal 0 only checks that the process exists.
        #[allow(unsafe_code)]
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(Instant::now() < gone_by, "scanner process {pid} survived the cancel");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    assert!(!w.root.join(OUTPUT_DIR).join("semgrep.failed.log").exists(), "a cancel is no scanner failure: {:?} {job:?}", fs::read_to_string(w.root.join(OUTPUT_DIR).join("semgrep.failed.log")));
    assert!(w.started().is_empty(), "bandit started after the cancel: {:?}", w.started());
    let status = w.status();
    let row = scanner_row(&status, "semgrep");
    assert_eq!((row["state"].as_str(), row["findings"].as_u64()), (Some("fresh"), Some(2)), "the snapshot is kept: {row}");
}

/// S-673: a cancel that lands between two scanners — here while the first one's
/// outcome is being reported — stops the scan before the next one: it is never
/// reported started, and no process of it runs.
#[test]
fn a_cancel_between_scanners_reports_no_later_scanner_started() {
    use crate::jobs::{JobKind, JobState, ScanJobs};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    let w = World::new();
    w.stub("semgrep", Some(&fixture("semgrep")), 0, "");
    w.stub("bandit", Some(&w.clean_bandit()), 0, "");
    crate::config::record_scan_consent(Some(&w.user), &w.root, &["semgrep", "bandit"]).unwrap();

    let jobs = Arc::new(ScanJobs::new());
    let heard = Arc::new(Mutex::new(Vec::new()));
    let (root, env, member) = (w.root.clone(), w.env(), w.root.clone());
    let (canceller, log) = (Arc::clone(&jobs), Arc::clone(&heard));
    jobs.start(JobKind::SecurityScan, &w.root, move |_| {
        let indexed = vec!["app/db.py".to_string()];
        let mut progress = |step: &ScanProgress| {
            log.lock().unwrap().push(step.clone());
            if matches!(step, ScanProgress::Finished(run) if run.scanner == "semgrep") {
                canceller.cancel(JobKind::SecurityScan, &member);
            }
        };
        scan(&root, &env, crate::tools::Manifest::builtin(), false, &ScanRequest::full(), &mut progress, |report, _| {
            crate::history::security::ingest(&root, report, &indexed)
        })?;
        Ok(serde_json::Value::Null)
    })
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    let job = loop {
        let job = jobs.snapshot(JobKind::SecurityScan, &w.root).unwrap();
        if job.state != JobState::Running {
            break job;
        }
        assert!(Instant::now() < deadline, "the scan is still running");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(job.state, JobState::Cancelled, "{job:?}");
    let heard = heard.lock().unwrap();
    assert!(!heard.contains(&ScanProgress::Started { scanner: "bandit" }), "bandit was reported started: {heard:?}");
    assert!(!w.rec.join("bandit.argv").exists(), "a bandit process started: {:?}", w.started());
}

// ── The workspace roll-up of one member (S-674, FR-SC-16) ─────────────────────

impl World {
    /// This project's [`MemberSecurity`](super::MemberSecurity): its status and
    /// its findings listed under each edition, as the workspace roll-up reads them.
    fn rollup(&self) -> super::MemberSecurity {
        let manifest = crate::tools::Manifest::builtin();
        let status = status(&self.root, &self.env(), manifest, TODAY.unwrap(), super::VersionProbes::Every).unwrap();
        let listings = super::listings(&self.root, &self.env(), manifest, TODAY.unwrap()).unwrap();
        super::MemberSecurity::from_reads(status, &listings)
    }
}

/// Reports stored but none fresh: the member is `stale`, it counts nothing and
/// no category is covered — an old scan's findings are neither a count nor a
/// zero, and every one of them is excluded as not fresh.
#[test]
fn a_member_whose_reports_are_all_stale_counts_nothing_and_covers_nothing() {
    let w = World::new();
    crate::history::security::ingest(&w.root, &fixture("semgrep"), &["app/db.py".to_string()]).unwrap();
    fs::write(w.root.join("app/more.py"), "x = 1\n").unwrap();
    git(&w.root, &["add", "."]);
    git(&w.root, &["commit", "-q", "-m", "more"]);

    let rollup = w.rollup();
    assert_eq!(rollup.scan, super::MemberScan::Stale);
    assert_eq!(rollup.findings, None, "a stale member has no count, not 0");
    assert_eq!((rollup.scanners_fresh, rollup.excluded.not_fresh, rollup.excluded.suppressed), (0, 2, 0));
    assert_eq!(rollup.owasp.len(), 2, "both editions");
    for edition in &rollup.owasp {
        assert!(edition.categories.iter().all(|c| c.findings.is_none()), "{edition:?}");
    }
}

/// A fresh report counts; a stored report from a scanner the config now
/// disables is excluded as not fresh, and the disabled scanner leaves the
/// denominator. A category Semgrep covers reads its count; one only Bandit
/// could see is not covered.
#[test]
fn a_disabled_scanner_s_stored_report_is_not_counted() {
    let w = World::new();
    let indexed = ["app/db.py".to_string()];
    crate::history::security::ingest(&w.root, &fixture("semgrep"), &indexed).unwrap();
    crate::history::security::ingest(&w.root, &fixture("bandit"), &indexed).unwrap();
    w.repo_config("[security_scan.bandit]\nenabled = false\n");

    let rollup = w.rollup();
    assert_eq!(rollup.scan, super::MemberScan::Fresh);
    let counts = rollup.findings.expect("a fresh member has a count");
    assert_eq!(counts.total, 2, "Semgrep's two findings: {counts:?}");
    assert_eq!(counts.total, counts.critical + counts.high + counts.medium + counts.low + counts.info);
    assert_eq!((rollup.excluded.not_fresh, rollup.excluded.suppressed), (2, 0), "Bandit's two");
    assert_eq!(rollup.scanners_fresh, 1);
    assert_eq!(rollup.scanners_enabled, REGISTRY.len() - 1, "every registered scanner but Bandit");
    let injection = rollup.owasp[0].categories.iter().find(|c| c.id == "A05:2025").unwrap();
    assert_eq!(injection.findings, Some(2), "{injection:?}");
    assert_eq!(rollup.owasp[0].unclassified, 0, "both sit under a category");
}

/// A counted finding with no CWE sits under no category: each edition states it
/// apart, as the member's own page does, so the category counts never read as
/// every finding there is.
#[test]
fn a_counted_finding_under_no_category_is_stated_apart() {
    let w = World::new();
    let report = w.rec.join("plain.sarif");
    fs::write(&report, semgrep_report(&[("app/db.py", "r.plain", 1)])).unwrap();
    crate::history::security::ingest(&w.root, &report, &["app/db.py".to_string()]).unwrap();

    let rollup = w.rollup();
    assert_eq!(rollup.findings.map(|c| c.total), Some(1));
    assert_eq!(rollup.owasp.iter().map(|e| e.unclassified).collect::<Vec<_>>(), [1, 1], "in both editions");
    assert!(rollup.owasp.iter().all(|e| e.categories.iter().all(|c| c.findings.unwrap_or(0) == 0)));
}

/// A stored report with no findings is still a scan: once stale it reads
/// `stale`, never `never_run` — a clean report is not an absent one ([BR-66]).
///
/// [BR-66]: ../../../docs/specs/software-spec.md#328-security-findings
#[test]
fn a_stale_clean_report_is_stale_not_never_run() {
    let w = World::new();
    crate::history::security::ingest(&w.root, &w.clean_bandit(), &[]).unwrap();
    fs::write(w.root.join("app/more.py"), "x = 1\n").unwrap();
    git(&w.root, &["add", "."]);
    git(&w.root, &["commit", "-q", "-m", "more"]);

    let rollup = w.rollup();
    assert_eq!(rollup.scan, super::MemberScan::Stale);
    assert_eq!((rollup.findings, rollup.excluded.not_fresh), (None, 0));
}

/// A member whose only stored report is from a scanner its config now
/// disables has been scanned: it reads `stale`, its findings are excluded as
/// not fresh, and it counts nothing.
#[test]
fn a_member_whose_only_report_is_from_a_disabled_scanner_is_stale() {
    let w = World::new();
    crate::history::security::ingest(&w.root, &fixture("semgrep"), &["app/db.py".to_string()]).unwrap();
    w.repo_config("[security_scan.semgrep]\nenabled = false\n");

    let rollup = w.rollup();
    assert_eq!(rollup.scan, super::MemberScan::Stale);
    assert_eq!(rollup.findings, None);
    assert_eq!((rollup.excluded.not_fresh, rollup.excluded.suppressed), (2, 0));
    assert_eq!(rollup.scanners_fresh, 0);
}

// ── FR-SC-17: a scan selected by scanner and scoped by path ─────────────────

/// A Semgrep report with one result per `(uri, rule, line)`.
fn semgrep_report(results: &[(&str, &str, i64)]) -> String {
    let results: Vec<Value> = results
        .iter()
        .map(|&(uri, rule, line)| {
            json!({
                "ruleId": rule,
                "message": { "text": format!("{rule} fired") },
                "locations": [{ "physicalLocation": { "artifactLocation": { "uri": uri }, "region": { "startLine": line } } }]
            })
        })
        .collect();
    json!({ "version": "2.1.0", "runs": [{ "tool": { "driver": { "name": "Semgrep OSS", "semanticVersion": "1.85.0" } }, "results": results }] })
        .to_string()
}

impl World {
    /// A second indexed file beside `app/db.py`, committed.
    fn with_views(&self) {
        fs::write(self.root.join("app/views.py"), "from app import db\n").unwrap();
        git(&self.root, &["add", "."]);
        git(&self.root, &["commit", "-q", "-m", "views"]);
    }

    /// [`scan`] with both fixture files indexed and every progress step kept.
    fn scan_indexed(&self, consent: bool, request: &ScanRequest, steps: &mut Vec<ScanProgress>) -> anyhow::Result<Value> {
        let indexed = vec!["app/db.py".to_string(), "app/views.py".to_string()];
        let summary = scan(
            &self.root,
            &self.env(),
            crate::tools::Manifest::builtin(),
            consent,
            request,
            &mut |step| steps.push(step.clone()),
            |report, scope| crate::history::security::ingest_scoped(&self.root, report, &indexed, scope),
        )?;
        Ok(serde_json::to_value(summary).unwrap())
    }

    /// Each recorded run of `name`, version probes left out.
    fn runs_of(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.rec.join(format!("{name}.argv")))
            .unwrap_or_default()
            .lines()
            .filter(|l| !matches!(*l, "--version" | "version" | "-version"))
            .map(str::to_string)
            .collect()
    }

    fn forget_runs(&self) {
        for entry in fs::read_dir(&self.rec).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "argv") {
                fs::remove_file(path).unwrap();
            }
        }
    }

    /// Every Semgrep finding row outside `path` (snapshot id left out) and
    /// every anchor row outside it, from Semgrep's latest snapshot.
    fn semgrep_rows_outside(&self, path: &str) -> Vec<String> {
        let conn = rusqlite::Connection::open(crate::history::db_path(&self.root)).unwrap();
        let id: i64 = conn
            .query_row("SELECT MAX(id) FROM security_snapshots WHERE scanner_name = 'Semgrep OSS'", [], |r| r.get(0))
            .unwrap();
        let mut rows = Vec::new();
        for sql in [
            "SELECT fingerprint || '|' || rule_id || '|' || severity || '|' || message || '|' || binding || '|' ||
                    COALESCE(path, '') || '|' || COALESCE(start_line, '') FROM security_findings
              WHERE snapshot_id = ?1 AND COALESCE(path, '') <> ?2",
            "SELECT path || '|' || content_hash FROM security_files WHERE snapshot_id = ?1 AND path <> ?2",
        ] {
            let mut stmt = conn.prepare(sql).unwrap();
            rows.extend(stmt.query_map(rusqlite::params![id, path], |r| r.get::<_, String>(0)).unwrap().map(Result::unwrap));
        }
        rows.sort();
        rows
    }
}

/// [FR-SC-17] acceptance 1 and the argv rule, end to end: `--scanner semgrep
/// --path app/db.py` runs the stub on that file only — its argv the full
/// run's with `.` swapped for the path — writes its report under `scoped/`
/// (the whole-project report untouched), and replaces only db.py's Semgrep
/// findings. Every other file's findings and freshness rows are byte-identical,
/// every other scanner is `not_selected`, and the progress events carry the
/// scope as the summary does.
///
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
#[test]
fn a_scoped_semgrep_scan_replaces_only_that_files_findings() {
    let w = World::new();
    w.with_views();
    let full = w.rec.join("full.sarif");
    fs::write(&full, semgrep_report(&[("app/db.py", "r.one", 1), ("app/views.py", "r.two", 1)])).unwrap();
    w.stub("semgrep", Some(&full), 0, "");
    w.stub("bandit", Some(&w.clean_bandit()), 0, "");
    w.scan_indexed(true, &ScanRequest::new(vec!["semgrep".into()], Vec::new()), &mut Vec::new()).unwrap();
    let full_argv = w.runs_of("semgrep");
    assert_eq!(full_argv.len(), 1);
    let whole_report = fs::read(w.root.join(OUTPUT_DIR).join("semgrep.sarif")).unwrap();
    let before = w.semgrep_rows_outside("app/db.py");
    assert!(before.iter().any(|r| r.contains("r.two")), "views.py's finding is stored: {before:?}");

    fs::write(w.root.join("app/db.py"), "import sqlite3  # fixed\n").unwrap();
    let scoped_report = w.rec.join("scoped.sarif");
    fs::write(&scoped_report, semgrep_report(&[("app/db.py", "r.three", 1)])).unwrap();
    w.stub("semgrep", Some(&scoped_report), 0, "");
    w.forget_runs();
    let mut steps = Vec::new();
    let payload = w
        .scan_indexed(false, &ScanRequest::new(vec!["semgrep".into()], vec!["app/db.py".into()]), &mut steps)
        .expect("the scoped scan runs under the recorded consent");

    let scoped_out = w.root.join(OUTPUT_DIR).join("scoped/semgrep.sarif");
    let want = full_argv[0]
        .replace(&w.root.join(OUTPUT_DIR).join("semgrep.sarif").display().to_string(), &scoped_out.display().to_string())
        .strip_suffix(" .")
        .map(|head| format!("{head} app/db.py"))
        .expect("the full run ends at its path position");
    assert_eq!(w.runs_of("semgrep"), [want], "only the path elements (and the scoped output) differ");
    assert_eq!(fs::read(w.root.join(OUTPUT_DIR).join("semgrep.sarif")).unwrap(), whole_report, "the whole-project report is untouched");

    let semgrep = scanner_row(&payload, "semgrep");
    assert_eq!(semgrep["outcome"], "ran", "{payload}");
    assert_eq!(semgrep["findings"], 1);
    assert_eq!(semgrep["sarif"], json!([".logos/security/scoped/semgrep.sarif"]));
    assert_eq!(semgrep["scope"], json!({ "kind": "paths", "paths": ["app/db.py"] }));
    assert_eq!(scanner_row(&payload, "bandit")["outcome"], "not_selected");
    assert!(scanner_row(&payload, "bandit").get("scope").is_none(), "a scanner the request did not name carries no scope note");
    assert!(w.started().iter().all(|l| !l.contains("clean-bandit")) && w.runs_of("bandit").is_empty(), "Bandit never started");
    let finished = steps.iter().find_map(|s| match s {
        ScanProgress::Finished(run) if run.scanner == "semgrep" => Some(serde_json::to_value(run).unwrap()),
        _ => None,
    });
    assert_eq!(finished.as_ref(), Some(semgrep), "the progress event is the summary's row");

    assert_eq!(w.semgrep_rows_outside("app/db.py"), before, "every other file's rows are byte-identical");
    let listed = w.findings(&FindingsFilter::default());
    let rules: Vec<(&str, &str)> = listed["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["path"].as_str().unwrap(), f["rule_id"].as_str().unwrap()))
        .collect();
    assert_eq!(rules, [("app/db.py", "r.three"), ("app/views.py", "r.two")], "db.py's finding replaced, views.py's kept");
}

/// A branch review names a scanner by the SARIF driver its findings carry, and
/// the security review skill re-scans by that name: `Semgrep OSS` runs Semgrep
/// alone, as `semgrep` does.
#[test]
fn a_scanner_named_by_its_sarif_driver_runs_as_its_registry_name() {
    let w = World::new();
    w.with_views();
    let report = w.rec.join("scoped.sarif");
    fs::write(&report, semgrep_report(&[("app/db.py", "r.one", 1)])).unwrap();
    w.stub("semgrep", Some(&report), 0, "");
    w.stub("bandit", Some(&w.clean_bandit()), 0, "");
    let payload = w
        .scan_indexed(true, &ScanRequest::new(vec!["Semgrep OSS".into()], vec!["app/db.py".into()]), &mut Vec::new())
        .expect("a driver name names its scanner");
    assert_eq!(scanner_row(&payload, "semgrep")["outcome"], "ran", "{payload}");
    assert_eq!(scanner_row(&payload, "bandit")["outcome"], "not_selected", "{payload}");
    assert_eq!(w.runs_of("semgrep").len(), 1);
}

/// [FR-SC-17] acceptance 2 and [NFR-SE-04]: a path outside the root — climbing
/// out, absolute, or through a symbolic link — and a path that is missing, the
/// root itself or flag-shaped, each stop the scan with exit 2 naming it. No
/// process starts (the stubs record every argv, version probes included) and no
/// consent is recorded, though consent was given.
///
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
/// [NFR-SE-04]: ../../../docs/specs/requirements/NFR-SE-04.md
#[test]
fn a_path_outside_the_root_is_refused_before_any_process_starts() {
    let w = World::new();
    for s in &REGISTRY {
        w.stub(s.name, None, 0, "");
    }
    let outside = w.root.parent().unwrap().join("outside");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, w.root.join("escape")).unwrap();
    fs::write(w.root.join("-rf"), "").unwrap();
    for (path, why) in [
        ("../outside", "resolves outside the project root"),
        ("app/../../outside", "resolves outside the project root"),
        (outside.to_str().unwrap(), "resolves outside the project root"),
        ("escape", "through a symbolic link"),
        ("app/missing.py", "does not exist"),
        ("app/*.py", "a glob is not expanded"),
        (".", "the project root itself"),
        ("-rf", "would read as a flag"),
        ("", "is empty"),
    ] {
        for scanners in [Vec::new(), vec!["semgrep".to_string()]] {
            let err = w
                .scan_indexed(true, &ScanRequest::new(scanners, vec!["app/db.py".into(), path.into()]), &mut Vec::new())
                .expect_err(path);
            assert_eq!(crate::error::exit_code(&err), 2, "{path:?}: {err:#}");
            let message = err.downcast_ref::<ConfigError>().expect("a usage error").to_string();
            assert!(message.contains(why) && message.contains(&format!("{path:?}")), "{path:?}: {message}");
        }
    }
    assert!(w.started().is_empty(), "nothing was spawned: {:?}", w.started());
    assert!(!w.user.join(SCAN_CONSENT_FILE).exists(), "no consent was recorded");

    // An unregistered scanner name is refused the same way.
    let err = w
        .scan_indexed(true, &ScanRequest::new(vec!["nmap".into()], vec!["app/db.py".into()]), &mut Vec::new())
        .expect_err("nmap is not registered");
    assert_eq!(crate::error::exit_code(&err), 2);
    assert!(w.started().is_empty());
}

/// [FR-SC-17] acceptance 3: Trivy, OSV-Scanner and Dependency-Check report
/// `whole-manifest only` (CodeQL `whole-project only`). Left out of a scoped
/// scan that names no scanner, nothing of theirs starts; named, Trivy runs
/// unscoped, its argv exactly the full scan's.
///
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
#[test]
fn the_whole_manifest_scanners_report_it_and_run_unscoped_only_when_named() {
    let w = World::new();
    let trivy = fixture("trivy");
    w.stub("semgrep", Some(&fixture("semgrep")), 0, "");
    w.stub("trivy", Some(&trivy), 0, "");
    w.stub("osv-scanner", Some(&fixture("osv-scanner")), 0, "");
    w.scan_indexed(true, &ScanRequest::new(vec!["trivy".into()], Vec::new()), &mut Vec::new()).unwrap();
    let full_trivy = w.runs_of("trivy");
    w.forget_runs();

    // No scanner named: Semgrep runs scoped, the whole-manifest ones are left out.
    let payload = w.scan_indexed(true, &ScanRequest::new(Vec::new(), vec!["app/db.py".into()]), &mut Vec::new()).unwrap();
    assert_eq!(scanner_row(&payload, "semgrep")["scope"], json!({ "kind": "paths", "paths": ["app/db.py"] }));
    for name in ["trivy", "osv-scanner"] {
        let row = scanner_row(&payload, name);
        assert_eq!(row["outcome"], "not_selected", "{name}: {row}");
        assert_eq!(row["scope"], json!({ "kind": "unscoped", "reason": "whole-manifest only" }), "{name}");
        assert!(w.runs_of(name).is_empty() && !w.started().iter().any(|l| l.contains(name)), "nothing of {name}'s started");
    }
    let not_installed = scanner_row(&payload, "dependency-check");
    assert_eq!((not_installed["outcome"].as_str(), not_installed["scope"]["reason"].as_str()), (Some("not_installed"), Some("whole-manifest only")));
    assert_eq!(scanner_row(&payload, "codeql")["scope"]["reason"], "whole-project only");

    // Named: Trivy runs on the whole project, exactly as a full scan runs it.
    w.forget_runs();
    let payload = w
        .scan_indexed(false, &ScanRequest::new(vec!["trivy".into()], vec!["app/db.py".into()]), &mut Vec::new())
        .unwrap();
    let row = scanner_row(&payload, "trivy");
    assert_eq!(row["outcome"], "ran", "{row}");
    assert_eq!(row["scope"], json!({ "kind": "unscoped", "reason": "whole-manifest only" }));
    assert_eq!(row["sarif"], json!([".logos/security/trivy.sarif"]), "an unscoped report is a whole-project one");
    assert_eq!(w.runs_of("trivy"), full_trivy, "Trivy's argv is the full scan's");
    assert_eq!(scanner_row(&payload, "semgrep")["outcome"], "not_selected");
    assert!(w.runs_of("semgrep").is_empty());
}

/// Gitleaks reads one `--source`, so a scoped Gitleaks scan over two paths is
/// two `--no-git` runs whose reports merge into one, ingested as one snapshot
/// (CRA-02). With no full scan stored before it, `status` reads that snapshot
/// stale — `scoped_only` — never fresh ([FR-SC-17]).
///
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
#[test]
fn a_scoped_gitleaks_scan_runs_once_per_path_and_alone_reads_stale() {
    let w = World::new();
    w.with_views();
    w.stub("gitleaks", Some(&fixture("gitleaks")), 0, "");
    let payload = w
        .scan_indexed(true, &ScanRequest::new(vec!["gitleaks".into()], vec!["app/views.py".into(), "app/db.py".into()]), &mut Vec::new())
        .unwrap();
    let runs = w.runs_of("gitleaks");
    let scoped = w.root.join(OUTPUT_DIR).join("scoped");
    assert_eq!(runs.len(), 2, "{runs:?}");
    for (run, (n, path)) in runs.iter().zip([(1, "app/views.py"), (2, "app/db.py")]) {
        let out = scoped.join(format!("gitleaks.{n}.sarif"));
        assert!(run.ends_with(&format!("--report-path {} --no-git --source {path}", out.display())), "{run}");
    }
    let row = scanner_row(&payload, "gitleaks");
    assert_eq!(row["outcome"], "ran", "{row}");
    assert_eq!(row["sarif"], json!([".logos/security/scoped/gitleaks.sarif"]));
    assert_eq!(row["ingests"].as_array().unwrap().len(), 1, "one merged report, one ingest");
    let merged: Value = serde_json::from_slice(&fs::read(scoped.join("gitleaks.sarif")).unwrap()).unwrap();
    assert_eq!(merged["runs"].as_array().unwrap().len(), 2, "both runs are in the merged report");

    let status = w.status();
    let gitleaks = scanner_row(&status, "gitleaks");
    assert_eq!((gitleaks["state"].as_str(), gitleaks["because"].as_str()), (Some("stale"), Some("scoped_only")), "{gitleaks}");
}

/// A path inside another given directory is dropped before anything runs: a
/// scanner that runs once per path would otherwise scan the file twice and
/// store each result twice — one secret, two findings, one of them a phantom
/// fingerprint ([FR-SC-17]).
///
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
#[test]
fn a_path_inside_another_given_directory_is_scanned_and_stored_once() {
    let w = World::new();
    w.with_views();
    let report = w.rec.join("one-secret.sarif");
    fs::write(
        &report,
        json!({ "version": "2.1.0", "runs": [{ "tool": { "driver": { "name": "gitleaks", "semanticVersion": "v8.18.4" } }, "results": [{
            "ruleId": "generic-api-key", "message": { "text": "generic-api-key has detected secret" },
            "locations": [{ "physicalLocation": { "artifactLocation": { "uri": "app/views.py" }, "region": { "startLine": 1 } } }]
        }] }] })
        .to_string(),
    )
    .unwrap();
    w.stub("gitleaks", Some(&report), 0, "");
    let payload = w
        .scan_indexed(true, &ScanRequest::new(vec!["gitleaks".into()], vec!["app/views.py".into(), "app".into(), "app/views.py".into()]), &mut Vec::new())
        .unwrap();
    let runs = w.runs_of("gitleaks");
    assert_eq!(runs.len(), 1, "only the directory runs: {runs:?}");
    assert!(runs[0].ends_with("--no-git --source app"), "{}", runs[0]);
    let row = scanner_row(&payload, "gitleaks");
    assert_eq!(row["scope"], json!({ "kind": "paths", "paths": ["app"] }));
    assert_eq!(row["findings"], 1, "one secret on one line of one file is one finding: {row}");
    let stored = w.findings(&FindingsFilter::default());
    assert_eq!(stored["findings"].as_array().unwrap().len(), 1, "{stored}");

    // One file spelled two ways is one path: one run, listed once.
    w.forget_runs();
    let payload = w
        .scan_indexed(false, &ScanRequest::new(vec!["gitleaks".into()], vec!["app/views.py".into(), "./app/views.py".into()]), &mut Vec::new())
        .unwrap();
    assert_eq!(w.runs_of("gitleaks").len(), 1, "{:?}", w.runs_of("gitleaks"));
    assert_eq!(scanner_row(&payload, "gitleaks")["scope"], json!({ "kind": "paths", "paths": ["app/views.py"] }));
}

/// A scoped run at a new HEAD carries the rows a full scan wrote at the old
/// one, so `status` reads it stale — `head_moved` since that full scan's HEAD —
/// never fresh for the files it did not re-scan ([FR-SC-17], migration 6).
///
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
#[test]
fn a_scoped_run_at_a_new_head_reads_stale_since_the_full_scans_head() {
    let w = World::new();
    w.with_views();
    let full = w.rec.join("full.sarif");
    fs::write(&full, semgrep_report(&[("app/db.py", "r.one", 1), ("app/views.py", "r.two", 1)])).unwrap();
    w.stub("semgrep", Some(&full), 0, "");
    w.scan_indexed(true, &ScanRequest::new(vec!["semgrep".into()], Vec::new()), &mut Vec::new()).unwrap();
    let head_a = crate::history::head_sha(&w.root).unwrap();
    let fresh = w.status();
    assert_eq!(scanner_row(&fresh, "semgrep")["state"], "fresh", "{fresh}");

    fs::write(w.root.join("app/db.py"), "import sqlite3  # fixed\n").unwrap();
    git(&w.root, &["commit", "-q", "-am", "fix"]);
    let scoped = w.rec.join("scoped.sarif");
    fs::write(&scoped, semgrep_report(&[])).unwrap();
    w.stub("semgrep", Some(&scoped), 0, "");
    w.scan_indexed(false, &ScanRequest::new(vec!["semgrep".into()], vec!["app/db.py".into()]), &mut Vec::new()).unwrap();

    let status = w.status();
    let semgrep = scanner_row(&status, "semgrep");
    assert_eq!((semgrep["state"].as_str(), semgrep["because"].as_str()), (Some("stale"), Some("head_moved")), "{semgrep}");
    assert_eq!(semgrep["snapshot_head"], head_a.as_str(), "the full scan's HEAD, not the scoped run's");
}
