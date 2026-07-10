//! The same-origin `/api/v1/*` JSON read-model API (S-183 / [FR-UI-21], [ADR-43],
//! [FR-UI-03], [NFR-RA-05], [NFR-SE-07], [UAT-UI-02]).
//!
//! Drives the **real** router in-process (`tower::ServiceExt::oneshot`, no socket)
//! over a started engine, asserting the suite's contract end-to-end:
//!
//! - every `/api/v1` read endpoint answers `200 application/json` under the
//!   byte-identical self-only CSP, its body the serialized `Engine` read-model
//!   ([FR-UI-21]);
//! - the surface stays GET-only — a `POST` to a `/api/v1` route is `405` before
//!   any handler runs ([UAT-UI-02]);
//! - loading every `/api/v1` endpoint once **and** repeatedly leaves the
//!   `metric_snapshots` and `temporal_snapshots` counts unchanged (no
//!   write-on-read, [FR-UI-03], [ADR-28]);
//! - the config-read endpoint returns the **masked** chat key only — never the raw
//!   secret ([FR-CF-06], [NFR-SE-07]);
//! - a `node`/`search` call without its required query param is a `400`, and an
//!   unknown wiki slug is an honest `404`.
//!
//! The snapshot-count and contract invariants are grammar-independent, so the bulk
//! of the suite runs under a bare `cargo test -p web` (no `lang-rust` gate); the
//! one populated-graph assertion is `#[cfg(feature = "lang-rust")]`.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use logos_core::Engine;
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;
use tower::ServiceExt;

// ── Fixtures & helpers ────────────────────────────────────────────────────────

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

/// An indexed + scanned + mined fixture — the durable state the read-only `/api/v1`
/// endpoints reflect without adding to. `scan` persists the metric snapshot even on
/// an un-parsed graph and `hotspots` mines the temporal snapshot, so the composite
/// endpoints (`/api/v1/health`, `/files`, …) succeed under a bare `-p web` run.
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
    engine.hotspots(None, false, false).expect("hotspots mines a temporal snapshot");
    (tmp, engine)
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .unwrap()
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

/// The full `/api/v1` read endpoint set — the routes the SPA shell will fetch.
const V1_ENDPOINTS: &[&str] = &[
    "/api/v1/overview",
    "/api/v1/health",
    "/api/v1/architecture",
    "/api/v1/gaps",
    "/api/v1/files",
    "/api/v1/coverage",
    "/api/v1/graph",
    "/api/v1/query?q=f",
    "/api/v1/impact?seed=f",
    "/api/v1/node?symbol=f",
    "/api/v1/search?q=f",
    "/api/v1/wiki",
    "/api/v1/wiki/nav",
    "/api/v1/wiki/search?q=f",
    "/api/v1/config",
    "/api/v1/statistics",
];

async fn body_string(resp: Response<Body>) -> (StatusCode, String, axum::http::HeaderMap) {
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap(), headers)
}

/// Like [`body_string`] but keeps the body as raw bytes — for the doc-asset route,
/// whose image payloads are not UTF-8.
async fn body_bytes(resp: Response<Body>) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, bytes.to_vec(), headers)
}

type Response<T> = axum::http::Response<T>;

/// The invariant every `/api/v1` read answer must satisfy: a JSON content-type
/// under the byte-identical self-only CSP ([FR-UI-21], [NFR-SE-06]).
fn assert_json_self_only_csp(headers: &axum::http::HeaderMap, path: &str) {
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/json",
        "{path} serves JSON",
    );
    let csp = headers
        .get(header::CONTENT_SECURITY_POLICY)
        .expect("every response carries a CSP")
        .to_str()
        .unwrap();
    assert!(csp.contains("default-src 'self'"), "{path} self-only CSP: {csp}");
    assert!(!csp.contains('*'), "{path} CSP allows no wildcard: {csp}");
}

// ── Contract: 200 JSON + self-only CSP on every endpoint ──────────────────────

/// Every `/api/v1` read endpoint answers `200 application/json` under the
/// byte-identical self-only CSP — the seam the SPA fetches ([FR-UI-21], [FR-UI-03]).
#[tokio::test]
async fn every_v1_endpoint_serves_json_under_self_only_csp() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);

    for path in V1_ENDPOINTS {
        let resp = router.clone().oneshot(get(path)).await.expect("route responds");
        let (status, _body, headers) = body_string(resp).await;
        assert_eq!(status, StatusCode::OK, "{path} answers 200");
        assert_json_self_only_csp(&headers, path);
    }
}

/// The composed bundles trace their fields to `Engine` read-models: spot-check the
/// keys the SPA reads ([FR-UI-21], [NFR-RA-05]).
#[tokio::test]
async fn composite_bundles_serialize_their_read_model_fields() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);

    let cases: &[(&str, &[&str])] = &[
        ("/api/v1/overview", &["\"status\"", "\"gate\"", "\"coverage\"", "\"rules\""]),
        ("/api/v1/health", &["\"status\"", "\"gate\"", "\"scan\"", "\"evolution\""]),
        ("/api/v1/architecture", &["\"status\"", "\"dsm\""]),
        ("/api/v1/gaps", &["\"status\"", "\"rules\""]),
        ("/api/v1/files", &["\"status\"", "\"hotspots\"", "\"temporal\""]),
        ("/api/v1/coverage", &["\"status\"", "\"coverage\"", "\"untested\""]),
        ("/api/v1/graph", &["\"nodes\"", "\"edges\"", "\"total_nodes\""]),
    ];
    for (path, keys) in cases {
        let resp = router.clone().oneshot(get(path)).await.unwrap();
        let (status, body, _h) = body_string(resp).await;
        assert_eq!(status, StatusCode::OK, "{path} answers 200");
        for key in *keys {
            assert!(body.contains(key), "{path} body carries {key}: {body}");
        }
    }

    // Contract-narrowing guard (CR-079): the Quadrant/test-gaps keys are GONE.
    // A regression re-adding them to the composed bundles must be caught here, not
    // just left uncovered by the presence-only checks above.
    let overview = router.clone().oneshot(get("/api/v1/overview")).await.unwrap();
    let (_s, overview_body, _h) = body_string(overview).await;
    for gone in ["\"cross\"", "\"hotspots\"", "\"gaps\""] {
        assert!(!overview_body.contains(gone), "overview no longer carries {gone}: {overview_body}");
    }
    let gaps = router.clone().oneshot(get("/api/v1/gaps")).await.unwrap();
    let (_s, gaps_body, _h) = body_string(gaps).await;
    assert!(!gaps_body.contains("\"test_gaps\""), "gaps no longer carries test_gaps: {gaps_body}");
}

/// CR-079: the `/api/v1/quadrant` route is unrouted — a GET resolves to `404`,
/// never a `200` bundle. Guards against the route being silently re-added.
#[tokio::test]
async fn quadrant_endpoint_is_unrouted() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);
    let resp = router.oneshot(get("/api/v1/quadrant")).await.expect("route responds");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "/api/v1/quadrant is unrouted (CR-079)");
}

/// The single-read endpoints serialize their read-model's own fields, not just an
/// opaque `200` — spot-check the keys the SPA reads off `query`/`node`/`wiki`/
/// `wiki/search` so a reshaped read-model can't silently pass the contract loop
/// ([FR-UI-21], [NFR-RA-05]).
#[tokio::test]
async fn single_read_endpoints_serialize_their_read_model_fields() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);

    let cases: &[(&str, &[&str])] = &[
        // QueryResponse — the interpreted mode + the ranked-hit channel.
        ("/api/v1/query?q=f", &["\"mode\"", "\"hits\"", "\"total\""]),
        // NodeInfo — the echoed query + the ADR-14 degradation channel.
        ("/api/v1/node?symbol=f", &["\"query\"", "\"warnings\"", "\"suggestions\""]),
        // WikiStatus — the dual-axis freshness read-model.
        ("/api/v1/wiki", &["\"page_count\"", "\"freshness_fraction\"", "\"current_revision\""]),
    ];
    for (path, keys) in cases {
        let resp = router.clone().oneshot(get(path)).await.unwrap();
        let (status, body, headers) = body_string(resp).await;
        assert_eq!(status, StatusCode::OK, "{path} answers 200");
        assert_json_self_only_csp(&headers, path);
        for key in *keys {
            assert!(body.contains(key), "{path} body carries {key}: {body}");
        }
    }

    // `wiki/search` returns a JSON array of hits — its shape is the array itself.
    let resp = router.oneshot(get("/api/v1/wiki/search?q=f")).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "wiki/search answers 200");
    assert_json_self_only_csp(&headers, "/api/v1/wiki/search?q=f");
    assert!(body.starts_with('['), "wiki/search serializes a JSON array: {body}");
}

/// The Decisions-panel impact endpoint (S-186, [FR-NV-10], [FR-DG-02]) serializes
/// the `ImpactResult` read-model — the resolved node, the labeled upstream/
/// downstream sets, and the `docs` trace channel the SPA's Decisions panel reads —
/// under the byte-identical self-only CSP. An absent/empty `seed` is the honest
/// empty default (`200`, `resolved` null), never an error ([NFR-CC-04]).
#[tokio::test]
async fn impact_endpoint_serializes_the_read_model_and_defaults_empty() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);

    // A seeded read carries the full read-model shape.
    let resp = router.clone().oneshot(get("/api/v1/impact?seed=f")).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "impact answers 200");
    assert_json_self_only_csp(&headers, "/api/v1/impact?seed=f");
    for key in ["\"resolved\"", "\"upstream\"", "\"downstream\"", "\"docs\"", "\"docs_label\""] {
        assert!(body.contains(key), "impact body carries {key}: {body}");
    }

    // No seed → the honest empty default (resolved null), not a 4xx/5xx.
    let resp = router.oneshot(get("/api/v1/impact")).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "seedless impact is an honest 200");
    assert_json_self_only_csp(&headers, "/api/v1/impact");
    assert!(body.contains("\"resolved\":null"), "seedless impact resolves to nothing: {body}");
}

/// `GET /api/v1/statistics` serializes the enriched telemetry read-model (S-234,
/// [FR-OB-04], [FR-UI-27]) — the usage/latency/value fields plus the S-233 daily
/// activity series and dev-vs-`main` origin split — and `?window=` scopes the
/// trailing window (default 7). A thin `Engine::stats(window)` pass-through; an empty
/// store degrades to an honest zeroed model, never an error ([NFR-CC-04]).
#[tokio::test]
async fn statistics_endpoint_serializes_the_enriched_read_model_and_honors_window() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);

    // The enriched read-model shape the Statistics tab reads — the S-233 additions
    // (`activity_by_day`, `calls_by_origin`) alongside the pre-existing fields.
    let resp = router.clone().oneshot(get("/api/v1/statistics")).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "statistics answers 200");
    assert_json_self_only_csp(&headers, "/api/v1/statistics");
    for key in [
        "\"window_days\"",
        "\"calls_total\"",
        "\"calls_by_tool\"",
        "\"tokens_saved_estimate\"",
        "\"activity_by_day\"",
        "\"calls_by_origin\"",
    ] {
        assert!(body.contains(key), "statistics carries {key}: {body}");
    }
    // The default window is 7 ([FR-OB-04]).
    assert!(body.contains("\"window_days\":7"), "the default window is 7: {body}");

    // `?window=30`/`90` scope the result — the window is echoed in the read-model.
    for days in [30_u32, 90] {
        let path = format!("/api/v1/statistics?window={days}");
        let resp = router.clone().oneshot(get(&path)).await.unwrap();
        let (status, body, headers) = body_string(resp).await;
        assert_eq!(status, StatusCode::OK, "{path} answers 200");
        assert_json_self_only_csp(&headers, &path);
        assert!(
            body.contains(&format!("\"window_days\":{days}")),
            "{path} scopes the window: {body}",
        );
    }

    // An unparseable window falls back to the core default (7), not a 4xx — the
    // lenient query contract the other endpoints share.
    let resp = router.oneshot(get("/api/v1/statistics?window=notanumber")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "an unparseable window is an honest 200");
    assert!(body.contains("\"window_days\":7"), "an unparseable window defaults to 7: {body}");
}

// ── Contract: GET-only ────────────────────────────────────────────────────────

/// A `POST` to a `/api/v1` route is `405` before any handler runs — the surface
/// stays read-only except the enumerated mutating routes ([UAT-UI-02], [FR-UI-03]).
#[tokio::test]
async fn post_to_a_v1_route_is_405() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/overview")
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED, "the v1 surface is GET-only");
}

// ── Acceptance: no write-on-read ──────────────────────────────────────────────

/// Loading every `/api/v1` endpoint once **and** repeatedly adds no
/// `metric_snapshots` or `temporal_snapshots` row — a GET reflects the last
/// persisted snapshot and never writes ([FR-UI-21] AC, [FR-UI-03], [ADR-28]).
#[tokio::test]
async fn loading_every_v1_endpoint_repeatedly_writes_no_snapshot() {
    let (tmp, engine) = scanned_engine();
    let repo = tmp.path().to_path_buf();

    let m0 = metric_count(&repo);
    let t0 = temporal_count(&repo);
    assert_eq!(m0, 1, "the fixture scan wrote exactly one metric snapshot");
    assert!(t0 >= 1, "the fixture mine wrote a temporal snapshot");

    let router = web::router(engine);
    for path in V1_ENDPOINTS {
        for _ in 0..2 {
            let resp = router.clone().oneshot(get(path)).await.expect("route responds");
            assert_eq!(resp.status(), StatusCode::OK, "{path} answers 200");
        }
    }

    assert_eq!(metric_count(&repo), m0, "no GET added a metric_snapshots row ([ADR-28])");
    assert_eq!(temporal_count(&repo), t0, "no GET added a temporal_snapshots row ([ADR-28])");
}

// ── Acceptance: config-read returns the masked key only ───────────────────────

/// `GET /api/v1/config` returns the chat key **masked** (presence + last-4) and
/// never echoes the raw secret ([FR-CF-06], [NFR-SE-07]). The raw key is written
/// to the gitignored `secrets.toml`; the read-model masks it by construction.
#[tokio::test]
async fn config_endpoint_returns_the_masked_key_never_the_raw_secret() {
    let (_tmp, engine) = scanned_engine();
    // The raw secret the editor would POST — it must never come back out a GET.
    let raw = "sk-logos-supersecret-tail9999";
    engine.config_write_secret(raw).expect("write the chat key");

    let router = web::router(Arc::clone(&engine));
    let resp = router.oneshot(get("/api/v1/config")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "config-read answers 200");

    // The masked projection: present + last-4; the raw secret appears nowhere.
    assert!(body.contains("\"present\":true"), "the masked key reports presence: {body}");
    assert!(body.contains("\"last4\":\"9999\""), "the masked key exposes only its last-4: {body}");
    assert!(!body.contains(raw), "the raw secret is never serialized: {body}");
    assert!(!body.contains("supersecret"), "no fragment of the raw secret leaks: {body}");
}

// ── Acceptance: the CR-067/BR-37 `defaults` projection ────────────────────────

/// `GET /api/v1/config` serializes the code-sourced `defaults` projection
/// (CR-067, FR-UI-12, BR-37): `config.toml` defaults from `Config::default()`,
/// `[metric_thresholds]` defaults keyed by the rules.toml key names, and
/// `[constraints]` recommended baselines from `Constraints::recommended()`.
#[tokio::test]
async fn config_endpoint_serializes_the_defaults_projection() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);

    let resp = router.oneshot(get("/api/v1/config")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "config-read answers 200");

    assert!(body.contains("\"defaults\""), "the defaults projection is present: {body}");
    // config.toml default (Config::default().max_file_size == 2 MiB).
    assert!(
        body.contains("\"max_file_size\":2097152"),
        "config defaults carry the real max_file_size default: {body}"
    );
    // [metric_thresholds] defaults, keyed by the rules.toml names, equal
    // Thresholds::default() (FR-QM-14).
    assert!(
        body.contains("\"nesting_depth\":4") && body.contains("\"brain_lines\":100"),
        "metric_thresholds defaults are keyed by rules.toml names: {body}"
    );
    // [constraints] recommended baseline (CR-067 CRA-01, e.g. max_fan_in -> 30).
    assert!(
        body.contains("\"max_fan_in\":30"),
        "constraints carry the recommended baseline: {body}"
    );
}

// ── Contract: required params and honest 404 ──────────────────────────────────

/// `node`/`search` without their required query param are a `400`; with it, `200`.
#[tokio::test]
async fn node_and_search_require_their_query_param() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);

    for bad in ["/api/v1/node", "/api/v1/node?symbol=", "/api/v1/search", "/api/v1/search?q="] {
        let resp = router.clone().oneshot(get(bad)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{bad} is a 400 (missing required param)");
    }
    for ok in ["/api/v1/node?symbol=anything", "/api/v1/search?q=anything"] {
        let resp = router.clone().oneshot(get(ok)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{ok} answers 200 (read-model handles the miss honestly)");
    }
}

/// `wiki/search` with an empty (or absent) `q` is an honest empty list (`200`
/// `[]`), never an error — the contract the SPA's empty search box relies on
/// ([FR-WK-05]).
#[tokio::test]
async fn wiki_search_empty_query_is_an_honest_empty_list() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);
    for path in ["/api/v1/wiki/search", "/api/v1/wiki/search?q="] {
        let resp = router.clone().oneshot(get(path)).await.unwrap();
        let (status, body, headers) = body_string(resp).await;
        assert_eq!(status, StatusCode::OK, "{path} answers 200");
        assert_json_self_only_csp(&headers, path);
        assert_eq!(body.trim(), "[]", "{path} is an honest empty list: {body}");
    }
}

/// `/api/v1/files?untested` exercises the untested-only board branch
/// (`wants_untested` ⇒ `latest_hotspots(.., true)`), distinct from the bare
/// `/files` board — both serve the same read-model contract ([FR-UI-21]).
#[tokio::test]
async fn files_untested_query_param_serves_the_untested_board() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);
    let resp = router.oneshot(get("/api/v1/files?untested")).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "files?untested answers 200");
    assert_json_self_only_csp(&headers, "/api/v1/files?untested");
    for key in ["\"status\"", "\"hotspots\"", "\"temporal\""] {
        assert!(body.contains(key), "files?untested carries {key}: {body}");
    }
}

/// `/api/v1/files?production_scope` exercises the optional production-scope
/// board branch (CR-076: `wants_flag(.., "production_scope")` ⇒
/// `latest_hotspots(.., production_scope=true)`), reached through the same
/// [`Engine::latest_hotspots`] call as the bare `/files` board and the CLI
/// `--production-scope` flag / MCP `production_scope` argument ([FR-UI-05]).
#[tokio::test]
async fn files_production_scope_query_param_serves_the_filtered_board() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);
    let resp = router
        .oneshot(get("/api/v1/files?production_scope"))
        .await
        .unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "files?production_scope answers 200");
    assert_json_self_only_csp(&headers, "/api/v1/files?production_scope");
    for key in ["\"status\"", "\"hotspots\"", "\"temporal\"", "\"production_scope\":true"] {
        assert!(
            body.contains(key),
            "files?production_scope carries {key}: {body}"
        );
    }
}

/// An unknown wiki slug is an honest `404`, never a fabricated page ([NFR-CC-04]).
#[tokio::test]
async fn unknown_wiki_page_is_404() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);
    let resp = router.oneshot(get("/api/v1/wiki/page/no/such/slug")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "an absent wiki page is a 404");
}

/// A written wiki page is reachable at `/api/v1/wiki/page/*slug` and serializes its
/// presentation bundle ([FR-UI-21], S-189): the title, provenance, and the
/// **server-rendered, already-safe HTML body** the SPA mounts. A GFM pipe table is
/// rendered to a real `<table>` server-side (the comrak boundary), so the SPA never
/// ships a Markdown engine ([FR-UI-06] GFM-table).
#[tokio::test]
async fn written_wiki_page_is_served_with_server_rendered_safe_html() {
    let (_tmp, engine) = scanned_engine();
    engine
        .wiki_write(
            "overview/project-overview",
            "Project Overview",
            // A GFM pipe table + a prose line — both must render server-side.
            "# Project Overview\n\nLogos is a local structural code-intelligence server.\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n",
            &[],
            "logos-wiki",
        )
        .expect("write the overview page");
    let router = web::router(Arc::clone(&engine));
    let path = "/api/v1/wiki/page/overview/project-overview";
    let resp = router.oneshot(get(path)).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "the written page answers 200");
    assert_json_self_only_csp(&headers, path);
    assert!(body.contains("Project Overview"), "the page title is serialized: {body}");
    assert!(body.contains("\"generator\":\"logos-wiki\""), "provenance is serialized: {body}");
    assert!(body.contains("\"placeholder\":false"), "a written page is not a placeholder: {body}");
    // The HTML body is rendered server-side (comrak) — the SPA mounts it verbatim.
    assert!(body.contains("rendered_html"), "the rendered HTML body is serialized: {body}");
    assert!(
        body.contains("\\u003ctable\\u003e") || body.contains("<table>"),
        "a GFM table renders to a real <table> server-side: {body}",
    );
}

/// A known **scaffold** slug with no agent prose yet is an honest `200` placeholder
/// (the "not yet generated" state), never a `404` ([NFR-CC-04], [FR-UI-06]) — the
/// four-tier menu links every scaffold page, so following one must land somewhere.
#[tokio::test]
async fn scaffold_slug_without_prose_is_an_honest_placeholder() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);
    // A Summary scaffold slug that has no written page in this fixture.
    let path = "/api/v1/wiki/page/overview/getting-started";
    let resp = router.oneshot(get(path)).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "a scaffold slug is a 200 placeholder, not a 404");
    assert_json_self_only_csp(&headers, path);
    assert!(body.contains("\"placeholder\":true"), "the placeholder flag is set: {body}");
    assert!(body.contains("Getting Started"), "the scaffold label is the title: {body}");
}

/// `GET /api/v1/wiki/nav` serializes the **four-tier** menu IA the SPA renders —
/// the Summary, Design, and Specs tiers plus the Search link ([FR-UI-06], S-189).
#[tokio::test]
async fn wiki_nav_serializes_the_four_tier_menu_ia() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);
    let resp = router.oneshot(get("/api/v1/wiki/nav")).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "wiki/nav answers 200");
    assert_json_self_only_csp(&headers, "/api/v1/wiki/nav");
    for key in ["\"tiers\"", "\"Summary\"", "\"Design\"", "\"Specs\"", "\"search_label\""] {
        assert!(body.contains(key), "wiki/nav carries {key}: {body}");
    }
    // The Design tier lists the presented Architecture page once, by its slug
    // (retired from the Summary tier by CR-062, presented from architecture.md).
    assert!(body.contains("overview/architecture"), "Design lists the Architecture page: {body}");
    // The Specs tier leads with the presented SRS hub (CR-064, S-269), ahead of the
    // consolidated requirement documents.
    assert!(body.contains("\"specs/srs\""), "Specs lists the presented SRS hub: {body}");
    let specs_at = body.find("\"Specs\"").expect("Specs tier present");
    let srs_at = body.find("\"specs/srs\"").expect("SRS hub listed");
    let fr_at = body.find("\"specs/functional-requirements\"").expect("FR doc listed");
    assert!(
        specs_at < srs_at && srs_at < fr_at,
        "the SRS hub leads the Specs tier, before the consolidated docs: {body}"
    );
    // No docs/howto/ in this fixture — the User Guide tier is absent, not empty.
    assert!(!body.contains("\"User Guide\""), "no docs/howto/ → no User Guide tier: {body}");
}

/// [FR-WK-23]/[CR-062]: `GET /api/v1/wiki/nav` lists a **User Guide** tier, right
/// after Summary and before Design, when `docs/howto/*.md` files exist **in SRS
/// mode** — one item per file — and a `guide/*` slug not yet materialized still
/// answers an honest `200` placeholder (never a `404`), since the User Guide
/// tier's dynamic file set is not in the fixed `scaffold_label` contract.
#[tokio::test]
async fn wiki_nav_lists_the_user_guide_tier_when_docs_howto_exists() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/lib.rs", "pub fn f() {}\n", "add f");
    // SRS-mode fixture: architecture.md + a requirement, so the User Guide gate
    // ([FR-WK-23], "docs/howto/ presence in SRS mode") is satisfied.
    std::fs::create_dir_all(repo.join("docs/specs/requirements")).unwrap();
    std::fs::write(repo.join("docs/specs/architecture.md"), "# Architecture\n").unwrap();
    std::fs::write(repo.join("docs/specs/requirements/FR-WK-01.md"), "# FR-WK-01\n").unwrap();
    std::fs::create_dir_all(repo.join("docs/howto")).unwrap();
    std::fs::write(repo.join("docs/howto/README.md"), "# User Guide\n\nStart here.\n").unwrap();
    std::fs::write(
        repo.join("docs/howto/installation.md"),
        "# Installation\n\nRun the installer.\n",
    )
    .unwrap();
    let engine = Arc::new(Engine::start(repo).expect("engine starts"));
    engine.index();
    let router = web::router(engine);

    let resp = router.clone().oneshot(get("/api/v1/wiki/nav")).await.unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"User Guide\""), "the User Guide tier is listed: {body}");
    assert!(body.contains("\"guide/overview\""), "README.md lands at guide/overview: {body}");
    assert!(body.contains("\"guide/installation\""), "installation.md is listed: {body}");
    // Tier order: Summary, then User Guide, then Design, then Specs ([FR-WK-11]).
    let summary_at = body.find("\"Summary\"").expect("Summary tier present");
    let user_guide_at = body.find("\"User Guide\"").expect("User Guide tier present");
    let design_at = body.find("\"Design\"").expect("Design tier present");
    let specs_at = body.find("\"Specs\"").expect("Specs tier present");
    assert!(
        summary_at < user_guide_at && user_guide_at < design_at && design_at < specs_at,
        "tier order is Summary / User Guide / Design / Specs: {body}"
    );

    // The guide page has not been materialized yet — an honest placeholder, not 404.
    let resp = router.oneshot(get("/api/v1/wiki/page/guide/overview")).await.unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "an un-materialized guide slug is a 200 placeholder");
    assert!(body.contains("\"placeholder\":true"), "the placeholder flag is set: {body}");
    assert!(body.contains("Overview"), "the guide title is the placeholder title: {body}");
}

/// [FR-WK-23]: in Case 2 (no `docs/specs/architecture.md` + requirement) the
/// User Guide tier is **absent** even though `docs/howto/` has files — the tier
/// is gated on "`docs/howto/` presence in SRS mode", not presence alone, so a
/// Case-2 project never shows a menu entry `wiki materialize` will never
/// fulfill.
#[tokio::test]
async fn wiki_nav_omits_the_user_guide_tier_in_case_2_even_with_docs_howto_present() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/lib.rs", "pub fn f() {}\n", "add f");
    // No docs/specs/architecture.md → Case 2, despite docs/howto/ being present.
    std::fs::create_dir_all(repo.join("docs/howto")).unwrap();
    std::fs::write(repo.join("docs/howto/README.md"), "# User Guide\n\nStart here.\n").unwrap();
    let engine = Arc::new(Engine::start(repo).expect("engine starts"));
    engine.index();
    let router = web::router(engine);

    let resp = router.clone().oneshot(get("/api/v1/wiki/nav")).await.unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("\"User Guide\""), "Case 2 → no User Guide tier: {body}");

    // A guide slug is not even an honest placeholder in Case 2 — it's a genuine 404.
    let resp = router.oneshot(get("/api/v1/wiki/page/guide/overview")).await.unwrap();
    let (status, _body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "Case 2 never presents guide/overview");
}

// ── Doc-image asset route (S-270 / [FR-WK-27], [ADR-58], [NFR-SE-04]) ─────────

/// The exact self-only CSP every response carries ([FR-UI-02], [BR-33]) — asserted
/// **byte-for-byte** on the asset route so [FR-WK-27]'s "CSP is byte-identical to
/// before this change; no `data:`/external host" is a real regression gate, not a
/// substring check.
const EXPECTED_CSP: &str = "default-src 'self'; base-uri 'none'; form-action 'none'; \
                            frame-ancestors 'none'; object-src 'none'";

/// A minimal SRS-mode repo with a doc-relative image under `docs/specs/**`, a
/// non-image doc file, and an image **outside** the doc roots (a repo-root sibling)
/// — the fixture the asset-route tests resolve against. `PNG_BYTES` is deliberately
/// non-UTF-8 so the route is proven to serve raw bytes, not a string.
const PNG_BYTES: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x01, 0x02];
const SECRET_BYTES: &[u8] = b"TOP-SECRET-NOT-AN-ASSET";

fn asset_fixture() -> (TempDir, Arc<Engine>) {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/lib.rs", "pub fn f() {}\n", "add f");
    std::fs::create_dir_all(repo.join("docs/specs/architecture/images")).unwrap();
    std::fs::write(repo.join("docs/specs/architecture.md"), "# Architecture\n").unwrap();
    std::fs::write(repo.join("docs/specs/architecture/images/diagram.png"), PNG_BYTES).unwrap();
    // An image file OUTSIDE the doc roots — a traversal/symlink must never reach it.
    std::fs::write(repo.join("escape.png"), SECRET_BYTES).unwrap();
    let engine = Arc::new(Engine::start(repo).expect("engine starts"));
    (tmp, engine)
}

/// A valid doc-relative image loads from the same-origin asset route with an image
/// content-type, its exact bytes, and the **byte-identical** self-only CSP
/// ([FR-WK-27] AC1/AC3): the Architecture page's diagrams become reachable without
/// weakening the CSP.
#[tokio::test]
async fn wiki_asset_serves_a_doc_relative_image_with_an_image_content_type() {
    let (_tmp, engine) = asset_fixture();
    let router = web::router(engine);

    let resp = router
        .oneshot(get("/api/v1/wiki/asset/docs/specs/architecture/images/diagram.png"))
        .await
        .unwrap();
    let (status, body, headers) = body_bytes(resp).await;
    assert_eq!(status, StatusCode::OK, "a valid doc-relative image is served");
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "image/png",
        "the image content-type is served",
    );
    assert_eq!(body, PNG_BYTES, "the exact image bytes are returned, not a re-encoding");
    // The declared image type is honored, never MIME-sniffed (defense in depth).
    assert_eq!(
        headers.get("x-content-type-options").unwrap(),
        "nosniff",
        "a served image carries X-Content-Type-Options: nosniff",
    );
    // The self-only CSP is byte-identical to every other response — no data:/host.
    assert_eq!(
        headers.get(header::CONTENT_SECURITY_POLICY).unwrap(),
        EXPECTED_CSP,
        "the asset route carries the byte-identical self-only CSP",
    );
}

/// [FR-WK-27] AC2 names two escape vectors — "a `..` traversal **or an absolute
/// path**". A request whose captured path is **absolute** (addressing the fixture's
/// out-of-root `escape.png` by its full path) is refused: `root.join(abs)` discards
/// the repo-root anchor (Rust `Path::join` semantics), leaving containment to the
/// canonicalized-prefix test alone — which the real target fails. The secret bytes
/// never leak.
#[tokio::test]
async fn wiki_asset_refuses_an_absolute_path_escape() {
    let (_tmp, engine) = asset_fixture();
    let abs = engine.root().join("escape.png");
    let abs = abs.to_str().expect("utf-8 temp path");
    let router = web::router(engine);

    // The leading `/` of `abs` makes the captured `*path` absolute (a `//` in the URI).
    let resp = router.oneshot(get(&format!("/api/v1/wiki/asset/{abs}"))).await.unwrap();
    let (status, body, _headers) = body_bytes(resp).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "an absolute-path escape is refused");
    assert_ne!(body, SECRET_BYTES, "the out-of-sandbox file's bytes must never be served");
    // The refusal comes from the sandbox handler (its body), proving the request
    // reached `resolve_doc_asset` and failed the canonicalized-prefix test — not a
    // mere router miss.
    assert_eq!(body, b"no such wiki asset", "the sandbox handler refused it");
}

/// The sandbox is **structural** (canonicalized-prefix), not a naive `..` ban: a
/// `..` path that still resolves *inside* the doc roots is served normally. This
/// pins that the containment check normalizes then prefix-tests, so legitimate
/// relative sources are not spuriously refused.
#[tokio::test]
async fn wiki_asset_serves_a_dotdot_path_that_stays_within_the_doc_roots() {
    let (_tmp, engine) = asset_fixture();
    let router = web::router(engine);

    let resp = router
        .oneshot(get("/api/v1/wiki/asset/docs/specs/architecture/../architecture/images/diagram.png"))
        .await
        .unwrap();
    let (status, body, headers) = body_bytes(resp).await;
    assert_eq!(status, StatusCode::OK, "a `..` staying inside the doc roots is served");
    assert_eq!(headers.get(header::CONTENT_TYPE).unwrap(), "image/png");
    assert_eq!(body, PNG_BYTES);
}

/// A `..` traversal escaping the doc roots is **refused** ([FR-WK-27] AC2), even
/// though the target is a real, image-typed file: the canonicalized target lands
/// outside every doc root and the prefix test fails. The secret bytes never leak.
#[tokio::test]
async fn wiki_asset_refuses_a_traversal_escaping_the_doc_roots() {
    let (_tmp, engine) = asset_fixture();
    let router = web::router(engine);

    let resp = router
        .oneshot(get("/api/v1/wiki/asset/docs/specs/../../escape.png"))
        .await
        .unwrap();
    let (status, body, _headers) = body_bytes(resp).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "an escape past the doc roots is refused");
    assert_ne!(body, SECRET_BYTES, "the out-of-sandbox file's bytes must never be served");
}

/// A symlink inside a doc root pointing **outside** it is refused — the strongest
/// proof the containment is canonicalized-prefix (symlink-resolving), not string
/// matching: the request path looks in-sandbox, but `canonicalize` resolves the link
/// to its real out-of-root target, which fails the prefix test ([NFR-SE-04]).
#[cfg(unix)]
#[tokio::test]
async fn wiki_asset_refuses_a_symlink_escaping_the_doc_roots() {
    let (_tmp, engine) = asset_fixture();
    let repo = engine.root().to_path_buf();
    // docs/specs/sneak.png → ../../escape.png (a real image outside the doc roots).
    std::os::unix::fs::symlink(
        repo.join("escape.png"),
        repo.join("docs/specs/sneak.png"),
    )
    .expect("create symlink");
    let router = web::router(engine);

    let resp = router.oneshot(get("/api/v1/wiki/asset/docs/specs/sneak.png")).await.unwrap();
    let (status, body, _headers) = body_bytes(resp).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a symlink escaping the doc roots is refused");
    assert_ne!(body, SECRET_BYTES, "the symlink target's bytes must never be served");
}

/// A non-image path under the doc roots is refused ([FR-WK-27] AC2, "only image
/// content-types are served") — a `415`, distinct from the containment `404`.
#[tokio::test]
async fn wiki_asset_refuses_a_non_image_content_type() {
    let (_tmp, engine) = asset_fixture();
    let router = web::router(engine);

    let resp = router.oneshot(get("/api/v1/wiki/asset/docs/specs/architecture.md")).await.unwrap();
    let (status, _body, headers) = body_bytes(resp).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "a non-image doc file is refused");
    // Even a refusal carries the byte-identical self-only CSP.
    assert_eq!(headers.get(header::CONTENT_SECURITY_POLICY).unwrap(), EXPECTED_CSP);
}

/// An absent (but image-typed and in-root) asset is an honest `404`, never a fabricated
/// body ([NFR-RA-05]).
#[tokio::test]
async fn wiki_asset_is_a_404_for_an_absent_image() {
    let (_tmp, engine) = asset_fixture();
    let router = web::router(engine);

    let resp = router
        .oneshot(get("/api/v1/wiki/asset/docs/specs/architecture/images/missing.png"))
        .await
        .unwrap();
    let (status, _body, _headers) = body_bytes(resp).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "an absent in-root image is an honest 404");
}

/// The asset route stays **GET-only** like the rest of the read surface — a `POST`
/// is `405` before any handler runs ([UAT-UI-02], [ADR-31]).
#[tokio::test]
async fn wiki_asset_rejects_a_post() {
    let (_tmp, engine) = asset_fixture();
    let router = web::router(engine);

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/wiki/asset/docs/specs/architecture/images/diagram.png")
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED, "the asset route is GET-only");
}

// ── Populated-graph: the graph/search endpoints reach real read-model fields ──

/// Over an indexed graph the `/api/v1/graph` snapshot carries real nodes (a `layer`
/// field) and `/api/v1/search` returns ranked hits — proving the endpoints reach
/// the accessors end-to-end, not merely parse-then-drop ([FR-UI-21], [NFR-RA-05]).
#[cfg(feature = "lang-rust")]
#[tokio::test]
async fn graph_and_search_serialize_real_graph_fields_over_an_indexed_repo() {
    let dir = TempDir::new().expect("temp dir");
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn alpha() { beta(); }\npub fn beta() {}\n",
    )
    .expect("write fixture");
    let engine = Arc::new(Engine::start(dir.path()).expect("engine starts"));
    engine.index();
    let router = web::router(engine);

    let resp = router.clone().oneshot(get("/api/v1/graph?cap=50")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"layer\":\"code\""), "nodes serialize a layer: {body}");
    assert!(body.contains("\"cap\":50"), "the applied cap is echoed: {body}");

    let resp = router.oneshot(get("/api/v1/search?q=alpha")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("alpha"), "the search hit names the symbol: {body}");
}

/// `node?code=1` serializes the declaration source excerpt; without it the excerpt
/// is withheld (the field is `null`) — the `truthy` toggle reaches the
/// `include_code` accessor end-to-end ([FR-NV-04]).
#[cfg(feature = "lang-rust")]
#[tokio::test]
async fn node_code_param_serializes_the_source_excerpt() {
    let dir = TempDir::new().expect("temp dir");
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn alpha() { beta(); }\npub fn beta() {}\n",
    )
    .expect("write fixture");
    let engine = Arc::new(Engine::start(dir.path()).expect("engine starts"));
    engine.index();
    let router = web::router(engine);

    // With ?code=1 the declaration source is serialized.
    let resp = router.clone().oneshot(get("/api/v1/node?symbol=alpha&code=1")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"code\":\"pub fn alpha"), "the excerpt is serialized with code=1: {body}");

    // Without it the excerpt is withheld — the field is null.
    let resp = router.oneshot(get("/api/v1/node?symbol=alpha")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"code\":null"), "the excerpt is withheld without code=1: {body}");
}
