//! `GET /api/v1/workspace/security` starts a member's scanner version probe
//! only for a scanner that member's scan consent names, and only once per
//! serve process for each binary, as the member's own Security page does
//! (HF-3, [ADR-38], [ADR-80] §4). A cross-site read starts nothing new.
//!
//! A workspace builds each member's engine itself, from the process
//! environment, so this binary holds exactly one test and points that
//! environment at stubs before anything starts: `PATH` holds only the stub
//! scanners and `git`, and the user-level config and data directories are
//! temporary. No scanner of this machine is found, and no other test shares
//! the process whose environment is changed. Each stub records every
//! invocation, its version probe included, in `rec/<name>.calls`.
//!
//! [ADR-38]: ../../docs/specs/architecture/decisions/ADR-38.md
//! [ADR-80]: ../../docs/specs/architecture/decisions/ADR-80.md
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use logos_core::federation::{discover, EngineRegistry};
use logos_core::Engine;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

const BOUND: Duration = Duration::from_secs(120);

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

fn member(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("app.py"), "import sqlite3\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "seed"]);
}

/// A stub scanner that records each invocation in `rec/<name>.calls` and
/// answers its version command.
fn stub(bin: &Path, rec: &Path, name: &str) {
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{rec}/{name}.calls'\nprintf '%s\\n' '{name} 1.2.3'\n",
        rec = rec.display(),
    );
    let path = bin.join(name);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn calls(rec: &Path, name: &str) -> usize {
    std::fs::read_to_string(rec.join(format!("{name}.calls"))).map(|s| s.lines().count()).unwrap_or(0)
}

/// `GET path`, with `origin` as its `Origin` when given; the body of its `200`.
async fn get(router: &Router, path: &str, origin: Option<&str>) -> Value {
    let mut req = Request::builder().method(Method::GET).uri(path).header(header::HOST, "127.0.0.1:4983");
    if let Some(origin) = origin {
        req = req.header(header::ORIGIN, origin).header("sec-fetch-site", "cross-site");
    }
    let resp = router.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::OK, "{path}: {body}");
    body
}

#[tokio::test]
async fn the_workspace_roll_up_probes_only_consented_scanners_once_per_process() {
    tokio::time::timeout(BOUND, async {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (root, bin, user, data, rec) =
            (base.join("ws"), base.join("bin"), base.join("user"), base.join("data"), base.join("rec"));
        for dir in [&root, &bin, &user, &data, &rec] {
            std::fs::create_dir_all(dir).unwrap();
        }
        member(&root.join("api"));
        member(&root.join("web"));
        std::fs::write(root.join("logos.workspace.toml"), "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"web\"]\ndefault = \"api\"\n").unwrap();

        let real_git = String::from_utf8(Command::new("sh").args(["-c", "command -v git"]).output().unwrap().stdout).unwrap();
        std::os::unix::fs::symlink(real_git.trim(), bin.join("git")).unwrap();
        for name in ["semgrep", "bandit"] {
            stub(&bin, &rec, name);
        }
        std::env::set_var("PATH", &bin);
        std::env::set_var(logos_core::config::USER_CONFIG_HOME_ENV, &user);
        std::env::set_var(logos_core::config::USER_DATA_HOME_ENV, &data);
        // Both members consent to Semgrep alone.
        let user_dir = logos_core::config::user_config_dir().expect("a user config directory");
        for name in ["api", "web"] {
            logos_core::config::record_scan_consent(Some(&user_dir), &root.join(name), &["semgrep"]).unwrap();
        }

        let federation = discover(&root).expect("discovery succeeds").expect("a workspace");
        let router = web::workspace_router(EngineRegistry::<Engine>::new_serve_default(federation))
            .expect("the workspace router builds");

        let first = get(&router, "/api/v1/workspace/security", None).await;
        assert_eq!(first["security"]["members_read"], 2, "both members are read: {first}");
        get(&router, "/api/v1/workspace/security", None).await;
        get(&router, "/api/v1/workspace/security", Some("https://attacker.example")).await;
        assert_eq!(calls(&rec, "bandit"), 0, "the unconsented scanner is never started");
        assert_eq!(calls(&rec, "semgrep"), 1, "one binary, probed once across members and reads");
    })
    .await
    .expect("the test ran past its bound");
}
