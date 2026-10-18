//! `logos security scan|ingest|status|findings` end to end through the shipped
//! binary (S-667, [FR-SC-06], [FR-SC-07], [FR-SC-08]).
//!
//! The binary runs on a git fixture outside every work tree, with
//! `LOGOS_CONFIG_HOME` and `LOGOS_DATA_HOME` pointing at a temporary user-level
//! directory and tools directory, and a `PATH` of two directories only: the
//! stub scanners (and a stub `curl`), and one holding a link to `git`. So no
//! scanner installed on this machine is found, no download leaves it, and this
//! machine's own consent records are never read or written. The MCP side of
//! the same payloads is `mcp/tests/security_parity.rs`; the install flows over
//! verified fixture artifacts are `logos-core/tests/security_install.rs`.
//!
//! [FR-SC-06]: ../../docs/specs/requirements/FR-SC-06.md
//! [FR-SC-07]: ../../docs/specs/requirements/FR-SC-07.md
//! [FR-SC-08]: ../../docs/specs/requirements/FR-SC-08.md

#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

/// Printed on stderr by the failing stub: it must reach neither stream.
const PLANTED: &str = "LOGOSPLANTEDSECRET0123456789abcdef";

/// `app/db.py` with a function around each line `semgrep.sarif` names (12 and
/// 18), so each finding binds to an enclosing symbol. Python resolves no call
/// across files here, so reachability is `unresolved` (S-669).
const DB_PY: &str = r#"import sqlite3


def connect():
    return sqlite3.connect("app.db")


def find(user_id):
    conn = connect()
    cursor = conn.cursor()
    # look the user up
    cursor.execute("SELECT * FROM users WHERE id = %s" % user_id)
    return cursor.fetchone()


def delete(uid):
    conn = connect()
    return conn.execute(f"DELETE FROM users WHERE id = {uid}")
"#;

const CLEAN_BANDIT: &str = r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Bandit","rules":[]}},"results":[]}]}"#;

struct World {
    _tmp: TempDir,
    root: PathBuf,
    bin: PathBuf,
    path: OsString,
    user: PathBuf,
    /// The user-level tools directory (`LOGOS_DATA_HOME`).
    data: PathBuf,
    clean: PathBuf,
}

/// The `git` this test process would run, found on its own `PATH`.
fn real_git() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").expect("PATH is set"))
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git is on PATH")
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(root)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0", "-c", "user.email=t@logos", "-c", "user.name=t"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}");
}

impl World {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let [root, bin, tools, user, data] = ["project", "bin", "tools", "user", "data"].map(|d| base.join(d));
        for dir in [&root.join("app"), &root.join(".logos"), &bin, &tools, &user] {
            fs::create_dir_all(dir).unwrap();
        }
        std::os::unix::fs::symlink(real_git(), tools.join("git")).unwrap();
        fs::write(root.join("app/db.py"), DB_PY).unwrap();
        fs::write(root.join(".logos/config.toml"), "[project]\nid = \"security-cli-fixture\"\n").unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "seed"]);
        let clean = base.join("clean-bandit.sarif");
        fs::write(&clean, CLEAN_BANDIT).unwrap();
        let path = std::env::join_paths([&bin, &tools]).unwrap();
        let w = Self { _tmp: tmp, root, bin, path, user, data, clean };
        let semgrep = Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep.sarif");
        w.stub("semgrep", Some(&semgrep), 0, "");
        w.stub("bandit", Some(&w.clean.clone()), 0, "");
        w.stub("gitleaks", None, 3, &format!("fatal: token={PLANTED}"));
        assert!(w.logos(&["index"]).status.success());
        w
    }

    fn stub(&self, name: &str, report: Option<&Path>, exit: i32, stderr: &str) {
        let copy = report.map_or(String::new(), |r| format!("if [ -n \"$out\" ]; then /bin/cp '{}' \"$out\"; fi", r.display()));
        let script = format!(
            "#!/bin/sh\n\
             case \"$1\" in --version|version|-version) printf '%s\\n' '{name} 1.2.3'; exit 0;; esac\n\
             out=''; prev=''\n\
             for a in \"$@\"; do case \"$prev\" in --output|--report-path) out=\"$a\";; esac; prev=\"$a\"; done\n\
             {copy}\n\
             printf '%s\\n' '{stderr}' >&2\n\
             exit {exit}\n"
        );
        let path = self.bin.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn logos(&self, args: &[&str]) -> Output {
        let out = Command::new(env!("CARGO_BIN_EXE_logos"))
            .arg("--project")
            .arg(&self.root)
            .args(args)
            .env("PATH", &self.path)
            .env("LOGOS_CONFIG_HOME", &self.user)
            .env("LOGOS_DATA_HOME", &self.data)
            .env("GIT_CEILING_DIRECTORIES", self.root.parent().unwrap())
            .output()
            .expect("the logos binary runs");
        for stream in [&out.stdout, &out.stderr] {
            assert!(
                !String::from_utf8_lossy(stream).contains(PLANTED),
                "logos {args:?} printed the planted secret"
            );
        }
        out
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = self.logos(&all);
        assert!(out.status.success(), "logos {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).expect("JSON on stdout")
    }
}

fn row<'a>(payload: &'a Value, scanner: &str) -> &'a Value {
    payload["scanners"].as_array().unwrap().iter().find(|r| r["scanner"] == scanner).unwrap()
}

/// Each listed finding's enclosing symbol and reachability `(confidence,
/// verdict)` — its graph context (S-669).
fn contexts(payload: &Value) -> Vec<(Option<&str>, Option<&str>, Option<&str>)> {
    payload["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let c = &f["context"];
            (c["enclosing"]["symbol"]["name"].as_str(), c["reachability"]["confidence"].as_str(), c["reachability"]["verdict"].as_str())
        })
        .collect()
}

/// What `contexts` reads on the fixture: each finding bound to its function,
/// and no route claimed either way, Python resolving no call across files.
const BOUND_UNRESOLVED: [(Option<&str>, Option<&str>, Option<&str>); 2] = [
    (Some("find"), Some("unresolved"), Some("unknown")),
    (Some("delete"), Some("unresolved"), Some("unknown")),
];

#[test]
fn the_security_commands_scan_with_consent_and_report_every_state() {
    let w = World::new();

    let before = w.json(&["security", "status"]);
    assert_eq!(row(&before, "semgrep")["state"], "never_run");
    assert!(row(&before, "semgrep").get("findings").is_none());
    assert_eq!(row(&before, "trivy")["state"], "not_installed");

    // No consent: exit 2, naming the flag and the scanners; nothing recorded.
    let refused = w.logos(&["security", "scan"]);
    assert_eq!(refused.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("--consent") && stderr.contains("semgrep, gitleaks, bandit"), "{stderr}");
    assert!(fs::read_dir(&w.user).unwrap().next().is_none(), "a refusal records nothing");

    let scan = w.json(&["security", "scan", "--consent"]);
    assert_eq!(scan["consent_recorded"], serde_json::json!(["semgrep", "gitleaks", "bandit"]));
    assert_eq!(row(&scan, "trivy")["outcome"], "not_installed");
    let failed = row(&scan, "gitleaks");
    assert_eq!((failed["outcome"].as_str(), failed["exit_code"].as_i64()), (Some("failed"), Some(3)));
    assert!(failed["stderr"].as_str().unwrap().contains("fatal: token="), "{failed}");
    assert_eq!((row(&scan, "semgrep")["outcome"].as_str(), scan["findings"].as_u64()), (Some("ran"), Some(2)));

    // Consent is not asked again; a newly installed scanner is asked for alone.
    assert_eq!(w.json(&["security", "scan"])["consent_recorded"], serde_json::json!([]));
    w.stub("osv-scanner", None, 0, "");
    let asked = w.logos(&["security", "scan"]);
    assert_eq!(asked.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&asked.stderr);
    assert!(stderr.contains("for osv-scanner —") && !stderr.contains("semgrep"), "{stderr}");
    assert_eq!(w.json(&["security", "scan", "--consent"])["consent_recorded"], serde_json::json!(["osv-scanner"]));

    let after = w.json(&["security", "status"]);
    assert_eq!((row(&after, "bandit")["state"].as_str(), row(&after, "bandit")["findings"].as_u64()), (Some("fresh"), Some(0)));
    assert_eq!(row(&after, "semgrep")["findings"], 2);
    assert_eq!(row(&after, "gitleaks")["last_failure"], ".logos/security/gitleaks.failed.log");

    let findings = w.json(&["security", "findings", "--scanner", "semgrep", "--path", "app"]);
    assert_eq!(findings["findings"].as_array().unwrap().len(), 2);
    assert_eq!(contexts(&findings), BOUND_UNRESOLVED, "the graph context rides on the CLI payload");
    // Every flag at once, each to a value only its own slot accepts.
    let every = w.json(&[
        "security", "findings", "--severity", "medium", "--scanner", "semgrep", "--category", "A05",
        "--path", "app", "--suppression", "active", "--edition", "2025",
    ]);
    assert_eq!(every["findings"].as_array().unwrap().len(), 2, "{every}");
    let ingest = w.json(&["security", "ingest", w.clean.to_str().unwrap()]);
    assert_eq!(ingest["already_ingested"], true);
    assert_eq!(w.logos(&["security", "findings", "--severity", "bogus"]).status.code(), Some(2));
}

// ── The `rules.toml [security]` gate (S-668, FR-SC-10, UAT-SC-02) ───────────

/// A contract with no `[security]` table: one constraint, so a contract is
/// present and `check` has a verdict.
const RULES_PLAIN: &str = "[constraints]\nmax_cc = 50\n";

/// The rule id of the high (`error`) finding the second report adds.
const NEW_RULE: &str = "python.lang.security.audit.new-high-finding";

impl World {
    fn rules(&self, text: Option<&str>) {
        let path = self.root.join(".logos/rules.toml");
        match text {
            Some(text) => fs::write(path, text).unwrap(),
            None => fs::remove_file(path).unwrap(),
        }
    }

    /// `logos --json <args>`: the exit code and the parsed stdout.
    fn verdict(&self, args: &[&str]) -> (Option<i32>, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = self.logos(&all);
        let json = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("logos {args:?}: {e}: {}", String::from_utf8_lossy(&out.stderr)));
        (out.status.code(), json)
    }

    fn ingest(&self, report: &Path) {
        let out = self.logos(&["security", "ingest", report.to_str().unwrap()]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    /// `semgrep.sarif` with one more result: a high finding at line 5.
    fn semgrep_with_a_new_high_finding(&self) -> PathBuf {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep.sarif");
        let mut report: Value = serde_json::from_str(&fs::read_to_string(base).unwrap()).unwrap();
        let results = report["runs"][0]["results"].as_array_mut().unwrap();
        results.push(serde_json::json!({
            "ruleId": NEW_RULE,
            "level": "error",
            "message": {"text": "a new high finding"},
            "locations": [{"physicalLocation": {
                "artifactLocation": {"uri": "app/db.py"},
                "region": {"startLine": 5, "startColumn": 5, "endLine": 5, "endColumn": 40}
            }}]
        }));
        let path = self.root.parent().unwrap().join("semgrep-new-high.sarif");
        fs::write(&path, serde_json::to_vec(&report).unwrap()).unwrap();
        path
    }
}

/// `check` and `gate` JSON with the run's own timestamp removed — the only field
/// that differs between two runs over one tree, evidence or none.
fn settled(mut payload: Value) -> String {
    if let Some(object) = payload.as_object_mut() {
        object.remove("ran_at");
    }
    payload.to_string()
}

fn missing(payload: &Value) -> Vec<(&str, &str)> {
    payload["missing_scans"]
        .as_array()
        .map(|rows| rows.iter().map(|m| (m["scanner"].as_str().unwrap(), m["state"].as_str().unwrap())).collect())
        .unwrap_or_default()
}

/// [UAT-SC-02] step 1, [FR-SC-10] acceptance 1: without a `[security]` table,
/// `check` and `gate` print the same bytes whether security evidence is absent,
/// present, or deleted with `history.db` — compared against a saved metric
/// baseline, so the gate's real comparison is what stays put. Adding the table
/// afterwards moves no metric: the next gate compares, it does not re-baseline.
#[test]
fn without_a_security_table_check_and_gate_ignore_the_evidence() {
    let w = World::new();
    w.rules(Some(RULES_PLAIN));
    assert_eq!(w.logos(&["gate", "--save"]).status.code(), Some(0));
    let semgrep = Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep.sarif");
    let outputs = |w: &World| {
        let (check_code, check) = w.verdict(&["check"]);
        let (gate_code, gate) = w.verdict(&["gate"]);
        let human = w.logos(&["gate"]).stdout;
        (check_code, settled(check), gate_code, settled(gate), human)
    };

    let absent = outputs(&w);
    assert_eq!((absent.0, absent.2), (Some(0), Some(0)), "{absent:?}");
    assert!(!absent.1.contains("missing_scans") && !absent.3.contains("security"), "{absent:?}");

    w.ingest(&semgrep);
    assert_eq!(outputs(&w), absent, "stored findings change nothing without the table");

    fs::remove_file(w.root.join(".logos/history.db")).unwrap();
    assert_eq!(outputs(&w), absent, "deleting the evidence store changes nothing");
    let (_, plain) = w.verdict(&["gate"]);
    assert!(plain["message"].as_str().unwrap().contains("holds the baseline"), "a real comparison: {plain}");

    w.rules(Some("[constraints]\nmax_cc = 50\n\n[security]\n"));
    let (code, gate) = w.verdict(&["gate"]);
    assert_eq!(code, Some(0), "the evidence was deleted, so nothing is found: {gate}");
    assert_eq!((&gate["saved"], &gate["signal"]), (&Value::Bool(false), &plain["signal"]), "{gate}");
    let message = gate["message"].as_str().unwrap();
    assert!(message.contains("holds the baseline") && !message.contains("baseline reset"), "{gate}");
}

/// [UAT-SC-02] steps 2–5, [FR-SC-10] acceptance 2–3: a required scanner that
/// never ran exits 5 and one the table does not require never does; after `gate
/// --save` a new high finding fails with exit 1 naming it and passes once
/// suppressed with a reason; a stale required scan exits 5 naming the scanner,
/// a stale unrequired one is not evaluated; exit 4 still takes precedence.
#[test]
fn a_security_table_gates_new_findings_and_exits_5_on_a_missing_scan() {
    let w = World::new();
    let table = "[constraints]\nmax_cc = 50\n\n[security]\nrequired_scanners = [\"semgrep\"]\n";
    w.rules(Some(table));

    // Never run: exit 5 for the required scanner only — bandit and gitleaks are
    // installed and have never run either, and are not named.
    for command in ["check", "gate", "session-end"] {
        let (code, payload) = w.verdict(&[command]);
        assert_eq!(code, Some(5), "{command}: {payload}");
        assert_eq!(missing(&payload), [("semgrep", "never_run")], "{command}: {payload}");
        assert_eq!(payload["passed"], false, "missing evidence is no pass: {payload}");
        assert!(payload["missing_scans"][0]["message"].as_str().unwrap().contains("logos security scan"));
    }
    // A save over a missing scan saves no security baseline (exit 5).
    let (code, refused) = w.verdict(&["gate", "--save"]);
    assert_eq!(code, Some(5), "{refused}");
    assert!(refused["message"].as_str().unwrap().contains("security baseline not saved: required scan missing or stale at HEAD: semgrep"), "{refused}");

    // Scan, then accept the current findings.
    let semgrep = Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep.sarif");
    w.ingest(&semgrep);
    w.ingest(&w.clean);
    let (code, unsaved) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "no baseline yet: the existing high finding is new: {unsaved}");
    assert!(unsaved["notes"].to_string().contains("no security baseline is recorded"), "the refused save recorded none: {unsaved}");
    assert_eq!(unsaved["checked_rules"], 2, "max_cc and the ratchet: {unsaved}");
    // `session-start` is the MCP spelling of `gate --save`: it records it too.
    let (code, saved) = w.verdict(&["session-start"]);
    assert_eq!(code, Some(0), "{saved}");
    assert!(saved["message"].as_str().unwrap().contains("security baseline saved (2 finding fingerprints)"), "{saved}");
    for command in ["check", "gate", "session-end"] {
        let (code, payload) = w.verdict(&[command]);
        assert_eq!(code, Some(0), "{command} after the save: {payload}");
        assert!(payload.get("missing_scans").is_none(), "{payload}");
    }

    // The OWASP caps fail on recorded findings, through the categories the
    // engine classifies — both SQL injections sit under A05:2025 and A03:2021.
    w.rules(Some(&format!("{table}fail_on_owasp = [\"A05\", \"A03:2021\"]\n")));
    let (code, capped) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "{capped}");
    let caps: Vec<&str> = capped["violations"].as_array().unwrap().iter().filter_map(|v| v["rule"].as_str()).collect();
    assert_eq!(
        caps,
        ["security:fail_on_owasp:A05:2025", "security:fail_on_owasp:A05:2025", "security:fail_on_owasp:A03:2021", "security:fail_on_owasp:A03:2021"],
        "{capped}"
    );
    assert_eq!(capped["checked_rules"], 4, "max_cc, the ratchet and two categories: {capped}");
    w.rules(Some(table));

    // A new high finding fails both, naming it.
    w.ingest(&w.semgrep_with_a_new_high_finding());
    let (code, check) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "{check}");
    let ratchet: Vec<&Value> = check["violations"].as_array().unwrap().iter().filter(|v| v["rule"] == "security:ratchet").collect();
    assert_eq!(ratchet.len(), 1, "only the new finding: {check}");
    assert!(ratchet[0]["message"].as_str().unwrap().contains(NEW_RULE), "{check}");
    assert_eq!(ratchet[0]["file"], "app/db.py");
    let (code, gate) = w.verdict(&["gate"]);
    assert_eq!(code, Some(1), "{gate}");
    assert_eq!(gate["passed"], false);
    assert!(gate["security_violations"][0]["message"].as_str().unwrap().contains(NEW_RULE), "{gate}");

    // Suppressed with a reason: both pass.
    let findings = w.json(&["security", "findings"]);
    let fingerprint = findings["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["rule_id"] == NEW_RULE)
        .and_then(|f| f["fingerprint"].as_str())
        .expect("the new finding is listed")
        .to_string();
    fs::write(
        w.root.join(".logos/security-ignore.toml"),
        format!("[[ignore]]\nfingerprint = \"{fingerprint}\"\nreason = \"fixture: never deployed\"\n"),
    )
    .unwrap();
    for command in ["check", "gate"] {
        let (code, payload) = w.verdict(&[command]);
        assert_eq!(code, Some(0), "{command} once suppressed: {payload}");
    }

    // A save does not accept a suppressed finding: once the suppression is gone
    // (expired or removed), the finding is new again (FR-SC-11).
    let ignore = fs::read_to_string(w.root.join(".logos/security-ignore.toml")).unwrap();
    let (code, resaved) = w.verdict(&["gate", "--save"]);
    assert_eq!(code, Some(0), "{resaved}");
    assert!(resaved["message"].as_str().unwrap().contains("security baseline saved (2 finding fingerprints)"), "the suppressed third is not recorded: {resaved}");
    fs::remove_file(w.root.join(".logos/security-ignore.toml")).unwrap();
    let (code, lifted) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "{lifted}");
    assert!(lifted["violations"].to_string().contains(NEW_RULE), "{lifted}");
    fs::write(w.root.join(".logos/security-ignore.toml"), &ignore).unwrap();

    // An invalid ignore file is a usage error (exit 2) that writes nothing: the
    // metric baseline `gate --save` / `session-start` would replace is kept.
    let baseline = || -> i64 {
        let db = rusqlite::Connection::open(w.root.join(".logos/logos.db")).unwrap();
        db.query_row("SELECT snapshot_id FROM baseline WHERE scope = 'project'", [], |r| r.get(0)).unwrap()
    };
    let kept = baseline();
    fs::write(w.root.join(".logos/security-ignore.toml"), "[[ignore]]\nfingerprint =\n").unwrap();
    for command in [&["gate", "--save"][..], &["session-start"]] {
        assert_eq!(w.logos(command).status.code(), Some(2), "{command:?}");
        assert_eq!(baseline(), kept, "{command:?} left the metric baseline alone");
    }
    fs::write(w.root.join(".logos/security-ignore.toml"), &ignore).unwrap();

    // HEAD moves: both scans go stale. The required one exits 5 naming itself;
    // the unrequired bandit is only noted as not evaluated.
    fs::write(w.root.join("README.md"), "moved\n").unwrap();
    git(&w.root, &["add", "README.md"]);
    git(&w.root, &["commit", "-q", "-m", "move HEAD"]);
    for command in ["check", "gate", "session-end"] {
        let (code, payload) = w.verdict(&[command]);
        assert_eq!(code, Some(5), "{command} over a stale scan: {payload}");
        assert_eq!(missing(&payload), [("semgrep", "stale")], "{payload}");
        assert_eq!(payload["passed"], false, "{payload}");
        assert!(payload["missing_scans"][0]["message"].as_str().unwrap().contains("HEAD moved"), "{payload}");
        let notes = payload["notes"].to_string();
        assert!(notes.contains("not evaluated") && notes.contains("Bandit"), "{command}: {payload}");
    }

    // A table that requires nothing never exits 5, however stale the evidence.
    w.rules(Some("[constraints]\nmax_cc = 50\n\n[security]\n"));
    let (code, payload) = w.verdict(&["check"]);
    assert_eq!(code, Some(0), "{payload}");
    assert!(payload.get("missing_scans").is_none(), "{payload}");

    // No contract at all outranks everything: exit 4.
    w.rules(None);
    assert_eq!(w.logos(&["check"]).status.code(), Some(4));
}

/// `security install|uninstall|scan --install` through the binary (S-744,
/// FR-SC-22). The builtin manifest's real pins are used, so the stub `curl`'s
/// bytes never verify: what is proved here is which tools are fetched, the
/// exit-2 refusal, and that the user's own copy is kept. Installs that verify
/// are the core suite's.
#[test]
fn install_asks_consent_fetches_only_missing_tools_and_never_touches_a_users_own_copy() {
    let w = World::new();
    let curl_log = w.user.parent().unwrap().join("curl.log");
    let curl = w.bin.join("curl");
    fs::write(
        &curl,
        format!(
            "#!/bin/sh\nout=; url=\nwhile [ $# -gt 0 ]; do case \"$1\" in --output) out=\"$2\"; shift 2;; *) url=\"$1\"; shift;; esac; done\n\
             printf '%s\\n' \"$url\" >> '{}'\nprintf 'not the release' > \"$out\"\n",
            curl_log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
    // The user's own Trivy, at the builtin manifest's minimum version.
    let trivy = w.bin.join("trivy");
    fs::write(&trivy, "#!/bin/sh\nprintf 'Version: 0.74.0\\n'\n").unwrap();
    fs::set_permissions(&trivy, fs::Permissions::from_mode(0o755)).unwrap();

    let refused = w.logos(&["security", "install", "--all"]);
    assert_eq!(refused.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    // The Python scanners come from the package index, and need `uv` and the
    // managed Python, each its own line (S-745).
    for line in [
        "gitleaks 8.18.4 from github.com (", "osv-scanner 1.9.2 from github.com (", "gosec 2.21.4 from github.com (",
        "semgrep 1.85.0 from pypi.org (", "bandit 1.7.10 from pypi.org (", "checkov 3.2.533 from pypi.org (",
        "uv 0.12.21 from github.com (", "python 3.12.14 from github.com (",
    ] {
        assert!(stderr.contains(line), "names `{line}`: {stderr}");
    }
    assert!(stderr.contains(" MB)") && stderr.contains("logos security install --all --consent"), "{stderr}");
    assert!(!stderr.contains("trivy 0.74.0"), "the user's own trivy is not asked for: {stderr}");
    for opt_in in ["dependency-check", "codeql", "java "] {
        assert!(!stderr.contains(opt_in), "--all never asks for {opt_in}: {stderr}");
    }
    assert!(!curl_log.exists(), "a refusal downloads nothing");
    assert!(fs::read_dir(&w.user).unwrap().next().is_none(), "a refusal records nothing");

    let installed = w.json(&["security", "install", "--all", "--consent"]);
    let fetched = fs::read_to_string(&curl_log).unwrap();
    // Three release binaries, and `uv` once for the three Python scanners.
    assert_eq!(fetched.lines().count(), 4, "{fetched}");
    assert_eq!(fetched.matches("/astral-sh/uv/").count(), 1, "{fetched}");
    assert!(!fetched.contains("trivy"), "the user's own trivy is never downloaded: {fetched}");
    let tools = installed["tools"].as_array().unwrap();
    assert!(
        tools.iter().all(|t| t["outcome"] == "failed" && t["stage"] == "checksum"),
        "bytes that are not the pinned release never install: {installed}"
    );
    assert_eq!((installed["kept"][0]["tool"].as_str(), installed["kept"][0]["copy"].as_str()), (Some("trivy"), Some("path")));
    assert!(fs::read_dir(&w.data).unwrap().next().is_none(), "nothing unverified is left in the tools directory");

    let status = w.json(&["security", "status"]);
    let installed_trivy = &row(&status, "trivy")["installed"];
    assert_eq!((installed_trivy["copy"].as_str(), installed_trivy["version"].as_str()), (Some("path"), Some("0.74.0")));
    assert_eq!(row(&status, "osv-scanner")["install"], "logos security install osv-scanner");

    let removed = w.json(&["security", "uninstall", "--all"]);
    assert!(removed["tools"].as_array().unwrap().iter().all(|t| t["outcome"] == "not_installed"), "{removed}");
    assert!(trivy.is_file(), "uninstall --all never touches the user's own copy");
    assert_eq!(row(&w.json(&["security", "status"]), "trivy")["installed"]["copy"], "path");

    // `scan --install` asks the same consent, naming its own command; a bare
    // `install` names neither a scanner nor `--all`, a usage error.
    let fresh = World::new();
    let refused = fresh.logos(&["security", "scan", "--install"]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("logos security scan --install --consent"));
    assert_eq!(fresh.logos(&["security", "install"]).status.code(), Some(2));
    // CodeQL is installed only by name and behind its licence (S-746): the
    // terms are shown until `--accept-codeql-licence`, and the install
    // consent is still asked for after it; neither refusal records anything.
    let codeql = fresh.logos(&["security", "install", "codeql"]);
    assert_eq!(codeql.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&codeql.stderr);
    for shown in ["open-source codebases", "logos security install codeql --accept-codeql-licence --consent"] {
        assert!(stderr.contains(shown), "shows `{shown}`: {stderr}");
    }
    let accepted = fresh.logos(&["security", "install", "codeql", "--accept-codeql-licence"]);
    assert_eq!(accepted.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&accepted.stderr);
    assert!(stderr.contains("no install consent is recorded for codeql 2.27.2 from github.com ("), "{stderr}");
    assert!(fs::read_dir(&fresh.user).unwrap().next().is_none(), "a refusal records nothing");
}

/// `security install` exits 1 when none of the tools it was asked to install
/// ended in place, and 0 when at least one did (HF-4): the per-tool outcomes are
/// printed either way. The stub `curl` hands back bytes that are not the pinned
/// release, so every download fails its checksum — no network is used — and a
/// copy the user already has is the tool that is in place.
#[test]
fn install_exits_1_when_every_requested_tool_failed_and_0_when_one_is_in_place() {
    let w = World::new();
    let curl = w.bin.join("curl");
    fs::write(&curl, "#!/bin/sh\nout=\nwhile [ $# -gt 0 ]; do case \"$1\" in --output) out=\"$2\"; shift 2;; *) shift;; esac; done\nprintf 'not the release' > \"$out\"\n").unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
    let outcomes = |out: &Output| -> Vec<(String, String)> {
        let payload: Value = serde_json::from_slice(&out.stdout).expect("JSON on stdout");
        let mut tools: Vec<_> = payload["tools"].as_array().unwrap().iter().map(|t| (t["tool"].as_str().unwrap().to_string(), t["outcome"].as_str().unwrap().to_string())).collect();
        tools.sort();
        tools
    };
    let failed = |tool: &str| (tool.to_string(), "failed".to_string());

    let all_failed = w.logos(&["--json", "security", "install", "gosec", "osv-scanner", "--consent"]);
    assert_eq!(all_failed.status.code(), Some(1), "{}", String::from_utf8_lossy(&all_failed.stderr));
    assert_eq!(outcomes(&all_failed), [failed("gosec"), failed("osv-scanner")], "the outcomes are stated as before");

    // The user's own Trivy is in place, so one tool failing no longer fails the call.
    let trivy = w.bin.join("trivy");
    fs::write(&trivy, "#!/bin/sh\nprintf 'Version: 0.74.0\\n'\n").unwrap();
    fs::set_permissions(&trivy, fs::Permissions::from_mode(0o755)).unwrap();
    let one_in_place = w.logos(&["--json", "security", "install", "gosec", "trivy", "--consent"]);
    assert_eq!(one_in_place.status.code(), Some(0), "{}", String::from_utf8_lossy(&one_in_place.stderr));
    assert_eq!(outcomes(&one_in_place), [failed("gosec")]);

    // A refusal (here CodeQL's unaccepted licence) stays exit 2, not 1.
    assert_eq!(w.logos(&["security", "install", "codeql"]).status.code(), Some(2));
}

/// `security scan --scanner --path` through the binary ([FR-SC-17]): consent
/// is asked for what the scan starts; Semgrep runs on the path alone and
/// reports its scope; a path outside the project is refused with exit 2 before
/// any scanner starts; a whole-manifest scanner says so; `--install` takes
/// neither flag.
///
/// [FR-SC-17]: ../../docs/specs/requirements/FR-SC-17.md
#[test]
fn a_scan_narrowed_by_scanner_and_path_runs_only_there_and_refuses_an_outside_path() {
    let w = World::new();
    let scan = w.json(&["security", "scan", "--consent", "--scanner", "semgrep", "--path", "app/db.py"]);
    assert_eq!(scan["consent_recorded"], serde_json::json!(["semgrep"]), "consent for what this scan starts");
    let semgrep = row(&scan, "semgrep");
    assert_eq!((semgrep["outcome"].as_str(), semgrep["findings"].as_u64()), (Some("ran"), Some(2)));
    assert_eq!(semgrep["scope"], serde_json::json!({ "kind": "paths", "paths": ["app/db.py"] }));
    assert_eq!(semgrep["sarif"], serde_json::json!([".logos/security/scoped/semgrep.sarif"]));
    assert_eq!(row(&scan, "bandit")["outcome"], "not_selected");
    assert_eq!(row(&scan, "gitleaks")["outcome"], "not_selected", "the failing stub never started");

    // Outside the root: refused before any process starts — the stub records a call.
    let calls = w.root.parent().unwrap().join("semgrep-calls");
    fs::write(w.bin.join("semgrep"), format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n", calls.display())).unwrap();
    let outside = w.root.parent().unwrap().display().to_string();
    for path in ["../outside", outside.as_str()] {
        let refused = w.logos(&["security", "scan", "--scanner", "semgrep", "--path", path]);
        assert_eq!(refused.status.code(), Some(2), "{path}");
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(stderr.contains("resolves outside the project root") && stderr.contains(path), "{stderr}");
    }
    assert!(!calls.exists(), "no scanner process was started");

    let trivy = w.json(&["security", "scan", "--scanner", "trivy", "--path", "app/db.py"]);
    assert_eq!(row(&trivy, "trivy")["outcome"], "not_installed");
    assert_eq!(row(&trivy, "trivy")["scope"], serde_json::json!({ "kind": "unscoped", "reason": "whole-manifest only" }));

    let conflict = w.logos(&["security", "scan", "--install", "--path", "app/db.py"]);
    assert_eq!(conflict.status.code(), Some(2), "--install takes no --path");
}

/// A Gitleaks report from a scoped (`--no-git`) run: one result per
/// `(line, start column, end column)` in `app/settings.py`, with no commit
/// fingerprint — content mode carries none.
fn gitleaks_content_report(dir: &Path, name: &str, secrets: &[(i64, i64, i64)]) -> PathBuf {
    let results: Vec<Value> = secrets
        .iter()
        .map(|&(line, start, end)| {
            serde_json::json!({
                "ruleId": "generic-api-key",
                "message": { "text": "generic-api-key has detected secret for file app/settings.py." },
                "locations": [{ "physicalLocation": {
                    "artifactLocation": { "uri": "app/settings.py" },
                    "region": { "startLine": line, "startColumn": start, "endLine": line, "endColumn": end }
                } }],
                "partialFingerprints": { "commitSha": "", "email": "", "author": "", "date": "", "commitMessage": "" }
            })
        })
        .collect();
    let report = serde_json::json!({
        "version": "2.1.0",
        "runs": [{ "tool": { "driver": { "name": "gitleaks", "semanticVersion": "v8.18.4" } }, "results": results }]
    });
    let path = dir.join(name);
    fs::write(&path, report.to_string()).unwrap();
    path
}

/// The coordinator's S-685 decision, through the binary and the `[security]`
/// gate: a scoped Gitleaks run reads the file (`--no-git`) while the full scan
/// read its git history, so
/// - an already-recorded secret re-scanned scoped keeps its fingerprint — it is
///   not new, and `check` still passes;
/// - a newly added secret is new, and `check` fails on it alone;
/// - a secret removed from the file stays listed: it is still in history.
#[test]
fn a_scoped_gitleaks_run_keeps_recorded_secrets_known_and_history_findings_listed() {
    let w = World::new();
    fs::write(w.root.join("app/settings.py"), "DEBUG = False\nAPI_TIMEOUT = 30\n\nAPI_KEY = 'redacted-for-the-fixture'\n").unwrap();
    git(&w.root, &["add", "."]);
    git(&w.root, &["commit", "-q", "-m", "settings"]);
    w.rules(Some("[security]\nratchet_min_severity = \"medium\"\n"));
    // The full scan: the fixture's two history findings of one secret at
    // app/settings.py 4:14-45, one per commit that touched it.
    let full = Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/gitleaks.sarif");
    w.ingest(&full);
    let fingerprints = |w: &World| -> Vec<String> {
        let mut prints: Vec<String> = w.json(&["security", "findings", "--scanner", "gitleaks"])["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["fingerprint"].as_str().unwrap().to_string())
            .collect();
        prints.sort();
        prints
    };
    let recorded = fingerprints(&w);
    assert_eq!(recorded.len(), 2, "{recorded:?}");
    let (code, saved) = w.verdict(&["gate", "--save"]);
    assert_eq!(code, Some(0), "{saved}");
    let (code, before) = w.verdict(&["check"]);
    assert_eq!(code, Some(0), "{before}");

    let scoped = ["security", "scan", "--consent", "--scanner", "gitleaks", "--path", "app/settings.py"];
    // The recorded secret, re-scanned in content mode at the same position.
    w.stub("gitleaks", Some(&gitleaks_content_report(w.root.parent().unwrap(), "same.sarif", &[(4, 14, 45)])), 0, "");
    assert_eq!(row(&w.json(&scoped), "gitleaks")["outcome"], "ran");
    assert_eq!(fingerprints(&w), recorded, "the recorded secret keeps its fingerprint; no copy is added");
    let (code, after) = w.verdict(&["check"]);
    assert_eq!(code, Some(0), "an already-recorded secret is not new: {after}");
    assert_eq!(after["violations"], before["violations"], "the gate verdict is unchanged");

    // A newly added secret on line 6: new, and the only violation.
    w.stub("gitleaks", Some(&gitleaks_content_report(w.root.parent().unwrap(), "added.sarif", &[(4, 14, 45), (6, 10, 40)])), 0, "");
    w.json(&scoped);
    let (code, check) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "{check}");
    let ratchet: Vec<&Value> = check["violations"].as_array().unwrap().iter().filter(|v| v["rule"] == "security:ratchet").collect();
    assert_eq!(ratchet.len(), 1, "only the added secret is new: {check}");
    assert_eq!(fingerprints(&w).len(), 3);

    // Both secrets removed from the file: the added one is gone, the recorded
    // one is still in history and still listed.
    fs::write(w.root.join("app/settings.py"), "DEBUG = False\n").unwrap();
    w.stub("gitleaks", Some(&gitleaks_content_report(w.root.parent().unwrap(), "removed.sarif", &[])), 0, "");
    w.json(&scoped);
    assert_eq!(fingerprints(&w), recorded, "a secret removed from the file stays: it is still in history");
    let (code, cleared) = w.verdict(&["check"]);
    assert_eq!(code, Some(0), "{cleared}");
}

// ── The manual's Security page (S-675, NFR-SE-01, NFR-SE-08) ────────────────
//
// `docs/howto/security.md` is held to the binary three ways: every `logos …`
// command line it prints exists with the flags it gives it, and every bare
// `--flag` span names a flag of a `logos security` command (or one of the
// scanners' own, listed in `SCANNER_FLAGS`), the `[security]` example it shows is a
// contract `check` accepts on the fixture, and the nine tested versions it
// lists are the ones the registry and the install manifest record. The span
// walk over the same page (tools and command paths) is `shipped_manual_pages`
// in `cli/src/lib.rs`.

fn security_page() -> String {
    repo_file("docs/howto/security.md")
}

/// `logos <args> --help` on stdout, or `None` when clap refuses the arguments.
fn help(args: &[&str]) -> Option<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_logos")).args(args).arg("--help").output().expect("the logos binary runs");
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The shell the page prints, as `(is_block, text)`: each line of a shell fence
/// (comment and `$ ` prompt removed) and each inline code span. A fence with an
/// info string this does not know fails loudly rather than being skipped, so a
/// block re-fenced as ```console is still read.
fn code_pieces(markdown: &str) -> Vec<(bool, String)> {
    let mut pieces = Vec::new();
    let mut fence: Option<bool> = None;
    for line in markdown.lines() {
        if let Some(info) = line.trim_start().strip_prefix("```") {
            fence = if fence.is_some() {
                None
            } else {
                Some(match info.trim() {
                    "bash" | "sh" | "shell" | "zsh" | "console" => true,
                    "toml" | "text" => false,
                    other => panic!("docs/howto/security.md opens a fence tagged {other:?}: say whether it is shell"),
                })
            };
        } else if fence == Some(true) {
            let code = line.split(" #").next().unwrap_or("").trim().trim_start_matches("$ ");
            if !code.is_empty() {
                pieces.push((true, code.to_string()));
            }
        } else if fence.is_none() {
            for span in line.split('`').skip(1).step_by(2) {
                pieces.push((false, span.split_whitespace().collect::<Vec<_>>().join(" ")));
            }
        }
    }
    pieces
}

/// The command lines `markdown` prints: the pieces that start `logos `.
fn command_lines(markdown: &str) -> Vec<String> {
    code_pieces(markdown).into_iter().map(|(_, text)| text).filter(|text| text.starts_with("logos ")).collect()
}

/// Whether `help` lists `--name` as a whole flag: `--scan` is not `--scanner`.
///
/// Only the flag column of the Options block counts: a group's help quotes
/// `--consent` and `--scanner` in its subcommands' summaries, where they are
/// not options of the group.
fn lists_flag(help: &str, name: &str) -> bool {
    let flag = format!("--{name}");
    let declared: String = help
        .lines()
        .filter(|l| l.starts_with(' ') && l.trim_start().starts_with('-'))
        .filter_map(|l| l.trim_start().split("  ").next())
        .collect::<Vec<_>>()
        .join("\n");
    declared.match_indices(&flag).any(|(at, _)| !declared[at + flag.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-'))
}

/// Flags the page names that belong to a scanner, not to Logos.
const SCANNER_FLAGS: [&str; 2] = ["metrics", "no-git"];

/// Every bare `--flag` span in `markdown` that no `logos security` command lists
/// and no scanner owns, and how many were checked.
fn unknown_bare_flags(markdown: &str) -> (Vec<String>, usize) {
    let helps: Vec<String> = ["scan", "install", "uninstall", "ingest", "status", "findings"]
        .iter()
        .map(|sub| help(&["security", sub]).unwrap_or_else(|| panic!("`logos security {sub}` has no help")))
        .collect();
    let (mut problems, mut checked) = (Vec::new(), 0);
    for (block, text) in code_pieces(markdown) {
        let Some(rest) = text.strip_prefix("--").filter(|_| !block) else { continue };
        let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        checked += 1;
        if !SCANNER_FLAGS.contains(&name.as_str()) && !helps.iter().any(|h| lists_flag(h, &name)) {
            problems.push(format!("`{text}`: no `logos security` command has `--{name}`"));
        }
    }
    (problems, checked)
}

/// Every way a command line in `markdown` is not a command the binary accepts,
/// and how many command lines were checked.
fn unknown_commands(markdown: &str) -> (Vec<String>, usize) {
    let (mut problems, mut checked) = (Vec::new(), 0);
    for line in command_lines(markdown) {
        let words: Vec<&str> = line.split_whitespace().skip(1).collect();
        let plain: Vec<&str> = words
            .iter()
            .copied()
            .take_while(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') && !w.starts_with('-'))
            .collect();
        // The longest run of plain words clap accepts: later ones are positional
        // values (`security install trivy gosec`).
        let Some((path, text)) = (1..=plain.len()).rev().find_map(|n| help(&plain[..n]).map(|text| (plain[..n].to_vec(), text))) else {
            problems.push(format!("`{line}`: no such command"));
            continue;
        };
        let usage = text.lines().find_map(|l| l.strip_prefix("Usage: logos ")).unwrap_or("");
        let named: Vec<&str> = usage.split_whitespace().take_while(|w| !w.starts_with(['[', '<', '-'])).collect();
        if named != path[..named.len().min(path.len())] || named.is_empty() {
            problems.push(format!("`{line}`: `{}` is not the command path (help says `{usage}`)", path.join(" ")));
            continue;
        }
        // A plain word left over is a positional value, which only a command
        // that takes one accepts: a group takes `<COMMAND>`, and a word it does
        // not name is no command (`security nope`).
        let takes_value = usage.split_whitespace().skip(named.len()).any(|w| {
            let w = w.trim_matches(|c| matches!(c, '[' | ']' | '<' | '>' | '.'));
            !w.is_empty() && w != "OPTIONS" && w != "COMMAND" && w.chars().all(|c| c.is_ascii_uppercase() || c == '_')
        });
        if plain.len() > named.len() && !takes_value {
            problems.push(format!("`{line}`: no such command `{}` (it takes no `{}`)", named.join(" "), plain[named.len()..].join(" ")));
            continue;
        }
        checked += 1;
        for word in &words {
            let word = word.trim_start_matches('[');
            if let Some(rest) = word.strip_prefix("--") {
                let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
                if !name.is_empty() && !lists_flag(&text, &name) {
                    problems.push(format!("`{line}`: `logos {}` has no `--{name}`", named.join(" ")));
                }
            }
        }
    }
    (problems, checked)
}

/// AC 1: every command and flag the page documents exists. The checker is
/// proved able to fail on a made-up command, a made-up flag and the near miss
/// `--scan` (a prefix of `--scanner`).
#[test]
fn every_command_and_flag_the_security_page_documents_exists() {
    let page = security_page();
    let (problems, checked) = unknown_commands(&page);
    let (bare_problems, bare) = unknown_bare_flags(&page);
    let all: Vec<String> = problems.into_iter().chain(bare_problems).collect();
    assert!(all.is_empty(), "docs/howto/security.md documents what the binary does not have:\n  {}", all.join("\n  "));
    assert!(checked >= 30, "only {checked} command lines were checked: the page stopped documenting commands, or the walk stopped reading them");
    assert!(bare >= 10, "only {bare} bare flag spans were checked");

    // Each can fail: a made-up command, a made-up flag, the near miss `--scan`,
    // a flag only a group's help prose quotes, a block fenced as `console`, and
    // bare spans that misspell a flag.
    let probe = "```console\nlogos security nope\nlogos security scan --bogus\nlogos security scan --scan\nlogos security --consent\nlogos security scan --scanner semgrep --path a\n```\nUse `--scanr <NAME>` or `--repoo`, not `--scanner <NAME>` or `--metrics off`.\n";
    let (found, probed) = unknown_commands(probe);
    assert_eq!(probed, 4, "{found:?}");
    assert_eq!(found.len(), 4, "{found:?}");
    assert!(
        found[0].contains("no such command") && found[1].contains("--bogus") && found[2].contains("`--scan`") && found[3].contains("`logos security` has no `--consent`"),
        "{found:?}"
    );
    let (misspelt, spans) = unknown_bare_flags(probe);
    assert_eq!(spans, 4, "{misspelt:?}");
    assert_eq!(misspelt.len(), 2, "{misspelt:?}");
}

/// The fenced block after `<!-- doc-test: {name} -->`.
fn marked_block(page: &str, name: &str) -> String {
    let marker = format!("<!-- doc-test: {name} -->");
    let after = page.split_once(&marker).unwrap_or_else(|| panic!("the page has no `{marker}`")).1;
    let body = after.split_once("```").expect("a fenced block follows the marker").1;
    let body = body.split_once('\n').expect("the fence line ends").1;
    body.split_once("```").expect("the fenced block closes").0.to_string()
}

/// AC 4: the `[security]` example is a contract `check` accepts on the fixture
/// once the scans it requires are fresh and `gate --save` recorded what was
/// already there, and `check` evaluates it (four rules: the ratchet, the
/// severity cap and the two categories) rather than skipping it. The same table
/// with its severity cap tightened one step fails on the fixture's high finding.
#[test]
fn the_security_example_on_the_page_passes_check_on_the_fixture_and_is_evaluated() {
    let example = marked_block(&security_page(), "security-example");
    assert!(example.contains("[security]") && example.contains("required_scanners"), "{example}");
    let w = World::new();
    let base = w.root.parent().unwrap();
    let semgrep = Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep.sarif");
    let clean_gitleaks = base.join("clean-gitleaks.sarif");
    fs::write(&clean_gitleaks, CLEAN_BANDIT.replace("Bandit", "gitleaks")).unwrap();
    w.ingest(&semgrep);
    w.rules(Some(&example));

    // Both required scanners must be fresh: gitleaks has not run yet.
    let (code, missing_gitleaks) = w.verdict(&["check"]);
    assert_eq!(code, Some(5), "{missing_gitleaks}");
    assert_eq!(missing(&missing_gitleaks), [("gitleaks", "never_run")], "{missing_gitleaks}");

    w.ingest(&clean_gitleaks);
    let (code, saved) = w.verdict(&["gate", "--save"]);
    assert_eq!(code, Some(0), "{saved}");
    let (code, check) = w.verdict(&["check"]);
    assert_eq!(code, Some(0), "{check}");
    assert_eq!((&check["passed"], &check["checked_rules"]), (&Value::Bool(true), &Value::from(4)), "the table is evaluated: {check}");
    assert!(check.get("missing_scans").is_none() && check["violations"].as_array().unwrap().is_empty(), "{check}");

    // The cap bites: tightened one step, it fails on the fixture's high finding.
    assert!(example.contains("max_severity = \"critical\""), "the probe below tightens this line: {example}");
    w.rules(Some(&example.replace("max_severity = \"critical\"", "max_severity = \"high\"")));
    let (code, tightened) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "{tightened}");
    assert!(tightened["violations"].to_string().contains("security:max_severity"), "{tightened}");

    // The ratchet bites: a new high finding fails the example as written.
    w.rules(Some(&example));
    w.ingest(&w.semgrep_with_a_new_high_finding());
    let (code, ratcheted) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "{ratcheted}");
    assert!(ratcheted["violations"].to_string().contains("security:ratchet"), "{ratcheted}");
}

/// The rows of the markdown table that follows `<!-- doc-test: {name} -->`,
/// cells trimmed and code ticks removed, header and rule rows excluded.
fn marked_table(page: &str, name: &str) -> Vec<Vec<String>> {
    let marker = format!("<!-- doc-test: {name} -->");
    let after = page.split_once(&marker).unwrap_or_else(|| panic!("the page has no `{marker}`")).1;
    after
        .lines()
        .skip_while(|l| !l.starts_with('|'))
        .take_while(|l| l.starts_with('|'))
        .skip(2)
        .map(|l| l.trim().trim_matches('|').split('|').map(|c| c.trim().replace('`', "")).collect())
        .collect()
}

/// `8.18.0` and `8.18` name one version: pad to three numeric components.
fn padded(version: &str) -> Vec<u64> {
    let mut parts: Vec<u64> = version.split('.').map(|p| p.parse().unwrap_or_else(|_| panic!("{version:?} is not a version"))).collect();
    parts.resize(3, 0);
    parts
}

/// AC 2: the page lists the tested version of each of the nine scanners, and
/// each is what the registry's module documentation records; the version Logos
/// installs and the minimum it accepts are the install manifest's. A tool the
/// manifest keeps out of the default set (S-746) reads `<version>, opt-in`, and
/// the page shows the command that names it, with its licence flag if it has one.
#[test]
fn the_page_lists_the_nine_tested_versions_the_registry_and_the_manifest_record() {
    let page = security_page();
    let rows = marked_table(&page, "tested-versions");
    let names: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
    assert_eq!(names, logos_core::config::SECURITY_SCANNERS, "one row per registered scanner, in registry order");

    // The registry's own statement: "Semgrep 1.85, Trivy 0.74, …" in its header.
    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/src/security/registry.rs")).unwrap();
    let paragraph: String = source
        .lines()
        .skip_while(|l| !l.contains("Tested versions"))
        .map(|l| l.trim_start_matches("//!").trim())
        .take_while(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    // Dependency-Check's 13.0 is where its template was verified, not written.
    assert!(paragraph.contains("written or last verified against"), "the registry no longer says what a tested version is: {paragraph}");
    let statement = paragraph.split_once("with its stderr: ").unwrap_or_else(|| panic!("the registry's statement moved: {paragraph}")).1.to_string();
    let recorded: Vec<(String, String)> = statement
        .trim_end_matches('.')
        .split(", ")
        .map(|pair| {
            let (name, version) = pair.rsplit_once(' ').unwrap_or_else(|| panic!("{pair:?}"));
            (name.to_lowercase(), version.to_string())
        })
        .collect();
    assert_eq!(recorded.len(), 9, "{statement}");
    for row in &rows {
        let (_, version) = recorded.iter().find(|(name, _)| *name == row[0]).unwrap_or_else(|| panic!("the registry records no version of {}: {recorded:?}", row[0]));
        assert_eq!(padded(&row[1]), padded(version), "{}: the page says {}, the registry {version}", row[0], row[1]);
    }

    let manifest = logos_core::tools::Manifest::builtin();
    let mut installed = 0;
    for row in &rows {
        match manifest.tool(&row[0]) {
            Some(entry) => {
                installed += 1;
                assert_eq!(padded(&row[1]), padded(&entry.min_version), "{}: the manifest accepts from {}", row[0], entry.min_version);
                let pinned = if entry.is_default() { entry.version.clone() } else { format!("{}, opt-in", entry.version) };
                assert_eq!(row[2], pinned, "{}: the manifest pins {}, default set: {}", row[0], entry.version, entry.is_default());
                if !entry.is_default() {
                    // The shown command, its comment cut off: a flag the comment
                    // mentions is not one the command passes.
                    let command = format!("logos security install {} ", entry.tool);
                    let shown: Vec<String> = command_lines(&page).into_iter().filter(|l| l.starts_with(&command)).collect();
                    let passes = |line: &String, flag: &str| line.split_whitespace().any(|word| word == flag);
                    let required: Vec<&str> = std::iter::once("--consent").chain(entry.licence().map(|licence| licence.flag)).collect();
                    assert!(
                        shown.iter().any(|line| required.iter().all(|flag| passes(line, flag))),
                        "{}: the page shows no `{command}…` command passing {required:?}, the only way it installs: {shown:?}",
                        entry.tool
                    );
                }
            }
            None => assert_eq!(row[2], "—", "{} is not installed by Logos, so the page must not say it is", row[0]),
        }
    }
    assert_eq!(installed, manifest.tools.len(), "every tool the manifest installs has a row");
}

/// AC 3 and the coordinator's S-669/S-685 notes: the page says, in words, that
/// Logos itself makes no network connection, what scan consent allows, that
/// `unknown` is never `unreachable` and what `unreachable` cannot see, and the
/// two reasons a scanner reads `stale` after a scoped re-scan. Each needle is
/// one sentence a rewrite would have to keep.
#[test]
fn the_page_states_the_network_posture_consent_and_the_reachability_limits() {
    let page = security_page().split_whitespace().collect::<Vec<_>>().join(" ");
    for needle in [
        "Logos itself makes no network connection.",
        "**What scan consent allows.**",
        "reach the network to refresh their rules or vulnerability databases",
        "It does not let a repository choose a command",
        "`serve --ui` listens on `127.0.0.1` and never dials",
        "**`unknown` is never rendered as unreachable**",
        "A function passed by value with no handoff marker, for example Rust's `iter().map(db::render)`",
        "reads `stale` with `scoped_only`",
        "`stale` with `head_moved`",
        "**Gitleaks reads content, not history, in a scoped run.**",
        "The default build links no HTTP client",
        "runs your system `curl` with a fixed argument list",
        "verifies each download's SHA-256 before it is used",
        "from `pypi.org` and the Python runtime itself from `github.com`",
        "*Tested* is the version each command line was written or last verified against",
        "**Dependency-Check and CodeQL install only on explicit opt-in.** `--all` and `security scan --install` never install them",
        "Without consent the command exits 2",
        "**Exit 5 means the evidence is missing, not that a rule failed.**",
        "A `gate --save` that exits 5 saves the metric baseline but not the security one.",
    ] {
        assert!(page.contains(needle), "docs/howto/security.md no longer says: {needle}");
    }
}

// ── The manual's security review loop (S-689, FR-SC-19, NFR-UX-05) ──────────
//
// `security.md` and `plugin.md` document the loop: the security section of
// `review --base`, the five verdicts, what an agent may and may not close, and
// the Stop hook's security leg. Each claim that can drift is held to its
// source: the classes to the enum `review` serialises, the entry fields to the
// struct, the verdicts to the skill and (where the specification is present)
// to FR-SC-19, and the hook's messages to `stop.sh`.

fn repo_file(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// `text` with every run of whitespace read as one space, so a sentence the
/// page wraps across lines is still one sentence.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The five verdicts in the order FR-SC-19 lists them.
const VERDICTS: [&str; 5] = ["fix", "escalate-secret", "escalate-dependency", "false-positive", "untrusted"];

/// The verdict table of `markdown` as `(verdict, edited)` rows.
fn verdict_rows(markdown: &str) -> Vec<(String, String)> {
    marked_table(markdown, "review-verdicts").into_iter().map(|r| (r[0].clone(), r[3].clone())).collect()
}

/// Every way `markdown`'s verdict table departs from the five verdicts: their
/// names and order, and that only a `fix` is edited.
fn verdict_problems(markdown: &str) -> Vec<String> {
    let rows = verdict_rows(markdown);
    let names: Vec<&str> = rows.iter().map(|(name, _)| name.as_str()).collect();
    let mut problems = Vec::new();
    if names != VERDICTS {
        problems.push(format!("the verdicts are {names:?}, not {VERDICTS:?}"));
    }
    let edited: Vec<&str> = rows.iter().filter(|(_, edited)| edited == "yes").map(|(name, _)| name.as_str()).collect();
    if edited != ["fix"] {
        problems.push(format!("the verdicts an agent edits are {edited:?}, not [\"fix\"]"));
    }
    // The phrases FR-SC-19 makes load-bearing: a secret is reported for
    // rotation and never closed, and a suppression stops at the guardrail.
    let table = flat(&marked_table(markdown, "review-verdicts").iter().map(|r| r.join(" | ")).collect::<Vec<_>>().join("\n"));
    for phrase in ["rotate the credential, and the value remains in git history", "never closed as fixed", "held by the guardrail"] {
        if !table.contains(phrase) {
            problems.push(format!("the verdict table no longer says: {phrase}"));
        }
    }
    problems
}

/// The verdicts FR-SC-19 lists: the bold code spans that open its bullets.
fn specified_verdicts(spec: &str) -> Vec<String> {
    spec.lines().filter_map(|l| l.strip_prefix("- **`")?.split_once("`**").map(|(name, _)| name.to_string())).collect()
}

/// The verdicts the skill's own table gives: the first cell of each row under
/// "The verdicts".
fn skill_verdicts(skill: &str) -> Vec<String> {
    let section = skill.split_once("## The verdicts").expect("the skill has a verdicts section").1;
    let section = section.split_once("\n## ").map_or(section, |(body, _)| body);
    section
        .lines()
        .filter_map(|l| l.strip_prefix("| `")?.split_once('`').map(|(name, _)| name.to_string()))
        .collect()
}

/// AC 2: the verdict table matches FR-SC-19, on both pages, and the two tables
/// are one. FR-SC-19 lives in the private specification, which the public tree
/// does not carry, so that comparison runs where it exists and the skill's own
/// table stands in everywhere else.
#[test]
fn the_verdict_tables_on_both_pages_match_the_skill_and_fr_sc_19() {
    let security = repo_file("docs/howto/security.md");
    let plugin = repo_file("docs/howto/plugin.md");
    for (label, markdown) in [("security.md", &security), ("plugin.md", &plugin)] {
        let problems = verdict_problems(markdown);
        assert!(problems.is_empty(), "docs/howto/{label}: {problems:?}");
    }
    assert_eq!(
        marked_table(&security, "review-verdicts"),
        marked_table(&plugin, "review-verdicts"),
        "the two pages state the verdicts differently"
    );

    let skill = skill_verdicts(&repo_file("plugins/logos/skills/logos-sec-review/SKILL.md"));
    let mut sorted = skill.clone();
    sorted.sort();
    let mut documented: Vec<String> = VERDICTS.iter().map(ToString::to_string).collect();
    documented.sort();
    assert_eq!(sorted, documented, "the skill's verdicts are not the documented five");

    let spec = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/specs/requirements/FR-SC-19.md");
    match fs::read_to_string(&spec) {
        Ok(text) => assert_eq!(specified_verdicts(&text), VERDICTS, "FR-SC-19 lists other verdicts than the pages"),
        Err(_) => eprintln!("FR-SC-19 is not in this tree (the public checkout): compared with the skill only"),
    }

    // The checker can fail: a renamed verdict, a reordered pair, and a second
    // verdict an agent edits.
    let renamed = security.replacen("| `escalate-secret` |", "| `escalate` |", 1);
    assert!(verdict_problems(&renamed).iter().any(|p| p.contains("not")), "{:?}", verdict_problems(&renamed));
    let edits_two: String = security
        .lines()
        .map(|l| {
            let body = l.trim_end().trim_end_matches('|');
            match l.starts_with("| `untrusted` |").then(|| body.rfind('|')).flatten() {
                Some(cut) => format!("{}| yes |", &body[..cut]),
                None => l.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(verdict_problems(&edits_two).iter().any(|p| p.contains("edits")), "{:?}", verdict_problems(&edits_two));
    let no_rotate = security.replace("rotate the credential, and the value remains in git history", "rotate");
    assert!(verdict_problems(&no_rotate).iter().any(|p| p.contains("rotate the credential")), "{:?}", verdict_problems(&no_rotate));
    assert_eq!(specified_verdicts("- **`fix`** — a\n- **`untrusted`** — b\n- not one\n"), ["fix", "untrusted"]);
}

/// The page says a clean review is not a claim that the code is secure, on
/// both pages (the statement a rewrite must keep).
#[test]
fn both_pages_say_a_clean_review_is_not_a_claim_that_the_code_is_secure() {
    const STATEMENT: &str = "**A clean review is not a claim that the code is secure.**";
    for relative in ["docs/howto/security.md", "docs/howto/plugin.md"] {
        assert!(flat(&repo_file(relative)).contains(STATEMENT), "{relative} no longer says: {STATEMENT}");
    }
}

/// The classes the page lists are the ones `review` serialises, in its order.
#[test]
fn the_review_classes_on_the_page_are_the_ones_review_serialises() {
    use logos_core::models::quality::SecurityDeltaStatus as Status;
    // Exhaustive on purpose: a sixth class stops this file compiling until it
    // has an arm here, which is where the page's table is held to it.
    fn spelled(status: Status) -> &'static str {
        match status {
            Status::Introduced => "introduced",
            Status::Touched => "touched",
            Status::Resolved => "resolved",
            Status::Stale => "stale",
            Status::PreExisting => "pre-existing",
        }
    }
    let all = [Status::Introduced, Status::Touched, Status::Resolved, Status::Stale, Status::PreExisting];
    let wire: Vec<String> = all.iter().map(|status| serde_json::to_value(status).unwrap().as_str().unwrap().to_string()).collect();
    assert_eq!(wire, all.iter().map(|s| spelled(*s)).collect::<Vec<_>>(), "the binary spells a class differently from this test");
    let documented: Vec<String> = marked_table(&repo_file("docs/howto/security.md"), "review-classes").into_iter().map(|r| r[0].clone()).collect();
    assert_eq!(documented, wire, "docs/howto/security.md lists other classes than the binary emits");
}

/// Every field the binary emits in the security section (the entry, a suppressed
/// entry, a scanner's row, the counts and the section) is named in the
/// `sections.security` subsection of `commands.md`.
#[test]
fn commands_md_names_the_fields_of_the_security_section_review_emits() {
    use logos_core::models::quality::{
        SecurityDelta, SecurityDeltaCounts, SecurityDeltaEntry, SecurityDeltaScanner, SecurityDeltaStatus, SuppressedDeltaEntry,
    };
    let entry = SecurityDeltaEntry {
        status: SecurityDeltaStatus::Introduced,
        method: SecurityDelta::METHOD_HUNK,
        scanner: "semgrep".into(),
        fingerprint: "f".into(),
        rule_id: "r".into(),
        severity: logos_core::model::Severity::High,
        owasp: vec![],
        message: "m".into(),
        redacted: false,
        path: None,
        start_line: None,
        end_line: None,
        freshness: "fresh".into(),
        reachability: "unknown".into(),
        confidence: "unbound".into(),
    };
    let keys = |value: Value| -> Vec<String> { value.as_object().expect("an object").keys().cloned().collect() };
    let mut emitted = keys(serde_json::to_value(&entry).unwrap());
    emitted.extend(keys(serde_json::to_value(SuppressedDeltaEntry { entry: entry.clone(), reason: "r".into(), expires: None }).unwrap()));
    emitted.extend(keys(
        serde_json::to_value(SecurityDeltaScanner { scanner: "s".into(), method: SecurityDelta::METHOD_HUNK, latest_head: "h".into() }).unwrap(),
    ));
    emitted.extend(keys(serde_json::to_value(SecurityDeltaCounts::default()).unwrap()));
    emitted.extend(keys(serde_json::to_value(SecurityDelta::default()).unwrap()));
    assert!(emitted.len() >= 30, "only {} fields were read: the struct stopped serialising", emitted.len());

    let commands = repo_file("docs/howto/commands.md");
    let section = commands.split_once("#### `sections.security`").expect("commands.md documents sections.security").1;
    let section = section.split_once("### `improvements`").expect("the section ends at the next command").0;
    let spans: Vec<String> = section.split('`').skip(1).step_by(2).map(str::to_string).collect();
    for key in &emitted {
        assert!(spans.contains(key), "commands.md does not name the field `{key}` of the security section");
    }
}

/// The Stop hook's messages the plugin page quotes are `stop.sh`'s own, and its
/// consent check is the scan's refusal, which is what the page says.
#[test]
fn the_plugin_page_quotes_the_stop_hooks_security_messages_and_its_consent_check() {
    let hook = repo_file("plugins/logos/hooks/stop.sh");
    let plugin = repo_file("docs/howto/plugin.md");
    let quoted: Vec<&str> = plugin
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|q| q.starts_with("security not measured"))
        .collect();
    assert!(quoted.len() >= 2, "the page quotes {} security messages: {quoted:?}", quoted.len());
    for message in &quoted {
        assert!(hook.contains(message), "plugin.md quotes `{message}`, which stop.sh does not say");
    }
    // What it runs: comments are dropped, and so is the text inside double
    // quotes (a message may name a command it never runs), but a call that
    // shares a line with a message is still read.
    let code_lines: Vec<&str> = hook.lines().filter(|l| !l.trim_start().starts_with('#')).collect();
    let unquoted = flat(&code_lines.iter().map(|l| l.split('"').step_by(2).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n"));
    assert!(!unquoted.contains("security status"), "stop.sh asks `security status`, which the page says it never does");
    assert!(!unquoted.contains("--consent") && !unquoted.contains("--install"), "stop.sh records consent or installs, which the page says it never does");
    assert!(code_lines.iter().any(|l| l.contains("invalid value for consent:")), "stop.sh's code no longer reads the scan's consent refusal");
}
