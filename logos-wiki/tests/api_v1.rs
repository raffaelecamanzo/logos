//! The wiki's `/api/v1/wiki*` read routes (S-183, S-189 / [FR-UI-21], [FR-UI-06],
//! [FR-WK-05], [FR-WK-27]) — the half of `web/tests/api_v1.rs` that left the web
//! adapter with the wiki (S-644).
//!
//! Drives the **real** router composed with the `wiki` extension in-process
//! (`logos_wiki::web::router`, `tower::ServiceExt::oneshot`, no socket) over a
//! started engine — the web surface's own contract applied to the extension's
//! routes: `200 application/json` under the byte-identical self-only CSP, GET-only,
//! write-free on read; plus the wiki's own behaviour (search, pages, the menu, the
//! sandboxed doc-asset route).
//!
//! [FR-UI-21]: ../../docs/specs/requirements/FR-UI-21.md
//! [FR-UI-06]: ../../docs/specs/requirements/FR-UI-06.md
//! [FR-WK-05]: ../../docs/specs/requirements/FR-WK-05.md
//! [FR-WK-27]: ../../docs/specs/requirements/FR-WK-27.md

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use logos_core::Engine;
use logos_wiki::WikiEngine as _;
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

/// The shared body behind [`scanned_engine`] and [`scanned_engine_with_rules`]:
/// an indexed + scanned + mined fixture, optionally with `.logos/rules.toml`
/// written before the engine starts.
fn build_scanned_engine(rules_toml: Option<&str>) -> (TempDir, Arc<Engine>) {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    if let Some(rules_toml) = rules_toml {
        std::fs::create_dir_all(repo.join(".logos")).expect("mkdir .logos");
        std::fs::write(repo.join(".logos/rules.toml"), rules_toml).expect("write rules.toml");
    }
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

/// An indexed + scanned + mined fixture — the durable state the read-only `/api/v1`
/// endpoints reflect without adding to. `scan` persists the metric snapshot even on
/// an un-parsed graph and `hotspots` mines the temporal snapshot, so the composite
/// endpoints succeed under a bare `-p logos-wiki` run.
fn scanned_engine() -> (TempDir, Arc<Engine>) {
    build_scanned_engine(None)
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

/// The wiki's `/api/v1` read endpoints — the routes the SPA's Wiki view fetches —
/// and the Dashboard bundle, which carries the wiki's overview field.
const V1_ENDPOINTS: &[&str] = &[
    "/api/v1/overview",
    "/api/v1/wiki",
    "/api/v1/wiki/nav",
    "/api/v1/wiki/search?q=f",
    "/api/v1/wiki/page/overview/project-overview",
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

// ── Contract: 200 JSON + self-only CSP, GET-only, no write-on-read ────────────

/// Every wiki read endpoint answers `200 application/json` under the
/// byte-identical self-only CSP ([FR-UI-21], [FR-UI-03]).
#[tokio::test]
async fn every_wiki_endpoint_serves_json_under_self_only_csp() {
    let (_tmp, engine) = scanned_engine();
    let router = logos_wiki::web::router(engine);

    for path in V1_ENDPOINTS {
        let resp = router.clone().oneshot(get(path)).await.expect("route responds");
        let (status, _body, headers) = body_string(resp).await;
        assert_eq!(status, StatusCode::OK, "{path} answers 200");
        assert_json_self_only_csp(&headers, path);
    }
}

/// The wiki's single reads serialize their read-model's own fields: `wiki` the
/// dual-axis freshness read-model, `wiki/search` a JSON array of hits
/// ([FR-UI-21], [NFR-RA-05]).
#[tokio::test]
async fn wiki_reads_serialize_their_read_model_fields() {
    let (_tmp, engine) = scanned_engine();
    let router = logos_wiki::web::router(engine);

    let resp = router.clone().oneshot(get("/api/v1/wiki")).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "wiki answers 200");
    assert_json_self_only_csp(&headers, "/api/v1/wiki");
    for key in ["\"page_count\"", "\"freshness_fraction\"", "\"current_revision\""] {
        assert!(body.contains(key), "/api/v1/wiki body carries {key}: {body}");
    }

    let resp = router.oneshot(get("/api/v1/wiki/search?q=f")).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "wiki/search answers 200");
    assert_json_self_only_csp(&headers, "/api/v1/wiki/search?q=f");
    assert!(body.starts_with('['), "wiki/search serializes a JSON array: {body}");
}

/// A `POST` to a wiki read route is `405` before any handler runs: an extension's
/// `GET` route is admitted for `GET` only ([UAT-UI-02], [FR-UI-03]).
#[tokio::test]
async fn post_to_a_wiki_read_route_is_405() {
    let (_tmp, engine) = scanned_engine();
    let router = logos_wiki::web::router(engine);
    for path in ["/api/v1/wiki", "/api/v1/wiki/nav", "/api/v1/wiki/search", "/api/v1/wiki/page/x"] {
        let req = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(header::HOST, "127.0.0.1:4983")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED, "{path} is GET-only");
    }
}

/// Loading every wiki endpoint once **and** repeatedly adds no `metric_snapshots`
/// or `temporal_snapshots` row ([FR-UI-21] AC, [FR-UI-03], [ADR-28]).
#[tokio::test]
async fn loading_every_wiki_endpoint_repeatedly_writes_no_snapshot() {
    let (tmp, engine) = scanned_engine();
    let repo = tmp.path().to_path_buf();

    let m0 = metric_count(&repo);
    let t0 = temporal_count(&repo);
    assert_eq!(m0, 1, "the fixture scan wrote exactly one metric snapshot");

    let router = logos_wiki::web::router(engine);
    for path in V1_ENDPOINTS {
        for _ in 0..2 {
            let resp = router.clone().oneshot(get(path)).await.expect("route responds");
            assert_eq!(resp.status(), StatusCode::OK, "{path} answers 200");
        }
    }

    assert_eq!(metric_count(&repo), m0, "no GET added a metric_snapshots row ([ADR-28])");
    assert_eq!(temporal_count(&repo), t0, "no GET added a temporal_snapshots row ([ADR-28])");
}

// ── The wiki's own read behaviour ────────────────────────────────────────────

/// `wiki/search` with an empty (or absent) `q` is an honest empty list (`200`
/// `[]`), never an error — the contract the SPA's empty search box relies on
/// ([FR-WK-05]).
#[tokio::test]
async fn wiki_search_empty_query_is_an_honest_empty_list() {
    let (_tmp, engine) = scanned_engine();
    let router = logos_wiki::web::router(engine);
    for path in ["/api/v1/wiki/search", "/api/v1/wiki/search?q="] {
        let resp = router.clone().oneshot(get(path)).await.unwrap();
        let (status, body, headers) = body_string(resp).await;
        assert_eq!(status, StatusCode::OK, "{path} answers 200");
        assert_json_self_only_csp(&headers, path);
        assert_eq!(body.trim(), "[]", "{path} is an honest empty list: {body}");
    }
}

/// An unknown wiki slug is an honest `404`, never a fabricated page ([NFR-CC-04]).
#[tokio::test]
async fn unknown_wiki_page_is_404() {
    let (_tmp, engine) = scanned_engine();
    let router = logos_wiki::web::router(engine);
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
    let router = logos_wiki::web::router(Arc::clone(&engine));
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
    let router = logos_wiki::web::router(engine);
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
    let router = logos_wiki::web::router(engine);
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
    let router = logos_wiki::web::router(engine);

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
    let router = logos_wiki::web::router(engine);

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
    let router = logos_wiki::web::router(engine);

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
    let router = logos_wiki::web::router(engine);

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
    let router = logos_wiki::web::router(engine);

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
    let router = logos_wiki::web::router(engine);

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
    let router = logos_wiki::web::router(engine);

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
    let router = logos_wiki::web::router(engine);

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
    let router = logos_wiki::web::router(engine);

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
    let router = logos_wiki::web::router(engine);

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/wiki/asset/docs/specs/architecture/images/diagram.png")
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED, "the asset route is GET-only");
}
