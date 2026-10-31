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
use super::scope::no_test_symbols;
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
    /// The NVD API key the scanning process's environment carries.
    nvd_api_key: Option<super::NvdApiKey>,
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
        Self { _tmp: tmp, root, bin, rec, user, nvd_api_key: None }
    }

    fn env(&self) -> ScanEnvironment {
        ScanEnvironment {
            user_dir: Some(self.user.clone()),
            search_path: Some(self.bin.clone().into_os_string()),
            tools_dir: None,
            nvd_api_key: self.nvd_api_key.clone(),
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
             for a in \"$@\"; do case \"$prev\" in --output|--output-file|--report-path) out=\"$a\";; esac; prev=\"$a\"; done\n\
             {copy}\n\
             printf '%s\\n' '{stderr}' >&2\n\
             exit {exit}\n",
            record = record.display(),
        );
        let path = self.bin.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A `go` that records its `GOTOOLCHAIN` and argv, prints `output` for
    /// `go version` and exits `exit`.
    fn stub_go(&self, output: &str, exit: i32) {
        let script = format!(
            "#!/bin/sh\nprintf 'GOTOOLCHAIN=%s %s\\n' \"$GOTOOLCHAIN\" \"$*\" >> '{}'\nprintf '%s\\n' '{output}'\nexit {exit}\n",
            self.rec.join("go.argv").display(),
        );
        let path = self.bin.join("go");
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A `name` that prints `line` on stdout alone, writes no report and exits `exit`.
    fn stub_stdout(&self, name: &str, exit: i32, line: &str) {
        let path = self.bin.join(name);
        fs::write(
            &path,
            format!("#!/bin/sh\ncase \"$1\" in --version|version|-version) echo '{name} 1.2.3'; exit 0;; esac\nprintf '%s\\n' '{line}'\nexit {exit}\n"),
        )
        .unwrap();
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
        serde_json::to_value(status(&self.root, &self.env(), crate::tools::Manifest::builtin(), TODAY.unwrap(), super::VersionProbes::Every, true, no_test_symbols).unwrap()).unwrap()
    }

    /// The listing with no graph behind it: every finding unbound. The graph
    /// context itself is `tests/security_context.rs`'s.
    fn findings(&self, filter: &FindingsFilter) -> Value {
        let no_graph = |listed: &[&SecurityFinding]| {
            Ok(listed.iter().map(|f| FindingContext::unbound(f.start_line, UnboundBecause::NoSymbol)).collect())
        };
        serde_json::to_value(findings(&self.root, &self.env(), crate::tools::Manifest::builtin(), filter, TODAY.unwrap(), no_test_symbols, no_graph).unwrap()).unwrap()
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

/// S-769: a failed scanner's outcome names its cause in one line taken from
/// its log — the first line that reports an error, not a line that merely
/// counts errors — masked, on the scan payload and on the status row, which
/// reads it back from the log a later read finds.
#[test]
fn a_failed_scanner_names_its_cause_from_its_log() {
    let w = World::new();
    let output = [
        "[INFO] Checking for updates",
        "[INFO] Analysis started; 0 errors so far",
        &format!("[ERROR] Error updating the NVD Data: apiKey={PLANTED}"),
        "org.owasp.dependencycheck.data.update.exception.UpdateException: Error updating the NVD Data",
        "Caused by: NVD Returned Status Code: 403",
        "[ERROR] Unable to continue dependency-check analysis.",
    ]
    .join("\n");
    w.stub("dependency-check", None, 1, &output);
    let payload = serde_json::to_value(w.scan(true).expect("the scan runs")).unwrap();
    let failed = scanner_row(&payload, "dependency-check");
    assert_eq!(failed["outcome"], "failed", "{failed}");
    let cause = failed["cause"].as_str().unwrap_or_else(|| panic!("no cause: {failed}"));
    assert!(cause.starts_with("[ERROR] Error updating the NVD Data: apiKey="), "{cause}");
    assert!(cause.contains(super::mask::MASK) && !cause.contains(PLANTED), "{cause}");
    assert!(!cause.contains('\n'), "one line: {cause:?}");

    let row = w.status();
    let row = scanner_row(&row, "dependency-check");
    assert_eq!(row["last_failure"], format!("{OUTPUT_DIR}/dependency-check.failed.log"), "{row}");
    assert_eq!(row["last_failure_cause"].as_str(), Some(cause), "{row}");
    assert_eq!(scanner_row(&w.status(), "semgrep").get("last_failure_cause"), None, "absent without a failure");
}

/// Dependency-Check 13 updates its NVD data through an API that rejects a
/// missing key. The first `[ERROR]` line then says only that the update
/// failed, so the cause names the key and the user-level route that supplies
/// it — whole, on one line, with no key value in it — on the scan payload and
/// on the status row.
#[test]
fn a_dependency_check_failing_for_want_of_an_nvd_api_key_names_the_remedy() {
    let w = World::new();
    let output = [
        "[INFO] Checking for updates",
        "[ERROR] Error updating the NVD Data",
        "org.owasp.dependencycheck.data.update.exception.UpdateException: Error updating the NVD Data",
        "\tat org.owasp.dependencycheck.data.update.NvdApiDataSource.processApi(NvdApiDataSource.java:387)",
        "Caused by: io.github.jeremylong.openvulnerability.client.nvd.NvdApiException: Invalid API Key, length of 0 too short to provided a masked partial key",
        "[ERROR] Unable to continue dependency-check analysis.",
    ]
    .join("\n");
    w.stub("dependency-check", None, 13, &output);
    let payload = serde_json::to_value(w.scan(true).expect("the scan runs")).unwrap();
    let failed = scanner_row(&payload, "dependency-check");
    assert_eq!((failed["outcome"].as_str(), failed["exit_code"].as_i64()), (Some("failed"), Some(13)), "{failed}");
    let cause = failed["cause"].as_str().unwrap_or_else(|| panic!("no cause: {failed}"));
    assert!(cause.contains("NVD API key") && cause.contains("missing or rejected"), "{cause}");
    assert!(cause.contains("https://nvd.nist.gov/developers/request-an-api-key"), "{cause}");
    assert!(cause.contains("[security_scan.dependency-check]") && cause.contains(r#"extra_args = ["--nvdApiKey", "<key>"]"#), "{cause}");
    assert!(cause.contains("user config"), "the repository config may not set extra_args: {cause}");
    // FR-SC-27: the environment variable is the first route named, `serve` with it.
    let (env, extra) = (cause.find("NVD_API_KEY").unwrap_or(usize::MAX), cause.find("extra_args").unwrap());
    assert!(env < extra, "NVD_API_KEY is named before extra_args: {cause}");
    assert!(cause.contains("logos serve"), "{cause}");
    assert!(!cause.contains('\n') && cause.chars().count() <= super::surface::CAUSE_CHARS, "one bounded line: {cause:?}");
    assert_eq!(scanner_row(&w.status(), "dependency-check")["last_failure_cause"].as_str(), Some(cause));
}

/// The NVD-key cause fires on the exception that names the API key and on
/// nothing nearer: another NVD failure (a 403, a rate limit) and a line that
/// merely says "API key" keep the first error line, and a rejected key's
/// value never reaches the cause.
#[test]
fn only_an_nvd_exception_naming_the_api_key_changes_the_cause() {
    use super::surface::failure_cause;
    let rate_limited = "[ERROR] Error updating the NVD Data\nCaused by: io.github.jeremylong.openvulnerability.client.nvd.NvdApiException: NVD Returned Status Code: 429";
    assert_eq!(failure_cause(rate_limited), "[ERROR] Error updating the NVD Data");
    let other_scanner = "[ERROR] Invalid API Key supplied to the registry\nCaused by: io.example.RegistryException: Invalid API Key";
    assert_eq!(failure_cause(other_scanner), "[ERROR] Invalid API Key supplied to the registry");
    let rejected = format!("[ERROR] Error updating the NVD Data\nCaused by: io.github.jeremylong.openvulnerability.client.nvd.NvdApiException: Invalid API Key apiKey={PLANTED}");
    let cause = failure_cause(&rejected);
    assert!(cause.contains("NVD API key") && !cause.contains(PLANTED), "{cause}");
}

/// The status cause is read back from as much of the log as a scan writes:
/// a scanner that prints only to stdout, more than the stderr cap of it,
/// before its error line, reads the same cause on the scan and on `status`.
#[test]
fn the_status_cause_reads_the_whole_log_a_scan_wrote() {
    let w = World::new();
    let script = "#!/bin/sh\n\
         case \"$1\" in --version) printf '%s\\n' 'dependency-check 13.0.0'; exit 0;; esac\n\
         i=0; while [ $i -lt 400 ]; do printf '[INFO] %060d\\n' $i; i=$((i+1)); done\n\
         printf '%s\\n' '[ERROR] Error updating the NVD Data'\n\
         exit 1\n";
    let path = w.bin.join("dependency-check");
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let payload = serde_json::to_value(w.scan(true).unwrap()).unwrap();
    let failed = scanner_row(&payload, "dependency-check");
    assert_eq!(failed["cause"], "[ERROR] Error updating the NVD Data", "{failed}");
    let log = fs::metadata(w.root.join(OUTPUT_DIR).join("dependency-check.failed.log")).unwrap().len();
    assert!(log > crate::jobs::spawn::STDERR_CAP as u64, "the log ({log} bytes) outgrows the stderr cap");
    assert_eq!(scanner_row(&w.status(), "dependency-check")["last_failure_cause"], failed["cause"]);
}

/// A gosec built for an older Go than the local one cannot type a
/// standard-library import and exits 1 with no report (S-771's observation).
/// That is a `failed` outcome, never `not_applicable` nor a clean scan, and its
/// cause names the gosec version, the local Go and the remedy (S-782) — on the
/// scan and again on `status`, which reads it from the log.
#[test]
fn a_gosec_that_cannot_type_a_stdlib_import_fails_naming_its_version_the_local_go_and_the_remedy() {
    let w = World::new();
    fs::write(w.root.join("app/hash.go"), "package app\n\nimport _ \"crypto/md5\"\n").unwrap();
    let line = r#"internal error: package "crypto/md5" without types was imported from "command-line-arguments""#;
    w.stub("gosec", None, 1, line);
    w.stub_go("go version go1.27.2 darwin/arm64", 0);
    let payload = serde_json::to_value(w.scan(true).expect("the scan runs")).unwrap();
    let row = scanner_row(&payload, "gosec");
    assert_eq!(row["outcome"], "failed", "never not_applicable or a clean scan: {row}");
    assert_eq!(row["exit_code"], 1, "{row}");
    let cause = row["cause"].as_str().expect("a cause");
    assert!(cause.contains("gosec 1.2.3"), "the gosec version: {cause}");
    assert!(cause.contains("go1.27.2"), "the local go version: {cause}");
    assert!(
        cause.contains("use a gosec release built for that Go, or a Go toolchain gosec supports"),
        "the remedy: {cause}"
    );
    assert!(cause.contains("without types was imported"), "the symptom: {cause}");
    assert!(cause.chars().count() <= super::surface::CAUSE_CHARS && !cause.ends_with('…'), "whole: {cause}");
    // gosec's own line stays in the log, with the added one after it.
    let log = fs::read_to_string(w.root.join(OUTPUT_DIR).join("gosec.failed.log")).unwrap();
    assert!(log.contains(line) && log.trim_end().ends_with(cause), "{log}");
    // `status` reads the same cause from the log.
    let status_payload = w.status();
    let status = scanner_row(&status_payload, "gosec");
    assert_eq!(status["last_failure_cause"].as_str(), Some(cause), "{status}");
    assert_ne!(status["state"], "not_applicable", "{status}");
    // The local Go was asked once, with `version` alone, and never allowed to
    // fetch a toolchain.
    assert_eq!(fs::read_to_string(w.rec.join("go.argv")).unwrap(), "GOTOOLCHAIN=local version\n");
}

/// The added cause is a fact of gosec's own report, so it needs no `go`: with
/// none on the `PATH` the Go version reads `unknown`, and the remedy stays. And
/// output without the line adds nothing — another gosec failure keeps its own.
#[test]
fn the_stdlib_cause_survives_a_missing_go_and_is_not_added_to_another_failure() {
    let w = World::new();
    fs::write(w.root.join("app/hash.go"), "package app\n").unwrap();
    w.stub("gosec", None, 1, r#"package "crypto/md5" without types was imported"#);
    let payload = serde_json::to_value(w.scan(true).unwrap()).unwrap();
    let cause = scanner_row(&payload, "gosec")["cause"].as_str().unwrap().to_string();
    assert!(cause.contains("with the local go unknown") && cause.contains("gosec 1.2.3"), "{cause}");
    assert!(cause.contains("a Go toolchain gosec supports"), "{cause}");

    let other = World::new();
    fs::write(other.root.join("app/hash.go"), "package app\n").unwrap();
    other.stub("gosec", None, 1, "loading packages failed: no Go files");
    other.stub_go("go version go1.27.2 darwin/arm64", 0);
    let payload = serde_json::to_value(other.scan(true).unwrap()).unwrap();
    assert_eq!(scanner_row(&payload, "gosec")["cause"], "loading packages failed: no Go files");
    assert!(!other.rec.join("go.argv").exists(), "go was asked for a failure that is not about types");
}

/// The line is built from two version tokens and a fixed sentence — a `go`
/// whose version token carries anything but version characters or runs long, or
/// that exits non-zero, is `unknown`, never repeated; and gosec's marker on its
/// stdout alone is named all the same.
#[test]
fn a_go_that_prints_no_trustworthy_version_is_unknown_and_never_quoted() {
    let long = format!("go version go1.{} darwin/arm64", "9".repeat(200));
    for (output, exit) in [
        ("go version go1.27.2;evil darwin/arm64 devel", 0),
        (long.as_str(), 0),
        ("go version go1.27.2 darwin/arm64", 1),
    ] {
        let w = World::new();
        fs::write(w.root.join("app/hash.go"), "package app\n").unwrap();
        w.stub("gosec", None, 1, "without types was imported");
        w.stub_go(output, exit);
        let payload = serde_json::to_value(w.scan(true).unwrap()).unwrap();
        let cause = scanner_row(&payload, "gosec")["cause"].as_str().unwrap().to_string();
        assert!(cause.contains("with the local go unknown"), "{output:?} (exit {exit}): {cause}");
        assert!(cause.ends_with("a Go toolchain gosec supports") && !cause.contains("evil"), "{cause}");
    }
}

/// gosec's own line on stdout, nothing on stderr, is the failure's output
/// (`step` falls back to stdout): the cause is added there too.
#[test]
fn the_stdlib_cause_is_named_when_gosec_prints_its_line_on_stdout() {
    let w = World::new();
    fs::write(w.root.join("app/hash.go"), "package app\n").unwrap();
    w.stub_stdout("gosec", 1, r#"package "crypto/md5" without types was imported"#);
    w.stub_go("go version go1.27.2 darwin/arm64", 0);
    let payload = serde_json::to_value(w.scan(true).unwrap()).unwrap();
    let cause = scanner_row(&payload, "gosec")["cause"].as_str().unwrap().to_string();
    assert!(cause.contains("gosec 1.2.3 with the local go1.27.2"), "{cause}");
}

/// The cause is one line whatever the output: the first error line, else the
/// last line, else a sentence saying nothing was printed; never longer than
/// the cap.
#[test]
fn a_failure_cause_is_one_bounded_line() {
    use super::surface::failure_cause;
    assert_eq!(failure_cause("downloading rules\nfatal: no git repository\nbye\n"), "fatal: no git repository");
    assert_eq!(failure_cause("panic: runtime error: index out of range"), "panic: runtime error: index out of range");
    assert_eq!(failure_cause("2 errors were ignored\nERRORS: none\nlast words  \n\n"), "last words", "a count of errors is no error");
    assert_eq!(failure_cause("could not start /bin/x: No such file or directory"), "could not start /bin/x: No such file or directory");
    assert_eq!(failure_cause(" \n\t\n"), super::surface::NO_OUTPUT);
    let cap = super::surface::CAUSE_CHARS;
    // Exactly at the cap a line is whole; one past it, it is cut to the cap,
    // its last character the `…`.
    let at_cap = format!("Error: {}", "x".repeat(cap - "Error: ".len()));
    assert_eq!(failure_cause(&at_cap), at_cap);
    let past_cap = format!("{at_cap}y");
    assert_eq!(failure_cause(&past_cap), format!("{}…", &at_cap[..cap - 1]));
    let cut = failure_cause(&format!("Error: {}", "é".repeat(1000)));
    assert!(cut.chars().count() == cap && cut.ends_with('…') && cut.starts_with("Error: é"), "{cut}");
}

/// S-769: a scanner with nothing to scan — gosec on a project without Go — is
/// `not applicable` on the scan, reported without a start and never spawned,
/// not even for its version; `status` reads it `not_applicable` with the
/// reason, and the roll-up leaves it out of the scanners a member should have
/// fresh.
#[test]
fn a_scanner_with_nothing_to_scan_is_not_applicable_on_every_surface() {
    let w = World::new();
    w.stub("semgrep", Some(&fixture("semgrep")), 0, "");
    w.stub("gosec", None, 2, "");
    let mut heard = Vec::new();
    let payload = serde_json::to_value(w.scan_reporting(true, &mut |p| heard.push(p.clone())).unwrap()).unwrap();
    let gosec = scanner_row(&payload, "gosec");
    assert_eq!(gosec["outcome"], "not_applicable", "{gosec}");
    let reason = "no Go source file (.go) among the files the project does not ignore";
    assert_eq!(gosec["reason"], reason);
    assert!(!heard.contains(&ScanProgress::Started { scanner: "gosec" }), "gosec was reported started: {heard:?}");
    assert!(!w.rec.join("gosec.argv").exists(), "gosec was invoked: {:?}", w.started());

    let status = w.status();
    let row = scanner_row(&status, "gosec");
    assert_eq!((row["state"].as_str(), row["reason"].as_str()), (Some("not_applicable"), Some(reason)), "{row}");
    assert_eq!((row.get("ignored_paths"), row["last_failure"].clone()), (None, Value::Null), "nothing would run: {row}");
    // UAT-SC-10 step 2: not even the CLI's status, which probes every other
    // installed copy, starts it; its copy is named, unprobed.
    assert!(!w.rec.join("gosec.argv").exists(), "status probed gosec: {:?}", w.started());
    assert_eq!((row["installed"]["copy"].as_str(), row["installed"]["unprobed"].as_str()), (Some("path"), Some("nothing_to_scan")), "{row}");

    // `security findings` lists `status`'s `never_run` scanners: not gosec.
    assert_eq!(w.findings(&FindingsFilter::default())["never_run"], json!([]), "one state per scanner on every surface");

    let rollup = w.rollup();
    assert_eq!(rollup.scanners_enabled, REGISTRY.len() - 1, "every registered scanner but the not-applicable gosec");
    assert_eq!(super::gate::freshness_of(&rollup.scanners[5].state), crate::governance::security::ScanFreshness::NotApplicable);
}

/// S-769: a stored report outranks `not_applicable`, as it outranks every
/// detected state — a gosec report ingested from CI on a project with no Go
/// file here reads `fresh`, and the roll-up still counts gosec.
#[test]
fn a_stored_report_outranks_not_applicable() {
    let w = World::new();
    w.stub("gosec", None, 0, "");
    crate::history::security::ingest(&w.root, &fixture("gosec"), &["app/db.py".to_string()]).unwrap();
    let status = w.status();
    let row = scanner_row(&status, "gosec");
    assert_eq!(row["state"], "fresh", "{row}");
    assert_eq!(w.rollup().scanners_enabled, REGISTRY.len(), "a stored report keeps gosec in the denominator");
}

/// S-769 × S-760: a stale report does not outrank `not_applicable`. Once the
/// project's last Go file is gone, no scan here can refresh gosec's report, so
/// it reads `not_applicable` on `status`, the roll-up and the gate's freshness
/// alike — never a stale scan whose remedy is a scan that starts nothing.
#[test]
fn a_stale_report_with_nothing_left_to_scan_is_not_applicable() {
    let w = World::new();
    fs::write(w.root.join("x.go"), "package x\n").unwrap();
    git(&w.root, &["add", "."]);
    git(&w.root, &["commit", "-q", "-m", "go"]);
    w.stub("gosec", None, 0, "");
    crate::history::security::ingest(&w.root, &fixture("gosec"), &["app/db.py".to_string()]).unwrap();
    assert_eq!(scanner_row(&w.status(), "gosec")["state"], "fresh");

    git(&w.root, &["rm", "-q", "x.go"]);
    git(&w.root, &["commit", "-q", "-m", "no go"]);
    let _ = fs::remove_file(w.rec.join("gosec.argv"));
    let status = w.status();
    let row = scanner_row(&status, "gosec");
    assert_eq!(row["state"], "not_applicable", "{row}");
    assert_eq!(row.get("ignored_paths"), None, "no coverage is forecast for a scan that starts nothing: {row}");
    assert!(!w.rec.join("gosec.argv").exists(), "status probed gosec: {:?}", w.started());
    let rollup = w.rollup();
    assert_eq!(rollup.scanners_enabled, REGISTRY.len() - 1, "gosec leaves the denominator");
    let gosec = rollup.scanners.iter().find(|s| s.scanner == "gosec").unwrap();
    assert_eq!(super::gate::freshness_of(&gosec.state), crate::governance::security::ScanFreshness::NotApplicable);
}

/// S-769: consent covers what a scan will start, so a scanner with nothing to
/// scan is neither asked for nor recorded, and is never pending — until the
/// project gives it something to scan.
#[test]
fn a_scanner_with_nothing_to_scan_needs_no_consent() {
    let w = World::new();
    w.stub("semgrep", Some(&fixture("semgrep")), 0, "");
    w.stub("gosec", None, 0, "");
    let first = w.scan(true).unwrap();
    assert_eq!(first.consent_recorded, vec!["semgrep".to_string()]);
    assert_eq!(w.status()["consent"]["pending"], json!([]));
    w.scan(false).expect("gosec, which starts nothing, asks for no consent");

    fs::write(w.root.join("x.go"), "package x\n").unwrap();
    assert_eq!(w.status()["consent"]["pending"], json!(["gosec"]));
    assert!(consent_refusal(&w.scan(false).unwrap_err()).contains("gosec"));
}

/// S-769: a scanner that failed while it had something to scan, and has
/// nothing now, leaves no stale failure behind — its log goes with the
/// `not_applicable` outcome, as with a clean run.
#[test]
fn a_scanner_with_nothing_to_scan_clears_its_earlier_failure() {
    let w = World::new();
    fs::write(w.root.join("x.go"), "package x\n").unwrap();
    w.stub("gosec", None, 2, "panic: boom");
    let first = serde_json::to_value(w.scan(true).unwrap()).unwrap();
    assert_eq!(scanner_row(&first, "gosec")["cause"], "panic: boom", "{first}");
    assert_eq!(scanner_row(&w.status(), "gosec")["last_failure_cause"], "panic: boom");

    fs::remove_file(w.root.join("x.go")).unwrap();
    let second = serde_json::to_value(w.scan(false).unwrap()).unwrap();
    assert_eq!(scanner_row(&second, "gosec")["outcome"], "not_applicable", "{second}");
    let status = w.status();
    let row = scanner_row(&status, "gosec");
    assert_eq!((row["state"].as_str(), row["last_failure"].clone(), row.get("last_failure_cause")), (Some("not_applicable"), Value::Null, None), "{row}");
    assert!(!w.root.join(OUTPUT_DIR).join("gosec.failed.log").exists(), "the stale log is gone");
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
        let filter = FindingsFilter::parse(None, None, None, None, Some(token), None, None).unwrap();
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

/// S-809 (FR-SC-29): one `go.mod` advisory OSV-Scanner (twice, byte for
/// byte) and Trivy both report is one finding. `findings` lists it once, under
/// either scanner, naming both reporters; `status` counts it on its first
/// reporter's row and states it as `merged` on the other's, so each row still
/// accounts for every finding of its report.
#[test]
fn one_advisory_two_scanners_report_is_counted_once_and_merged_on_the_other_row() {
    let w = World::new();
    fs::write(w.root.join("go.mod"), "module example.test/app\n").unwrap();
    git(&w.root, &["add", "."]);
    git(&w.root, &["commit", "-q", "-m", "go.mod"]);
    let osv = json!({
        "ruleId": "CVE-2026-107214", "level": "warning",
        "message": {"text": "Package 'github.com/xuri/excelize/v2@2.11.0' is vulnerable to 'CVE-2026-107214' (also known as 'GO-2026-6684')."},
        "locations": [{"physicalLocation": {"artifactLocation": {"uri": format!("file://{}/go.mod", w.root.display())}}}]
    });
    let trivy = json!({
        "ruleId": "CVE-2026-107214", "level": "error",
        "message": {"text": "Package: github.com/xuri/excelize/v2\nInstalled Version: v2.11.0\nVulnerability CVE-2026-107214"},
        "locations": [{"physicalLocation": {"artifactLocation": {"uri": "go.mod", "uriBaseId": "ROOTPATH"}}}]
    });
    let report = w.rec.join("deps.sarif");
    let runs = json!({"version": "2.1.0", "runs": [
        {"tool": {"driver": {"name": "osv-scanner", "version": "2.7.0", "rules": []}}, "results": [osv.clone(), osv]},
        {"tool": {"driver": {"name": "Trivy", "version": "0.76.0", "rules": []}}, "results": [trivy]}
    ]});
    fs::write(&report, runs.to_string()).unwrap();
    crate::history::security::ingest(&w.root, &report, &[]).unwrap();

    let listed = w.findings(&FindingsFilter::default());
    assert_eq!((listed["total"].as_u64(), listed["findings"].as_array().map(Vec::len)), (Some(1), Some(1)), "{listed}");
    let reporters: Vec<&str> =
        listed["findings"][0]["reporters"].as_array().unwrap().iter().map(|r| r["scanner_name"].as_str().unwrap()).collect();
    assert_eq!(reporters, ["Trivy", "osv-scanner"]);
    assert_eq!(listed["findings"][0]["severity"], "high", "Trivy's `error`; OSV-Scanner's unrated never lowers it");
    for scanner in ["osv-scanner", "trivy"] {
        let filter = FindingsFilter { scanner: Some(scanner.to_string()), ..FindingsFilter::default() };
        assert_eq!(w.findings(&filter)["findings"].as_array().map(Vec::len), Some(1), "listed under {scanner}");
    }

    let rows = w.status();
    let counts = |scanner: &str| {
        let row = scanner_row(&rows, scanner);
        (row["state"].as_str().map(str::to_string), row["findings"].as_u64(), row["merged"].as_u64())
    };
    assert_eq!(counts("trivy"), (Some("fresh".to_string()), Some(1), Some(0)), "{rows}");
    assert_eq!(counts("osv-scanner"), (Some("fresh".to_string()), Some(0), Some(1)), "{rows}");

    // The Dashboard's Security widget counts it once too.
    let manifest = crate::tools::Manifest::builtin();
    let read = status(&w.root, &w.env(), manifest, TODAY.unwrap(), super::VersionProbes::Never, false, no_test_symbols).unwrap();
    let widget = super::summary(&w.root, &w.env(), manifest, TODAY.unwrap(), read, None, no_test_symbols).unwrap();
    assert_eq!(widget.tally.map(|t| (t.counted.total, t.counted.high)), Some((1, 1)));
}

/// S-809 review: a merged finding's row is fresh by its first reporter's scan,
/// yet each other reporter's own report still reads `stale` once a file it
/// anchors moved since its scan: OSV-Scanner scanned `go.mod` before an edit,
/// Trivy after it.
#[test]
fn a_later_reporters_moved_file_makes_its_own_row_stale() {
    let w = World::new();
    fs::write(w.root.join("go.mod"), "module example.test/app\n").unwrap();
    git(&w.root, &["add", "."]);
    git(&w.root, &["commit", "-q", "-m", "go.mod"]);
    let result = |driver: &str| match driver {
        "osv-scanner" => json!({
            "ruleId": "CVE-2026-107214", "level": "warning",
            "message": {"text": "Package 'github.com/xuri/excelize/v2@2.11.0' is vulnerable to 'CVE-2026-107214'."},
            "locations": [{"physicalLocation": {"artifactLocation": {"uri": format!("file://{}/go.mod", w.root.display())}}}]
        }),
        _ => json!({
            "ruleId": "CVE-2026-107214", "level": "error",
            "message": {"text": "Package: github.com/xuri/excelize/v2\nInstalled Version: v2.11.0\nVulnerability CVE-2026-107214"},
            "locations": [{"physicalLocation": {"artifactLocation": {"uri": "go.mod", "uriBaseId": "ROOTPATH"}}}]
        }),
    };
    let ingest = |driver: &str| {
        let report = w.rec.join(format!("{driver}.sarif"));
        let sarif = json!({"version": "2.1.0", "runs": [
            {"tool": {"driver": {"name": driver, "version": "1", "rules": []}}, "results": [result(driver)]}
        ]});
        fs::write(&report, sarif.to_string()).unwrap();
        crate::history::security::ingest(&w.root, &report, &["go.mod".to_string()]).unwrap();
    };
    ingest("osv-scanner");
    fs::write(w.root.join("go.mod"), "module example.test/app\n\ngo 1.23\n").unwrap();
    ingest("Trivy");

    let rows = w.status();
    let state = |scanner: &str| {
        let row = scanner_row(&rows, scanner);
        (row["state"].as_str().map(str::to_string), row["because"].as_str().map(str::to_string), row["findings"].as_u64(), row["merged"].as_u64())
    };
    assert_eq!(state("trivy"), (Some("fresh".to_string()), None, Some(1), Some(0)), "{rows}");
    assert_eq!(state("osv-scanner"), (Some("stale".to_string()), Some("files_changed".to_string()), Some(0), Some(1)), "{rows}");
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
    assert_eq!(status["other_reports"], json!([{"scanner": "ESLint", "driver": "ESLint", "state": "fresh", "snapshot_head": semgrep["snapshot_head"], "version": "1.7.10", "findings": 0, "suppressed": 0, "not_fresh": 0, "test": 0, "dev": 0, "needs_review": 0, "merged": 0, "installed": null, "install": null, "last_failure": null}]));
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
    let parse = |args: [Option<&str>; 6]| FindingsFilter::parse(args[0], args[1], args[2], args[3], args[4], args[5], None).unwrap();
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
    let bad = FindingsFilter::parse(Some("severe"), None, None, None, None, None, None).unwrap_err();
    assert!(bad.to_string().contains("severe"), "{bad}");
}

/// S-768: an `unrated` finding is listed with its reason and is filterable.
/// `--severity unrated` lists the unrated findings alone; a rated least
/// severity compares it as a gate does, as `medium`, so `medium` lists it and
/// `high` does not. A rated finding carries no reason.
#[test]
fn unrated_findings_carry_their_reason_and_are_filterable() {
    let w = World::new();
    let indexed = vec!["app/db.py".to_string()];
    for name in ["osv-scanner-2.7.0", "semgrep"] {
        crate::history::security::ingest(&w.root, &fixture(name), &indexed).unwrap();
    }
    let least = |severity: &str| -> Vec<(String, String)> {
        let filter = FindingsFilter::parse(Some(severity), None, None, None, None, None, None).unwrap();
        let mut got: Vec<(String, String)> = w.findings(&filter)["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (f["rule_id"].as_str().unwrap().to_string(), f["severity"].as_str().unwrap().to_string()))
            .collect();
        got.sort();
        got
    };
    let pair = |rule: &str, severity: &str| (rule.to_string(), severity.to_string());
    assert_eq!(least("unrated"), [pair("CVE-2021-3121", "unrated")]);
    assert_eq!(least("UNRATED"), least("unrated"), "tokens ignore case");
    let medium = least("medium");
    assert!(medium.contains(&pair("CVE-2021-3121", "unrated")), "{medium:?}");
    assert!(medium.contains(&pair("CVE-2022-24713", "high")), "{medium:?}");
    assert!(medium.iter().any(|(_, s)| s == "medium"), "a rated medium is listed too: {medium:?}");
    assert!(!least("high").iter().any(|(_, s)| s == "unrated"), "{:?}", least("high"));

    let all = w.findings(&FindingsFilter::default());
    for f in all["findings"].as_array().unwrap() {
        match f["severity"].as_str().unwrap() {
            "unrated" => assert_eq!(f["severity_reason"], "the scanner gave none", "{f}"),
            _ => assert!(f.get("severity_reason").is_none(), "a rating carries no reason: {f}"),
        }
    }
    let bad = FindingsFilter::parse(Some("unrate"), None, None, None, None, None, None).unwrap_err();
    assert!(bad.to_string().contains("unrated"), "the error lists the token: {bad}");
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
    use crate::jobs::{JobKind, JobState, AnalysisJobs};
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

    let jobs = AnalysisJobs::new();
    let (root, env) = (w.root.clone(), w.env());
    jobs.start(JobKind::SecurityScan, &w.root, move |_| {
        let indexed = vec!["app/db.py".to_string()];
        let summary = scan(&root, &env, crate::tools::Manifest::builtin(), false, &ScanRequest::full(), &mut |_| {}, |report, _| {
            crate::history::security::ingest(&root, report, &indexed)
        })?;
        Ok(serde_json::to_value(summary)?)
    })
    .unwrap();

    let deadline = Instant::now() + crate::perf::scaled(Duration::from_secs(20));
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
        let gone_by = Instant::now() + crate::perf::scaled(Duration::from_secs(10));
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
    use crate::jobs::{JobKind, JobState, AnalysisJobs};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    let w = World::new();
    w.stub("semgrep", Some(&fixture("semgrep")), 0, "");
    w.stub("bandit", Some(&w.clean_bandit()), 0, "");
    crate::config::record_scan_consent(Some(&w.user), &w.root, &["semgrep", "bandit"]).unwrap();

    let jobs = Arc::new(AnalysisJobs::new());
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

    let deadline = Instant::now() + crate::perf::scaled(Duration::from_secs(20));
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
        let status = status(&self.root, &self.env(), manifest, TODAY.unwrap(), super::VersionProbes::Every, true, no_test_symbols).unwrap();
        let listings = super::listings(&self.root, &self.env(), manifest, TODAY.unwrap(), no_test_symbols).unwrap();
        super::MemberSecurity::from_reads(status, &listings)
    }
}

/// Reports stored but none fresh: the member is `stale` and covers no category
/// — an old scan says nothing about the files it never read — yet its findings
/// are counted by their own freshness ([S-760]): HEAD moved, but the file each
/// sits in did not, so both count and none is excluded.
///
/// [S-760]: ../../../docs/planning/journal.md#s-760-one-freshness-definition-across-the-security-surfaces
#[test]
fn a_member_whose_reports_are_all_stale_counts_its_fresh_findings_and_covers_nothing() {
    let w = World::new();
    crate::history::security::ingest(&w.root, &fixture("semgrep"), &["app/db.py".to_string()]).unwrap();
    fs::write(w.root.join("app/more.py"), "x = 1\n").unwrap();
    git(&w.root, &["add", "."]);
    git(&w.root, &["commit", "-q", "-m", "more"]);

    let rollup = w.rollup();
    assert_eq!(rollup.scan, super::MemberScan::Stale);
    assert_eq!(rollup.findings.map(|c| c.total), Some(2), "db.py is unchanged, so both findings are fresh");
    assert_eq!((rollup.scanners_fresh, rollup.excluded.not_fresh, rollup.excluded.suppressed), (0, 0, 0));
    assert_eq!((rollup.reports_fresh, rollup.reports_stored), (0, 1), "the one report is not fresh at the new HEAD");
    assert_eq!(rollup.owasp.len(), 2, "both editions");
    for edition in &rollup.owasp {
        assert!(edition.categories.iter().all(|c| c.findings.is_none()), "{edition:?}");
    }
}

/// A stored report from a scanner the config now disables is still counted
/// finding by finding — its findings are on the Security page and in Files &
/// Risk, so the roll-up cannot read fewer — and it stays in the reports
/// denominator, though the disabled scanner leaves the scanners' one. A category
/// Semgrep covers reads its count; one only Bandit could see is not covered.
#[test]
fn a_disabled_scanner_s_stored_findings_are_counted_by_their_own_freshness() {
    let w = World::new();
    let indexed = ["app/db.py".to_string()];
    crate::history::security::ingest(&w.root, &fixture("semgrep"), &indexed).unwrap();
    crate::history::security::ingest(&w.root, &fixture("bandit"), &indexed).unwrap();
    w.repo_config("[security_scan.bandit]\nenabled = false\n");

    let rollup = w.rollup();
    assert_eq!(rollup.scan, super::MemberScan::Fresh);
    let counts = rollup.findings.expect("a fresh member has a count");
    assert_eq!(counts.total, 4, "Semgrep's two and Bandit's two: {counts:?}");
    assert_eq!(counts.total, counts.critical + counts.high + counts.medium + counts.low + counts.info);
    assert_eq!((rollup.excluded.not_fresh, rollup.excluded.suppressed), (0, 0));
    assert_eq!((rollup.scanners_fresh, rollup.reports_fresh, rollup.reports_stored), (1, 1, 2));
    assert_eq!(rollup.scanners_enabled, REGISTRY.len() - 1, "every registered scanner but Bandit");
    let injection = rollup.owasp[0].categories.iter().find(|c| c.id == "A05:2025").unwrap();
    assert_eq!(injection.findings, Some(3), "Semgrep's two and one of Bandit's: {injection:?}");
    assert_eq!(rollup.owasp[0].unclassified, 0, "every counted finding sits under a category");
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
/// `stale`, never `never_run`, and counts `0` — a clean report is not an absent
/// one ([BR-66]).
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
    assert_eq!((rollup.findings.map(|c| c.total), rollup.excluded.not_fresh), (Some(0), 0));
    assert_eq!(w.files_and_risk(), Some((0, 0)), "a stored clean report is a column of zeros, not an absent column");
}

/// A member whose only stored report is from a scanner its config now
/// disables has been scanned: it reads `stale`, and its findings still count
/// by their own freshness.
#[test]
fn a_member_whose_only_report_is_from_a_disabled_scanner_is_stale() {
    let w = World::new();
    crate::history::security::ingest(&w.root, &fixture("semgrep"), &["app/db.py".to_string()]).unwrap();
    w.repo_config("[security_scan.semgrep]\nenabled = false\n");

    let rollup = w.rollup();
    assert_eq!(rollup.scan, super::MemberScan::Stale);
    assert_eq!(rollup.findings.map(|c| c.total), Some(2));
    assert_eq!((rollup.excluded.not_fresh, rollup.excluded.suppressed), (0, 0));
    assert_eq!((rollup.scanners_fresh, rollup.reports_fresh, rollup.reports_stored), (0, 0, 1));
}

// ── S-760: one freshness definition across the security surfaces ─────────────

impl World {
    /// Ingest the Semgrep fixture (two findings, both in `app/db.py`) as a
    /// `--path app/db.py` run alone: no full scan stands behind it, so the
    /// scanner reads `stale` with `scoped_only` ([FR-SC-17]).
    ///
    /// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
    fn ingest_scoped_only(&self) {
        let paths = ["app/db.py".to_string()];
        crate::history::security::ingest_scoped(
            &self.root,
            &fixture("semgrep"),
            &paths,
            Some(crate::history::security::IngestScope::replacing(&paths)),
        )
        .unwrap();
    }

    /// What the Security page states as its count: the tally of every stored
    /// finding, as [`Engine::security_page`](crate::Engine::security_page)
    /// takes it off the unfiltered listing.
    fn page_counts(&self) -> super::FindingTally {
        let no_graph = |listed: &[&SecurityFinding]| {
            Ok(listed.iter().map(|f| FindingContext::unbound(f.start_line, UnboundBecause::NoSymbol)).collect())
        };
        let listed = findings(&self.root, &self.env(), crate::tools::Manifest::builtin(), &FindingsFilter::default(), TODAY.unwrap(), no_test_symbols, no_graph).unwrap();
        super::FindingTally::of(&listed.findings)
    }

    /// What Files & Risk reads for this project: every file's counted findings
    /// summed, and the stale ones apart. Each finding's scope is the path
    /// rule's: no graph is behind this project.
    fn files_and_risk(&self) -> Option<(usize, usize)> {
        let cells = self.cells()?;
        Some((cells.values().map(|c| c.count).sum(), cells.values().map(|c| c.stale).sum()))
    }

    /// Files & Risk's cells, each finding classified as the engine classifies
    /// it, by the path rule alone here.
    fn cells(&self) -> Option<std::collections::BTreeMap<String, crate::history::PathFindings>> {
        crate::history::security::path_findings(&self.root, TODAY.unwrap(), |stored| {
            Ok(super::scope::scopes(&self.root, stored, no_test_symbols)?.into_iter().map(|s| s.code_scope).collect())
        })
        .unwrap()
    }
}

/// The owner's report (review D6): a project whose only report is a `--path`
/// scan read "2 MEDIUM" on its page and "no fresh scan" in the roll-up. Its two
/// findings sit in a file nothing has touched, so each is fresh, and every
/// surface reads that one count.
#[test]
fn a_scoped_only_project_reads_one_count_on_every_surface() {
    let w = World::new();
    w.ingest_scoped_only();

    let rollup = w.rollup();
    assert_eq!(rollup.scan, super::MemberScan::Stale, "the report itself is not fresh: scoped_only");
    let counts = rollup.findings.expect("a stored report is counted from, finding by finding");
    assert_eq!(counts.total, 2, "{counts:?}");
    assert_eq!((rollup.excluded.not_fresh, rollup.excluded.suppressed), (0, 0));
    assert_eq!((rollup.reports_fresh, rollup.reports_stored), (0, 1), "the roll-up states the report's own freshness apart");

    let page = w.page_counts();
    assert_eq!(Some(page.counted), rollup.findings, "the page's count is the roll-up row's, severity by severity");
    assert_eq!(page.excluded, rollup.excluded);
    assert_eq!(w.files_and_risk(), Some((2, 0)), "Files & Risk reads the same two");
}

/// A finding whose file moved since the scan is stale on every surface, and
/// the roll-up tallies it apart rather than counting it.
#[test]
fn a_finding_in_an_edited_file_is_left_out_of_every_count() {
    let w = World::new();
    w.ingest_scoped_only();
    fs::write(w.root.join("app/db.py"), "import sqlite3  # edited\n").unwrap();

    let rollup = w.rollup();
    assert_eq!(rollup.findings.map(|c| c.total), Some(0), "stored, scanned, and nothing current in it");
    assert_eq!((rollup.excluded.not_fresh, rollup.excluded.suppressed), (2, 0));
    let page = w.page_counts();
    assert_eq!((page.counted.total, page.excluded), (0, rollup.excluded));
    assert_eq!(w.files_and_risk(), Some((0, 2)));
}

/// A suppressed finding is left out of every count, and stated apart, whatever
/// the state of its file.
#[test]
fn a_suppressed_finding_is_left_out_of_the_page_and_the_roll_up_alike() {
    let w = World::new();
    w.ingest_scoped_only();
    let fingerprint = w.findings(&FindingsFilter::default())["findings"][0]["fingerprint"].as_str().unwrap().to_string();
    crate::history::security::suppress(&w.root, &fingerprint, "accepted", None, TODAY.unwrap()).unwrap();

    let rollup = w.rollup();
    assert_eq!(rollup.findings.map(|c| c.total), Some(1));
    assert_eq!((rollup.excluded.not_fresh, rollup.excluded.suppressed), (0, 1));
    let page = w.page_counts();
    assert_eq!(Some(page.counted), rollup.findings);
    assert_eq!(page.excluded, rollup.excluded);
    assert_eq!(w.files_and_risk(), Some((1, 0)));
}

/// A path entry suppresses every finding under it on every surface: `status`
/// lists each entry with its count and the scanner row counts them apart,
/// `findings` labels each with the entry's reason and selector, and a glob
/// matching nothing is listed orphaned ([S-770]).
///
/// [S-770]: ../../../docs/planning/journal.md#s-770-a-suppression-can-cover-a-path
#[test]
fn a_path_entry_is_counted_in_status_and_labels_every_finding_it_covers() {
    let w = World::new();
    w.ingest_scoped_only();
    let stored = w.findings(&FindingsFilter::default())["total"].as_u64().unwrap();
    assert!(stored >= 2, "the fixture stores several findings under app/");
    fs::create_dir_all(w.root.join(".logos")).unwrap();
    fs::write(
        w.root.join(crate::history::security::IGNORE_FILE),
        "[[ignore]]\npath = \"app\"\nreason = \"fixture app\"\n\n[[ignore]]\npath = \"tests/fixtures\"\nreason = \"none yet\"\n",
    )
    .unwrap();

    let status = w.status();
    let entries: Vec<(&str, u64, bool)> = status["suppressions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["entry"]["path"].as_str().unwrap(), e["suppressed"].as_u64().unwrap(), e["orphaned"].as_bool().unwrap()))
        .collect();
    assert_eq!(entries, [("app", stored, false), ("tests/fixtures", 0, true)]);
    let row = scanner_row(&status, "semgrep");
    assert_eq!((row["findings"].as_u64(), row["suppressed"].as_u64()), (Some(0), Some(stored)));

    let listed = w.findings(&FindingsFilter::default());
    for finding in listed["findings"].as_array().unwrap() {
        assert_eq!(
            finding["suppression"],
            serde_json::json!({"state": "suppressed", "reason": "fixture app", "expires": null, "by": {"path": "app"}})
        );
    }
    assert_eq!(listed["suppressions"], status["suppressions"], "the page and status audit the entries alike");
    let rollup = w.rollup();
    assert_eq!((rollup.findings.map(|c| c.total), rollup.excluded.suppressed), (Some(0), stored as usize));
}

impl World {
    /// The fingerprint of the stored finding classified under `category`.
    fn fingerprint_under(&self, category: &str) -> String {
        let listed = self.findings(&FindingsFilter::default());
        listed["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["owasp"].as_array().unwrap().iter().any(|c| c == category))
            .unwrap_or_else(|| panic!("a stored finding under {category}: {listed}"))["fingerprint"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

/// Nothing stored is `never_run`, which has no count at all — never a count of
/// zero ([BR-66]) — and no Files & Risk column.
///
/// [BR-66]: ../../../docs/specs/software-spec.md#328-security-findings
#[test]
fn a_never_scanned_member_has_no_count_and_no_column() {
    let w = World::new();
    let rollup = w.rollup();
    assert_eq!(rollup.scan, super::MemberScan::NeverRun);
    assert_eq!((rollup.findings, rollup.reports_stored, rollup.reports_fresh), (None, 0, 0));
    assert_eq!(w.files_and_risk(), None);
}

/// A finding both suppressed and in an edited file is left out once, as
/// suppressed — "whatever their file's state" — and Files & Risk, which skips
/// suppressed findings, does not tally it as stale.
#[test]
fn a_suppressed_finding_in_an_edited_file_is_counted_apart_as_suppressed() {
    let w = World::new();
    w.ingest_scoped_only();
    let fingerprint = w.fingerprint_under("A05:2025");
    crate::history::security::suppress(&w.root, &fingerprint, "accepted", None, TODAY.unwrap()).unwrap();
    fs::write(w.root.join("app/db.py"), "import sqlite3  # edited\n").unwrap();

    let rollup = w.rollup();
    assert_eq!((rollup.excluded.not_fresh, rollup.excluded.suppressed), (1, 1));
    assert_eq!(w.page_counts().excluded, rollup.excluded);
    assert_eq!(w.files_and_risk(), Some((0, 1)), "only the unsuppressed one is stale there");
}

/// An expired entry no longer suppresses: its finding counts while its file is
/// unchanged, and is stale — not suppressed — once the file moves.
#[test]
fn an_expired_suppression_does_not_hold_the_finding_out() {
    let w = World::new();
    w.ingest_scoped_only();
    let fingerprint = w.fingerprint_under("A05:2025");
    let entered = NaiveDate::from_ymd_opt(2026, 10, 1);
    let expires = NaiveDate::from_ymd_opt(2026, 10, 10);
    crate::history::security::suppress(&w.root, &fingerprint, "accepted until the 10th", expires, entered.unwrap()).unwrap();

    let rollup = w.rollup();
    assert_eq!(rollup.findings.map(|c| c.total), Some(2), "TODAY is past the expiry, so both count");
    assert_eq!((rollup.excluded.not_fresh, rollup.excluded.suppressed), (0, 0));

    fs::write(w.root.join("app/db.py"), "import sqlite3  # edited\n").unwrap();
    let rollup = w.rollup();
    assert_eq!((rollup.excluded.not_fresh, rollup.excluded.suppressed), (2, 0), "stale, not suppressed");
    assert_eq!(w.page_counts().excluded, rollup.excluded);
}

/// A category a fresh report covers counts only counted findings: a suppressed
/// one is out of it, and so is a stale finding of another scanner's report.
#[test]
fn a_category_count_holds_only_counted_findings() {
    let w = World::new();
    w.with_views();
    // Semgrep's report is fresh (views.py), with one finding under no category.
    let plain = w.rec.join("plain.sarif");
    fs::write(&plain, semgrep_report(&[("app/views.py", "r.plain", 1)])).unwrap();
    let indexed = ["app/db.py".to_string(), "app/views.py".to_string()];
    crate::history::security::ingest(&w.root, &plain, &indexed).unwrap();
    // Bandit's findings: B608 in db.py (under A05) and B105 in app/settings.py,
    // which is not indexed, so it is bound to no file.
    crate::history::security::ingest(&w.root, &fixture("bandit"), &indexed).unwrap();
    let a05 = |r: &super::MemberSecurity| r.owasp[0].categories.iter().find(|c| c.id == "A05:2025").unwrap().findings;
    assert_eq!(a05(&w.rollup()), Some(1), "Bandit's injection finding, while both reports are fresh");

    // Suppressed: out of the category.
    let fingerprint = w.fingerprint_under("A05:2025");
    crate::history::security::suppress(&w.root, &fingerprint, "accepted", None, TODAY.unwrap()).unwrap();
    assert_eq!(a05(&w.rollup()), Some(0), "a suppressed finding is not in its category's count");
    crate::history::security::unsuppress(&w.root, &fingerprint).unwrap();

    // Stale: db.py moves, so Bandit's report is stale; Semgrep's stays fresh and
    // still covers the category.
    fs::write(w.root.join("app/db.py"), "import sqlite3  # edited\n").unwrap();
    let rollup = w.rollup();
    assert_eq!(rollup.scan, super::MemberScan::Fresh);
    assert_eq!(a05(&rollup), Some(0), "a stale finding is not in its category's count");
    assert_eq!(rollup.excluded.not_fresh, 1, "Bandit's db.py finding; its unbound one has no file to have moved");
    assert_eq!(rollup.owasp[0].unclassified, 1, "only Semgrep's plain finding is counted, under no category");
}

/// A finding bound to no file counts on the page and in the roll-up, and has no
/// row on Files & Risk, which is per file — the one place the three differ, and
/// the manual says so.
#[test]
fn a_finding_bound_to_no_file_counts_on_the_page_and_in_the_roll_up_only() {
    let w = World::new();
    crate::history::security::ingest(&w.root, &fixture("semgrep"), &[]).unwrap();

    let rollup = w.rollup();
    assert_eq!(rollup.findings.map(|c| c.total), Some(2), "no file to compare, so each counts");
    assert_eq!(Some(w.page_counts().counted), rollup.findings);
    assert_eq!(w.files_and_risk(), Some((0, 0)), "and joins no file's cell");
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

/// FR-SC-26: the coverage fact rides on every surface — each scanner's row in
/// the scan summary (the CLI's and MCP's `security scan` payload, and the
/// scan job's progress), and each installed scanner's row in `security
/// status` (which the Security page reads) — as `ignored_paths` with a
/// `coverage_note` exactly when it is `scanned`. A scoped re-scan of an
/// ignored path states none for the scanner that read only that path.
#[test]
fn every_scan_surface_states_what_each_scanner_did_with_the_ignored_directories() {
    let w = World::new();
    fs::write(w.root.join(".gitignore"), "/target/\n").unwrap();
    fs::create_dir_all(w.root.join("target")).unwrap();
    fs::write(w.root.join("target/gen.py"), "x\n").unwrap();
    w.stub("semgrep", Some(&fixture("semgrep")), 0, "");
    w.stub("osv-scanner", Some(&fixture("osv-scanner")), 0, "");
    w.stub("gitleaks", Some(&fixture("gitleaks")), 0, "");
    let mut finished = Vec::new();
    let summary = w.scan_reporting(true, &mut |p| {
        if let ScanProgress::Finished(run) = p {
            finished.push(serde_json::to_value(run).unwrap());
        }
    });
    let payload = serde_json::to_value(summary.expect("the scan runs")).unwrap();
    for name in ["semgrep", "osv-scanner"] {
        let row = scanner_row(&payload, name);
        assert_eq!(row["ignored_paths"], "excluded", "{row}");
        assert!(row.get("coverage_note").is_none(), "no note when excluded: {row}");
    }
    let gitleaks = scanner_row(&payload, "gitleaks");
    assert_eq!(gitleaks["ignored_paths"], "scanned", "{gitleaks}");
    let (kind, note) = super::registry::scanner("gitleaks").unwrap().reads_ignored.unwrap();
    assert_eq!(gitleaks["coverage_note"], note);
    assert_eq!(gitleaks["coverage_kind"], "history", "the kind the page maps to a marker: {gitleaks}");
    assert_eq!(serde_json::to_value(kind).unwrap(), "history");
    // The names the page maps to a marker, one per kind: the wire's spelling.
    let wire = |k: super::CoverageKind| serde_json::to_value(k).unwrap();
    assert_eq!(
        [wire(super::CoverageKind::Unproven), wire(super::CoverageKind::WholeProject), wire(super::CoverageKind::Partial)],
        ["unproven", "whole_project", "partial"]
    );
    for name in ["semgrep", "osv-scanner"] {
        assert!(scanner_row(&payload, name).get("coverage_kind").is_none(), "no kind when excluded: {name}");
    }
    for name in ["trivy", "bandit", "codeql"] {
        assert!(scanner_row(&payload, name).get("ignored_paths").is_none(), "{name} did not run");
    }
    for row in &finished {
        let summary_row = scanner_row(&payload, row["scanner"].as_str().unwrap());
        assert_eq!(row.get("ignored_paths"), summary_row.get("ignored_paths"), "the job's progress carries it: {row}");
    }

    let status = w.status();
    assert_eq!(scanner_row(&status, "semgrep")["ignored_paths"], "excluded");
    assert_eq!(scanner_row(&status, "gitleaks")["ignored_paths"], "scanned");
    assert!(scanner_row(&status, "gitleaks")["coverage_note"].is_string());
    assert_eq!(scanner_row(&status, "gitleaks")["coverage_kind"], "history", "status carries the kind too");
    assert!(scanner_row(&status, "semgrep").get("coverage_kind").is_none(), "no kind when excluded");
    assert!(scanner_row(&status, "trivy").get("ignored_paths").is_none(), "not installed here");
    // A readout that does not ask for it walks no tree, whatever it probes.
    let without = status_without_coverage(&w);
    assert!(without.scanners.iter().all(|row| row.coverage.is_none()), "{without:?}");
    // Installed but disabled, or refused by the repository's config: a scan
    // would not run it, so its row states nothing.
    w.stub("bandit", Some(&w.clean_bandit()), 0, "");
    w.stub("checkov", None, 0, "");
    fs::write(w.root.join(".checkov.yaml"), "external-checks-dir:\n  - checks\n").unwrap();
    w.repo_config("[security_scan.bandit]\nenabled = false\n");
    let status = w.status();
    for name in ["bandit", "checkov"] {
        assert!(scanner_row(&status, name).get("ignored_paths").is_none(), "{name}: {}", scanner_row(&status, name));
    }
    assert_eq!(scanner_row(&status, "semgrep")["ignored_paths"], "excluded");
    fs::remove_file(w.root.join(".checkov.yaml")).unwrap();
    fs::remove_file(w.root.join(".logos/config.toml")).unwrap();

    let scoped = w
        .scan_request(false, &ScanRequest::new(strings(&["semgrep", "osv-scanner"]), strings(&["target"])), &mut |_| {})
        .expect("a scoped scan runs");
    let scoped = serde_json::to_value(scoped).unwrap();
    let semgrep = scanner_row(&scoped, "semgrep");
    assert_eq!(semgrep["scope"]["paths"], json!(["target"]), "{semgrep}");
    assert!(semgrep.get("ignored_paths").is_none(), "a run on named paths states no coverage: {semgrep}");
    assert!(w.started().iter().any(|line| line.ends_with("--exclude .logos target")), "{:?}", w.started());
    assert_eq!(scanner_row(&scoped, "osv-scanner")["ignored_paths"], "excluded", "it ran whole");
}

fn status_without_coverage(w: &World) -> super::SecurityStatus {
    status(&w.root, &w.env(), crate::tools::Manifest::builtin(), TODAY.unwrap(), super::VersionProbes::Every, false, no_test_symbols).unwrap()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

// ── Test code is labelled, listed apart and counted apart (S-795, FR-SC-28) ──

/// A Semgrep report with one result per `(uri, rule, line, level)`.
fn rated_report(results: &[(&str, &str, i64, &str)]) -> String {
    let results: Vec<Value> = results
        .iter()
        .map(|(uri, rule, line, level)| {
            json!({
                "ruleId": rule,
                "level": level,
                "message": {"text": "tainted"},
                "locations": [{"physicalLocation": {"artifactLocation": {"uri": uri}, "region": {"startLine": line}}}]
            })
        })
        .collect();
    json!({"version": "2.1.0", "runs": [{"tool": {"driver": {"name": "Semgrep OSS", "semanticVersion": "1.85.0", "rules": []}}, "results": results}]})
        .to_string()
}

impl World {
    /// Index `app/db.py`, `tests/test_db.py` and the fixture
    /// `tests/fixtures/axum/Dockerfile`, then ingest one Semgrep report with
    /// an `error` finding in each and a `note` in `app/db.py`: two production
    /// findings and two in test code, one of each by the path conventions and
    /// the fixture segment.
    fn with_test_code(&self) {
        for (path, text) in [("tests/test_db.py", "import sqlite3\n"), ("tests/fixtures/axum/Dockerfile", "FROM rust\n")] {
            let full = self.root.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, text).unwrap();
        }
        git(&self.root, &["add", "."]);
        git(&self.root, &["commit", "-q", "-m", "tests"]);
        let report = self.rec.join("scoped.sarif");
        fs::write(
            &report,
            rated_report(&[
                ("app/db.py", "sql", 1, "error"),
                ("app/db.py", "weak", 1, "note"),
                ("tests/test_db.py", "sql", 1, "error"),
                ("tests/fixtures/axum/Dockerfile", "root-user", 1, "error"),
            ]),
        )
        .unwrap();
        let indexed = ["app/db.py", "tests/test_db.py", "tests/fixtures/axum/Dockerfile"].map(str::to_string);
        crate::history::security::ingest(&self.root, &report, &indexed).unwrap();
    }
}

fn scoped(filter: &FindingsFilter, w: &World) -> (Vec<(String, String, Value)>, u64) {
    let listed = w.findings(filter);
    let rows = listed["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["path"].as_str().unwrap().to_string(), f["code_scope"].as_str().unwrap().to_string(), f["code_scope_reason"].clone()))
        .collect();
    (rows, listed["hidden_by_scope"].as_u64().unwrap())
}

/// Every finding carries its scope and reason; no scope lists production and
/// states how many it hid; `test` and `all` narrow exactly, and the scope
/// intersects with every other filter — its hidden count is of what they kept.
#[test]
fn a_finding_in_test_code_is_labelled_and_listed_apart_by_the_scope() {
    let w = World::new();
    w.with_test_code();
    let parse = |severity: Option<&str>, path: Option<&str>, scope: Option<&str>| {
        FindingsFilter::parse(severity, None, None, path, None, None, scope).unwrap()
    };

    let (default, hidden) = scoped(&parse(None, None, None), &w);
    let production = |p: &str| (p.to_string(), "production".to_string(), Value::Null);
    assert_eq!(default, [production("app/db.py"), production("app/db.py")]);
    assert_eq!(hidden, 2, "the two findings in test code are hidden, and said to be");
    assert_eq!(w.findings(&parse(None, None, None))["scope"], "production");

    let (test, hidden) = scoped(&parse(None, None, Some("test")), &w);
    assert_eq!(
        test,
        [
            ("tests/fixtures/axum/Dockerfile".to_string(), "test".to_string(), json!("test_path")),
            ("tests/test_db.py".to_string(), "test".to_string(), json!("test_path")),
        ],
        "store order"
    );
    assert_eq!(hidden, 2, "the production ones are what `test` hides");
    let (all, hidden) = scoped(&parse(None, None, Some("all")), &w);
    assert_eq!((all.len(), hidden), (4, 0));
    assert_eq!(scoped(&FindingsFilter::default(), &w), (all, 0), "an unset filter keeps every scope");

    // Intersection: severity × scope, path × scope. A finding another filter
    // dropped is never counted as hidden by the scope.
    let (high_prod, hidden) = scoped(&parse(Some("high"), None, None), &w);
    assert_eq!((high_prod, hidden), (vec![production("app/db.py")], 2), "the note is dropped by severity, not hidden");
    let (fixture, hidden) = scoped(&parse(None, Some("tests/fixtures"), None), &w);
    assert_eq!((fixture.len(), hidden), (0, 1), "a test-code path under the default scope lists nothing and says one was hidden");
    let (fixture, hidden) = scoped(&parse(None, Some("tests/fixtures"), Some("test")), &w);
    assert_eq!((fixture.len(), hidden), (1, 0));
    assert_eq!(scoped(&parse(Some("high"), Some("tests"), Some("all")), &w).0.len(), 2, "deterministic and intersecting");

    let bad = FindingsFilter::parse(None, None, None, None, None, None, Some("unit")).unwrap_err();
    assert!(bad.to_string().contains("\"unit\""), "{bad}");
}

/// One counting rule behind every count: `status`, the page, the roll-up and
/// Files & Risk count the same production findings and state the test-code
/// remainder; the excluded buckets — suppressed, not fresh, test, each finding
/// in the first that holds — partition the rest with the counted ones.
#[test]
fn every_count_leaves_test_code_out_and_the_buckets_partition_the_rest() {
    let w = World::new();
    w.with_test_code();
    // Suppress one production finding and one test-code finding; edit the
    // test file so its unsuppressed finding is stale.
    let listed = w.findings(&FindingsFilter::default());
    let fingerprint = |path: &str, rule: &str| {
        listed["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == path && f["rule_id"] == rule)
            .unwrap()["fingerprint"]
            .as_str()
            .unwrap()
            .to_string()
    };
    crate::history::security::suppress(&w.root, fingerprint("app/db.py", "weak"), "accepted", None, TODAY.unwrap()).unwrap();
    crate::history::security::suppress(&w.root, fingerprint("tests/fixtures/axum/Dockerfile", "root-user"), "fixture", None, TODAY.unwrap())
        .unwrap();
    fs::write(w.root.join("tests/test_db.py"), "import sqlite3  # edited\n").unwrap();

    // 4 stored: 1 counted (app/db.py sql), 2 suppressed (one per scope), 1
    // not fresh (the edited test file, stale before it is test code).
    let status = w.status();
    let row = &scanner_row(&status, "semgrep");
    let partition = |v: &Value| ["findings", "suppressed", "not_fresh", "test"].map(|k| v[k].as_u64().unwrap());
    assert_eq!(partition(row), [1, 2, 1, 0], "{row}");

    let page = w.page_counts();
    assert_eq!(page.counted.total, 1);
    assert_eq!((page.excluded.suppressed, page.excluded.not_fresh, page.excluded.test), (2, 1, 0));
    let rollup = w.rollup();
    assert_eq!((rollup.findings.unwrap(), rollup.excluded), (page.counted, page.excluded), "one rule, two surfaces");

    // Restore the test file: its finding is fresh again, and now test code.
    fs::write(w.root.join("tests/test_db.py"), "import sqlite3\n").unwrap();
    assert_eq!(partition(scanner_row(&w.status(), "semgrep")), [1, 2, 0, 1]);
    let page = w.page_counts();
    assert_eq!((page.counted.total, page.excluded.suppressed, page.excluded.not_fresh, page.excluded.test), (1, 2, 0, 1));
    let rollup = w.rollup();
    assert_eq!(rollup.excluded, page.excluded);
    // No result here carries a CWE, so the one counted finding is unclassified
    // in each edition, and the test-code one adds to no edition's figures.
    for edition in &rollup.owasp {
        assert_eq!(edition.unclassified, 1, "{:?}", edition.edition);
        assert!(edition.categories.iter().all(|c| c.findings.unwrap_or(0) == 0), "{:?}", edition.edition);
    }
    let cells = w.cells().unwrap();
    assert_eq!((cells["app/db.py"].count, cells["app/db.py"].test), (1, 0));
    assert_eq!((cells["tests/test_db.py"].count, cells["tests/test_db.py"].test), (0, 1), "Files & Risk states it apart");
    assert!(!cells.contains_key("tests/fixtures/axum/Dockerfile"), "a suppressed finding joins no cell");
    let stored = w.findings(&FindingsFilter::default())["total"].as_u64().unwrap();
    assert_eq!(partition(scanner_row(&w.status(), "semgrep")).iter().sum::<u64>(), stored, "every finding once");
}

// ── A finding its scanner marks low-confidence reads "needs review" (S-817, FR-SC-31) ──

/// A Semgrep report whose rules carry the confidence tag and the constant
/// `precision: very-high` Semgrep writes on every rule: `sql` is `LOW
/// CONFIDENCE`, `cookie` is `MEDIUM CONFIDENCE`, `weak` names no rule at all.
/// Each result is `(uri, rule, line)`, `error` level, under CWE-89 except `weak`.
fn confidence_report(results: &[(&str, &str, i64)]) -> String {
    let rule = |id: &str, tag: &str| {
        json!({"id": id, "properties": {"precision": "very-high", "tags": ["CWE-89: SQL Injection", tag, "security"]}})
    };
    let results: Vec<Value> = results
        .iter()
        .map(|(uri, rule, line)| {
            json!({
                "ruleId": rule,
                "level": "error",
                "message": {"text": "tainted"},
                "locations": [{"physicalLocation": {"artifactLocation": {"uri": uri}, "region": {"startLine": line}}}]
            })
        })
        .collect();
    json!({"version": "2.1.0", "runs": [{"tool": {"driver": {
        "name": "Semgrep OSS", "semanticVersion": "1.85.0",
        "rules": [rule("sql", "LOW CONFIDENCE"), rule("cookie", "MEDIUM CONFIDENCE")]}}, "results": results}]})
    .to_string()
}

impl World {
    /// Two production findings in `app/db.py` — `sql` (low confidence) and
    /// `cookie` (medium, beside `precision: very-high`) — a `weak` one with
    /// no rule, and a low-confidence `sql` in `tests/test_db.py`.
    fn with_confidence(&self) {
        let full = self.root.join("tests/test_db.py");
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, "import sqlite3\n").unwrap();
        git(&self.root, &["add", "."]);
        git(&self.root, &["commit", "-q", "-m", "tests"]);
        let report = self.rec.join("confidence.sarif");
        fs::write(
            &report,
            confidence_report(&[("app/db.py", "sql", 1), ("app/db.py", "cookie", 1), ("app/db.py", "weak", 1), ("tests/test_db.py", "sql", 1)]),
        )
        .unwrap();
        let indexed = ["app/db.py", "tests/test_db.py"].map(str::to_string);
        crate::history::security::ingest(&self.root, &report, &indexed).unwrap();
    }
}

/// The rule, confidence and `needs_review` of every listed finding, by path.
fn reviewed(listed: &Value) -> Vec<(String, String, String, bool)> {
    listed["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["path"].as_str().unwrap().to_string(),
                f["rule_id"].as_str().unwrap().to_string(),
                f["confidence"].as_str().unwrap().to_string(),
                f["needs_review"].as_bool().unwrap(),
            )
        })
        .collect()
}

/// The default scope leaves a low-confidence finding out and says how many;
/// `--scope all` lists it, labelled; `MEDIUM CONFIDENCE` over a constant
/// `precision: very-high` stays; a low-confidence finding in test code is
/// counted as test code, once.
#[test]
fn a_low_confidence_finding_reads_needs_review_and_the_default_scope_states_it_left_out() {
    let w = World::new();
    w.with_confidence();
    let parse = |scope: Option<&str>| FindingsFilter::parse(None, None, None, None, None, None, scope).unwrap();
    let row = |path: &str, rule: &str, confidence: &str, review: bool| (path.to_string(), rule.to_string(), confidence.to_string(), review);

    let default = w.findings(&parse(None));
    assert_eq!(
        reviewed(&default),
        [row("app/db.py", "cookie", "medium", false), row("app/db.py", "weak", "unstated", false)],
        "medium and unstated stay in the headline"
    );
    assert_eq!(
        (default["hidden_needs_review"].as_u64(), default["hidden_by_scope"].as_u64()),
        (Some(1), Some(1)),
        "one production finding needs review; the low-confidence one in test code is test code: {default}"
    );

    let all = w.findings(&parse(Some("all")));
    assert_eq!(
        reviewed(&all),
        [
            row("app/db.py", "cookie", "medium", false),
            row("app/db.py", "sql", "low", true),
            row("app/db.py", "weak", "unstated", false),
            row("tests/test_db.py", "sql", "low", true),
        ],
        "`--scope all` lists every finding, the low-confidence ones labelled"
    );
    assert_eq!((all["hidden_needs_review"].as_u64(), all["hidden_by_scope"].as_u64()), (Some(0), Some(0)));

    // The hidden count is of what the other filters kept: a severity no finding
    // reaches leaves nothing for the scope to hide, and one they all reach hides the one.
    let severe = |severity: &str, scope: Option<&str>| {
        FindingsFilter::parse(Some(severity), None, None, None, None, None, scope).unwrap()
    };
    let none = w.findings(&severe("critical", None));
    assert_eq!((none["hidden_needs_review"].as_u64(), none["hidden_by_scope"].as_u64()), (Some(0), Some(0)), "{none}");
    let high = w.findings(&severe("high", None));
    assert_eq!((high["hidden_needs_review"].as_u64(), high["hidden_by_scope"].as_u64()), (Some(1), Some(1)), "{high}");
    let by_path = FindingsFilter::parse(None, None, None, Some("tests"), None, None, None).unwrap();
    let tests_dir = w.findings(&by_path);
    assert_eq!((tests_dir["hidden_needs_review"].as_u64(), tests_dir["hidden_by_scope"].as_u64()), (Some(0), Some(1)), "{tests_dir}");

    let test = w.findings(&parse(Some("test")));
    assert_eq!(reviewed(&test), [row("tests/test_db.py", "sql", "low", true)]);
    assert_eq!((test["hidden_needs_review"].as_u64(), test["hidden_by_scope"].as_u64()), (Some(0), Some(3)));
    assert_eq!(
        reviewed(&w.findings(&FindingsFilter::default())).len(),
        4,
        "an unset filter keeps every finding, as every read-model built over it does"
    );
}

/// Every count leaves a needs-review finding out and states it; the excluded
/// buckets — suppressed, not fresh, test, needs review — still partition the
/// rest, each finding in the first that holds, and the figures agree across
/// `status`, the page, the roll-up and Files & Risk.
#[test]
fn every_count_states_needs_review_and_the_buckets_still_partition_the_rest() {
    let w = World::new();
    w.with_confidence();
    let partition = |v: &Value| ["findings", "suppressed", "not_fresh", "test", "needs_review"].map(|k| v[k].as_u64().unwrap());
    assert_eq!(partition(scanner_row(&w.status(), "semgrep")), [2, 0, 0, 1, 1], "cookie and weak count; sql in test code is test; sql needs review");

    // A report whose HEAD moved is `stale`, and counts the findings of files nothing
    // touched as a fresh one does — the Stale state states needs review too.
    git(&w.root, &["commit", "--allow-empty", "-q", "-m", "later"]);
    let stale = w.status();
    let row = scanner_row(&stale, "semgrep");
    assert_eq!(row["state"], "stale", "{row}");
    assert_eq!(partition(row), [2, 0, 0, 1, 1], "{row}");
    git(&w.root, &["reset", "-q", "--hard", "HEAD~1"]);

    let page = w.page_counts();
    assert_eq!((page.counted.total, page.excluded.test, page.excluded.needs_review), (2, 1, 1));
    let rollup = w.rollup();
    assert_eq!((rollup.findings.unwrap(), rollup.excluded), (page.counted, page.excluded), "one rule, two surfaces");
    let cells = w.cells().unwrap();
    assert_eq!((cells["app/db.py"].count, cells["app/db.py"].needs_review, cells["app/db.py"].test), (2, 1, 0));
    assert_eq!((cells["tests/test_db.py"].count, cells["tests/test_db.py"].needs_review, cells["tests/test_db.py"].test), (0, 0, 1));

    // The OWASP count under the default scope leaves it out too, and the
    // three figures partition `findings`: 1 production (cookie), 1 needs review.
    let all = w.findings(&FindingsFilter::default());
    let injection = all["categories"].as_array().unwrap().iter().find(|c| c["id"] == "A05:2025").unwrap();
    assert_eq!(
        (injection["state"]["findings"].as_u64(), injection["production_findings"].as_u64(), injection["needs_review_findings"].as_u64()),
        (Some(3), Some(1), Some(1)),
        "{injection}"
    );

    // A suppressed needs-review finding is suppressed first, and a stale one is stale first.
    let sql = all["findings"].as_array().unwrap().iter().find(|f| f["path"] == "app/db.py" && f["rule_id"] == "sql").unwrap();
    crate::history::security::suppress(&w.root, sql["fingerprint"].as_str().unwrap().to_string(), "accepted", None, TODAY.unwrap()).unwrap();
    assert_eq!(partition(scanner_row(&w.status(), "semgrep")), [2, 1, 0, 1, 0]);
    let stored = w.findings(&FindingsFilter::default())["total"].as_u64().unwrap();
    assert_eq!(partition(scanner_row(&w.status(), "semgrep")).iter().sum::<u64>(), stored, "every finding once");
}

// ── dev-only dependencies (FR-SC-30, S-810) ──────────────────────────────────

/// An npm lockfile (v3) marking `vite` and `esbuild` dev-only and `react` not.
const NPM_LOCK: &str = r#"{"lockfileVersion":3,"packages":{
  "":{"name":"web","version":"1.0.0"},
  "node_modules/vite":{"version":"5.0.0","dev":true},
  "node_modules/esbuild":{"version":"0.19.0","dev":true},
  "node_modules/react":{"version":"18.2.0"}}}"#;

impl World {
    /// An OSV-Scanner report over `web/package-lock.json` (vite and esbuild
    /// dev-only, react not) and a `Cargo.lock` that names a package called
    /// `vite` too: the Cargo one never guesses, so it reads production.
    fn with_dev_dependencies(&self) {
        fs::create_dir_all(self.root.join("web")).unwrap();
        fs::write(self.root.join("web/package-lock.json"), NPM_LOCK).unwrap();
        fs::write(self.root.join("Cargo.lock"), "# lockfile\n").unwrap();
        git(&self.root, &["add", "."]);
        git(&self.root, &["commit", "-q", "-m", "lockfiles"]);
        let uri = |file: &str| format!("file://{}/{file}", self.root.display());
        let result = |package: &str, advisory: &str, file: &str, level: &str| {
            json!({
                "ruleId": advisory,
                "level": level,
                "message": {"text": format!("Package '{package}' is vulnerable to '{advisory}'.")},
                // CWE-1104 (unmaintained third-party components) falls under A03:2025,
                // the category OSV-Scanner covers, so the categories carry a count.
                "properties": {"tags": ["CWE-1104"]},
                "locations": [{"physicalLocation": {"artifactLocation": {"uri": uri(file)}}}]
            })
        };
        let report = self.rec.join("dev-dependencies.sarif");
        fs::write(
            &report,
            json!({"version": "2.1.0", "runs": [{
                "tool": {"driver": {"name": "osv-scanner", "version": "1.9.2", "rules": []}},
                "results": [
                    result("vite@5.0.0", "GHSA-vite", "web/package-lock.json", "error"),
                    result("esbuild@0.19.0", "GHSA-esbuild", "web/package-lock.json", "warning"),
                    result("react@18.2.0", "GHSA-react", "web/package-lock.json", "warning"),
                    result("vite@5.0.0", "RUSTSEC-vite", "Cargo.lock", "error"),
                ]}]})
            .to_string(),
        )
        .unwrap();
        crate::history::security::ingest(&self.root, &report, &["app/db.py".to_string()]).unwrap();
    }
}

/// `(advisory, code_scope, reason)` of every listed finding, in store order.
fn dependency_scopes(listed: &Value) -> Vec<(String, String, Value)> {
    listed["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["rule_id"].as_str().unwrap().to_string(), f["code_scope"].as_str().unwrap().to_string(), f["code_scope_reason"].clone()))
        .collect()
}

/// A finding on a `"dev": true` package reads `dev`; any other lockfile finding
/// reads `production`. The default scope leaves `dev` out and states how many
/// ("N more in dev-only dependencies"), apart from the findings the scope hid;
/// `--scope all` lists it, labelled; `--scope test` lists test code alone.
#[test]
fn a_dev_only_dependency_finding_is_labelled_dev_and_the_default_scope_states_it_left_out() {
    let w = World::new();
    w.with_dev_dependencies();
    let parse = |severity: Option<&str>, scope: Option<&str>| FindingsFilter::parse(severity, None, None, None, None, None, scope).unwrap();
    let row = |rule: &str, scope: &str, reason: Value| (rule.to_string(), scope.to_string(), reason);
    let dev = |rule: &str| row(rule, "dev", json!("dev_dependency"));
    let production = |rule: &str| row(rule, "production", Value::Null);

    let default = w.findings(&parse(None, None));
    assert_eq!(dependency_scopes(&default), [production("GHSA-react"), production("RUSTSEC-vite")], "{default}");
    assert_eq!((default["hidden_dev"].as_u64(), default["hidden_by_scope"].as_u64()), (Some(2), Some(0)), "{default}");

    let all = w.findings(&parse(None, Some("all")));
    let mut listed = dependency_scopes(&all);
    listed.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(listed, [dev("GHSA-esbuild"), production("GHSA-react"), dev("GHSA-vite"), production("RUSTSEC-vite")], "{all}");
    assert_eq!((all["hidden_dev"].as_u64(), all["hidden_by_scope"].as_u64()), (Some(0), Some(0)), "`--scope all` hides nothing");

    let test = w.findings(&parse(None, Some("test")));
    assert_eq!(dependency_scopes(&test), []);
    assert_eq!((test["hidden_dev"].as_u64(), test["hidden_by_scope"].as_u64()), (Some(0), Some(4)), "`test` hides every finding: {test}");

    // The hidden count is of what the other filters kept: osv-scanner's `error` is high.
    let high = w.findings(&parse(Some("high"), None));
    assert_eq!(dependency_scopes(&high), [production("RUSTSEC-vite")]);
    assert_eq!(high["hidden_dev"], 1, "only the high dev one: {high}");
    assert_eq!(w.findings(&FindingsFilter::default())["findings"].as_array().unwrap().len(), 4, "an unset filter keeps every scope");
}

/// Every count leaves a `dev` finding out and states it; the excluded buckets —
/// suppressed, not fresh, test, dev, needs review — still partition the rest,
/// each finding in the first that holds, and `status`, the page and the
/// roll-up agree.
#[test]
fn every_count_states_dev_only_dependencies_and_the_buckets_still_partition_the_rest() {
    let w = World::new();
    w.with_dev_dependencies();
    let partition = |v: &Value| ["findings", "suppressed", "not_fresh", "test", "dev", "needs_review"].map(|k| v[k].as_u64().unwrap());
    assert_eq!(partition(scanner_row(&w.status(), "osv-scanner")), [2, 0, 0, 0, 2, 0], "react and the Cargo.lock one count; vite and esbuild are dev");

    // A report whose HEAD moved is `stale`, and counts a finding bound to no file
    // as a fresh one does — the Stale state states `dev` too.
    git(&w.root, &["commit", "--allow-empty", "-q", "-m", "later"]);
    let stale = w.status();
    let row = scanner_row(&stale, "osv-scanner");
    assert_eq!(row["state"], "stale", "{row}");
    assert_eq!(partition(row), [2, 0, 0, 0, 2, 0], "{row}");
    git(&w.root, &["reset", "-q", "--hard", "HEAD~1"]);

    // The OWASP count under the default scope leaves a dev finding out of
    // `production_findings` and states it as `dev_findings`, so production and
    // dev partition the category's `findings`.
    let before = w.findings(&FindingsFilter::default());
    let supply_chain = before["categories"].as_array().unwrap().iter().find(|c| c["id"] == "A03:2025").unwrap();
    assert_eq!(
        (
            supply_chain["state"]["findings"].as_u64(),
            supply_chain["production_findings"].as_u64(),
            supply_chain["dev_findings"].as_u64(),
            supply_chain["needs_review_findings"].as_u64()
        ),
        (Some(4), Some(2), Some(2), Some(0)),
        "{supply_chain}"
    );

    let page = w.page_counts();
    assert_eq!((page.counted.total, page.excluded.dev, page.excluded.test), (2, 2, 0));
    let rollup = w.rollup();
    assert_eq!((rollup.findings.unwrap(), rollup.excluded), (page.counted, page.excluded), "one rule, two surfaces");

    // Suppressed first: a suppressed dev finding is suppressed, not dev.
    let all = w.findings(&FindingsFilter::default());
    let vite = all["findings"].as_array().unwrap().iter().find(|f| f["rule_id"] == "GHSA-vite").unwrap();
    crate::history::security::suppress(&w.root, vite["fingerprint"].as_str().unwrap().to_string(), "accepted", None, TODAY.unwrap()).unwrap();
    let after = partition(scanner_row(&w.status(), "osv-scanner"));
    assert_eq!(after, [2, 1, 0, 0, 1, 0]);
    assert_eq!(after.iter().sum::<u64>(), all["total"].as_u64().unwrap(), "every finding once");
}

// ── the NVD API key (FR-SC-27, S-794) ────────────────────────────────────────

/// A planted NVD API key. The scanning process's environment carries it; it
/// must reach Dependency-Check's private file and nothing Logos writes or
/// returns.
const NVD_KEY: &str = "LOGOSPLANTEDNVDKEY-0123-4567-89ab-cdef01234567";

/// Every file under `dir`, recursively, that holds `needle`: the reports, the
/// failure logs, `history.db` and its WAL.
fn files_holding(dir: &Path, needle: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                pending.push(path);
            } else if meta.is_file() && fs::read(&path).unwrap().windows(needle.len()).any(|w| w == needle.as_bytes()) {
                found.push(path);
            }
        }
    }
    found
}

/// A Dependency-Check stub that copies the file its `--propertyfile` names to
/// the record directory and echoes it on stdout and stderr — the worst a
/// scanner could do with the key — then writes `report` (when given) and
/// exits `exit`.
fn echoing_dependency_check(w: &World, report: Option<&Path>, exit: i32) {
    let write = report.map_or(String::new(), |r| {
        format!("/bin/mkdir -p \"$out\" && /bin/cp '{}' \"$out/dependency-check-report.sarif\"", r.display())
    });
    let script = format!(
        "#!/bin/sh\n\
         printf '%s\\n' \"$*\" >> '{argv}'\n\
         case \"$1\" in --version) printf '%s\\n' 'Dependency-Check Core version 13.0.0'; exit 0;; esac\n\
         out=''; pf=''; prev=''\n\
         for a in \"$@\"; do case \"$prev\" in --out) out=\"$a\";; --propertyfile) pf=\"$a\";; esac; prev=\"$a\"; done\n\
         if [ -f \"$pf\" ]; then /bin/cp \"$pf\" '{props}'; /bin/cat \"$pf\"; /bin/cat \"$pf\" >&2; fi\n\
         {write}\n\
         exit {exit}\n",
        argv = w.rec.join("dependency-check.argv").display(),
        props = w.rec.join("nvd.props").display(),
    );
    let path = w.bin.join("dependency-check");
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// FR-SC-08, FR-SC-27: `security status` carries `nvd_api_key` on the
/// Dependency-Check row only — `not_set`, then `set` with a key in the
/// scanning process's environment, and `set` when the user's own
/// `extra_args` hand one — and never the key.
#[test]
fn status_says_whether_dependency_check_has_an_nvd_key_never_what_it_is() {
    let mut w = World::new();
    w.stub("dependency-check", None, 0, "");
    let rows = |status: &Value| -> Vec<(String, Value)> {
        status["scanners"]
            .as_array()
            .unwrap()
            .iter()
            .chain(status["other_reports"].as_array().unwrap())
            .filter_map(|row| row.get("nvd_api_key").map(|k| (row["scanner"].as_str().unwrap().to_string(), k.clone())))
            .collect()
    };
    assert_eq!(rows(&w.status()), [("dependency-check".to_string(), json!("not_set"))]);

    w.nvd_api_key = super::NvdApiKey::new(NVD_KEY);
    let status = w.status();
    assert_eq!(rows(&status), [("dependency-check".to_string(), json!("set"))]);
    assert!(!status.to_string().contains(NVD_KEY), "{status}");

    // Not installed, it still says whether a key is there for it.
    fs::remove_file(w.bin.join("dependency-check")).unwrap();
    assert_eq!(rows(&w.status()), [("dependency-check".to_string(), json!("set"))]);

    w.nvd_api_key = None;
    fs::write(w.user.join("config.toml"), "[security_scan.dependency-check]\nextra_args = [\"--nvdApiKey\", \"users-own\"]\n").unwrap();
    assert_eq!(rows(&w.status()), [("dependency-check".to_string(), json!("set"))]);
}

/// FR-SC-27 AC 3: a planted key, handed to a Dependency-Check that echoes it on
/// both streams, appears in no scan payload, failure log, cause, status row or
/// stored row — on a failed run and on one that ran and ingested — and no key
/// file remains after either. The log lines are `tests/security_nvd_key.rs`'s,
/// in a test binary of its own.
#[test]
fn a_planted_nvd_key_appears_in_no_payload_log_cause_or_row() {
    let mut w = World::new();
    fs::write(w.root.join("requirements.txt"), "Django==2.2.0\n").unwrap();
    w.nvd_api_key = super::NvdApiKey::new(NVD_KEY);
    let key_file = w.root.join(OUTPUT_DIR).join(super::NVD_KEY_FILE);

    for (report, exit) in [(None, 1), (Some(fixture("dependency-check")), 0)] {
        echoing_dependency_check(&w, report.as_deref(), exit);
        let payload = serde_json::to_value(w.scan(true).expect("the scan runs")).unwrap();
        let row = scanner_row(&payload, "dependency-check");
        assert_eq!(row["outcome"], if exit == 0 { "ran" } else { "failed" }, "{row}");
        assert_eq!(fs::read_to_string(w.rec.join("nvd.props")).unwrap(), format!("nvd.api.key={NVD_KEY}\n"), "the key was handed over");
        fs::remove_file(w.rec.join("nvd.props")).unwrap();
        let argv = fs::read_to_string(w.rec.join("dependency-check.argv")).unwrap();
        assert!(argv.contains("--propertyfile") && !argv.contains(NVD_KEY), "{argv}");

        assert!(!payload.to_string().contains(NVD_KEY), "scan payload: {payload}");
        let status = w.status();
        assert!(!status.to_string().contains(NVD_KEY), "status: {status}");
        let listed = w.findings(&FindingsFilter::default());
        assert!(!listed.to_string().contains(NVD_KEY), "findings: {listed}");
        assert_eq!(files_holding(&w.root.join(".logos"), NVD_KEY), Vec::<PathBuf>::new(), "a file under .logos holds the key");
        assert!(fs::symlink_metadata(&key_file).is_err(), "the key file outlived the run");
    }
    let stored = w.findings(&FindingsFilter::default());
    assert!(stored["findings"].as_array().is_some_and(|f| !f.is_empty()), "the ran scan was ingested: {stored}");
}

/// FR-SC-27 AC 2: a cancelled Dependency-Check run leaves no key file — the
/// file existed while it ran — and its job's events never carry the key.
#[test]
fn a_cancelled_dependency_check_run_leaves_no_key_file() {
    use crate::jobs::{JobKind, JobState, AnalysisJobs};
    use std::time::{Duration, Instant};

    let mut w = World::new();
    w.nvd_api_key = super::NvdApiKey::new(NVD_KEY);
    let seen = w.rec.join("seen");
    let sleeper = format!(
        "#!/bin/sh\ncase \"$1\" in --version) printf '%s\\n' 'Dependency-Check Core version 13.0.0'; exit 0;; esac\n\
         pf=''; prev=''\nfor a in \"$@\"; do if [ \"$prev\" = '--propertyfile' ]; then pf=\"$a\"; fi; prev=\"$a\"; done\n\
         /bin/ls \"$pf\" > '{seen}.tmp' && /bin/mv '{seen}.tmp' '{seen}'\n\
         /bin/sleep 60 & wait\n",
        seen = seen.display()
    );
    let path = w.bin.join("dependency-check");
    fs::write(&path, sleeper).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();

    let jobs = AnalysisJobs::new();
    let (root, env) = (w.root.clone(), w.env());
    jobs.start(JobKind::SecurityScan, &w.root, move |_| {
        let indexed = vec!["app/db.py".to_string()];
        let request = ScanRequest::new(vec!["dependency-check".to_string()], Vec::new());
        let summary = scan(&root, &env, crate::tools::Manifest::builtin(), true, &request, &mut |_| {}, |report, _| {
            crate::history::security::ingest(&root, report, &indexed)
        })?;
        Ok(serde_json::to_value(summary)?)
    })
    .unwrap();

    let key_file = w.root.join(OUTPUT_DIR).join(super::NVD_KEY_FILE);
    let deadline = Instant::now() + crate::perf::scaled(Duration::from_secs(20));
    while !seen.exists() {
        assert!(Instant::now() < deadline, "the sleeping scanner never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fs::read_to_string(&seen).unwrap().trim(), key_file.display().to_string(), "the file existed during the run");
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
    assert!(fs::symlink_metadata(&key_file).is_err(), "the key file outlived a cancelled run");
    let events = serde_json::to_string(&job).unwrap();
    assert!(!events.contains(NVD_KEY), "{events}");
}

/// FR-SC-27 AC 3: a scanner's output is kept to a bound — each stream's head
/// and tail, with a note between — and a cut can split an echoed key, at the
/// end of the head or at the start of the tail. Neither piece reaches the
/// failure log, the scan payload or the status row.
#[test]
fn a_key_split_by_the_output_bound_leaves_no_piece_of_it_behind() {
    use crate::jobs::spawn::{head_cap, tail_cap, STDERR_CAP, STDOUT_CAP};
    let filler = |n: usize| format!("/usr/bin/head -c {n} /dev/zero | /usr/bin/tr '\\0' x");
    // The key file is `nvd.api.key=<key>` and a newline: each case cuts 20
    // bytes inside the key — the head ending there, or the tail starting there.
    let file = "nvd.api.key=".len() + NVD_KEY.len() + 1;
    let inside = "nvd.api.key=".len() + 20;
    let cases = [("stderr", STDERR_CAP, " >&2"), ("stdout", STDOUT_CAP, "")].into_iter().flat_map(|(stream, cap, redirect)| {
        let head_cut = format!("{{ {}; /bin/cat \"$pf\"; {}; }}{redirect}", filler(head_cap(cap) - inside), filler(cap * 2));
        let tail_cut = format!("{{ {}; /bin/cat \"$pf\"; {}; }}{redirect}", filler(cap * 2), filler(tail_cap(cap) - (file - inside)));
        [(format!("{stream}, head ends in the key"), head_cut), (format!("{stream}, tail starts in the key"), tail_cut)]
    });
    for (stream, print) in cases {
        let mut w = World::new();
        w.nvd_api_key = super::NvdApiKey::new(NVD_KEY);
        let script = format!(
            "#!/bin/sh\n\
             case \"$1\" in --version) printf '%s\\n' 'Dependency-Check Core version 13.0.0'; exit 0;; esac\n\
             pf=''; prev=''\nfor a in \"$@\"; do if [ \"$prev\" = '--propertyfile' ]; then pf=\"$a\"; fi; prev=\"$a\"; done\n\
             {print}\nexit 1\n"
        );
        let path = w.bin.join("dependency-check");
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let payload = serde_json::to_value(w.scan(true).unwrap()).unwrap();
        assert_eq!(scanner_row(&payload, "dependency-check")["outcome"], "failed", "{stream}: {payload}");
        let log = fs::read_to_string(w.root.join(OUTPUT_DIR).join("dependency-check.failed.log")).unwrap();
        assert!(log.contains(super::mask::MASK), "{stream}: the cut piece is masked, not dropped");
        let shown = format!("{payload} {} {log}", w.status());
        for piece in NVD_KEY.as_bytes().windows(8).map(|w| std::str::from_utf8(w).unwrap()) {
            assert!(!shown.contains(piece), "{stream}: {piece:?} of the key is left behind");
            assert_eq!(files_holding(&w.root.join(".logos"), piece), Vec::<PathBuf>::new(), "{stream}: {piece:?}");
        }
    }
}

/// The key's pieces around an omission note are masked wherever the note lies
/// — a piece ending the head, a piece starting the tail, the newline the capture
/// sets before the note — and a note a scanner printed itself only adds a place
/// to look.
#[test]
fn redact_masks_the_key_pieces_each_side_of_an_omission_note() {
    let key = super::NvdApiKey::new("abcd1234-efgh-5678-ijkl").unwrap();
    let cut = "[… 300000 bytes omitted …]\n";
    let masked = super::mask::MASK;
    assert_eq!(key.redact(&format!("head abcd1234-e\n{cut}fgh-5678-ijkl tail")), format!("head {masked}\n{cut}{masked} tail"));
    assert_eq!(key.redact(&format!("a abcd1234-efgh-5678-ijkl\n{cut}z")), format!("a {masked}\n{cut}z"));
    // Two notes (one printed by the scanner), a piece on each side of each.
    assert_eq!(
        key.redact(&format!("x abcd1234\n{cut}1234-efgh-5678-ijkl y abcd1234-e\n{cut}ijkl")),
        format!("x {masked}\n{cut}{masked} y {masked}\n{cut}{masked}")
    );
    // Under the fragment floor nothing is masked; no note, the ends still are.
    assert_eq!(key.redact(&format!("abc\n{cut}ijk")), format!("abc\n{cut}ijk"));
    assert_eq!(key.redact("tail ends abcd1234-"), format!("tail ends {masked}"));
}

/// FR-SC-27 AC 2: a key file a crash left is removed at the next scan's start
/// even when that scan starts no scanner — the one it names is not installed.
#[test]
fn a_scan_that_starts_no_scanner_still_sweeps_a_stale_key_file() {
    let w = World::new();
    let stale = w.root.join(OUTPUT_DIR).join(super::NVD_KEY_FILE);
    fs::create_dir_all(stale.parent().unwrap()).unwrap();
    fs::write(&stale, format!("nvd.api.key={NVD_KEY}\n")).unwrap();
    let request = ScanRequest::new(vec!["dependency-check".to_string()], Vec::new());
    let payload = serde_json::to_value(w.scan_request(true, &request, &mut |_| {}).unwrap()).unwrap();
    assert_eq!(scanner_row(&payload, "dependency-check")["outcome"], "not_installed", "{payload}");
    assert!(w.started().is_empty(), "nothing started: {:?}", w.started());
    assert!(fs::symlink_metadata(&stale).is_err(), "the stale key file survived a scan's start");
}
