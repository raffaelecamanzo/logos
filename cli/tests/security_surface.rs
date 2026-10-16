//! `logos security scan|ingest|status|findings` end to end through the shipped
//! binary (S-667, [FR-SC-06], [FR-SC-07], [FR-SC-08]).
//!
//! The binary runs on a git fixture outside every work tree, with
//! `LOGOS_CONFIG_HOME` pointing at a temporary user-level directory and a `PATH`
//! of two directories only: the stub scanners, and one holding a link to `git`.
//! So no scanner installed on this machine is found, and this machine's own
//! consent record is never read or written. The MCP side of the same payloads
//! is `mcp/tests/security_parity.rs`.
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

const CLEAN_BANDIT: &str = r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Bandit","rules":[]}},"results":[]}]}"#;

struct World {
    _tmp: TempDir,
    root: PathBuf,
    bin: PathBuf,
    path: OsString,
    user: PathBuf,
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
        let [root, bin, tools, user] = ["project", "bin", "tools", "user"].map(|d| base.join(d));
        for dir in [&root.join("app"), &root.join(".logos"), &bin, &tools, &user] {
            fs::create_dir_all(dir).unwrap();
        }
        std::os::unix::fs::symlink(real_git(), tools.join("git")).unwrap();
        fs::write(root.join("app/db.py"), "import sqlite3\n").unwrap();
        fs::write(root.join(".logos/config.toml"), "[project]\nid = \"security-cli-fixture\"\n").unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "seed"]);
        let clean = base.join("clean-bandit.sarif");
        fs::write(&clean, CLEAN_BANDIT).unwrap();
        let path = std::env::join_paths([&bin, &tools]).unwrap();
        let w = Self { _tmp: tmp, root, bin, path, user, clean };
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
