//! The workspace manifest routes book their telemetry events (S-430 finding
//! A2-F2, Sprint 77 HF-1, [FR-OB-09], [ADR-67]).
//!
//! Drives the **real** workspace router in-process over a **real** telemetry
//! install — `observability::init(ProcessSurface::Web, <workspace-root>)` →
//! `GET /api/v1/workspace/manifest` + `POST /api/v1/workspace/manifest/save` →
//! `telemetry.db` — because the route reaching the traced seam is the claim. The
//! core unit test (`the_workspace_manifest_seam_emits_the_facade_config_events`)
//! proves the seam emits; only this proves the shipped routes call it.
//!
//! It also pins where the events land: the serve root's own store, which in a
//! workspace is `<workspace-root>/.logos/` — never a member's. Every file under
//! each member's `.logos/` keeps its size and mtime across the read and the save,
//! so the S-430 "no member moved" property survives the telemetry the routes now
//! emit.
//!
//! One test function, for the reason `wikigen_surface.rs` records: `init`
//! installs the *global* subscriber, so a second test in this binary would book
//! its calls into the same store and perturb the rows.
//!
//! [FR-OB-09]: ../../docs/specs/requirements/FR-OB-09.md
//! [ADR-67]: ../../docs/specs/architecture/decisions/ADR-67.md

use std::path::Path;
use std::process::Command;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use logos_core::federation::{discover, EngineRegistry};
use logos_core::observability::{self, ProcessSurface};
use logos_core::Engine;
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;
use tower::ServiceExt;
use web::{IntentToken, INTENT_HEADER};

const HOST: &str = "127.0.0.1:4983";
const MANIFEST: &str = "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"web\"]\ndefault = \"api\"\n";

fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
}

/// A committed, indexed git repo — `discover` keeps only members that are
/// distinct git roots, and an indexed store gives the stat comparison real
/// files to watch.
fn member(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    sh_git(dir, &["init", "-q", "-b", "main"]);
    sh_git(dir, &["add", "."]);
    sh_git(dir, &["commit", "-q", "-m", "init"]);
    Engine::start(dir).expect("engine starts").index();
}

/// Every file under `<root>/<member>/.logos/` with its size and mtime.
fn member_logos_stat(root: &Path, name: &str) -> Vec<(String, u64, std::time::SystemTime)> {
    let dir = root.join(name).join(".logos");
    let mut out = Vec::new();
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).expect("readable").flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let meta = std::fs::metadata(&p).unwrap();
                let rel = p.strip_prefix(&dir).unwrap().display().to_string();
                out.push((format!("{name}/.logos/{rel}"), meta.len(), meta.modified().unwrap()));
            }
        }
    }
    out.sort();
    out
}

/// `application/x-www-form-urlencoded` for one field.
fn form_field(name: &str, value: &str) -> String {
    let mut out = format!("{name}=");
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

async fn send(router: &axum::Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = router.clone().oneshot(req).await.expect("route responds");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

/// Every `(tool, ok, surface)` row the store holds, as the emission path wrote it.
fn rows(db: &Path) -> Vec<(String, bool, String)> {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open telemetry.db read-only");
    let mut stmt = conn.prepare("SELECT tool, ok, surface FROM events ORDER BY id").expect("prepare");
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?, r.get::<_, String>(2)?)))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows");
    rows
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_manifest_routes_book_config_events_in_the_serve_roots_store_and_move_no_member() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    member(&root.join("api"));
    member(&root.join("web"));
    std::fs::write(root.join("logos.workspace.toml"), MANIFEST).unwrap();
    // The serve root's own `.logos/` — telemetry activates only where it exists.
    std::fs::create_dir_all(root.join(".logos")).unwrap();

    // Members indexed BEFORE telemetry is installed, so the store holds only the
    // routes' own rows.
    let guard = observability::init(ProcessSurface::Web, root);
    let federation = discover(root).expect("discovery succeeds").expect("a workspace");
    let registry = EngineRegistry::<Engine>::new_serve_default(federation);
    let intent = IntentToken::generate();
    let router = web::workspace_router_with_intent(registry, intent.clone()).expect("router builds");

    let stat_before = [member_logos_stat(root, "api"), member_logos_stat(root, "web")];
    assert!(
        stat_before.iter().all(|files| files.iter().any(|(p, ..)| p.ends_with("logos.db"))),
        "both members were indexed, so the comparison watches real stores: {stat_before:?}"
    );

    let get = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/workspace/manifest")
        .header(header::HOST, HOST)
        .body(Body::empty())
        .unwrap();
    let (status, body) = send(&router, get).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let loaded: serde_json::Value = serde_json::from_str(&body).unwrap();
    let fingerprint = loaded["fingerprint"].as_str().expect("a fingerprint").to_string();

    let candidate = format!("{MANIFEST}# edited from the workspace Config view\n");
    let post = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/workspace/manifest/save")
        .header(header::HOST, HOST)
        .header(header::ORIGIN, format!("http://{HOST}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(INTENT_HEADER, intent.as_str())
        .body(Body::from(format!(
            "{}&{}",
            form_field("content", &candidate),
            form_field("fingerprint", &fingerprint)
        )))
        .unwrap();
    let (status, body) = send(&router, post).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"outcome\":\"written\""), "{body}");

    // Flush the last telemetry batch exactly as a process exit would.
    drop(guard);

    let db = root.join(".logos/telemetry.db");
    assert!(db.is_file(), "the serve root's store holds the events");
    let manifest_rows: Vec<_> = rows(&db)
        .into_iter()
        .filter(|(tool, ..)| tool.starts_with("config_"))
        .collect();
    assert_eq!(
        manifest_rows,
        [
            ("config_read".to_string(), true, "web".to_string()),
            ("config_write".to_string(), true, "web".to_string()),
        ],
        "one config_read for the manifest read and one config_write for its save, \
         under the route's surface — as the S-450 workspace config routes book theirs"
    );

    assert_eq!(
        [member_logos_stat(root, "api"), member_logos_stat(root, "web")],
        stat_before,
        "no member's .logos/ moved: the events land in the serve root's store, never a member's"
    );
    for name in ["api", "web"] {
        assert!(
            !root.join(name).join(".logos/telemetry.db").exists(),
            "no telemetry store was created in member {name}"
        );
    }
}
