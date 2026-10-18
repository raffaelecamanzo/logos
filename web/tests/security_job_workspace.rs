//! The Run scan job is per member (S-673, [FR-SC-15]): in a workspace, `?repo=`
//! scopes the start, the read and the cancel, so one member's scan is neither
//! seen nor stopped through another.
//!
//! A workspace builds each member's engine itself, from the process
//! environment, so this binary holds exactly one test and points that
//! environment at stubs before anything starts: `PATH` holds only a stub
//! scanner and `git`, and the user-level config and tools directories are
//! temporary. No scanner of this machine is found, and no other test shares
//! the process whose environment is changed. Bounded by [`BOUND`].
//!
//! [FR-SC-15]: ../../docs/specs/requirements/FR-SC-15.md
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use logos_core::federation::{discover, EngineRegistry};
use logos_core::Engine;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;
use web::{IntentToken, INTENT_HEADER};

const BOUND: Duration = Duration::from_secs(120);
const ORIGIN: &str = "http://127.0.0.1:4983";

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

async fn send(router: &Router, method: Method, path: &str, intent: &IntentToken, body: &str) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method.clone()).uri(path).header(header::HOST, "127.0.0.1:4983");
    if method == Method::POST {
        req = req.header(header::ORIGIN, ORIGIN).header(INTENT_HEADER, intent.as_str());
    }
    let resp = router.clone().oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn each_member_starts_follows_and_cancels_only_its_own_scan() {
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

        // The only scanner reachable is a Semgrep stub that leaves a grandchild
        // sleeping until it is killed; `git` stays reachable for the engines.
        let real_git = String::from_utf8(Command::new("sh").args(["-c", "command -v git"]).output().unwrap().stdout).unwrap();
        std::os::unix::fs::symlink(real_git.trim(), bin.join("git")).unwrap();
        let stub = bin.join("semgrep");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\ncase \"$1\" in --version) printf '%s\\n' 'semgrep 1.2.3'; exit 0;; esac\n/bin/sleep 60 & echo $$ $! > '{}/pids'\nwait\n",
                rec.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("PATH", &bin);
        std::env::set_var(logos_core::config::USER_CONFIG_HOME_ENV, &user);
        std::env::set_var(logos_core::config::USER_DATA_HOME_ENV, &data);

        let federation = discover(&root).expect("discovery succeeds").expect("a workspace");
        let intent = IntentToken::generate();
        let router = web::workspace_router_with_intent(EngineRegistry::<Engine>::new_serve_default(federation), intent.clone())
            .expect("the workspace router builds");

        let (status, started) = send(&router, Method::POST, "/api/v1/security/scan?repo=web", &intent, r#"{"consent":["semgrep"]}"#).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let deadline = Instant::now() + Duration::from_secs(30);
        while !std::fs::read_to_string(rec.join("pids")).is_ok_and(|s| s.ends_with('\n')) {
            assert!(Instant::now() < deadline, "the web member's scan never started");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // The other member has no scan: nothing to read, nothing to cancel.
        assert_eq!(send(&router, Method::GET, "/api/v1/security/scan?repo=api", &intent, "").await, (StatusCode::OK, json!({ "job": null })));
        assert_eq!(send(&router, Method::POST, "/api/v1/security/scan/cancel?repo=api", &intent, "").await, (StatusCode::OK, json!({ "job": null })));
        let (_, web_job) = send(&router, Method::GET, "/api/v1/security/scan?repo=web", &intent, "").await;
        assert_eq!((web_job["job"]["state"].as_str(), web_job["job"]["id"].clone()), (Some("running"), started["job"]["id"].clone()), "{web_job}");

        let (status, cancelling) = send(&router, Method::POST, "/api/v1/security/scan/cancel?repo=web", &intent, "").await;
        assert_eq!((status, cancelling["job"]["id"].clone()), (StatusCode::OK, started["job"]["id"].clone()), "{cancelling}");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let (_, job) = send(&router, Method::GET, "/api/v1/security/scan?repo=web", &intent, "").await;
            if job["job"]["state"] != "running" {
                assert_eq!(job["job"]["state"], "cancelled", "{job}");
                break;
            }
            assert!(Instant::now() < deadline, "the web member's scan is still running");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the test ran past its bound");
}
