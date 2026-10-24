//! `GET /api/v1/workspace/coverage`, the Workspace Coverage page's read
//! (S-740, [FR-CV-16]), starts no coverage tool process — not even a version
//! probe — read same-origin or cross-site, as each member's own Coverage page
//! reads it ([ADR-38]). The control is a probing `coverage status` of one
//! member, which asks the same stub for its version once: the stub is found,
//! so its silence under the workspace read is the read's, not a missing tool's.
//!
//! A workspace builds each member's engine itself, from the process
//! environment, so this binary holds exactly one test and points that
//! environment at stubs before anything starts: `PATH` holds only the stub
//! `cargo-llvm-cov` and `git`, and the user-level config and data directories
//! are temporary. No tool of this machine is found, and no other test shares
//! the process whose environment is changed. The stub records every
//! invocation in `rec/cargo-llvm-cov.calls`.
//!
//! [FR-CV-16]: ../../docs/specs/requirements/FR-CV-16.md
//! [ADR-38]: ../../docs/specs/architecture/decisions/ADR-38.md
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

/// A Rust member: its `Cargo.toml` is the marker cargo-llvm-cov applies to.
fn member(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(dir.join("src/lib.rs"), "pub fn m() {}\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "seed"]);
}

/// A stub `cargo-llvm-cov` that records each invocation and answers its
/// version command.
fn stub(bin: &Path, rec: &Path) {
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{rec}/cargo-llvm-cov.calls'\nprintf '%s\\n' 'cargo-llvm-cov 0.6.0'\n",
        rec = rec.display(),
    );
    let path = bin.join("cargo-llvm-cov");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn calls(rec: &Path) -> usize {
    std::fs::read_to_string(rec.join("cargo-llvm-cov.calls")).map(|s| s.lines().count()).unwrap_or(0)
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
async fn the_workspace_coverage_read_starts_no_tool() {
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
        stub(&bin, &rec);
        std::env::set_var("PATH", &bin);
        std::env::set_var(logos_core::config::USER_CONFIG_HOME_ENV, &user);
        std::env::set_var(logos_core::config::USER_DATA_HOME_ENV, &data);

        let federation = discover(&root).expect("discovery succeeds").expect("a workspace");
        let router = web::workspace_router(EngineRegistry::<Engine>::new_serve_default(federation))
            .expect("the workspace router builds");

        for origin in [None, Some("https://attacker.example"), None] {
            let v = get(&router, "/api/v1/workspace/coverage", origin).await;
            // The data proves the read ran rather than failed: both members
            // read, never run, and the stub's tool applies to each.
            assert_eq!(v["coverage"]["members_read"], 2, "both members are read: {v}");
            for row in v["coverage"]["members"].as_array().unwrap() {
                assert_eq!(row["result"]["coverage"], "never_run", "{v}");
                assert_eq!(row["result"]["tools_applicable"], 1, "cargo-llvm-cov applies: {v}");
            }
        }
        assert_eq!(calls(&rec), 0, "the workspace coverage read starts no tool");

        // The control: a probing read asks the stub for its version.
        Engine::start(root.join("api")).expect("api engine").coverage_status().expect("a probing status");
        assert_eq!(calls(&rec), 1, "the stub is found, and a probing read asks it once");
    })
    .await
    .expect("the test ran past its bound");
}
