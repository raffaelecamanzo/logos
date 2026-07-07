//! Read-only-on-read fitness test for the dashboard views (S-082, CR-018,
//! [ADR-28], [FR-UI-03]).
//!
//! The headline CR-018 regression: loading `/health` + `/metrics` once each grew
//! `metric_snapshots` 7→10, and every Hotspots/Commits/Coverage load mined-and-
//! appended a `temporal_snapshots` row — a GET mutated a store. These tests drive
//! the **real** router in-process (`tower::ServiceExt::oneshot`, no socket) over a
//! started engine on an indexed + scanned + mined git fixture, then assert that
//! loading every view — once **and** repeatedly — leaves both the
//! `metric_snapshots` and `temporal_snapshots` row counts byte-for-byte
//! unchanged. This proves the handlers read through the read-only `latest_*`
//! accessors, not the evaluate-and-persist `scan`/`gate`/`hotspots` ones.
//!
//! The snapshot-count invariant is grammar-independent: `scan` records a
//! `metric_snapshots` row even on an empty (un-parsed) graph, and the temporal
//! mine is git-based — so this fitness test runs under a bare `cargo test -p web`
//! alongside the other carve-out tests, with no `lang-rust` gate (it must not be
//! silently skipped — it is the only HTTP-layer guard for the CR-018 regression).

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use logos_core::Engine;
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;
use tower::ServiceExt;

fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(out.status.success(), "git {args:?} failed");
}

fn commit(cwd: &Path, rel: &str, contents: &str, msg: &str) {
    let path = cwd.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    sh_git(cwd, &["add", rel]);
    sh_git(cwd, &["commit", "-q", "-m", msg]);
}

fn snapshot_count(db: &Path, table: &str) -> i64 {
    if !db.exists() {
        return 0;
    }
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open store read-only");
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .expect("count rows")
}

fn metric_count(repo: &Path) -> i64 {
    snapshot_count(&repo.join(".logos/logos.db"), "metric_snapshots")
}

fn temporal_count(repo: &Path) -> i64 {
    snapshot_count(&repo.join(".logos/history.db"), "temporal_snapshots")
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .unwrap()
}

/// Build an indexed + scanned + mined fixture and return its temp guard plus a
/// started engine. `scan` persists the metric snapshot and `hotspots` mines and
/// persists the temporal snapshot — the durable state a page view must reflect
/// **without** adding to.
fn scanned_engine() -> (TempDir, Arc<Engine>) {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(
        repo,
        "src/lib.rs",
        "pub fn f(x: i64) -> i64 { if x > 0 { x } else { -x } }\n",
        "fix: add f",
    );
    let engine = Arc::new(Engine::start(repo).expect("engine starts"));
    engine.index();
    engine.scan(false).expect("scan persists a metric snapshot");
    engine
        .hotspots(None, false)
        .expect("hotspots mines + persists a temporal snapshot");
    (tmp, engine)
}

/// Loading every dashboard view — once and repeatedly — adds **no**
/// `metric_snapshots` or `temporal_snapshots` row: a GET reflects the last
/// persisted snapshot/mine and never writes ([FR-UI-03] AC, [ADR-28]).
#[tokio::test]
async fn loading_every_view_repeatedly_writes_no_snapshot() {
    let (tmp, engine) = scanned_engine();
    let repo = tmp.path().to_path_buf();

    let m0 = metric_count(&repo);
    let t0 = temporal_count(&repo);
    assert_eq!(m0, 1, "the fixture scan wrote exactly one metric snapshot");
    assert!(t0 >= 1, "the fixture mine wrote a temporal snapshot");

    let router = web::router(engine);
    // The CR-049 decommission (S-192) removed the server-rendered views; the SPA
    // now reads every dashboard through the `/api/v1/*` read-models, which compose
    // the SAME read-only accessors the views used (`latest_gate`/`latest_scan`/
    // `coverage_status`/`test_gaps`/`latest_hotspots`/`coverage_cross`/`search`/
    // `node`/`impact`/`graph_elements`/`config_read`/`wiki_read`). The CR-018/ADR-28
    // write-free-on-read guard follows them to the surviving routes: the static SPA
    // host (`/`, which touches no store) plus the full `/api/v1/*` read suite —
    // including `/api/v1/search?q=f` (FTS), `/api/v1/node?symbol=f` and
    // `/api/v1/impact?seed=f` (the folded node-detail path). Each must add no
    // metric/temporal snapshot row on read ([ADR-28], [FR-UI-03]).
    for path in [
        "/", "/api/v1/overview", "/api/v1/health", "/api/v1/architecture", "/api/v1/files",
        "/api/v1/coverage", "/api/v1/quadrant", "/api/v1/config", "/api/v1/graph",
        "/api/v1/search?q=f", "/api/v1/node?symbol=f", "/api/v1/impact?seed=f",
    ] {
        for _ in 0..2 {
            let resp = router
                .clone()
                .oneshot(get(path))
                .await
                .expect("route responds");
            assert_eq!(resp.status(), StatusCode::OK, "{path} answers 200");
        }
    }

    assert_eq!(
        metric_count(&repo),
        m0,
        "no GET added a metric_snapshots row (CR-018 regression guard)"
    );
    assert_eq!(
        temporal_count(&repo),
        t0,
        "no GET added a temporal_snapshots row (CR-018 regression guard)"
    );
}

/// The Dashboard's Project Overview widget (CR-034, S-135, [FR-UI-09], [ADR-28])
/// reads the `overview/project-overview` agent wiki page through the read-only
/// `wiki_read` accessor seam — the handler fetches it on **every** `/overview`
/// load, before the view composes. With a page present, loading `/overview` once
/// and repeatedly must leave the wiki store logically unchanged — no page added,
/// none pruned-on-read — alongside the metric/temporal snapshot invariant. This
/// is the HTTP-layer guard for the widget's write-free-on-read AC; the rendered
/// snippet + `/wiki` link are covered by the pure-render view tests and the
/// coordinator's live Playwright pass (this bare `-p web` fixture loads no
/// grammar, so the dashboard takes its un-indexed branch).
#[tokio::test]
async fn loading_dashboard_with_a_wiki_page_writes_no_wiki_or_snapshot_row() {
    let (tmp, engine) = scanned_engine();
    let repo = tmp.path().to_path_buf();

    // A zero-anchor overview page, as the embedded skill writes it.
    engine
        .wiki_write(
            "overview/project-overview",
            "Project Overview",
            "# Project Overview\n\nLogos is a local structural code-intelligence server.\n",
            &[],
            "logos-wiki",
        )
        .expect("write the overview page");

    let pages0 = engine.wiki_status().expect("wiki status").page_count;
    let pruned0 = engine.wiki_pruned_log().expect("pruned log").len();
    let m0 = metric_count(&repo);
    let t0 = temporal_count(&repo);
    assert_eq!(pages0, 1, "the fixture wrote exactly one wiki page");

    let router = web::router(Arc::clone(&engine));
    for _ in 0..2 {
        let resp = router.clone().oneshot(get("/api/v1/overview")).await.expect("route responds");
        assert_eq!(resp.status(), StatusCode::OK, "/api/v1/overview answers 200");
    }

    // The wiki store is logically unchanged: no page added, none pruned on read.
    assert_eq!(
        engine.wiki_status().expect("wiki status").page_count,
        pages0,
        "no GET added or removed a wiki page (write-free on read, ADR-28)"
    );
    assert_eq!(
        engine.wiki_pruned_log().expect("pruned log").len(),
        pruned0,
        "no GET pruned a wiki page on read"
    );
    // And the snapshot invariant still holds.
    assert_eq!(metric_count(&repo), m0, "no GET added a metric_snapshots row");
    assert_eq!(temporal_count(&repo), t0, "no GET added a temporal_snapshots row");
}

/// The complementary write-free guard for the **absent** Project Overview page
/// (CR-034, S-135, [NFR-CC-04], [ADR-28]): with no wiki page written, loading `/`
/// once and repeatedly must not seed, fabricate, or otherwise write one — the
/// widget renders its honest empty state from `Ok(None)` without a store write.
#[tokio::test]
async fn loading_dashboard_without_a_wiki_page_writes_no_wiki_row() {
    let (tmp, engine) = scanned_engine();
    let repo = tmp.path().to_path_buf();

    // No wiki_write — the overview page is absent.
    let pages0 = engine.wiki_status().expect("wiki status").page_count;
    let pruned0 = engine.wiki_pruned_log().expect("pruned log").len();
    let m0 = metric_count(&repo);
    let t0 = temporal_count(&repo);
    assert_eq!(pages0, 0, "the fixture has no wiki page");

    let router = web::router(Arc::clone(&engine));
    for _ in 0..2 {
        let resp = router.clone().oneshot(get("/api/v1/overview")).await.expect("route responds");
        assert_eq!(resp.status(), StatusCode::OK, "/api/v1/overview answers 200 with the empty-state model");
    }

    assert_eq!(
        engine.wiki_status().expect("wiki status").page_count,
        pages0,
        "a missing overview page is never seeded on read (write-free, ADR-28)"
    );
    assert_eq!(engine.wiki_pruned_log().expect("pruned log").len(), pruned0, "no prune on read");
    assert_eq!(metric_count(&repo), m0, "no GET added a metric_snapshots row");
    assert_eq!(temporal_count(&repo), t0, "no GET added a temporal_snapshots row");
}
