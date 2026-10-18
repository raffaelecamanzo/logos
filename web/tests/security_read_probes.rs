//! `GET /api/v1/security` starts a scanner's version probe only for a scanner
//! the project's scan consent names, and only once per serve process for each
//! binary (HF-3, [ADR-38], [ADR-80] §4): a page read, a cross-site one
//! included, starts no other process. The reads that run `check` over a
//! `[security]` table start none at all.
//!
//! Every fixture finds scanners only in its own stub directory and keeps its
//! consent record in a temporary user directory, so no scanner of this
//! machine runs and the process environment is never changed. Each stub
//! records every invocation, the version probe included, in `rec/<name>.calls`.
//!
//! [ADR-38]: ../../docs/specs/architecture/decisions/ADR-38.md
//! [ADR-80]: ../../docs/specs/architecture/decisions/ADR-80.md
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use logos_core::Engine;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

struct Fixture {
    _tmp: TempDir,
    repo: PathBuf,
    bin: PathBuf,
    user: PathBuf,
    rec: PathBuf,
    router: Router,
}

impl Fixture {
    /// A one-commit repository whose engine finds `semgrep` and `bandit` stubs
    /// in `bin` and keeps its consent in `user`.
    fn new() -> Self {
        Self::with_rules(None)
    }

    /// [`new`](Self::new), with `rules` as its `.logos/rules.toml`.
    fn with_rules(rules: Option<&str>) -> Self {
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
        if let Some(rules) = rules {
            std::fs::create_dir_all(repo.join(".logos")).unwrap();
            std::fs::write(repo.join(".logos/rules.toml"), rules).unwrap();
        }
        for name in ["semgrep", "bandit"] {
            stub(&bin, &rec, name, "1.2.3");
        }
        let env = logos_core::security::ScanEnvironment {
            user_dir: Some(user.clone()),
            search_path: Some(bin.clone().into_os_string()),
            tools_dir: None,
        };
        let engine = Engine::start(&repo).expect("engine starts").with_scan_environment(env);
        engine.index();
        let router = web::router(Arc::new(engine));
        Self { _tmp: tmp, repo, bin, user, rec, router }
    }

    fn consent(&self, scanners: &[&str]) {
        logos_core::config::record_scan_consent(Some(&self.user), &self.repo, scanners).unwrap();
    }

    /// `GET /api/v1/security`, with `origin` as its `Origin` when given; the
    /// page's scanner rows.
    async fn security(&self, origin: Option<&str>) -> Vec<Value> {
        let body = self.get("/api/v1/security", origin).await;
        body["security"]["status"]["scanners"].as_array().expect("a scanner matrix").clone()
    }

    /// `GET path`, with `origin` as its `Origin` when given; the body of its
    /// `200`.
    async fn get(&self, path: &str, origin: Option<&str>) -> Value {
        let mut builder = Request::builder()
            .method(Method::GET)
            .uri(path)
            .header(header::HOST, "127.0.0.1:4983");
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin).header("sec-fetch-site", "cross-site");
        }
        let resp = self.router.clone().oneshot(builder.body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        body
    }

    /// Every recorded invocation of `name`, its version probe included.
    fn calls(&self, name: &str) -> usize {
        std::fs::read_to_string(self.rec.join(format!("{name}.calls")))
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }
}

/// A stub scanner that records each invocation in `rec/<name>.calls` and
/// answers its version command with `version`.
fn stub(bin: &Path, rec: &Path, name: &str, version: &str) {
    let script = format!(
        "#!/bin/sh\n\
         printf '%s\\n' \"$*\" >> '{rec}/{name}.calls'\n\
         printf '%s\\n' '{name} {version}'\n",
        rec = rec.display(),
    );
    let path = bin.join(name);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn row<'a>(rows: &'a [Value], scanner: &str) -> &'a Value {
    rows.iter().find(|r| r["scanner"] == scanner).unwrap_or_else(|| panic!("no {scanner} row: {rows:?}"))
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

/// An unconsented scanner is never probed: it reads installed, its version
/// unknown with the reason, and no install command is made up for it. A
/// consented one is probed once across two page reads and a cross-site one.
#[tokio::test]
async fn the_page_probes_only_consented_scanners_once_per_process() {
    let f = Fixture::new();
    f.consent(&["semgrep"]);

    let rows = f.security(None).await;
    assert_eq!(f.calls("bandit"), 0, "the unconsented scanner is never started");
    assert_eq!(f.calls("semgrep"), 1, "the consented scanner is probed");
    let bandit = row(&rows, "bandit");
    assert_eq!(bandit["state"], "never_run", "{bandit}");
    assert_eq!(bandit["installed"]["copy"], "path", "{bandit}");
    assert_eq!(bandit["installed"]["version"], Value::Null, "never a made-up version: {bandit}");
    assert_eq!(bandit["installed"]["unprobed"], "not_consented", "the reason is stated: {bandit}");
    assert_eq!(bandit["install"], Value::Null, "an unread version is not below the minimum: {bandit}");
    let semgrep = row(&rows, "semgrep");
    assert_eq!(semgrep["installed"]["version"], "1.2.3", "{semgrep}");
    assert!(semgrep["installed"].get("unprobed").is_none(), "a probed copy carries no reason: {semgrep}");

    let again = f.security(None).await;
    assert_eq!(f.calls("semgrep"), 1, "the second read is served the cached probe");
    assert_eq!(row(&again, "semgrep")["installed"]["version"], "1.2.3");

    let cross = f.security(Some("https://attacker.example")).await;
    assert_eq!((f.calls("semgrep"), f.calls("bandit")), (1, 0), "a cross-site read starts nothing new");
    assert_eq!(row(&cross, "bandit")["installed"]["unprobed"], "not_consented");
}

/// The cache is keyed by the binary and its modification time: a reinstalled
/// copy is probed again, and its new version read.
#[tokio::test]
async fn a_reinstalled_scanner_is_probed_again() {
    let f = Fixture::new();
    f.consent(&["semgrep", "bandit"]);
    f.security(None).await;
    assert_eq!((f.calls("semgrep"), f.calls("bandit")), (1, 1));

    stub(&f.bin, &f.rec, "semgrep", "1.3.0");
    let later = SystemTime::now() + Duration::from_secs(60);
    std::fs::File::options().write(true).open(f.bin.join("semgrep")).unwrap().set_modified(later).unwrap();
    let rows = f.security(None).await;
    assert_eq!((f.calls("semgrep"), f.calls("bandit")), (2, 1), "only the changed binary is probed again");
    assert_eq!(row(&rows, "semgrep")["installed"]["version"], "1.3.0");
}

/// `check`'s `[security]` evidence reads only each scanner's state, which no
/// version changes: the Dashboard and the gaps view, which both run `check`,
/// start no scanner process, consented or not, cross-site or not.
#[tokio::test]
async fn the_reads_that_run_check_start_no_scanner() {
    let f = Fixture::with_rules(Some("[security]\nrequired_scanners = [\"semgrep\"]\n"));
    f.consent(&["semgrep"]);
    for path in ["/api/v1/overview", "/api/v1/gaps"] {
        f.get(path, None).await;
        f.get(path, Some("https://attacker.example")).await;
    }
    assert_eq!((f.calls("semgrep"), f.calls("bandit")), (0, 0), "no read that runs check starts a scanner");
}
