//! The Security page's Run scan job (S-673, [FR-SC-15], [FR-SC-07],
//! [NFR-SE-06], [UAT-SC-05]) through the real router, in process.
//!
//! Every fixture finds scanners only in a stub directory and keeps its consent
//! record in a temporary user directory, so no scanner of this machine runs
//! and the process environment is never changed. The stubs that must still be
//! running when the test acts sleep (and leave a grandchild behind) until the
//! job is cancelled; the scanner `PATH` is the stub directory alone, so they
//! name `/bin/sleep` absolutely. Every test runs under [`BOUND`].
//!
//! [FR-SC-15]: ../../docs/specs/requirements/FR-SC-15.md
//! [FR-SC-07]: ../../docs/specs/requirements/FR-SC-07.md
//! [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md
//! [UAT-SC-05]: ../../docs/specs/requirements/UAT-SC-05.md
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

/// The whole of any one test, scanners included.
const BOUND: Duration = Duration::from_secs(120);

/// How long a job is waited for before the test fails.
const JOB_WAIT: Duration = Duration::from_secs(30);

const ORIGIN: &str = "http://127.0.0.1:4983";

/// The stderr bytes a scanner run keeps (`jobs::spawn::STDERR_CAP`): the
/// failure log holds at most this much.
const STDERR_CAP: usize = 16 * 1024;

/// A Bandit report with no finding.
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
    /// A one-commit repository whose engine looks for scanners in `bin` and
    /// keeps its consent in `user`. Stubs are written into `bin` afterwards: a
    /// scanner is looked up when a scan starts.
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
        let env = logos_core::security::ScanEnvironment {
            user_dir: Some(user.clone()),
            search_path: Some(bin.clone().into_os_string()),
            tools_dir: None,
        };
        let engine = Engine::start(&repo).expect("engine starts").with_scan_environment(env);
        let intent = IntentToken::generate();
        let router = web::router_with_intent(Arc::new(engine), intent.clone());
        Self { _tmp: tmp, repo, bin, user, rec, intent, router }
    }

    fn consent(&self, scanners: &[&str]) {
        logos_core::config::record_scan_consent(Some(&self.user), &self.repo, scanners).unwrap();
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

    /// The member's job, once it is no longer running.
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

    /// The `$$ $!` line a sleeping stub wrote, once it is there: the stub's
    /// shell (its group leader) and the sleeping grandchild.
    async fn sleeping(&self, name: &str) -> (u32, u32) {
        let path = self.rec.join(format!("{name}.pids"));
        let deadline = Instant::now() + JOB_WAIT;
        loop {
            if let Some(line) = std::fs::read_to_string(&path).ok().filter(|s| s.ends_with('\n')) {
                let pids: Vec<u32> = line.split_whitespace().map(|p| p.parse().unwrap()).collect();
                return (pids[0], pids[1]);
            }
            assert!(Instant::now() < deadline, "{name} never started sleeping");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Every recorded non-version invocation of `name`.
    fn runs(&self, name: &str) -> usize {
        std::fs::read_to_string(self.rec.join(format!("{name}.runs")))
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }
}

/// A stub scanner: answers the version probe, records each other invocation in
/// `rec/<name>.runs`, then runs `body` with `$out` set to its `--output` path.
fn stub(bin: &Path, rec: &Path, name: &str, body: &str) {
    let script = format!(
        "#!/bin/sh\n\
         case \"$1\" in --version|version) printf '%s\\n' '{name} 1.2.3'; exit 0;; esac\n\
         printf '%s\\n' \"$*\" >> '{rec}/{name}.runs'\n\
         out=''; prev=''\n\
         for a in \"$@\"; do [ \"$prev\" = --output ] && out=\"$a\"; prev=\"$a\"; done\n\
         {body}\n",
        rec = rec.display(),
    );
    let path = bin.join(name);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A stub body that leaves a grandchild sleeping and waits for it, writing
/// both pids to `rec/<name>.pids` first.
fn sleeper(rec: &Path, name: &str) -> String {
    format!("/bin/sleep 60 & echo $$ $! > '{}/{name}.pids'\nwait", rec.display())
}

/// A stub body that writes a clean Bandit report.
fn clean_report() -> String {
    format!("printf '%s' '{CLEAN_BANDIT}' > \"$out\"")
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

/// `(pid, pgid)` of every process in the process table.
fn process_table() -> Vec<(u32, u32)> {
    let out = Command::new("ps").args(["-A", "-o", "pid=,pgid="]).output().expect("ps runs");
    assert!(out.status.success(), "ps failed");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace().map(|c| c.parse::<u32>());
            Some((cols.next()?.ok()?, cols.next()?.ok()?))
        })
        .collect()
}

/// Wait until the process table holds no member of `group` and not `pid`.
async fn group_gone(group: u32, pid: u32) -> Result<(), Vec<(u32, u32)>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let left: Vec<(u32, u32)> = process_table()
            .into_iter()
            .filter(|&(p, g)| g == group || p == pid || p == group)
            .collect();
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

/// AC 1: a second start while the scan runs joins it — the same job, and the
/// scanner runs once.
#[tokio::test]
async fn a_second_start_while_running_joins_the_same_job() {
    bounded(async {
        let f = Fixture::new();
        stub(&f.bin, &f.rec, "semgrep", &sleeper(&f.rec, "semgrep"));
        f.consent(&["semgrep"]);

        let (status, first) = f.post("/api/v1/security/scan", "").await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert_eq!((first["joined"].as_bool(), first["job"]["state"].as_str()), (Some(false), Some("running")), "{first}");
        assert_eq!(first["job"]["kind"], "security-scan");
        f.sleeping("semgrep").await;

        let (status, second) = f.post("/api/v1/security/scan", "{}").await;
        assert_eq!(status, StatusCode::OK, "{second}");
        assert_eq!(second["joined"], true, "the second click joins: {second}");
        assert_eq!(second["job"]["id"], first["job"]["id"], "the same job");

        f.post("/api/v1/security/scan/cancel", "").await;
        assert_eq!(f.ended().await["state"], "cancelled");
        assert_eq!(f.runs("semgrep"), 1, "the joined start ran no second scan");
    })
    .await;
}

/// AC 2: cancel kills the scanner's whole process group — the stub's shell and
/// the grandchild it left — so the process table holds none of it; the scanner
/// after it never starts, and the cancel is no scanner failure.
#[tokio::test]
async fn cancel_leaves_no_scanner_process_alive() {
    bounded(async {
        let f = Fixture::new();
        stub(&f.bin, &f.rec, "semgrep", &sleeper(&f.rec, "semgrep"));
        stub(&f.bin, &f.rec, "bandit", &clean_report());
        f.consent(&["semgrep", "bandit"]);

        let (status, started) = f.post("/api/v1/security/scan", "").await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let (shell, sleeper) = f.sleeping("semgrep").await;
        assert!(process_table().iter().any(|&(p, g)| p == sleeper && g == shell), "the grandchild runs in the scanner's group");

        let (status, cancelling) = f.post("/api/v1/security/scan/cancel", "").await;
        assert_eq!(status, StatusCode::OK, "{cancelling}");
        assert_eq!(cancelling["job"]["id"], started["job"]["id"]);
        let job = f.ended().await;
        assert_eq!(job["state"], "cancelled", "{job}");
        if let Err(left) = group_gone(shell, sleeper).await {
            panic!("scanner processes survived the cancel (pid, pgid): {left:?}");
        }
        assert_eq!(f.runs("bandit"), 0, "a scanner started after the cancel");
        assert!(!f.repo.join(".logos/security/semgrep.failed.log").exists(), "a cancel is no scanner failure");
        let events = job["events"].as_array().unwrap();
        assert_eq!(events.first().map(|e| e["event"].clone()), Some(json!({ "phase": "started", "scanner": "semgrep" })), "{job}");
    })
    .await;
}

/// AC 3: while a scan runs, serve answers other API requests — the status read
/// returns while the job is still running.
#[tokio::test]
async fn serve_answers_other_api_requests_during_a_scan() {
    bounded(async {
        let f = Fixture::new();
        stub(&f.bin, &f.rec, "semgrep", &sleeper(&f.rec, "semgrep"));
        f.consent(&["semgrep"]);
        f.post("/api/v1/security/scan", "").await;
        f.sleeping("semgrep").await;

        let answered = tokio::time::timeout(Duration::from_secs(10), f.get("/api/v1/status")).await;
        let (status, body) = answered.expect("/api/v1/status answered during the scan");
        assert_eq!(status, StatusCode::OK, "{body}");
        let (_, job) = f.get("/api/v1/security/scan").await;
        assert_eq!(job["job"]["state"], "running", "the scan was still running: {job}");

        f.post("/api/v1/security/scan/cancel", "").await;
        assert_eq!(f.ended().await["state"], "cancelled");
    })
    .await;
}

/// A scanner that floods both pipes and fails: what is kept is bounded — the
/// failure log holds the last part of stderr, at most the capture bound, and
/// the job's payload carries only a short excerpt.
#[tokio::test]
async fn scanner_output_capture_is_bounded() {
    bounded(async {
        let f = Fixture::new();
        let flood = "printf '%s\\n' START-OF-OUTPUT >&2\n\
                     /usr/bin/yes 'stdout line of a flooding scanner, fifty bytes long' | /usr/bin/head -n 40000\n\
                     /usr/bin/yes 'stderr line of a flooding scanner, fifty bytes long' | /usr/bin/head -n 40000 >&2\n\
                     printf '%s\\n' END-OF-STDERR >&2\n\
                     exit 3";
        stub(&f.bin, &f.rec, "bandit", flood);
        f.consent(&["bandit"]);
        f.post("/api/v1/security/scan", "").await;
        let job = f.ended().await;
        assert_eq!(job["state"], "completed", "{job}");

        let bandit = job["result"]["scanners"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["scanner"] == "bandit")
            .unwrap();
        assert_eq!((bandit["outcome"].as_str(), bandit["exit_code"].as_i64()), (Some("failed"), Some(3)), "{bandit}");
        let log = std::fs::read_to_string(f.repo.join(".logos/security/bandit.failed.log")).unwrap();
        assert!(log.len() <= STDERR_CAP, "the failure log holds {} bytes", log.len());
        assert!(log.contains("END-OF-STDERR"), "the tail of stderr is kept");
        assert!(!log.contains("START-OF-OUTPUT"), "the head of a flood is dropped");
        assert!(bandit["stderr"].as_str().unwrap().len() <= 1024, "{bandit}");
        assert!(job.to_string().len() < 64 * 1024, "the job payload is bounded: {} bytes", job.to_string().len());
    })
    .await;
}

/// AC 4: without consent, a start answers `409` naming the scanners and starts
/// nothing; the start that carries the dialog's consent records it in the user
/// config directory — never in the repository — and scans; later starts are
/// not asked again.
#[tokio::test]
async fn consent_is_asked_once_and_recorded_in_the_user_config_directory() {
    bounded(async {
        let f = Fixture::new();
        stub(&f.bin, &f.rec, "bandit", &clean_report());

        let (status, refused) = f.post("/api/v1/security/scan", "").await;
        assert_eq!(status, StatusCode::CONFLICT, "{refused}");
        assert_eq!(refused["consent_required"], json!(["bandit"]), "{refused}");
        assert_eq!(f.get("/api/v1/security/scan").await.1["job"], Value::Null, "nothing started");
        assert_eq!(f.runs("bandit"), 0);

        let (status, started) = f.post("/api/v1/security/scan", r#"{"consent":["bandit"]}"#).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let job = f.ended().await;
        assert_eq!(job["state"], "completed", "{job}");
        let bandit = job["result"]["scanners"].as_array().unwrap().iter().find(|s| s["scanner"] == "bandit").unwrap().clone();
        assert_eq!(bandit["outcome"], "ran", "{job}");

        let record = std::fs::read_to_string(f.user.join(logos_core::config::SCAN_CONSENT_FILE))
            .expect("the consent lands in the user config directory");
        assert!(record.contains("bandit"), "{record}");
        let in_repo: Vec<PathBuf> = walk(&f.repo)
            .into_iter()
            .filter(|p| p.file_name().is_some_and(|n| n == logos_core::config::SCAN_CONSENT_FILE))
            .collect();
        assert!(in_repo.is_empty(), "a consent record was written inside the repository: {in_repo:?}");

        let (status, again) = f.post("/api/v1/security/scan", "").await;
        assert_eq!(status, StatusCode::OK, "consent is not asked twice: {again}");
        assert_eq!(f.ended().await["state"], "completed");
        assert_eq!(f.runs("bandit"), 2);
    })
    .await;
}

/// [FR-SC-07]: consent is recorded only for the scanners the dialog showed. A
/// dialog drawn before Semgrep was installed accepted Bandit alone, so the
/// start is refused `409` naming both, nothing is recorded and nothing starts;
/// accepting the list the refusal named records both and scans.
#[tokio::test]
async fn consent_is_recorded_only_for_the_scanners_the_dialog_showed() {
    bounded(async {
        let f = Fixture::new();
        stub(&f.bin, &f.rec, "bandit", &clean_report());
        stub(&f.bin, &f.rec, "semgrep", "exit 0");

        let (status, refused) = f.post("/api/v1/security/scan", r#"{"consent":["bandit"]}"#).await;
        assert_eq!(status, StatusCode::CONFLICT, "{refused}");
        assert_eq!(refused["consent_required"], json!(["semgrep", "bandit"]), "{refused}");
        assert!(!f.user.join(logos_core::config::SCAN_CONSENT_FILE).exists(), "consent was recorded for a scanner never shown");
        assert_eq!(f.get("/api/v1/security/scan").await.1["job"], Value::Null, "nothing started");

        let (status, started) = f.post("/api/v1/security/scan", r#"{"consent":["semgrep","bandit"]}"#).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        f.ended().await;
        let record = std::fs::read_to_string(f.user.join(logos_core::config::SCAN_CONSENT_FILE)).unwrap();
        assert!(record.contains("semgrep") && record.contains("bandit"), "{record}");
    })
    .await;
}

/// [NFR-SE-06]: a cross-origin start, and one without the session's intent
/// token, are refused before any handler runs — no job starts, and no scanner.
#[tokio::test]
async fn a_cross_origin_or_tokenless_start_is_refused_and_starts_nothing() {
    bounded(async {
        let f = Fixture::new();
        stub(&f.bin, &f.rec, "bandit", &clean_report());
        f.consent(&["bandit"]);
        let token = f.intent.as_str().to_string();
        for (req, why) in [
            (post("/api/v1/security/scan", Some(&token), Some("http://evil.example"), "{}"), "a cross-origin page"),
            (post("/api/v1/security/scan", Some(&token), None, "{}"), "no Origin"),
            (post("/api/v1/security/scan", None, Some(ORIGIN), "{}"), "no intent token"),
            (post("/api/v1/security/scan", Some("forged"), Some(ORIGIN), "{}"), "a forged token"),
            (post("/api/v1/security/scan/cancel", Some(&token), Some("http://evil.example"), ""), "a cross-origin cancel"),
        ] {
            let (status, body) = f.send(req).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{why} must be refused: {body}");
        }
        assert_eq!(f.get("/api/v1/security/scan").await.1["job"], Value::Null, "no job started");
        assert_eq!(f.runs("bandit"), 0, "no scanner started");

        let (status, body) = f.post("/api/v1/security/scan", r#"{"consent":"yes please"}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(f.get("/api/v1/security/scan").await.1["job"], Value::Null, "a malformed start started nothing");
    })
    .await;
}

/// The job's events replay each scanner's start and outcome, and its result
/// is the scan summary; the page then reads the stored report.
#[tokio::test]
async fn a_completed_scan_replays_its_progress_and_stores_its_report() {
    bounded(async {
        let f = Fixture::new();
        stub(&f.bin, &f.rec, "bandit", &clean_report());
        f.consent(&["bandit"]);
        assert_eq!(f.get("/api/v1/security/scan").await.1, json!({ "job": null }));
        f.post("/api/v1/security/scan", "").await;
        let job = f.ended().await;
        assert_eq!(job["state"], "completed", "{job}");
        let phases: Vec<(String, String)> = job["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e["event"]["phase"].as_str().unwrap().to_string(), e["event"]["scanner"].as_str().unwrap().to_string()))
            .collect();
        let bandit_started = phases.iter().position(|p| *p == ("started".to_string(), "bandit".to_string())).expect("bandit started");
        assert_eq!(phases[bandit_started + 1], ("finished".to_string(), "bandit".to_string()));
        assert_eq!(phases.iter().filter(|(phase, _)| phase == "finished").count(), 9, "every registered scanner settles: {phases:?}");
        let bandit = job["result"]["scanners"].as_array().unwrap().iter().find(|s| s["scanner"] == "bandit").unwrap();
        assert_eq!(bandit["outcome"], "ran", "{bandit}");

        let (_, page) = f.get("/api/v1/security").await;
        let row = page["security"]["status"]["scanners"].as_array().unwrap().iter().find(|s| s["scanner"] == "bandit").unwrap().clone();
        assert_eq!(row["state"], "fresh", "{row}");
    })
    .await;
}

/// A cancel with no scan answers `{job: null}` and starts nothing; one after
/// the scan ended leaves it `completed`, its result kept.
#[tokio::test]
async fn a_cancel_with_no_scan_running_changes_nothing() {
    bounded(async {
        let f = Fixture::new();
        stub(&f.bin, &f.rec, "bandit", &clean_report());
        f.consent(&["bandit"]);
        assert_eq!(f.post("/api/v1/security/scan/cancel", "").await, (StatusCode::OK, json!({ "job": null })));
        assert_eq!(f.runs("bandit"), 0);

        f.post("/api/v1/security/scan", "").await;
        let done = f.ended().await;
        let (status, after) = f.post("/api/v1/security/scan/cancel", "").await;
        assert_eq!(status, StatusCode::OK, "{after}");
        assert_eq!(after["job"]["state"], "completed", "{after}");
        assert_eq!(after["job"]["result"], done["result"], "the result is kept");
    })
    .await;
}

/// The consent check before a start reads the repository and user config: an
/// invalid one answers `500` with the reason, and nothing starts.
#[tokio::test]
async fn an_invalid_config_refuses_the_start_and_starts_nothing() {
    bounded(async {
        let f = Fixture::new();
        stub(&f.bin, &f.rec, "bandit", &clean_report());
        std::fs::create_dir_all(f.repo.join(".logos")).unwrap();
        std::fs::write(f.repo.join(".logos/config.toml"), "[security_scan]\ncommand = \"rm -rf /\"\n").unwrap();
        let (status, body) = f.post("/api/v1/security/scan", r#"{"consent":["bandit"]}"#).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        assert!(body["error"].as_str().unwrap().contains("command"), "the reason names the key: {body}");
        assert_eq!(f.get("/api/v1/security/scan").await.1["job"], Value::Null, "nothing started");
        assert!(!f.user.join(logos_core::config::SCAN_CONSENT_FILE).exists(), "no consent was recorded");
        assert_eq!(f.runs("bandit"), 0);
    })
    .await;
}

/// AC 1 with a scanner installed mid-scan: a second click while the scan runs
/// joins it even though that scanner now lacks consent — the running scan does
/// not include it, so nothing new is asked.
#[tokio::test]
async fn a_click_while_running_joins_without_asking_consent() {
    bounded(async {
        let f = Fixture::new();
        stub(&f.bin, &f.rec, "semgrep", &sleeper(&f.rec, "semgrep"));
        let (status, first) = f.post("/api/v1/security/scan", r#"{"consent":["semgrep"]}"#).await;
        assert_eq!(status, StatusCode::OK, "{first}");
        f.sleeping("semgrep").await;
        stub(&f.bin, &f.rec, "bandit", &clean_report());

        let (status, second) = f.post("/api/v1/security/scan", "{}").await;
        assert_eq!(status, StatusCode::OK, "a click while running is never refused for consent: {second}");
        assert_eq!((second["joined"].as_bool(), second["job"]["id"].clone()), (Some(true), first["job"]["id"].clone()));

        f.post("/api/v1/security/scan/cancel", "").await;
        assert_eq!(f.ended().await["state"], "cancelled");
    })
    .await;
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() && !path.is_symlink() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}
