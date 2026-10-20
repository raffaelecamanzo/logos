//! One scan at a time per project, whichever surface starts it (S-758,
//! [FR-SC-15], [FR-SC-24]): the web UI's Run scan through the real router, the
//! MCP `security_scan` call on the same engine — what `serve --mcp --ui` runs —
//! and a CLI `security scan` on an engine of its own, all against one project.
//!
//! The scanner is a stub that records each run and then waits, to a bound,
//! for a release file the test writes, so the test decides when a scan ends
//! and can look at the process table while it runs. The engine finds scanners
//! only in the stub directory and keeps its consent record in a temporary user
//! directory; the process environment is never changed. Every test runs under
//! [`BOUND`].
//!
//! [FR-SC-15]: ../../docs/specs/requirements/FR-SC-15.md
//! [FR-SC-24]: ../../docs/specs/requirements/FR-SC-24.md
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use logos_core::config::INSTALL_CONSENT_FILE;
use logos_core::observability::{in_surface, Surface};
use logos_core::security::{ScanEnvironment, ScanRequest, Selection};
use logos_core::tools::surface::InstallRequest;
use logos_core::Engine;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;
use web::{IntentToken, INTENT_HEADER};

/// The whole of any one test, scanners included.
const BOUND: Duration = Duration::from_secs(120);

/// How long a job or a stub is waited for before the test fails.
const JOB_WAIT: Duration = Duration::from_secs(30);

const ORIGIN: &str = "http://127.0.0.1:4983";

/// A Semgrep report with no finding.
const CLEAN_SEMGREP: &str = r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Semgrep OSS","rules":[]}},"results":[]}]}"#;

struct Fixture {
    _tmp: TempDir,
    repo: PathBuf,
    bin: PathBuf,
    user: PathBuf,
    rec: PathBuf,
    intent: IntentToken,
    router: Router,
    /// The engine the router serves: the one an MCP call in the same `serve`
    /// reaches.
    engine: Arc<Engine>,
}

impl Fixture {
    /// A one-commit repository whose engine finds scanners in `bin` alone and
    /// keeps its consent in `user`, with `semgrep` the one scanner, consented.
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let [repo, bin, user, rec] = ["project", "bin", "user", "rec"].map(|d| base.join(d));
        for dir in [&repo, &bin, &user, &rec] {
            std::fs::create_dir_all(dir).unwrap();
        }
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("app.py"), "import sqlite3\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "seed"]);
        let engine = Arc::new(open(&repo, &bin, &user, &rec));
        let intent = IntentToken::generate();
        let router = web::router_with_intent(Arc::clone(&engine), intent.clone());
        let f = Self { _tmp: tmp, repo, bin, user, rec, intent, router, engine };
        f.stub_semgrep();
        logos_core::config::record_scan_consent(Some(&f.user), &f.repo, &["semgrep"]).unwrap();
        f
    }

    /// Another engine on the project, as a CLI process opens one.
    fn open(&self) -> Engine {
        open(&self.repo, &self.bin, &self.user, &self.rec)
    }

    /// `semgrep`: answers its version probe, records each run in
    /// `rec/semgrep.runs`, then waits — at most 60 s — for `rec/release`
    /// before it writes a clean report.
    fn stub_semgrep(&self) {
        let rec = self.rec.display();
        let script = format!(
            "#!/bin/sh\n\
             case \"$1\" in --version|version) printf '%s\\n' 'semgrep 1.2.3'; exit 0;; esac\n\
             printf '%s\\n' \"$$\" >> '{rec}/semgrep.runs'\n\
             out=''; prev=''\n\
             for a in \"$@\"; do [ \"$prev\" = --output ] && out=\"$a\"; prev=\"$a\"; done\n\
             n=0; while [ ! -e '{rec}/release' ] && [ $n -lt 1200 ]; do /bin/sleep 0.05; n=$((n+1)); done\n\
             printf '%s' '{CLEAN_SEMGREP}' > \"$out\"\n"
        );
        let path = self.bin.join("semgrep");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn release(&self) {
        std::fs::write(self.rec.join("release"), "").unwrap();
    }

    /// Every run of the stub so far.
    fn runs(&self) -> usize {
        std::fs::read_to_string(self.rec.join("semgrep.runs")).map(|s| s.lines().count()).unwrap_or(0)
    }

    /// Wait until the stub has started `n` runs.
    async fn running(&self, n: usize) {
        let deadline = Instant::now() + JOB_WAIT;
        while self.runs() < n {
            assert!(Instant::now() < deadline, "semgrep never started");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The stub's processes in the process table: each a shell running this
    /// fixture's `bin/semgrep`, version probes included.
    fn scanner_processes(&self) -> Vec<String> {
        let out = Command::new("ps").args(["-A", "-o", "pid=,command="]).output().expect("ps runs");
        assert!(out.status.success(), "ps failed");
        let stub = self.bin.join("semgrep").display().to_string();
        String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.contains(&stub)).map(str::to_string).collect()
    }

    async fn send(&self, req: Request<Body>) -> (StatusCode, Value) {
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, body)
    }

    /// A same-origin, intent-bearing `POST`.
    async fn post(&self, path: &str, body: &str) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(header::HOST, "127.0.0.1:4983")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, ORIGIN)
            .header(INTENT_HEADER, self.intent.as_str())
            .body(Body::from(body.to_string()))
            .unwrap();
        self.send(req).await
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(Method::GET)
            .uri(path)
            .header(header::HOST, "127.0.0.1:4983")
            .body(Body::empty())
            .unwrap();
        self.send(req).await
    }

    /// The web scan job, once it is no longer running.
    async fn ended(&self) -> Value {
        let deadline = Instant::now() + JOB_WAIT;
        loop {
            let (status, body) = self.get("/api/v1/security/scan").await;
            assert_eq!(status, StatusCode::OK, "{body}");
            if body["job"]["state"] != "running" {
                return body["job"].clone();
            }
            assert!(Instant::now() < deadline, "the job is still running: {body}");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

/// An engine on `repo` finding scanners in `bin` alone, its consent in `user`
/// and its tools directory under `rec`.
fn open(repo: &Path, bin: &Path, user: &Path, rec: &Path) -> Engine {
    Engine::open(repo).with_scan_environment(ScanEnvironment {
        user_dir: Some(user.to_path_buf()),
        search_path: Some(bin.as_os_str().to_owned()),
        tools_dir: Some(rec.join("tools")),
    })
}

/// `security_scan` over MCP on `engine`, as `serve --mcp` runs it: on the
/// blocking pool, on the MCP surface.
async fn mcp_scan(engine: &Arc<Engine>) -> anyhow::Result<logos_core::security::SecurityScanSummary> {
    let engine = Arc::clone(engine);
    tokio::task::spawn_blocking(move || {
        in_surface(Surface::Mcp, || engine.security_scan_selected(false, &ScanRequest::full(), |_| {}))
    })
    .await
    .unwrap()
}

/// `logos security scan` in a CLI process: an engine of its own.
async fn cli_scan(engine: Engine) -> anyhow::Result<logos_core::security::SecurityScanSummary> {
    tokio::task::spawn_blocking(move || in_surface(Surface::Cli, || engine.security_scan(false))).await.unwrap()
}

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0", "-c", "user.email=t@logos", "-c", "user.name=t"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

async fn bounded(test: impl std::future::Future<Output = ()>) {
    tokio::time::timeout(BOUND, test).await.expect("the test ran past its bound");
}

/// AC 1 and 2: while the web UI's scan runs, an MCP scan in the same `serve`
/// and a CLI scan of the same project are refused, each naming the web job; a
/// second web start joins it. One scanner process runs throughout. Once the
/// job ends, a CLI scan runs.
#[tokio::test]
async fn beside_a_web_scan_mcp_and_cli_scans_are_refused_naming_it_and_a_second_web_start_joins() {
    bounded(async {
        let f = Fixture::new();
        let (status, first) = f.post("/api/v1/security/scan", "").await;
        assert_eq!(status, StatusCode::OK, "{first}");
        let job = first["job"]["id"].as_u64().unwrap();
        f.running(1).await;

        let named = format!(
            "a security scan is already running on this project: job {job}, started from the web UI by Logos process {} ",
            std::process::id()
        );
        let mcp = mcp_scan(&f.engine).await.expect_err("an MCP scan beside the web job is refused");
        assert!(mcp.to_string().starts_with(&named), "{mcp}");
        let cli = cli_scan(f.open()).await.expect_err("a CLI scan beside the web job is refused");
        assert!(cli.to_string().starts_with(&named), "{cli}");

        let (status, second) = f.post("/api/v1/security/scan", "{}").await;
        assert_eq!(status, StatusCode::OK, "{second}");
        assert_eq!((second["joined"].as_bool(), second["job"]["id"].as_u64()), (Some(true), Some(job)), "{second}");

        let processes = f.scanner_processes();
        assert_eq!(processes.len(), 1, "one scanner process runs on the project: {processes:?}");
        assert_eq!(f.runs(), 1, "the refused and joined starts ran no scanner");

        f.release();
        assert_eq!(f.ended().await["state"], "completed");
        let after = cli_scan(f.open()).await.expect("the project is free once the job ended");
        assert!(after.scanners.iter().any(|s| s.scanner == "semgrep"), "{after:?}");
        assert_eq!(f.runs(), 2);
    })
    .await;
}

/// AC 2 from the other side: while an MCP scan runs in the same `serve`, the
/// web UI's Run scan, its Install, and a CLI scan are refused naming it; the
/// refused web starts record no job, and no consent their dialog accepted.
#[tokio::test]
async fn beside_an_mcp_scan_web_and_cli_starts_are_refused_naming_it() {
    bounded(async {
        let f = Fixture::new();
        let engine = Arc::clone(&f.engine);
        let mcp = tokio::task::spawn_blocking(move || {
            in_surface(Surface::Mcp, || engine.security_scan_selected(false, &ScanRequest::full(), |_| {}))
        });
        f.running(1).await;
        // A scanner installed since, which the project has not consented to.
        let bandit = f.bin.join("bandit");
        std::fs::write(&bandit, "#!/bin/sh\nprintf '%s\\n' 'bandit 1.7.10'\n").unwrap();
        std::fs::set_permissions(&bandit, std::fs::Permissions::from_mode(0o755)).unwrap();

        let named = format!(
            "a security scan is already running on this project: started over MCP by Logos process {} ",
            std::process::id()
        );
        let (status, scan) = f.post("/api/v1/security/scan", r#"{"consent":["bandit"]}"#).await;
        assert_eq!(status, StatusCode::CONFLICT, "{scan}");
        assert!(scan["error"].as_str().unwrap().starts_with(&named), "{scan}");
        let consented = logos_core::config::load_scan_consent(Some(&f.user)).unwrap().consented(&f.repo);
        assert!(!consented.iter().any(|s| s == "bandit"), "the refused start recorded no consent: {consented:?}");
        let (status, install) = f.post("/api/v1/security/install", r#"{"tools":["gitleaks"]}"#).await;
        assert_eq!(status, StatusCode::CONFLICT, "{install}");
        assert!(install["error"].as_str().unwrap().starts_with(&named), "{install}");
        assert_eq!(f.get("/api/v1/security/scan").await.1["job"], Value::Null, "the refused start recorded no job");
        let cli = cli_scan(f.open()).await.expect_err("a CLI scan beside the MCP scan is refused");
        assert!(cli.to_string().starts_with(&named), "{cli}");
        assert_eq!(f.scanner_processes().len(), 1, "one scanner process runs on the project");

        f.release();
        mcp.await.unwrap().expect("the MCP scan completes");
        assert_eq!(f.runs(), 1);
    })
    .await;
}

/// AC 3: while a scan runs, a CLI install of the project is refused naming
/// the scan, before it downloads or records anything.
#[tokio::test]
async fn beside_a_web_scan_a_cli_install_is_refused_naming_it() {
    bounded(async {
        let f = Fixture::new();
        let (status, started) = f.post("/api/v1/security/scan", "").await;
        assert_eq!(status, StatusCode::OK, "{started}");
        f.running(1).await;

        let engine = f.open();
        let install = tokio::task::spawn_blocking(move || {
            in_surface(Surface::Cli, || {
                engine.security_install(&InstallRequest {
                    tools: Selection::Only(vec!["gitleaks".into()]),
                    update: false,
                    from: None,
                    consent: true,
                    accept_licences: Vec::new(),
                })
            })
        })
        .await
        .unwrap()
        .expect_err("an install beside the scan is refused");
        let job = started["job"]["id"].as_u64().unwrap();
        assert!(
            install.to_string().starts_with(&format!("a security scan is already running on this project: job {job}, ")),
            "{install}"
        );
        assert!(!f.rec.join("tools").exists(), "the refused install touched no tools directory");
        assert!(!f.user.join(INSTALL_CONSENT_FILE).exists(), "the refused install recorded no consent");

        f.release();
        assert_eq!(f.ended().await["state"], "completed");
    })
    .await;
}
