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

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

/// Printed on stderr by the failing stub: it must reach neither stream.
const PLANTED: &str = "LOGOSPLANTEDSECRET0123456789abcdef";

/// A planted NVD API key in the binary's environment: it must reach
/// Dependency-Check's private file and neither stream (FR-SC-27).
const NVD_KEY: &str = "LOGOSPLANTEDNVDKEY-0123-4567-89ab-cdef01234567";

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
             for a in \"$@\"; do case \"$prev\" in --output|--output-file|--report-path) out=\"$a\";; esac; prev=\"$a\"; done\n\
             {copy}\n\
             printf '%s\\n' '{stderr}' >&2\n\
             exit {exit}\n"
        );
        let path = self.bin.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn logos(&self, args: &[&str]) -> Output {
        self.logos_in(&self.root, args)
    }

    /// [`World::logos`] on the project at `root` — a clone of the fixture —
    /// with the same user-level directories and scanners.
    fn logos_in(&self, root: &Path, args: &[&str]) -> Output {
        self.logos_keyed(root, args, None)
    }

    /// [`World::logos_in`] with `NVD_API_KEY` set to `nvd_api_key` in the
    /// binary's environment, or removed from it when `None` — never inherited
    /// from this test process.
    fn logos_keyed(&self, root: &Path, args: &[&str], nvd_api_key: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_logos"));
        command
            .arg("--project")
            .arg(root)
            .args(args)
            .env("PATH", &self.path)
            .env("LOGOS_CONFIG_HOME", &self.user)
            .env("LOGOS_DATA_HOME", &self.data)
            .env("GIT_CEILING_DIRECTORIES", self.root.parent().unwrap());
        match nvd_api_key {
            Some(key) => command.env("NVD_API_KEY", key),
            None => command.env_remove("NVD_API_KEY"),
        };
        let out = command.output().expect("the logos binary runs");
        for (stream, secret) in [&out.stdout, &out.stderr].into_iter().flat_map(|s| [(s, PLANTED), (s, NVD_KEY)]) {
            assert!(
                !String::from_utf8_lossy(stream).contains(secret),
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
    // S-769: the cause, in one masked line of its log.
    assert_eq!(failed["cause"], "fatal: token=[masked]", "{failed}");
    assert_eq!((row(&scan, "semgrep")["outcome"].as_str(), scan["findings"].as_u64()), (Some("ran"), Some(2)));
    // FR-SC-26: each run states its coverage of the ignored directories — the
    // project ignores none — and a scanner that did not run states none.
    assert_eq!((row(&scan, "semgrep")["ignored_paths"].as_str(), row(&scan, "gitleaks")["ignored_paths"].as_str()), (Some("excluded"), Some("excluded")));
    assert!(row(&scan, "trivy").get("ignored_paths").is_none());

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
    assert_eq!(row(&after, "gitleaks")["last_failure_cause"], "fatal: token=[masked]");
    assert_eq!(row(&after, "semgrep")["ignored_paths"], "excluded");

    let findings = w.json(&["security", "findings", "--scanner", "semgrep", "--path", "app"]);
    assert_eq!(findings["findings"].as_array().unwrap().len(), 2);
    assert_eq!(contexts(&findings), BOUND_UNRESOLVED, "the graph context rides on the CLI payload");
    // S-795: no scope is production, and says so; `app/db.py` is production code.
    assert_eq!((findings["scope"].as_str(), findings["hidden_by_scope"].as_u64()), (Some("production"), Some(0)));
    assert!(findings["findings"].as_array().unwrap().iter().all(|f| f["code_scope"] == "production"), "{findings}");
    // Every flag at once, each to a value only its own slot accepts.
    let every = w.json(&[
        "security", "findings", "--severity", "medium", "--scanner", "semgrep", "--category", "A05",
        "--path", "app", "--suppression", "active", "--edition", "2025", "--scope", "all",
    ]);
    assert_eq!(every["findings"].as_array().unwrap().len(), 2, "{every}");
    assert_eq!(every["scope"], "all", "the scope reaches its own slot");
    let tests_only = w.json(&["security", "findings", "--scanner", "semgrep", "--scope", "test"]);
    assert_eq!((tests_only["findings"].as_array().unwrap().len(), tests_only["hidden_by_scope"].as_u64()), (0, Some(2)), "{tests_only}");
    assert_eq!(w.logos(&["security", "findings", "--scope", "bogus"]).status.code(), Some(2));
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
    // The refused save recorded the metric baseline but no ratchet. Each
    // baseline is decided on its own (S-763, sprint 107 review D2):
    // `session-start` keeps the one and records the missing other, before the
    // edits, so the findings already there are the session's starting point.
    let (code, started) = w.verdict(&["session-start"]);
    assert_eq!(code, Some(0), "{started}");
    let message = started["message"].as_str().unwrap();
    assert!(message.contains("baseline kept (snapshot #") && message.contains("security baseline saved (2 finding fingerprints)"), "{started}");
    assert_eq!(w.verdict(&["check"]).0, Some(0), "the findings before the edits are the baseline");
    let (code, saved) = w.verdict(&["gate", "--save"]);
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

/// [CR-238] / S-763: `session-start` on a first run records both baselines as
/// before; with a security ratchet baseline present it leaves it unchanged —
/// header and fingerprints — so a finding introduced since is still new, and
/// it decides the gate baseline on its own (sprint 107 review D2).
///
/// [CR-238]: ../../docs/requests/CR-238-sprint-106-review-follow-ups.md
#[test]
fn session_start_leaves_an_existing_security_baseline_unchanged() {
    let w = World::new();
    w.rules(Some("[constraints]\nmax_cc = 50\n\n[security]\nrequired_scanners = [\"semgrep\"]\n"));
    let semgrep = Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep.sarif");
    w.ingest(&semgrep);
    /// The ratchet's baseline header rows and its fingerprints.
    type Ratchet = (Vec<(Option<String>, String, i64)>, Vec<(String, String)>);
    let ratchet = || -> Ratchet {
        let db = rusqlite::Connection::open(w.root.join(".logos/history.db")).unwrap();
        let header = db
            .prepare("SELECT head_sha, config_hash, created_at FROM security_baseline")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let fingerprints = db
            .prepare("SELECT fingerprint, scanner_name FROM security_baseline_findings ORDER BY fingerprint")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        (header, fingerprints)
    };

    // First run: no baseline of either kind, so the session records both.
    let (code, first) = w.verdict(&["session-start"]);
    assert_eq!(code, Some(0), "{first}");
    let message = first["message"].as_str().unwrap();
    assert!(message.contains("baseline saved") && message.contains("security baseline saved (2 finding fingerprints)"), "{first}");
    // Backdated, so a rewrite within the same second still shows.
    rusqlite::Connection::open(w.root.join(".logos/history.db"))
        .unwrap()
        .execute("UPDATE security_baseline SET created_at = 1", [])
        .unwrap();
    let blessed = ratchet();
    assert_eq!((blessed.0.len(), blessed.1.len()), (1, 2), "{blessed:?}");

    // A new high finding, then a session: the ratchet's baseline is unchanged
    // and the finding is still new.
    w.ingest(&w.semgrep_with_a_new_high_finding());
    let (code, started) = w.verdict(&["session-start"]);
    assert_eq!(code, Some(0), "{started}");
    assert!(started["message"].as_str().unwrap().contains("security baseline kept"), "{started}");
    assert_eq!(ratchet(), blessed, "the session moved the ratchet's baseline");
    let (code, check) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "{check}");
    assert!(check["violations"].to_string().contains(NEW_RULE), "{check}");

    // Each baseline is decided on its own (sprint 107 review D2): with no gate
    // baseline the session saves one, whatever the ratchet's source, and still
    // leaves the ratchet's untouched — so the new finding stays new.
    let logos_db = || rusqlite::Connection::open(w.root.join(".logos/logos.db")).unwrap();
    logos_db().execute("DELETE FROM baseline", []).unwrap();
    let (code, started) = w.verdict(&["session-start"]);
    assert_eq!(code, Some(0), "{started}");
    let message = started["message"].as_str().unwrap();
    assert!(message.contains("— baseline saved (snapshot #") && message.contains("security baseline kept"), "{started}");
    let gate_baselines: i64 = logos_db().query_row("SELECT COUNT(*) FROM baseline", [], |r| r.get(0)).unwrap();
    assert_eq!(gate_baselines, 1, "{started}");
    assert_eq!(ratchet(), blessed, "{started}");
    let (_, ended) = w.verdict(&["session-end"]);
    assert!(ended["message"].as_str().unwrap().contains("holds the baseline"), "session_end compares: {ended}");
}

/// A path entry holds out an advisory osv-scanner reported against the
/// lockfile by its absolute path: every surface labels from the project's own
/// root, which is what makes that path `Cargo.lock`, so the entry holds in any
/// checkout where a fingerprint taken from it would not (S-770).
#[test]
fn a_path_entry_holds_an_absolute_lockfile_finding_out_of_every_surface() {
    let w = World::new();
    fs::write(w.root.join("Cargo.lock"), "# lockfile\n").unwrap();
    let report = w.root.parent().unwrap().join("osv.sarif");
    let result = |rule: &str| {
        serde_json::json!({
            "ruleId": rule,
            "level": "error",
            "message": {"text": format!("Package 'rmcp@1.7.0' is vulnerable to '{rule}'.")},
            "locations": [{"physicalLocation": {"artifactLocation": {"uri": format!("file://{}/Cargo.lock", w.root.display())}}}]
        })
    };
    let sarif = serde_json::json!({"version": "2.1.0", "runs": [{
        "tool": {"driver": {"name": "osv-scanner", "version": "1.9.2", "rules": []}},
        "results": [result("CVE-2026-63127"), result("CVE-2026-99999")]
    }]});
    fs::write(&report, sarif.to_string()).unwrap();
    w.ingest(&report);
    w.rules(Some("[security]\nmax_severity = \"info\"\n"));
    // The advisories each violation names: the cap and the ratchet both fail.
    let named = |verdict: &Value| -> BTreeSet<String> {
        let messages = verdict["violations"].as_array().unwrap().iter().filter_map(|v| v["message"].as_str());
        messages.flat_map(|m| ["CVE-2026-63127", "CVE-2026-99999"].into_iter().filter(|a| m.contains(a)).map(str::to_string)).collect()
    };
    let (code, before) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "{before}");
    assert_eq!(named(&before), BTreeSet::from(["CVE-2026-63127".to_string(), "CVE-2026-99999".to_string()]), "{before}");

    fs::write(
        w.root.join(".logos/security-ignore.toml"),
        "[[ignore]]\npath = \"Cargo.lock\"\nrule = \"CVE-2026-63127\"\nreason = \"code never compiled\"\n",
    )
    .unwrap();
    let (code, after) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "the other advisory still fails: {after}");
    assert_eq!(named(&after), BTreeSet::from(["CVE-2026-99999".to_string()]), "{after}");

    let findings = w.json(&["security", "findings"]);
    let held = findings["findings"].as_array().unwrap().iter().find(|f| f["rule_id"] == "CVE-2026-63127").unwrap();
    assert!(held["report_path"].as_str().unwrap().starts_with('/'), "the report named it absolutely: {held}");
    assert_eq!(held["suppression"]["state"], "suppressed", "{held}");
    assert_eq!(held["suppression"]["by"], serde_json::json!({"path": "Cargo.lock", "rule": "CVE-2026-63127"}));
    let status = w.json(&["security", "status"]);
    assert_eq!(status["suppressions"][0]["suppressed"], 1, "{status}");
    assert_eq!(status["suppressions"][0]["orphaned"], false, "{status}");
}

/// A rule-wide entry (a rule and its scanner, no path, no fingerprint) holds
/// that rule's findings out of every file on every surface and out of the
/// `[security]` gate — and an expired one hands them back (S-814).
#[test]
fn a_rule_wide_entry_holds_a_rule_out_of_every_file_and_the_gate_until_it_expires() {
    let w = World::new();
    for lock in ["Cargo.lock", "web/Cargo.lock"] {
        fs::create_dir_all(w.root.join(lock).parent().unwrap()).unwrap();
        fs::write(w.root.join(lock), "# lockfile\n").unwrap();
    }
    let report = w.root.parent().unwrap().join("osv-rule-wide.sarif");
    let result = |rule: &str, lock: &str| {
        serde_json::json!({
            "ruleId": rule,
            "level": "error",
            "message": {"text": format!("Package 'rmcp@1.7.0' is vulnerable to '{rule}'.")},
            "locations": [{"physicalLocation": {"artifactLocation": {"uri": format!("file://{}/{lock}", w.root.display())}}}]
        })
    };
    let sarif = serde_json::json!({"version": "2.1.0", "runs": [{
        "tool": {"driver": {"name": "osv-scanner", "version": "1.9.2", "rules": []}},
        "results": [
            result("CVE-2026-63127", "Cargo.lock"),
            result("CVE-2026-63127", "web/Cargo.lock"),
            result("CVE-2026-99999", "Cargo.lock"),
        ]
    }]});
    fs::write(&report, sarif.to_string()).unwrap();
    w.ingest(&report);
    w.rules(Some("[security]\nmax_severity = \"info\"\n"));
    let named = |verdict: &Value| -> BTreeSet<String> {
        let messages = verdict["violations"].as_array().unwrap().iter().filter_map(|v| v["message"].as_str());
        messages.flat_map(|m| ["CVE-2026-63127", "CVE-2026-99999"].into_iter().filter(|a| m.contains(a)).map(str::to_string)).collect()
    };
    let (code, before) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "{before}");
    assert_eq!(named(&before), BTreeSet::from(["CVE-2026-63127".to_string(), "CVE-2026-99999".to_string()]), "{before}");

    let entry = |expires: &str| {
        format!(
            "[[ignore]]\nrule = \"CVE-2026-63127\"\nscanner = \"OSV-Scanner\"\nreason = \"code never compiled\"\n{expires}"
        )
    };
    fs::write(w.root.join(".logos/security-ignore.toml"), entry("")).unwrap();
    let (code, after) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "the other advisory still fails: {after}");
    assert_eq!(named(&after), BTreeSet::from(["CVE-2026-99999".to_string()]), "{after}");
    let (code, gated) = w.verdict(&["gate"]);
    assert_eq!(code, Some(1), "the gate fails on the other advisory alone: {gated}");
    let gated = gated.to_string();
    assert!(gated.contains("CVE-2026-99999") && !gated.contains("CVE-2026-63127"), "{gated}");

    let findings = w.json(&["security", "findings"]);
    let by_rule = |rule: &str| -> Vec<&Value> {
        findings["findings"].as_array().unwrap().iter().filter(|f| f["rule_id"] == rule).collect()
    };
    assert_eq!(by_rule("CVE-2026-63127").len(), 2, "{findings}");
    for held in by_rule("CVE-2026-63127") {
        assert_eq!(held["suppression"]["state"], "suppressed", "{held}");
        assert_eq!(held["suppression"]["reason"], "code never compiled", "{held}");
        assert_eq!(held["suppression"]["by"], serde_json::json!({"rule": "CVE-2026-63127", "scanner": "OSV-Scanner"}), "{held}");
    }
    assert!(by_rule("CVE-2026-99999")[0]["suppression"].is_null(), "a rule one entry does not name stays listed active");
    let status = w.json(&["security", "status"]);
    assert_eq!(status["suppressions"][0]["suppressed"], 2, "{status}");
    assert_eq!(status["suppressions"][0]["orphaned"], false, "{status}");
    assert_eq!(status["suppressions"][0]["entry"]["rule"], "CVE-2026-63127", "{status}");

    // Dated in the past, the entry no longer holds anything out.
    fs::write(w.root.join(".logos/security-ignore.toml"), entry("expires = 2020-01-01\n")).unwrap();
    let (code, lapsed) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "{lapsed}");
    assert_eq!(named(&lapsed), BTreeSet::from(["CVE-2026-63127".to_string(), "CVE-2026-99999".to_string()]), "{lapsed}");
    let findings = w.json(&["security", "findings"]);
    assert_eq!(by_rule_state(&findings, "CVE-2026-63127"), ["expired", "expired"], "{findings}");

    // A rule with no scanner is a configuration fault: exit 2, naming the key.
    fs::write(w.root.join(".logos/security-ignore.toml"), "[[ignore]]\nrule = \"CVE-2026-63127\"\nreason = \"r\"\n").unwrap();
    let out = w.logos(&["security", "findings"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("has a `rule` but no `scanner`"), "{out:?}");
}

fn by_rule_state(findings: &Value, rule: &str) -> Vec<String> {
    findings["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["rule_id"] == rule)
        .map(|f| f["suppression"]["state"].as_str().unwrap_or("none").to_string())
        .collect()
}

/// A finding in a fixtures directory reads `test`, is hidden from the default
/// listing and still fails the `[security]` gate: with no `.logos/security-ignore.toml`
/// entry for the directory, which is what this repository's own file dropped
/// once code scope told fixtures from shipped code (S-796, FR-SC-28, BR-98).
#[test]
fn a_fixtures_finding_reads_test_is_hidden_by_default_and_still_reaches_the_gate() {
    let w = World::new();
    let fixture = w.root.join("tests/fixtures/security-context/axum");
    fs::create_dir_all(&fixture).unwrap();
    fs::write(fixture.join("Cargo.lock"), "# lockfile\n").unwrap();
    let report = w.root.parent().unwrap().join("osv-fixture.sarif");
    let sarif = serde_json::json!({"version": "2.1.0", "runs": [{
        "tool": {"driver": {"name": "osv-scanner", "version": "1.9.2", "rules": []}},
        "results": [{
            "ruleId": "CVE-2026-11111",
            "level": "error",
            "message": {"text": "Package 'axum@0.7.0' is vulnerable to 'CVE-2026-11111'."},
            "locations": [{"physicalLocation": {"artifactLocation": {"uri": format!("file://{}/Cargo.lock", fixture.display())}}}]
        }]
    }]});
    fs::write(&report, sarif.to_string()).unwrap();
    w.ingest(&report);
    w.rules(Some("[security]\nmax_severity = \"info\"\n"));
    assert!(!w.root.join(".logos/security-ignore.toml").exists(), "no entry holds the fixtures out");

    let listed = |scope: &[&str]| {
        let mut args = vec!["security", "findings"];
        args.extend_from_slice(scope);
        w.json(&args)
    };
    let default = listed(&[]);
    assert_eq!(default["findings"].as_array().unwrap().len(), 0, "the default lists production only: {default}");
    assert_eq!(default["hidden_by_scope"], 1, "and says it hid one: {default}");
    let all = listed(&["--scope", "all"]);
    let finding = &all["findings"][0];
    assert_eq!((finding["code_scope"].as_str(), finding["code_scope_reason"].as_str()), (Some("test"), Some("test_path")), "{finding}");
    assert_eq!(listed(&["--scope", "test"])["findings"].as_array().unwrap().len(), 1);

    // The gate keeps it: the scope is not the gate's.
    for verb in ["check", "gate"] {
        let (code, verdict) = w.verdict(&[verb]);
        assert_eq!(code, Some(1), "{verb} fails on the fixture finding: {verdict}");
        assert!(verdict.to_string().contains("CVE-2026-11111"), "{verb} names it: {verdict}");
    }
}

/// `app/cookie.py`: the symbol a verdict about `delete`'s query rests on.
const COOKIE_PY: &str = "def default_cookie_config():\n    return {\"secure\": True}\n";

/// A suppression whose evidence changed counts again, fails a `[security]` cap
/// and reads "verdict out of date" naming the symbol, until `security reconfirm`
/// re-records the hashes and keeps the reason ([S-818], [FR-SC-32], BR-102):
/// - `reconfirm --evidence` gives an entry that records none its first evidence,
///   adding the finding's own enclosing symbol;
/// - flipping a literal in the evidence symbol re-opens the finding, which the
///   cap then fails on; re-confirming holds it out again with the original reason;
/// - deleting the symbol re-opens it too, and `reconfirm` then refuses, writing
///   nothing;
/// - an entry that records no evidence is unchanged and reads as such.
///
/// [S-818]: ../../docs/planning/journal.md#s-818-a-suppression-records-its-evidence-and-re-opens-when-the-evidence-changes
/// [FR-SC-32]: ../../docs/specs/requirements/FR-SC-32.md
#[test]
fn a_suppression_whose_evidence_changed_counts_again_until_it_is_reconfirmed() {
    let w = World::new();
    let cookie = w.root.join("app/cookie.py");
    fs::write(&cookie, COOKIE_PY).unwrap();
    assert!(w.logos(&["index"]).status.success());
    let semgrep = Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep.sarif");
    w.ingest(&semgrep);
    w.rules(Some("[security]\nmax_severity = \"high\"\n"));
    let findings = w.json(&["security", "findings"]);
    let high = findings["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["severity"] == "high")
        .unwrap_or_else(|| panic!("the high finding: {findings}"));
    let fingerprint = high["fingerprint"].as_str().unwrap().to_string();
    let state = |payload: &Value| -> Value {
        payload["findings"].as_array().unwrap().iter().find(|f| f["fingerprint"] == fingerprint.as_str()).unwrap()["suppression"].clone()
    };
    let cap_fails = || {
        let (code, verdict) = w.verdict(&["check"]);
        (code == Some(1) && verdict["violations"].to_string().contains("security:max_severity"), verdict)
    };
    assert!(cap_fails().0, "the unsuppressed high finding fails the cap");

    // A person's entry with no evidence: suppressed as before, labelled so.
    let ignore = w.root.join(".logos/security-ignore.toml");
    let entry = format!("[[ignore]]\nfingerprint = \"{fingerprint}\"\nreason = \"the query is built from constants\"\n");
    fs::write(&ignore, &entry).unwrap();
    let held = state(&w.json(&["security", "findings"]));
    assert_eq!((held["state"].as_str(), held.get("evidence").is_none()), (Some("suppressed"), true), "{held}");
    assert!(!cap_fails().0, "the suppressed finding is out of the cap");
    let out = w.logos(&["security", "reconfirm", &fingerprint]);
    assert_eq!(out.status.code(), Some(2), "nothing to re-confirm without evidence: {out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("records no evidence"), "{out:?}");
    assert_eq!(fs::read_to_string(&ignore).unwrap(), entry, "a refusal writes nothing");

    // `reconfirm --evidence` records the named symbol and the enclosing one.
    let written = w.json(&["security", "reconfirm", &fingerprint, "--evidence", "default_cookie_config"]);
    assert_eq!(written["file"], ".logos/security-ignore.toml");
    let symbols: Vec<&str> = written["entry"]["evidence"].as_array().unwrap().iter().map(|e| e["symbol"].as_str().unwrap()).collect();
    assert_eq!(symbols.len(), 2, "{written}");
    assert!(symbols[0].ends_with("default_cookie_config().") && symbols[1].ends_with("delete()."), "{symbols:?}");
    let held = state(&w.json(&["security", "findings"]));
    assert_eq!(held["state"], "suppressed", "{held}");
    let names: Vec<&str> = held["evidence"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["default_cookie_config", "delete"], "{held}");
    assert!(!cap_fails().0);
    let with_evidence = fs::read_to_string(&ignore).unwrap();

    // The literal flips: the verdict is out of date, the finding counts again.
    fs::write(&cookie, COOKIE_PY.replace("True", "False")).unwrap();
    assert!(w.logos(&["sync", "app/cookie.py"]).status.success());
    let findings = w.json(&["security", "findings"]);
    let reopened = state(&findings);
    assert_eq!(reopened["state"], "out_of_date", "{reopened}");
    assert_eq!(reopened["reason"], "the query is built from constants", "{reopened}");
    assert_eq!((reopened["changed"][0]["name"].as_str(), reopened["changed"][0]["change"].as_str()), (Some("default_cookie_config"), Some("changed")));
    let (fails, verdict) = cap_fails();
    assert!(fails, "an out-of-date verdict fails the cap: {verdict}");
    let status = w.json(&["security", "status"]);
    assert_eq!(status["suppressions"][0]["out_of_date"][0]["name"], "default_cookie_config", "{status}");
    assert_eq!(status["suppressions"][0]["suppressed"], 0, "{status}");
    assert_eq!(fs::read_to_string(&ignore).unwrap(), with_evidence, "reading rewrites nothing");

    // Re-confirmed, it is suppressed again with its original reason.
    let confirmed = w.json(&["security", "reconfirm", &fingerprint]);
    assert_eq!(confirmed["entry"]["reason"], "the query is built from constants");
    let back = state(&w.json(&["security", "findings"]));
    assert_eq!((back["state"].as_str(), back["reason"].as_str()), (Some("suppressed"), Some("the query is built from constants")), "{back}");
    assert!(!cap_fails().0);

    // Deleting the symbol re-opens it, and it cannot be re-confirmed.
    fs::remove_file(&cookie).unwrap();
    assert!(w.logos(&["sync", "app/cookie.py"]).status.success());
    let gone = state(&w.json(&["security", "findings"]));
    assert_eq!((gone["state"].as_str(), gone["changed"][0]["change"].as_str()), (Some("out_of_date"), Some("gone")), "{gone}");
    assert!(cap_fails().0, "a deleted evidence symbol fails the cap");
    let before = fs::read(&ignore).unwrap();
    let out = w.logos(&["security", "reconfirm", &fingerprint]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("default_cookie_config"), "{out:?}");
    assert_eq!(fs::read(&ignore).unwrap(), before, "a refused re-confirm writes nothing");
}

/// A finding its scanner marks low-confidence (the low-confidence Semgrep fixture's
/// two rules are `LOW CONFIDENCE`) reads "needs review": left out of the default list and
/// of `status`'s count with the number stated, listed and labelled by `--scope
/// all`, and still failing a `[security]` cap and still in the blessed baseline
/// (S-817, FR-SC-31, BR-101).
#[test]
fn a_needs_review_finding_is_left_out_of_every_list_and_count_and_still_reaches_the_cap_and_bless() {
    let w = World::new();
    let semgrep = Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep-low-confidence.sarif");
    w.ingest(&semgrep);
    w.rules(Some("[security]\nmax_severity = \"high\"\n"));

    let default = w.json(&["security", "findings"]);
    assert_eq!(default["findings"].as_array().unwrap().len(), 0, "the default lists no finding that needs review: {default}");
    assert_eq!((&default["hidden_needs_review"], &default["hidden_by_scope"]), (&serde_json::json!(2), &serde_json::json!(0)), "{default}");
    let all = w.json(&["security", "findings", "--scope", "all"]);
    let listed = all["findings"].as_array().unwrap();
    assert_eq!(listed.len(), 2, "{all}");
    for finding in listed {
        assert_eq!((finding["confidence"].as_str(), finding["needs_review"].as_bool()), (Some("low"), Some(true)), "{finding}");
    }
    assert_eq!(all["hidden_needs_review"], 0, "`--scope all` hides nothing: {all}");

    let status = w.json(&["security", "status"]);
    let semgrep_row = row(&status, "semgrep");
    assert_eq!((&semgrep_row["findings"], &semgrep_row["needs_review"]), (&serde_json::json!(0), &serde_json::json!(2)), "{semgrep_row}");

    // The cap counts them: the report holds a `high` one.
    for verb in ["check", "gate"] {
        let (code, verdict) = w.verdict(&[verb]);
        assert_eq!(code, Some(1), "{verb} fails on a finding that needs review: {verdict}");
        assert!(verdict.to_string().contains("python.sqlalchemy.security.sqlalchemy-execute-raw-query"), "{verb} names it: {verdict}");
    }
    let (code, blessed) = w.verdict(&["security", "bless"]);
    assert_eq!((code, &blessed["findings"]), (Some(0), &serde_json::json!(2)), "bless records both: {blessed}");
}

/// A finding on a package its `package-lock.json` marks `"dev": true` reads `dev`:
/// left out of the default list and of `status`'s count with the number stated,
/// listed and labelled by `--scope all`, a production npm finding beside it
/// staying in the count — and the dev one alone still fails a `[security]` cap
/// and is in the blessed baseline (S-810, FR-SC-30, BR-100).
#[test]
fn a_dev_only_dependency_finding_is_left_out_of_every_list_and_count_and_still_reaches_the_cap_and_bless() {
    let w = World::new();
    let web = w.root.join("web");
    fs::create_dir_all(&web).unwrap();
    fs::write(
        web.join("package-lock.json"),
        r#"{"lockfileVersion":3,"packages":{"node_modules/vite":{"version":"5.0.0","dev":true},"node_modules/react":{"version":"18.2.0"}}}"#,
    )
    .unwrap();
    let result = |package: &str, advisory: &str, level: &str| {
        serde_json::json!({
            "ruleId": advisory,
            "level": level,
            "message": {"text": format!("Package '{package}' is vulnerable to '{advisory}'.")},
            "locations": [{"physicalLocation": {"artifactLocation": {"uri": format!("file://{}/package-lock.json", web.display())}}}]
        })
    };
    let report = w.root.parent().unwrap().join("osv-dev.sarif");
    // OSV-Scanner's `error` is `high`; the production one is a `note`, so only the dev one reaches the cap.
    let sarif = serde_json::json!({"version": "2.1.0", "runs": [{
        "tool": {"driver": {"name": "osv-scanner", "version": "1.9.2", "rules": []}},
        "results": [result("vite@5.0.0", "GHSA-vite-dev", "error"), result("react@18.2.0", "GHSA-react-prod", "note")]
    }]});
    fs::write(&report, sarif.to_string()).unwrap();
    w.ingest(&report);
    w.rules(Some("[security]\nmax_severity = \"high\"\n"));

    let default = w.json(&["security", "findings"]);
    let rules = |v: &Value| v["findings"].as_array().unwrap().iter().map(|f| f["rule_id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(rules(&default), ["GHSA-react-prod"], "the default lists no dev-only finding: {default}");
    assert_eq!((&default["hidden_dev"], &default["hidden_by_scope"]), (&serde_json::json!(1), &serde_json::json!(0)), "{default}");
    let all = w.json(&["security", "findings", "--scope", "all"]);
    let dev = all["findings"].as_array().unwrap().iter().find(|f| f["rule_id"] == "GHSA-vite-dev").unwrap();
    assert_eq!((dev["code_scope"].as_str(), dev["code_scope_reason"].as_str()), (Some("dev"), Some("dev_dependency")), "{dev}");
    assert_eq!((all["findings"].as_array().unwrap().len(), &all["hidden_dev"]), (2, &serde_json::json!(0)), "`--scope all` hides nothing: {all}");
    let prod = all["findings"].as_array().unwrap().iter().find(|f| f["rule_id"] == "GHSA-react-prod").unwrap();
    assert_eq!(prod["code_scope"], "production", "{prod}");

    let status = w.json(&["security", "status"]);
    let osv = row(&status, "osv-scanner");
    assert_eq!((&osv["findings"], &osv["dev"]), (&serde_json::json!(1), &serde_json::json!(1)), "{osv}");

    // The cap counts it: only the dev one is high.
    for verb in ["check", "gate"] {
        let (code, verdict) = w.verdict(&[verb]);
        assert_eq!(code, Some(1), "{verb} fails on a dev-only dependency's high finding: {verdict}");
        assert!(verdict.to_string().contains("GHSA-vite-dev"), "{verb} names it: {verdict}");
    }
    let (code, blessed) = w.verdict(&["security", "bless"]);
    assert_eq!((code, &blessed["findings"]), (Some(0), &serde_json::json!(2)), "bless records both: {blessed}");
}

/// [UAT-SC-14] (S-809, FR-SC-29) through the binary, on a fixture shaped on
/// desk-picker: OSV-Scanner holds one `go.mod` result twice, byte for byte,
/// and Trivy repeats that advisory under its CVE; OSV-Scanner also reports a
/// dev-only and a production npm advisory.
/// - `security findings` lists the `go.mod` advisory once, naming both
///   scanners, at Trivy's `high` (OSV-Scanner's own is unrated); the
///   headline holds it and the production npm one, and states the dev one.
/// - `status` counts it once, on Trivy's row, and states it merged on
///   OSV-Scanner's.
/// - A `[security]` cap counts each of the three advisories once.
/// - A checked-in baseline and an ignore file written before the merge, which
///   name OSV-Scanner's key alone, still match it: the ratchet reads nothing
///   new, and the entry suppresses both reporters on every surface.
///
/// [UAT-SC-14]: ../../docs/specs/requirements/UAT-SC-14.md
#[test]
fn one_lockfile_advisory_two_scanners_report_reads_once_on_every_surface() {
    let w = World::new();
    let web = w.root.join("web");
    fs::create_dir_all(&web).unwrap();
    fs::write(
        web.join("package-lock.json"),
        r#"{"lockfileVersion":3,"packages":{"node_modules/vite":{"version":"5.0.0","dev":true},"node_modules/react":{"version":"18.2.0"}}}"#,
    )
    .unwrap();
    fs::write(w.root.join("go.mod"), "module example.test/app\n").unwrap();
    let osv = |package: &str, advisory: &str, aka: &str, path: &str, level: &str| {
        serde_json::json!({
            "ruleId": advisory, "level": level,
            "message": {"text": format!("Package '{package}' is vulnerable to '{advisory}' (also known as {aka}).")},
            "locations": [{"physicalLocation": {"artifactLocation": {"uri": format!("file://{}/{path}", w.root.display())}}}]
        })
    };
    let go = osv("github.com/xuri/excelize/v2@2.11.0", "CVE-2026-107214", "'GO-2026-6684', 'GHSA-2j4c-ffch-9f23'", "go.mod", "warning");
    let trivy = serde_json::json!({
        "ruleId": "CVE-2026-107214", "level": "error",
        "message": {"text": "Package: github.com/xuri/excelize/v2\nInstalled Version: v2.11.0\nVulnerability CVE-2026-107214\nSeverity: HIGH"},
        "locations": [{"physicalLocation": {"artifactLocation": {"uri": "go.mod", "uriBaseId": "ROOTPATH"}, "region": {"startLine": 1}}}]
    });
    let report = w.root.parent().unwrap().join("desk-picker.sarif");
    let sarif = serde_json::json!({"version": "2.1.0", "runs": [
        {"tool": {"driver": {"name": "osv-scanner", "version": "2.7.0", "rules": []}}, "results": [
            go.clone(),
            go,
            osv("vite@5.0.0", "GHSA-vite-dev", "'CVE-2026-1'", "web/package-lock.json", "error"),
            osv("react@18.2.0", "GHSA-react-prod", "'CVE-2026-2'", "web/package-lock.json", "error"),
        ]},
        {"tool": {"driver": {"name": "Trivy", "version": "0.76.0", "rules": []}}, "results": [trivy]}
    ]});
    fs::write(&report, sarif.to_string()).unwrap();
    w.ingest(&report);

    let all = w.json(&["security", "findings", "--scope", "all"]);
    let listed = all["findings"].as_array().unwrap();
    assert_eq!(listed.len(), 3, "three advisories, the go.mod one once: {all}");
    let go_mod = listed.iter().find(|f| f["rule_id"] == "CVE-2026-107214").unwrap();
    let reporters = |f: &Value| -> Vec<(String, String)> {
        f["reporters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["scanner_name"].as_str().unwrap().to_string(), r["fingerprint"].as_str().unwrap().to_string()))
            .collect()
    };
    let named = reporters(go_mod);
    assert_eq!(named.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(), ["Trivy", "osv-scanner"], "{go_mod}");
    assert_eq!(go_mod["severity"], "high", "{go_mod}");
    let dev = listed.iter().find(|f| f["rule_id"] == "GHSA-vite-dev").unwrap();
    assert_eq!(dev["code_scope"], "dev", "{dev}");
    let default = w.json(&["security", "findings"]);
    let rules = |v: &Value| v["findings"].as_array().unwrap().iter().map(|f| f["rule_id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(rules(&default), ["CVE-2026-107214", "GHSA-react-prod"], "{default}");
    assert_eq!(default["hidden_dev"], 1, "1 more in dev-only dependencies: {default}");

    let status = w.json(&["security", "status"]);
    let counts = |scanner: &str| {
        let r = row(&status, scanner);
        (r["findings"].as_u64(), r["dev"].as_u64(), r["merged"].as_u64())
    };
    assert_eq!(counts("trivy"), (Some(1), Some(0), Some(0)), "{status}");
    assert_eq!(counts("osv-scanner"), (Some(1), Some(1), Some(1)), "{status}");

    w.rules(Some("[security]\nratchet_min_severity = \"critical\"\nmax_severity = \"high\"\n"));
    let (code, check) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "{check}");
    let capped: Vec<&str> = check["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["rule"] == "security:max_severity")
        .map(|v| v["message"].as_str().unwrap())
        .collect();
    assert_eq!(capped.len(), 3, "each advisory once: {capped:?}");
    assert_eq!(capped.iter().filter(|m| m.contains("CVE-2026-107214") && m.contains("(Trivy, osv-scanner)")).count(), 1, "{capped:?}");

    // Written before the merge, both name OSV-Scanner's key alone.
    let osv_key = &named[1].1;
    let others: Vec<String> = listed.iter().filter(|f| f["rule_id"] != "CVE-2026-107214").map(|f| f["fingerprint"].as_str().unwrap().to_string()).collect();
    let entry = |fingerprint: &str, rule: &str, path: &str| serde_json::json!({"fingerprint": fingerprint, "rule_id": rule, "path": path});
    let baseline = serde_json::json!({"version": 1, "findings": [
        entry(osv_key, "CVE-2026-107214", "go.mod"),
        entry(&others[0], "x", "web/package-lock.json"),
        entry(&others[1], "y", "web/package-lock.json"),
    ]});
    fs::write(w.root.join(".logos/security-baseline.json"), serde_json::to_string_pretty(&baseline).unwrap() + "\n").unwrap();
    w.rules(Some("[security]\nratchet_min_severity = \"high\"\n"));
    let (code, check) = w.verdict(&["check"]);
    assert_eq!((code, ratchet_violations(&check).len()), (Some(0), 0), "OSV-Scanner's key holds the merged finding: {check}");

    fs::write(w.root.join(".logos/security-ignore.toml"), format!("[[ignore]]\nfingerprint = \"{osv_key}\"\nreason = \"fixed upstream\"\n")).unwrap();
    let default = w.json(&["security", "findings"]);
    let go_mod = default["findings"].as_array().unwrap().iter().find(|f| f["rule_id"] == "CVE-2026-107214").unwrap();
    assert_eq!(go_mod["suppression"]["state"], "suppressed", "{go_mod}");
    let status = w.json(&["security", "status"]);
    assert_eq!((row(&status, "trivy")["findings"].as_u64(), row(&status, "trivy")["suppressed"].as_u64()), (Some(0), Some(1)), "{status}");
    assert_eq!(status["suppressions"][0]["suppressed"], 1, "{status}");
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
        "gitleaks 8.18.4 from github.com (", "osv-scanner 2.7.0 from github.com (", "gosec 2.29.0 from github.com (",
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

/// A Gitleaks 8.18.4 report on `app/settings.py`, one result per
/// `(commit, line, start column, end column)` in `app/settings.py`: a
/// history-mode result names its commit, a scoped (`--no-git`) one names none,
/// and history-mode results are dated a day apart in the order given, oldest
/// first. Written as Gitleaks writes it with `--redact`: the snippet holds no
/// secret.
fn gitleaks_report(dir: &Path, name: &str, results: &[(Option<&str>, i64, i64, i64)]) -> PathBuf {
    let results: Vec<_> = results.iter().map(|&(commit, line, start, end)| (commit, "app/settings.py", line, start, end)).collect();
    gitleaks_report_in(dir, name, &results)
}

/// [`gitleaks_report`] over results in any file: `(commit, path, line, start
/// column, end column)`.
fn gitleaks_report_in(dir: &Path, name: &str, results: &[(Option<&str>, &str, i64, i64, i64)]) -> PathBuf {
    let results: Vec<Value> = results
        .iter()
        .enumerate()
        .map(|(day, &(commit, uri, line, start, end))| {
            let (sha, date) = commit.map_or((String::new(), String::new()), |sha| (sha.to_string(), format!("2026-10-{:02}T09:00:00Z", day + 1)));
            serde_json::json!({
                "ruleId": "generic-api-key",
                "message": { "text": format!("generic-api-key has detected secret for file {uri}.") },
                "locations": [{ "physicalLocation": {
                    "artifactLocation": { "uri": uri },
                    "region": { "startLine": line, "startColumn": start, "endLine": line, "endColumn": end,
                                "snippet": { "text": "REDACTED" } }
                } }],
                "partialFingerprints": { "commitSha": sha, "email": "", "author": "", "date": date, "commitMessage": "" }
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

/// The commit `rev` names in `root`.
fn rev(root: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .current_dir(root)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .args(["rev-parse", rev])
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git rev-parse {rev}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// One Gitleaks identity across scan modes and commits ([S-765]), through the
/// binary and the `[security]` gate. The full scan read the secret in the two
/// commits that wrote its line — at columns `2..42` and `1..41`, as Gitleaks
/// 8.18.4 reports a line that is or is not the first it matched — and lists it
/// once with both commits; a scoped run reads the file (`--no-git`), so
/// - the recorded secret re-scanned scoped keeps its fingerprint — it is not
///   new, and `check` still passes;
/// - a newly added secret is new, and `check` fails on it alone;
/// - a secret removed from the file stays listed: it is still in history.
///
/// [S-765]: ../../docs/planning/journal.md#s-765-a-gitleaks-finding-keeps-one-identity-across-scan-modes-and-commits
#[test]
fn a_scoped_gitleaks_run_keeps_recorded_secrets_known_and_history_findings_listed() {
    let w = World::new();
    let key = "API_KEY = \"lgs_pl4nted_S3cr3t_q7Vx9Rz2Kw\"";
    fs::write(w.root.join("app/settings.py"), format!("DEBUG = False\nAPI_TIMEOUT = 30\n\n{key}\n")).unwrap();
    git(&w.root, &["add", "app/settings.py"]);
    git(&w.root, &["commit", "-q", "-m", "settings"]);
    fs::write(w.root.join("app/settings.py"), format!("DEBUG = False\nAPI_TIMEOUT = 30\n\n{key}  # rotate me\n")).unwrap();
    git(&w.root, &["commit", "-q", "-am", "note the rotation"]);
    let (first, second) = (rev(&w.root, "HEAD~1"), rev(&w.root, "HEAD"));
    w.rules(Some("[security]\nratchet_min_severity = \"medium\"\n"));
    let dir = w.root.parent().unwrap();
    w.ingest(&gitleaks_report(dir, "history.sarif", &[(Some(&first), 4, 2, 42), (Some(&second), 4, 1, 41)]));
    let listed = |w: &World| -> Vec<Value> {
        w.json(&["security", "findings", "--scanner", "gitleaks"])["findings"].as_array().unwrap().clone()
    };
    let fingerprints = |w: &World| -> Vec<String> {
        let mut prints: Vec<String> = listed(w).iter().map(|f| f["fingerprint"].as_str().unwrap().to_string()).collect();
        prints.sort();
        prints
    };
    let recorded = listed(&w);
    assert_eq!(recorded.len(), 1, "one secret, one finding: {recorded:?}");
    assert_eq!(recorded[0]["commits"], serde_json::json!([second, first]), "its commits, newest first");
    let recorded = fingerprints(&w);
    let (code, saved) = w.verdict(&["gate", "--save"]);
    assert_eq!(code, Some(0), "{saved}");
    let (code, before) = w.verdict(&["check"]);
    assert_eq!(code, Some(0), "{before}");

    let scoped = ["security", "scan", "--consent", "--scanner", "gitleaks", "--path", "app/settings.py"];
    // The recorded secret, re-scanned in content mode.
    w.stub("gitleaks", Some(&gitleaks_report(dir, "same.sarif", &[(None, 4, 2, 42)])), 0, "");
    assert_eq!(row(&w.json(&scoped), "gitleaks")["outcome"], "ran");
    assert_eq!(fingerprints(&w), recorded, "the recorded secret keeps its fingerprint; no copy is added");
    let (code, after) = w.verdict(&["check"]);
    assert_eq!(code, Some(0), "an already-recorded secret is not new: {after}");
    assert_eq!(after["violations"], before["violations"], "the gate verdict is unchanged");

    // A newly added secret on line 6: new, and the only violation.
    let added = format!("DEBUG = False\nAPI_TIMEOUT = 30\n\n{key}  # rotate me\n\nTOKEN = \"lgs_s3cond_W4xT9mQ2pR7vK5\"\n");
    fs::write(w.root.join("app/settings.py"), added).unwrap();
    w.stub("gitleaks", Some(&gitleaks_report(dir, "added.sarif", &[(None, 4, 2, 42), (None, 6, 2, 36)])), 0, "");
    w.json(&scoped);
    let (code, check) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "{check}");
    let ratchet: Vec<&Value> = check["violations"].as_array().unwrap().iter().filter(|v| v["rule"] == "security:ratchet").collect();
    assert_eq!(ratchet.len(), 1, "only the added secret is new: {check}");
    assert_eq!(fingerprints(&w).len(), 2);

    // Both secrets removed from the file: the added one is gone, the recorded
    // one is still in history and still listed.
    fs::write(w.root.join("app/settings.py"), "DEBUG = False\n").unwrap();
    w.stub("gitleaks", Some(&gitleaks_report(dir, "removed.sarif", &[])), 0, "");
    w.json(&scoped);
    assert_eq!(fingerprints(&w), recorded, "a secret removed from the file stays: it is still in history");
    let (code, cleared) = w.verdict(&["check"]);
    assert_eq!(code, Some(0), "{cleared}");
}

/// A secret found only in history is anchored to its commit ([S-767]),
/// through the binary and the `[security]` gate. Three secrets are committed in
/// one commit, each on its function's second line; the next commit deletes one
/// file and puts other code on another's line. History mode reports all three
/// at the first commit, one column late as Gitleaks 8.18.4 reads a line that
/// is not its hunk's first.
/// - the secret still on its line is bound to it, with today's enclosing
///   symbol;
/// - the one in the deleted file and the one whose line holds other code are
///   `historical`, with the short commit, no symbol and no blast radius;
/// - both still count: each is a new finding to the ratchet, and a cap fails
///   on each, until `gate --save` records them.
///
/// [S-767]: ../../docs/planning/journal.md#s-767-a-secret-found-only-in-history-is-anchored-to-its-commit
#[test]
fn a_secret_found_only_in_history_is_anchored_to_its_commit_and_still_counts() {
    let w = World::new();
    let files = [
        ("app/settings.py", "def load():\n    API_KEY = \"lgs_pl4nted_S3cr3t_q7Vx9Rz2Kw\"\n    return API_KEY\n"),
        ("app/legacy.py", "def old():\n    API_KEY = \"lgs_l3gacy_R8wY2pQ5nM4kT7\"\n    return API_KEY\n"),
        ("app/keys.py", "def keys():\n    API_KEY = \"lgs_r0tated_X3mK8pQ1wZ6vB4\"\n    return API_KEY\n"),
    ];
    for (path, body) in files {
        fs::write(w.root.join(path), body).unwrap();
    }
    git(&w.root, &["add", "app"]);
    git(&w.root, &["commit", "-q", "-m", "keys"]);
    let added = rev(&w.root, "HEAD");
    fs::remove_file(w.root.join("app/legacy.py")).unwrap();
    fs::write(w.root.join("app/keys.py"), "def keys():\n    API_KEY = load_from_vault()\n    return API_KEY\n").unwrap();
    git(&w.root, &["commit", "-q", "-am", "move the keys out"]);
    assert!(w.logos(&["index"]).status.success());
    w.rules(Some("[security]\nratchet_min_severity = \"medium\"\n"));
    let (code, saved) = w.verdict(&["gate", "--save"]);
    assert_eq!(code, Some(0), "the baseline holds the semgrep findings alone: {saved}");

    // Each match is `API_KEY = "…"` from byte 5 of line 2, read one column late
    // (`6..5+len`): the exact text of each, so every one is read from its commit.
    let dir = w.root.parent().unwrap();
    let results: Vec<_> = files
        .iter()
        .map(|&(path, body)| {
            let line = body.lines().nth(1).unwrap().trim_start();
            (Some(added.as_str()), path, 2, 6, 5 + line.len() as i64)
        })
        .collect();
    w.ingest(&gitleaks_report_in(dir, "history.sarif", &results));
    let listed = w.json(&["security", "findings", "--scanner", "gitleaks"]);
    let short = &added[..7];
    let at = |path: &str| -> Value {
        let found: Vec<&Value> =
            listed["findings"].as_array().unwrap().iter().filter(|f| f["report_path"] == path).collect();
        assert_eq!(found.len(), 1, "{path}: {listed}");
        found[0].clone()
    };
    let bound = at("app/settings.py");
    assert_eq!((bound["binding"].as_str(), bound["path"].as_str()), (Some("bound"), Some("app/settings.py")));
    assert_eq!(bound["context"]["enclosing"]["symbol"]["name"], "load", "today's enclosing symbol");
    assert_eq!(bound["anchor"]["short_commit"], short);
    for path in ["app/legacy.py", "app/keys.py"] {
        let historical = at(path);
        assert_eq!(historical["binding"], "historical", "{path}: {historical}");
        assert_eq!(historical["path"], Value::Null, "{path}: no file of today's tree");
        assert_eq!(historical["anchor"], serde_json::json!({ "commit": added, "short_commit": short, "path": path }));
        let context = &historical["context"];
        assert_eq!(context["enclosing"]["symbol"], Value::Null, "{path}: no enclosing symbol");
        assert_eq!(context["enclosing"]["unbound_because"], "historical");
        assert_eq!(context["blast_radius"], serde_json::json!({ "confidence": "unbound" }), "{path}: no blast radius");
    }

    // Both historical findings count: new to the ratchet, and over a cap.
    let ratchet = |check: &Value| -> usize {
        check["violations"].as_array().unwrap().iter().filter(|v| v["rule"] == "security:ratchet").count()
    };
    let (code, check) = w.verdict(&["check"]);
    assert_eq!((code, ratchet(&check)), (Some(1), 3), "all three secrets are new: {check}");
    let (code, saved) = w.verdict(&["gate", "--save"]);
    assert_eq!(code, Some(0), "{saved}");
    let (code, check) = w.verdict(&["check"]);
    assert_eq!((code, ratchet(&check)), (Some(0), 0), "recorded, so no longer new: {check}");
    w.rules(Some("[security]\nratchet_min_severity = \"medium\"\nmax_severity = \"medium\"\n"));
    let (code, check) = w.verdict(&["check"]);
    let capped: Vec<&str> = check["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["rule"] == "security:max_severity")
        .filter_map(|v| v["file"].as_str())
        .collect();
    assert_eq!((code, ratchet(&check)), (Some(1), 0), "a cap fails on recorded findings too: {check}");
    for path in ["app/settings.py", "app/legacy.py", "app/keys.py"] {
        assert!(capped.contains(&path), "the cap counts {path}: {check}");
    }
    // Each violation names its place as history, with its short commit, never
    // as today's code.
    let message = |path: &str| -> String {
        let found = check["violations"].as_array().unwrap().iter().find(|v| v["rule"] == "security:max_severity" && v["file"] == path);
        found.unwrap()["message"].as_str().unwrap().to_string()
    };
    for path in ["app/legacy.py", "app/keys.py"] {
        assert!(message(path).contains(&format!("at {path}:2 (historical, commit {short})")), "{}", message(path));
    }
    assert!(!message("app/settings.py").contains("historical"), "{}", message("app/settings.py"));
}

/// `logos --json <args>` on the project at `root`: the exit code and stdout.
fn verdict_in(w: &World, root: &Path, args: &[&str]) -> (Option<i32>, Value) {
    let mut all = vec!["--json"];
    all.extend_from_slice(args);
    let out = w.logos_in(root, &all);
    let json = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("logos {args:?}: {e}: {}", String::from_utf8_lossy(&out.stderr)));
    (out.status.code(), json)
}

/// The ratchet's violations in a `check` verdict.
fn ratchet_violations(check: &Value) -> Vec<&Value> {
    check["violations"].as_array().unwrap().iter().filter(|v| v["rule"] == "security:ratchet").collect()
}

/// A CI clone with a checked-in security baseline reads only new findings as
/// new ([FR-SC-25], [UAT-SC-09]). The fixture records a Semgrep scan (two
/// findings) and a Gitleaks one (a secret), and `security bless` writes them
/// to `.logos/security-baseline.json`:
/// - the file holds fingerprint, rule id and project-relative path per
///   finding and nothing else — no message, snippet, secret or checkout path;
/// - blessing the same state again writes the same bytes;
/// - `gate --save` records no `history.db` set beside it;
/// - a fresh `git clone` (no `history.db`) scanned with the same stubs reads no
///   blessed finding as new, its first `session_start` keeps the file as the
///   baseline and records none in `history.db`, and a third finding is the
///   only new one, named against the file.
///
/// The secret's fingerprint is keyed by the repository's first commit
/// (S-765, coordinator decision 7), which the clone shares.
///
/// [FR-SC-25]: ../../docs/specs/requirements/FR-SC-25.md
/// [UAT-SC-09]: ../../docs/specs/requirements/UAT-SC-09.md
#[test]
fn a_fresh_clone_with_a_checked_in_security_baseline_reads_only_new_findings_as_new() {
    let w = World::new();
    let secret = "lgs_pl4nted_S3cr3t_q7Vx9Rz2Kw";
    fs::write(w.root.join("app/settings.py"), format!("DEBUG = False\nAPI_TIMEOUT = 30\n\nAPI_KEY = \"{secret}\"\n")).unwrap();
    w.rules(Some("[security]\nratchet_min_severity = \"medium\"\n"));
    git(&w.root, &["add", "app/settings.py", ".logos/rules.toml"]);
    git(&w.root, &["commit", "-q", "-m", "settings"]);
    let dir = w.root.parent().unwrap().to_path_buf();
    let semgrep = Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep.sarif");
    let gitleaks = gitleaks_report(&dir, "scoped.sarif", &[(None, 4, 2, 42)]);
    let scan = |root: &Path| {
        for report in [&semgrep, &gitleaks] {
            let out = w.logos_in(root, &["security", "ingest", report.to_str().unwrap()]);
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        }
    };
    scan(&w.root);
    let (code, unblessed) = w.verdict(&["check"]);
    assert_eq!((code, ratchet_violations(&unblessed).len()), (Some(1), 3), "nothing is blessed yet: {unblessed}");

    // UAT-SC-09 step 1: bless.
    let (code, blessed) = w.verdict(&["security", "bless"]);
    assert_eq!(code, Some(0), "{blessed}");
    assert_eq!((&blessed["recorded"], &blessed["changed"], &blessed["findings"]), (&Value::Bool(true), &Value::Bool(true), &serde_json::json!(3)), "{blessed}");
    let file = w.root.join(".logos/security-baseline.json");
    let bytes = fs::read_to_string(&file).unwrap();
    let doc: Value = serde_json::from_str(&bytes).unwrap();
    let entries = doc["findings"].as_array().unwrap();
    assert_eq!(entries.len(), 3, "{bytes}");
    for entry in entries {
        let keys: Vec<&str> = entry.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["fingerprint", "rule_id", "path"], "{bytes}");
    }
    let paths: BTreeSet<&str> = entries.iter().filter_map(|e| e["path"].as_str()).collect();
    assert_eq!(paths, BTreeSet::from(["app/db.py", "app/settings.py"]), "{bytes}");
    let root = w.root.display().to_string();
    for absent in [secret, "REDACTED", "Detected possible formatted SQL", "has detected secret", "cursor.execute", root.as_str()] {
        assert!(!bytes.contains(absent), "the file holds {absent:?}: {bytes}");
    }
    let (code, again) = w.verdict(&["security", "bless"]);
    assert_eq!((code, &again["changed"]), (Some(0), &Value::Bool(false)), "{again}");
    assert_eq!(fs::read_to_string(&file).unwrap(), bytes, "a second bless of one state is byte-identical");

    // With the file checked in, `gate --save` records no second baseline.
    let (code, saved) = w.verdict(&["gate", "--save"]);
    assert_eq!(code, Some(0), "{saved}");
    assert!(saved["message"].as_str().unwrap().contains("security baseline not saved: the checked-in .logos/security-baseline.json"), "{saved}");
    let recorded: i64 = rusqlite::Connection::open(w.root.join(".logos/history.db"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM security_baseline", [], |r| r.get(0))
        .unwrap();
    assert_eq!(recorded, 0, "{saved}");

    // UAT-SC-09 step 2: a fresh clone carrying the file, scanned again.
    git(&w.root, &["add", ".logos/security-baseline.json"]);
    git(&w.root, &["commit", "-q", "-m", "bless the security baseline"]);
    let clone = dir.join("clone");
    git(&dir, &["clone", "-q", w.root.to_str().unwrap(), clone.to_str().unwrap()]);
    assert!(!clone.join(".logos/history.db").exists(), "the clone starts with no history.db");
    assert_eq!(fs::read_to_string(clone.join(".logos/security-baseline.json")).unwrap(), bytes);
    assert!(w.logos_in(&clone, &["index"]).status.success());
    scan(&clone);
    let (code, check) = verdict_in(&w, &clone, &["check"]);
    assert_eq!(code, Some(0), "no blessed finding reads as new in the clone: {check}");
    assert!(ratchet_violations(&check).is_empty(), "{check}");
    // The clone's first session saves its missing metric baseline, keeps the
    // checked-in file — which it never writes — and records no history.db
    // ratchet beside it.
    let (code, started) = verdict_in(&w, &clone, &["session-start"]);
    assert_eq!(code, Some(0), "{started}");
    let message = started["message"].as_str().unwrap();
    assert!(message.contains("— baseline saved (snapshot #"), "{started}");
    assert!(message.contains("security baseline kept (3 finding fingerprints, checked in at .logos/security-baseline.json)"), "{started}");
    let recorded: i64 = rusqlite::Connection::open(clone.join(".logos/history.db"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM security_baseline", [], |r| r.get(0))
        .unwrap();
    assert_eq!(recorded, 0, "the clone's first session records no history.db baseline: {started}");
    let gate_baselines: i64 = rusqlite::Connection::open(clone.join(".logos/logos.db"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM baseline", [], |r| r.get(0))
        .unwrap();
    assert_eq!(gate_baselines, 1, "session_end has a metric baseline to compare against: {started}");
    let (code, again) = verdict_in(&w, &clone, &["session-start"]);
    assert_eq!(code, Some(0), "{again}");
    let message = again["message"].as_str().unwrap();
    assert!(message.contains("baseline kept (snapshot #"), "{again}");
    assert!(message.contains("security baseline kept (3 finding fingerprints, checked in at .logos/security-baseline.json)"), "{again}");

    // UAT-SC-09 step 3: a third finding is the only new one.
    let out = w.logos_in(&clone, &["security", "ingest", w.semgrep_with_a_new_high_finding().to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let (code, check) = verdict_in(&w, &clone, &["check"]);
    assert_eq!(code, Some(1), "{check}");
    let new = ratchet_violations(&check);
    assert_eq!(new.len(), 1, "only the third finding is new: {check}");
    let message = new[0]["message"].as_str().unwrap();
    assert!(message.contains(NEW_RULE) && message.contains("not in the findings .logos/security-baseline.json records"), "{message}");
}

/// `security bless` under a `[security]` table whose required scan is missing
/// writes nothing and exits 5, as `gate --save` records nothing then; a
/// malformed checked-in file is refused by `check` (exit 2, naming it) and
/// replaced by the next bless ([FR-SC-25]).
///
/// [FR-SC-25]: ../../docs/specs/requirements/FR-SC-25.md
#[test]
fn security_bless_refuses_over_a_missing_scan_and_replaces_a_malformed_file() {
    let w = World::new();
    w.rules(Some("[security]\nrequired_scanners = [\"semgrep\"]\n"));
    let file = w.root.join(".logos/security-baseline.json");
    let (code, refused) = w.verdict(&["security", "bless"]);
    assert_eq!(code, Some(5), "{refused}");
    assert_eq!(refused["recorded"], false, "{refused}");
    assert_eq!(refused["missing_scans"][0]["scanner"], "semgrep", "{refused}");
    assert!(!file.exists(), "nothing is written over a missing scan");

    w.ingest(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep.sarif"));
    fs::write(&file, "{\"version\":1,\"findings\":[],\"message\":\"x\"}\n").unwrap();
    let out = w.logos(&["check"]);
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(String::from_utf8_lossy(&out.stderr).contains(".logos/security-baseline.json"), "{}", String::from_utf8_lossy(&out.stderr));
    let (code, blessed) = w.verdict(&["security", "bless"]);
    assert_eq!((code, &blessed["findings"]), (Some(0), &serde_json::json!(2)), "{blessed}");
    let (code, check) = w.verdict(&["check"]);
    assert_eq!(code, Some(0), "{check}");

    // HEAD moves, so the required scan is stale: `gate --save` beside the file
    // exits 5 and still names the scanner it is missing.
    git(&w.root, &["commit", "-q", "--allow-empty", "-m", "move HEAD"]);
    let (code, saved) = w.verdict(&["gate", "--save"]);
    assert_eq!(code, Some(5), "{saved}");
    let message = saved["message"].as_str().unwrap();
    assert!(
        message.contains("the checked-in .logos/security-baseline.json") && message.contains("required scan missing or stale at HEAD: semgrep"),
        "{saved}"
    );
}

/// One source per checkout ([FR-SC-25]): beside a set `gate --save` recorded
/// in `history.db`, the checked-in file alone is the ratchet's baseline. A
/// finding the stored set holds but the file does not is new, named against
/// the file; a malformed file is exit 2 even though a stored set could stand
/// in for it.
///
/// [FR-SC-25]: ../../docs/specs/requirements/FR-SC-25.md
#[test]
fn the_checked_in_baseline_is_the_one_source_beside_a_stored_one() {
    let w = World::new();
    w.rules(Some("[security]\nratchet_min_severity = \"medium\"\n"));
    w.ingest(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep.sarif"));
    let (code, saved) = w.verdict(&["gate", "--save"]);
    assert!(code == Some(0) && saved["message"].as_str().unwrap().contains("security baseline saved (2 finding fingerprints)"), "{saved}");
    let (code, blessed) = w.verdict(&["security", "bless"]);
    assert_eq!((code, &blessed["findings"]), (Some(0), &serde_json::json!(2)), "{blessed}");

    // Drop one blessed finding from the file: the stored set still holds it.
    let file = w.root.join(".logos/security-baseline.json");
    let mut doc: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    let dropped = doc["findings"].as_array_mut().unwrap().remove(0);
    fs::write(&file, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    let (code, check) = w.verdict(&["check"]);
    assert_eq!(code, Some(1), "the file, not the stored set, is the baseline: {check}");
    let new = ratchet_violations(&check);
    assert_eq!(new.len(), 1, "{check}");
    let message = new[0]["message"].as_str().unwrap();
    assert!(message.contains(dropped["fingerprint"].as_str().unwrap()), "{message}");
    assert!(message.contains("not in the findings .logos/security-baseline.json records"), "{message}");

    fs::write(&file, "{\"version\":1}\n").unwrap();
    let out = w.logos(&["check"]);
    assert_eq!(out.status.code(), Some(2), "a malformed file never falls back to the stored set");
    assert!(String::from_utf8_lossy(&out.stderr).contains(".logos/security-baseline.json"), "{}", String::from_utf8_lossy(&out.stderr));
}

/// `security bless` records a finding reported by an absolute path — as
/// osv-scanner reports a lockfile — by its path relative to the project, and
/// one reported outside the project with no path, so the checked-in file
/// never carries the checkout's directory ([FR-SC-25]).
///
/// [FR-SC-25]: ../../docs/specs/requirements/FR-SC-25.md
#[test]
fn security_bless_records_an_absolutely_reported_finding_by_its_project_path() {
    let w = World::new();
    fs::write(w.root.join("Cargo.lock"), "# lockfile\n").unwrap();
    let result = |rule: &str, uri: String| {
        serde_json::json!({
            "ruleId": rule,
            "level": "error",
            "message": {"text": format!("Package 'rmcp@1.7.0' is vulnerable to '{rule}'.")},
            "locations": [{"physicalLocation": {"artifactLocation": {"uri": uri}}}]
        })
    };
    let sarif = serde_json::json!({"version": "2.1.0", "runs": [{
        "tool": {"driver": {"name": "osv-scanner", "version": "1.9.2", "rules": []}},
        "results": [
            result("CVE-2026-63127", format!("file://{}/Cargo.lock", w.root.display())),
            result("CVE-2026-99999", "file:///elsewhere-s764/Cargo.lock".to_string())
        ]
    }]});
    let report = w.root.parent().unwrap().join("osv.sarif");
    fs::write(&report, sarif.to_string()).unwrap();
    w.ingest(&report);

    let (code, blessed) = w.verdict(&["security", "bless"]);
    assert_eq!((code, &blessed["findings"]), (Some(0), &serde_json::json!(2)), "{blessed}");
    let bytes = fs::read_to_string(w.root.join(".logos/security-baseline.json")).unwrap();
    let doc: Value = serde_json::from_str(&bytes).unwrap();
    let paths: Vec<(&str, &Value)> = doc["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["rule_id"].as_str().unwrap(), &e["path"]))
        .collect();
    assert_eq!(
        paths,
        [("CVE-2026-99999", &Value::Null), ("CVE-2026-63127", &serde_json::json!("Cargo.lock"))],
        "{bytes}"
    );
    let root = w.root.display().to_string();
    assert!(!bytes.contains(&root) && !bytes.contains("elsewhere-s764"), "{bytes}");
}

/// Without a `[security]` table `security bless` still writes the file
/// (exit 0) and says the ratchet reads it once a table exists ([FR-SC-25]).
///
/// [FR-SC-25]: ../../docs/specs/requirements/FR-SC-25.md
#[test]
fn security_bless_without_a_security_table_writes_the_file_and_says_who_reads_it() {
    let w = World::new();
    assert!(!w.root.join(".logos/rules.toml").exists(), "the fixture has no rules contract");
    w.ingest(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep.sarif"));
    let (code, blessed) = w.verdict(&["security", "bless"]);
    assert_eq!(code, Some(0), "{blessed}");
    assert_eq!((&blessed["recorded"], &blessed["findings"]), (&Value::Bool(true), &serde_json::json!(2)), "{blessed}");
    assert!(blessed["message"].as_str().unwrap().contains("once .logos/rules.toml has a [security] table"), "{blessed}");
    assert!(w.root.join(".logos/security-baseline.json").is_file());
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

/// Flags the page names that belong to a scanner, not to Logos: Dependency-Check's
/// two that hand it an NVD API key (S-794) among them.
const SCANNER_FLAGS: [&str; 4] = ["metrics", "no-git", "nvdApiKey", "propertyfile"];

/// Every bare `--flag` span in `markdown` that no `logos security` command lists
/// and no scanner owns, and how many were checked.
fn unknown_bare_flags(markdown: &str) -> (Vec<String>, usize) {
    let helps: Vec<String> = ["scan", "install", "uninstall", "ingest", "status", "findings", "reconfirm"]
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

/// S-760: the manual names the one definition of a counted finding, says freshness
/// is the finding's and not its report's, and says the roll-up states how many
/// reports are fresh as its own field. S-795 adds test code to the definition, in
/// its order. Each needle is one sentence a rewrite would have to keep.
#[test]
fn the_page_names_the_one_definition_of_a_counted_finding() {
    let page = security_page().split_whitespace().collect::<Vec<_>>().join(" ");
    for needle in [
        "**One definition of a counted finding.**",
        "it counts unless a current entry in `.logos/security-ignore.toml` suppresses it, its own file changed since the scan, or it sits in [test code](#findings-in-test-code)",
        "A finding left out is left out for the first of those that holds, in that order.",
        "Freshness is the finding's, never its report's",
        "reads the same count on all of them",
        "states it as its own field, `N of M reports fresh`",
        "`N of M reports fresh` says how many of the stored reports are themselves fresh at HEAD",
        "A scanner you disable keeps its stored findings in every count until you re-scan without it.",
        "A file a `--path` scan never read reads `0` in Files & Risk, which is the absence of a finding and not proof of a scan",
        "The OWASP table below the findings is the one exception",
        "a production finding its scanner marks low-confidence ([needs review](#findings-that-need-review)) is left out too",
        "and so is a finding on a [dev-only dependency](#findings-on-a-dev-only-dependency)",
        "### Findings on a dev-only dependency",
        "### Findings that need review",
        "**The gate keeps every finding.** The `[security]` ratchet and caps, `gate --save`, `security bless` and the branch delta of `review --base` count a finding that needs review like any other",
    ] {
        assert!(page.contains(needle), "docs/howto/security.md no longer says: {needle}");
    }
}

/// S-814: the manual shows the rule-wide entry, and the example it prints is a
/// file Logos accepts and writes back unchanged — so a reader who pastes it can
/// still triage from the Security page, which refuses a file spelled otherwise.
#[test]
fn the_page_shows_the_rule_wide_entry_in_the_canonical_spelling() {
    let page = security_page();
    let section = page.split("### One entry for a rule, in every file").nth(1).expect("the rule-wide section");
    let section = section.split("\n## ").next().unwrap();
    let example = section.split("```toml\n").nth(1).and_then(|rest| rest.split("```").next()).expect("a toml example");
    let file = logos_core::history::security::IgnoreFile::parse(example).expect("the example is a file Logos accepts");
    assert_eq!(file.len(), 1);
    assert_eq!(file.to_toml(), example, "the example is in the canonical spelling");
    assert!(matches!(file.entries()[0].selector, logos_core::history::security::Selector::Rule { .. }));

    let section = section.split_whitespace().collect::<Vec<_>>().join(" ");
    for needle in [
        "A `rule` without a `scanner`, or a `scanner` without a `rule`",
        "the message names the key that is missing",
        "A whole-project `path` glob (`*`, `**`, `.`) is still refused",
        "suppressed by a rule-wide entry",
        "so the rule-wide entry holds the rule out of the gate",
    ] {
        assert!(section.contains(needle), "the rule-wide section no longer says: {needle}");
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
        // Unrated, so its optional `severity_reason` is emitted and named too.
        severity: logos_core::model::Severity::Unrated,
        severity_reason: logos_core::model::Severity::Unrated.reason(),
        owasp: vec![],
        message: "m".into(),
        redacted: false,
        path: None,
        start_line: None,
        end_line: None,
        freshness: "fresh".into(),
        reachability: "unknown".into(),
        confidence: "unbound".into(),
        // Merged (S-809), so its optional `reporters` is emitted and named too.
        reporters: vec!["semgrep".into(), "Trivy".into()],
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

/// Kills the process on drop, so a failing assertion leaves no `logos` behind.
struct Reaped(std::process::Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// S-758 across processes: while one `logos security scan` runs, a second
/// scan and an install of the same project, each a process of its own, exit 3
/// naming the running scan and its process, and start no scanner; one scanner
/// process runs throughout. Once the first ends, a scan runs.
#[test]
fn a_second_cli_scan_or_install_is_refused_naming_the_running_scan() {
    let w = World::new();
    let rec = w.data.parent().unwrap().join("rec");
    fs::create_dir_all(&rec).unwrap();
    let semgrep = Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep.sarif");
    // Records each run, then waits — at most 60 s — for `rec/release`.
    let script = format!(
        "#!/bin/sh\n\
         case \"$1\" in --version|version|-version) printf '%s\\n' 'semgrep 1.2.3'; exit 0;; esac\n\
         printf '%s\\n' \"$$\" >> '{rec}/semgrep.runs'\n\
         out=''; prev=''\n\
         for a in \"$@\"; do [ \"$prev\" = --output ] && out=\"$a\"; prev=\"$a\"; done\n\
         n=0; while [ ! -e '{rec}/release' ] && [ $n -lt 1200 ]; do /bin/sleep 0.05; n=$((n+1)); done\n\
         /bin/cp '{report}' \"$out\"\n",
        rec = rec.display(),
        report = semgrep.display(),
    );
    let stub = w.bin.join("semgrep");
    fs::write(&stub, script).unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    let runs = || fs::read_to_string(rec.join("semgrep.runs")).map(|s| s.lines().count()).unwrap_or(0);

    let first = Reaped(
        Command::new(env!("CARGO_BIN_EXE_logos"))
            .arg("--project")
            .arg(&w.root)
            .args(["security", "scan", "--consent", "--scanner", "semgrep"])
            .env("PATH", &w.path)
            .env("LOGOS_CONFIG_HOME", &w.user)
            .env("LOGOS_DATA_HOME", &w.data)
            .env("GIT_CEILING_DIRECTORIES", w.root.parent().unwrap())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the logos binary runs"),
    );
    let deadline =
        std::time::Instant::now() + logos_core::perf::scaled(std::time::Duration::from_secs(60));
    while runs() == 0 {
        assert!(std::time::Instant::now() < deadline, "the first scan never started semgrep");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let named = format!(
        "a security scan is already running on this project: started from the command line by Logos process {} ",
        first.0.id()
    );
    for args in [&["security", "scan", "--scanner", "semgrep"][..], &["security", "install", "gitleaks", "--consent"][..]] {
        let out = w.logos(args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(3), "logos {args:?}: {stderr}");
        assert!(stderr.contains(&named), "logos {args:?} names the running scan: {stderr}");
    }
    let ps = Command::new("ps").args(["-A", "-o", "pid=,command="]).output().expect("ps runs");
    let stub_path = stub.display().to_string();
    let scanners: Vec<String> =
        String::from_utf8_lossy(&ps.stdout).lines().filter(|l| l.contains(&stub_path)).map(str::to_string).collect();
    assert_eq!(scanners.len(), 1, "one scanner process runs on the project: {scanners:?}");
    assert_eq!(runs(), 1, "the refused commands started no scanner");

    fs::write(rec.join("release"), "").unwrap();
    let mut first = first;
    let status = first.0.wait().unwrap();
    assert!(status.success(), "the first scan completes: {status:?}");
    let out = w.logos(&["security", "scan", "--scanner", "semgrep"]);
    assert!(out.status.success(), "a scan runs once the first ended: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(runs(), 2);
}

/// Sprint 107 review D1 (S-758 × S-759): a `logos security scan` stopped by
/// SIGINT, or by SIGTERM as the plugin's Stop hook stops one past its budget,
/// kills the scanner it started before the project lock goes with it — the
/// scanner runs in a process group of its own, out of the signal's reach — and
/// exits 128 + the signal. No scanner of it is left, so the next scan is
/// admitted with exactly one scanner running. SIGKILL is not covered.
#[test]
fn a_scan_stopped_by_a_signal_leaves_no_scanner_beside_the_next_one() {
    for (signal, code) in [("INT", 130), ("TERM", 143)] {
        let w = World::new();
        let rec = w.data.parent().unwrap().join("rec");
        fs::create_dir_all(&rec).unwrap();
        let semgrep = Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep.sarif");
        // Records each run, then waits — at most 60 s — for `rec/release`.
        let script = format!(
            "#!/bin/sh\n\
             case \"$1\" in --version|version|-version) printf '%s\\n' 'semgrep 1.2.3'; exit 0;; esac\n\
             printf '%s\\n' \"$$\" >> '{rec}/semgrep.runs'\n\
             out=''; prev=''\n\
             for a in \"$@\"; do [ \"$prev\" = --output ] && out=\"$a\"; prev=\"$a\"; done\n\
             n=0; while [ ! -e '{rec}/release' ] && [ $n -lt 1200 ]; do /bin/sleep 0.05; n=$((n+1)); done\n\
             /bin/cp '{report}' \"$out\"\n",
            rec = rec.display(),
            report = semgrep.display(),
        );
        let stub = w.bin.join("semgrep");
        fs::write(&stub, script).unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        let runs = || fs::read_to_string(rec.join("semgrep.runs")).map(|s| s.lines().count()).unwrap_or(0);
        let stub_path = stub.display().to_string();
        let scanners = || -> Vec<String> {
            let ps = Command::new("ps").args(["-A", "-o", "pid=,command="]).output().expect("ps runs");
            String::from_utf8_lossy(&ps.stdout).lines().filter(|l| l.contains(&stub_path)).map(str::to_string).collect()
        };
        let scan = |args: &[&str]| {
            Reaped(
                Command::new(env!("CARGO_BIN_EXE_logos"))
                    .arg("--project")
                    .arg(&w.root)
                    .args(args)
                    .env("PATH", &w.path)
                    .env("LOGOS_CONFIG_HOME", &w.user)
                    .env("LOGOS_DATA_HOME", &w.data)
                    .env("GIT_CEILING_DIRECTORIES", w.root.parent().unwrap())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .expect("the logos binary runs"),
            )
        };
        let wait_for = |what: &str, done: &dyn Fn() -> bool| {
            let deadline = std::time::Instant::now()
                + logos_core::perf::scaled(std::time::Duration::from_secs(60));
            while !done() {
                assert!(std::time::Instant::now() < deadline, "SIG{signal}: {what}");
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };

        let mut first = scan(&["security", "scan", "--consent", "--scanner", "semgrep"]);
        wait_for("the first scan never started semgrep", &|| runs() == 1);
        let sent = Command::new("kill").arg(format!("-{signal}")).arg(first.0.id().to_string()).status().unwrap();
        assert!(sent.success());
        let status = first.0.wait().unwrap();
        assert_eq!(status.code(), Some(code), "SIG{signal}: exits 128 + the signal: {status:?}");
        // SIGKILL lands at once, but a killed process can linger in the table
        // for a moment: what counts is that none outlives it.
        let gone = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !scanners().is_empty() && std::time::Instant::now() < gone {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(scanners(), Vec::<String>::new(), "SIG{signal}: no scanner outlives the stopped scan");

        let mut second = scan(&["security", "scan", "--scanner", "semgrep"]);
        wait_for("the next scan was not admitted", &|| runs() == 2);
        assert_eq!(scanners().len(), 1, "SIG{signal}: exactly one scanner runs: {:?}", scanners());
        fs::write(rec.join("release"), "").unwrap();
        let status = second.0.wait().unwrap();
        assert!(status.success(), "SIG{signal}: the next scan completes: {status:?}");
    }
}

/// FR-SC-27 through the binary: the CLI reads `NVD_API_KEY` from its own
/// environment — an empty one sets none, a repository `.env` none — and
/// `security status` reads `set` / `not_set`; a Dependency-Check scan is
/// handed the key in a private file named by `--propertyfile`, never in its
/// argv or on either stream, and the file is gone once the scan ends.
#[test]
fn the_cli_reads_the_nvd_key_from_its_environment_and_never_prints_it() {
    let w = World::new();
    let status = |key: Option<&str>| -> Value {
        let out = w.logos_keyed(&w.root, &["--json", "security", "status"], key);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).expect("JSON on stdout")
    };
    assert_eq!(row(&status(None), "dependency-check")["nvd_api_key"], "not_set");
    assert_eq!(row(&status(Some("")), "dependency-check")["nvd_api_key"], "not_set", "an empty key sets none");
    assert_eq!(row(&status(Some(NVD_KEY)), "dependency-check")["nvd_api_key"], "set");
    assert_eq!(row(&status(Some(NVD_KEY)), "semgrep").get("nvd_api_key"), None, "Dependency-Check's row alone");
    fs::write(w.root.join(".env"), format!("NVD_API_KEY={NVD_KEY}\n")).unwrap();
    assert_eq!(row(&status(None), "dependency-check")["nvd_api_key"], "not_set", "a repository .env supplies none");

    let props = w.root.parent().unwrap().join("dependency-check.props");
    let argv = w.root.parent().unwrap().join("dependency-check.argv");
    let script = format!(
        "#!/bin/sh\n\
         printf '%s\\n' \"$*\" >> '{argv}'\n\
         case \"$1\" in --version) printf '%s\\n' 'Dependency-Check Core version 13.0.0'; exit 0;; esac\n\
         pf=''; prev=''\n\
         for a in \"$@\"; do if [ \"$prev\" = '--propertyfile' ]; then pf=\"$a\"; fi; prev=\"$a\"; done\n\
         if [ -f \"$pf\" ]; then /bin/cp \"$pf\" '{props}'; /bin/cat \"$pf\" >&2; fi\n\
         exit 1\n",
        argv = argv.display(),
        props = props.display(),
    );
    let path = w.bin.join("dependency-check");
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let out = w.logos_keyed(&w.root, &["--json", "security", "scan", "--consent", "--scanner", "dependency-check"], Some(NVD_KEY));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let scan: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(row(&scan, "dependency-check")["outcome"], "failed", "{scan}");
    assert_eq!(fs::read_to_string(&props).unwrap(), format!("nvd.api.key={NVD_KEY}\n"));
    let argv = fs::read_to_string(&argv).unwrap();
    assert!(argv.contains("--propertyfile") && !argv.contains(NVD_KEY), "{argv}");
    let log = fs::read_to_string(w.root.join(".logos/security/dependency-check.failed.log")).unwrap();
    assert!(!log.contains(NVD_KEY), "{log}");
    assert!(fs::symlink_metadata(w.root.join(".logos/security/nvd-api-key.properties")).is_err(), "the key file outlived the scan");
}

/// FR-SC-27 AC 2 (S-794 review): a `logos security scan` stopped by SIGINT or
/// SIGTERM while Dependency-Check runs with the key exits without unwinding
/// the scan, and still leaves no key file: the stop's cancel removes it.
#[test]
fn a_keyed_scan_stopped_by_a_signal_leaves_no_key_file() {
    for (signal, code) in [("INT", 130), ("TERM", 143)] {
        let w = World::new();
        let seen = w.root.parent().unwrap().join("dependency-check.seen");
        let key_file = w.root.join(".logos/security/nvd-api-key.properties");
        // Records that the key file is there, then waits, at most 60 s.
        let script = format!(
            "#!/bin/sh\n\
             case \"$1\" in --version) printf '%s\\n' 'Dependency-Check Core version 13.0.0'; exit 0;; esac\n\
             pf=''; prev=''\nfor a in \"$@\"; do if [ \"$prev\" = '--propertyfile' ]; then pf=\"$a\"; fi; prev=\"$a\"; done\n\
             /bin/ls \"$pf\" > '{seen}.tmp' && /bin/mv '{seen}.tmp' '{seen}'\n\
             /bin/sleep 60\n",
            seen = seen.display(),
        );
        let stub = w.bin.join("dependency-check");
        fs::write(&stub, script).unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        let mut scan = Reaped(
            Command::new(env!("CARGO_BIN_EXE_logos"))
                .arg("--project")
                .arg(&w.root)
                .args(["security", "scan", "--consent", "--scanner", "dependency-check"])
                .env("PATH", &w.path)
                .env("LOGOS_CONFIG_HOME", &w.user)
                .env("LOGOS_DATA_HOME", &w.data)
                .env("GIT_CEILING_DIRECTORIES", w.root.parent().unwrap())
                .env("NVD_API_KEY", NVD_KEY)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("the logos binary runs"),
        );
        let deadline = std::time::Instant::now()
            + logos_core::perf::scaled(std::time::Duration::from_secs(60));
        while !seen.exists() {
            assert!(std::time::Instant::now() < deadline, "SIG{signal}: Dependency-Check never started");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(fs::read_to_string(&seen).unwrap().trim(), key_file.display().to_string(), "SIG{signal}: the file existed during the run");
        let sent = Command::new("kill").arg(format!("-{signal}")).arg(scan.0.id().to_string()).status().unwrap();
        assert!(sent.success());
        let status = scan.0.wait().unwrap();
        assert_eq!(status.code(), Some(code), "SIG{signal}: exits 128 + the signal: {status:?}");
        assert!(fs::symlink_metadata(&key_file).is_err(), "SIG{signal}: the key file outlived the stopped scan");
    }
}
