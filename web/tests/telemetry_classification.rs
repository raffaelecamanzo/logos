//! The app shell's own furniture is not somebody using the tool (S-316,
//! [CR-097], [FR-OB-09] as widened, [FR-UI-34], [BR-42]).
//!
//! Drives the **real** router in-process over a **real** telemetry install —
//! `observability::init` → `/api/v1` requests → `telemetry.db` →
//! `Engine::stats` — because that whole chain is the claim. The unit tests
//! prove the pieces (the surface is classified, the bridge enters the scope);
//! only this proves the shipped endpoint's reads actually land outside the
//! figures.
//!
//! It asserts the pair, never one half alone: six navigations contribute
//! **nothing** to the usage figures, while a genuine graph query issued over
//! the same surface in the same session still **appears** in them. Either half
//! on its own is satisfied by a blanket filter — which is exactly the defect
//! the per-event classification replaced ([CR-091]) — and the acceptance
//! criterion names both for that reason.
//!
//! The assertion runs against the **classification path**, not the retired
//! `surface <> 'web'` filter: the graph query below is stamped `web` and is
//! required to be counted, so a filter that excluded the surface wholesale
//! would fail this test rather than pass it.
//!
//! Everything lives in **one** test function: `init` installs the *global*
//! subscriber, so a second parallel test in this binary would record its own
//! calls into the same store and perturb the counts.
//!
//! [CR-091]: ../../docs/requests/CR-091-telemetry-surface-classification-and-usage-attribution.md
//! [CR-097]: ../../docs/requests/CR-097-header-graph-state-readout.md

use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;
use tower::ServiceExt;

use logos_core::observability::{self, ProcessSurface};
use logos_core::Engine;

/// How many client-side navigations the fixture performs. The header re-reads
/// the graph state on every one of them ([FR-UI-34]) — six is the acceptance
/// criterion's own figure ("navigating between six views").
const NAVIGATIONS: usize = 6;

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .unwrap()
}

fn count_events(db: &Path, where_clause: &str) -> i64 {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open telemetry.db read-only");
    conn.query_row(
        &format!("SELECT count(*) FROM events WHERE {where_clause}"),
        [],
        |r| r.get(0),
    )
    .expect("count rows")
}

#[tokio::test]
async fn the_shells_status_reads_never_enter_the_usage_figures() {
    let dir = TempDir::new().expect("temp root");
    let root = dir.path();
    fs::create_dir_all(root.join(".logos")).expect("pre-create .logos");
    fs::write(root.join("lib.rs"), "pub fn f() {}\n").expect("write fixture");

    let engine = Arc::new(Engine::start(root).expect("engine starts"));
    // The adapter wiring, exactly as `logos serve --ui` installs it.
    let guard = observability::init(ProcessSurface::Web, root);
    let router = web::router(Arc::clone(&engine));

    // A graph query the user issued through the SPA — real tool use.
    let resp = router
        .clone()
        .oneshot(get("/api/v1/search?q=f"))
        .await
        .expect("route responds");
    assert_eq!(resp.status(), StatusCode::OK, "the search endpoint answers");

    // …and the shell's own furniture, once per navigation.
    for _ in 0..NAVIGATIONS {
        let resp = router
            .clone()
            .oneshot(get("/api/v1/status"))
            .await
            .expect("route responds");
        assert_eq!(resp.status(), StatusCode::OK, "the status endpoint answers");
    }

    // Flush the last telemetry batch exactly as a process exit would.
    drop(guard);

    let telemetry_db = root.join(".logos").join("telemetry.db");
    assert!(telemetry_db.is_file(), "telemetry.db created ([FR-OB-03])");

    // ── One chrome-issued read of a tool the TOOL arm counts ────────────────
    //
    // Without this the test cannot tell the two classification arms apart. The
    // only chrome route that exists calls `status`, which is independently
    // excluded as a *tool* — so every assertion below would hold just as well
    // with the surface arm switched off entirely, and a review proved exactly
    // that by running the mutation. Seeding a `shell`-surface `search` — a wire
    // name the tool arm counts — into the store the real emission path just
    // wrote makes the exclusion below attributable to the surface arm and to
    // nothing else, while the test stays end-to-end.
    //
    // It is written directly because no route can produce it yet, which is the
    // point: the classification must be right *before* the second chrome read
    // exists, not after it has already been miscounted.
    {
        let conn = Connection::open(&telemetry_db).expect("open telemetry.db read-write");
        conn.execute(
            "INSERT INTO events (at, surface, tool, duration_ms, ok, origin, session_id)
             VALUES (strftime('%s','now'), 'shell', 'search', 5, 1, 'main', 'probe')",
            [],
        )
        .expect("seed a chrome-issued search");
    }

    // ── Nothing was destroyed: the reads are in the raw store ([NFR-CC-04]) ──
    assert_eq!(
        count_events(&telemetry_db, "surface = 'shell' AND tool = 'status'"),
        NAVIGATIONS as i64,
        "every navigation's readout is recorded, attributed to the shell"
    );
    assert_eq!(
        count_events(&telemetry_db, "surface = 'web' AND tool = 'search'"),
        1,
        "and so is the user's own query, attributed to the web surface"
    );

    // ── …only attributed out of the figures ([FR-OB-09], [FR-UI-27]) ────────
    let stats = Engine::open(root).stats(None);
    let counted: Vec<(&str, &str)> = stats
        .calls_by_tool
        .iter()
        .map(|u| (u.surface.as_str(), u.tool.as_str()))
        .collect();
    assert!(
        counted.contains(&("web", "search")),
        "a genuine SPA graph query is real tool use and still appears: {counted:?}"
    );
    assert!(
        !counted.iter().any(|(surface, _)| *surface == "shell"),
        "no shell-chrome read reaches a usage figure: {counted:?}"
    );
    assert!(
        !counted.iter().any(|(_, tool)| *tool == "status"),
        "and the status read counts on no surface at all: {counted:?}"
    );
    // The surface arm, isolated: `search` is counted by the tool arm, so the
    // only thing that can keep the chrome-issued one out of this figure is the
    // classification on `Surface::Shell`.
    assert_eq!(
        stats
            .calls_by_tool
            .iter()
            .filter(|u| u.tool == "search")
            .map(|u| u.calls)
            .sum::<u64>(),
        1,
        "the chrome-issued search is excluded by the SURFACE arm, \
         leaving only the user's own: {counted:?}"
    );

    // The daily series and the origin split each carry their own copy of the
    // predicate, so the exclusion is asserted where it could independently leak
    // rather than only in the headline total.
    let day_calls: u64 = stats.activity_by_day.iter().map(|d| d.calls).sum();
    let counted_calls: u64 = stats.calls_by_tool.iter().map(|u| u.calls).sum();
    assert_eq!(
        day_calls, counted_calls,
        "the daily-activity series applies the same rule as the usage counts"
    );
    let by_origin: u64 = stats.calls_by_origin.iter().map(|o| o.calls).sum();
    assert_eq!(
        by_origin, counted_calls,
        "and so does the origin breakdown"
    );
}
