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
//! - a `node`/`search` call without its required query param is a `400`.
//!
//! The wiki's read routes are the `wiki` extension's since S-644; their half of
//! this suite moved with them, to that extension's own suites.
//!
//! The whole suite runs under a bare `cargo test -p web` with no `lang-rust` gate:
//! the populated-graph assertions index Rust fixtures, and the test build always
//! carries the Rust grammar (the `[dev-dependencies]` logos-core enables it) while
//! neither `gate.sh` nor CI passes that feature to `web` itself, so a gated test
//! would compile out of both.

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
/// endpoints (`/api/v1/health`, `/files`, …) succeed under a bare `-p web` run.
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
    "/api/v1/impact-intersection?item=S-1=f&item=S-2=g",
    "/api/v1/precedent?target=f",
    "/api/v1/branch-overlap?ref=main&ref=main",
    "/api/v1/node?symbol=f",
    "/api/v1/search?q=f",
    // S-622 / [FR-UI-47]: the chat answer's code-reference resolution — a GET,
    // so the `200`+CSP, POST-`405` and write-free loops all walk it.
    "/api/v1/code-refs?ref=f&ref=src/lib.rs",
    // The wiki's `/api/v1/wiki*` reads are the `wiki` extension's routes since
    // S-644, walked by the same three loops in that extension's own suite.
    "/api/v1/config",
    "/api/v1/statistics",
    "/api/v1/status",
];

async fn body_string(resp: Response<Body>) -> (StatusCode, String, axum::http::HeaderMap) {
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap(), headers)
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

/// S-505 ([FR-QM-17]): `GET /api/v1/health` carries the "where to start" list of
/// the snapshot it displays as `scan.where_to_start` — the value
/// `Engine::improvements` (the CLI `improvements` and the MCP tool) serves, field
/// for field. The fixture holds one function nested six deep among eight shallow
/// ones, so the list has a ranked unit and an outlier label to compare, not two
/// empty lists agreeing.
///
/// [FR-QM-17]: ../../docs/specs/requirements/FR-QM-17.md
#[tokio::test]
async fn health_carries_the_where_to_start_list_the_engine_serves() {
    let tmp = TempDir::new().expect("temp root");
    let mut source: String = (0..8)
        .map(|i| format!("pub fn shallow_{i}(x: i32) -> i32 {{\n    if x > {i} {{\n        return x;\n    }}\n    0\n}}\n"))
        .collect();
    let open: String = (0..6).map(|i| format!("if x > {i} {{\n")).collect();
    source.push_str(&format!("pub fn deep_one(x: i32) -> i32 {{\n{open}return x;\n{}0\n}}\n", "}\n".repeat(6)));
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/lib.rs"), source).unwrap();
    let engine = Arc::new(Engine::start(tmp.path()).expect("engine starts"));
    engine.index();
    engine.scan(true).expect("scan persists a snapshot");

    let router = web::router(Arc::clone(&engine));
    let resp = router.oneshot(get("/api/v1/health")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK);
    let health: serde_json::Value = serde_json::from_str(&body).expect("health is JSON");

    let served = serde_json::to_value(engine.improvements().expect("improvements reads")).unwrap();
    assert_eq!(served["units"][0]["name"], "deep_one", "{served}");
    assert_eq!(health["scan"]["where_to_start"], served, "Health and the engine's list are one value");
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

/// Like [`scanned_engine`] but writes `.logos/rules.toml` before starting the
/// engine, so `check_rules` loads it as a present contract.
fn scanned_engine_with_rules(rules_toml: &str) -> (TempDir, Arc<Engine>) {
    build_scanned_engine(Some(rules_toml))
}

/// `/api/v1/overview` and `/api/v1/gaps` both serialize `RulesReport` verbatim
/// (S-352/S-354): the three states must round-trip through JSON distinctly — a
/// clean loaded contract (`passed:true`), a violated one (`passed:false`), and
/// no contract at all (`rules_present:false`, `passed:null` — never `true`,
/// which would read as a false pass, CR-112). Both endpoints are asserted for
/// every state, not just `/gaps`: `overview`'s `OverviewModel.rules` is an
/// independent `e.check_rules(...)` call ([`web/src/api_v1.rs`]'s `overview`
/// handler), so a divergence between the two is exactly the kind of bug this
/// story exists to catch.
#[tokio::test]
async fn overview_and_gaps_serialize_all_three_rules_states() {
    const PATHS: [&str; 2] = ["/api/v1/overview", "/api/v1/gaps"];

    // State 1: no contract loaded (the `scanned_engine` fixture writes none).
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);
    for path in PATHS {
        let resp = router.clone().oneshot(get(path)).await.unwrap();
        let (status, body, _h) = body_string(resp).await;
        assert_eq!(status, StatusCode::OK, "{path} answers 200");
        assert!(body.contains("\"rules_present\":false"), "{path} absent contract: {body}");
        assert!(
            body.contains("\"passed\":null"),
            "{path} an empty evaluated set carries no verdict, never `true`: {body}"
        );
    }

    // State 2: a loaded, clean contract.
    let (_tmp, engine) = scanned_engine_with_rules("[constraints]\nmax_cycles = 0\n");
    let router = web::router(engine);
    for path in PATHS {
        let resp = router.clone().oneshot(get(path)).await.unwrap();
        let (status, body, _h) = body_string(resp).await;
        assert_eq!(status, StatusCode::OK, "{path} answers 200");
        assert!(body.contains("\"rules_present\":true"), "{path} a loaded rules.toml: {body}");
        assert!(body.contains("\"passed\":true"), "{path} no cycles in the fixture: {body}");
    }

    // State 3: a loaded contract with a violation (`max_cc = 0` fires on `f`'s branch).
    let (_tmp, engine) = scanned_engine_with_rules("[constraints]\nmax_cc = 0\n");
    let router = web::router(engine);
    for path in PATHS {
        let resp = router.clone().oneshot(get(path)).await.unwrap();
        let (status, body, _h) = body_string(resp).await;
        assert_eq!(status, StatusCode::OK, "{path} answers 200");
        assert!(body.contains("\"rules_present\":true"), "{path} a loaded rules.toml: {body}");
        assert!(body.contains("\"passed\":false"), "{path} the CC budget fires: {body}");
    }
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
/// opaque `200` — spot-check the keys the SPA reads off `query`/`node`/`status`
/// so a reshaped read-model can't silently pass the contract loop ([FR-UI-21],
/// [NFR-RA-05]). The wiki's (`wiki`, `wiki/search`) are spot-checked with the
/// extension that serves them (S-644).
#[tokio::test]
async fn single_read_endpoints_serialize_their_read_model_fields() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);

    let cases: &[(&str, &[&str])] = &[
        // QueryResponse — the interpreted mode + the ranked-hit channel.
        ("/api/v1/query?q=f", &["\"mode\"", "\"hits\"", "\"total\""]),
        // NodeInfo — the echoed query + the ADR-14 degradation channel.
        ("/api/v1/node?symbol=f", &["\"query\"", "\"warnings\"", "\"suggestions\""]),
        // StatusInfo — the FR-NV-07 index-health read-model the app header's
        // graph-state readout renders (S-315, [FR-UI-34], [CR-097]).
        (
            "/api/v1/status",
            &["\"indexed\"", "\"graph_revision\"", "\"node_count\"", "\"edge_count\""],
        ),
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

}

/// `GET /api/v1/status` carries **only** the [FR-NV-07] status projection (S-315,
/// [FR-UI-34], [CR-097]) — the point of adding it. The header used to set one
/// boolean by pulling the ~1 MB [FR-UI-04] Health bundle and discarding all of it,
/// so this asserts both halves: the status fields ARE there, and the Health
/// bundle's sibling blocks (`gate`, `scan`, `evolution`) are NOT. It also probes
/// the near miss: `/api/v1/statistics` shares this path's first twelve characters
/// and is a different read-model, so the two must answer distinguishable bodies.
#[tokio::test]
async fn status_endpoint_carries_only_the_status_projection() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);

    let resp = router.clone().oneshot(get("/api/v1/status")).await.expect("route responds");
    let (code, body, headers) = body_string(resp).await;
    assert_eq!(code, StatusCode::OK, "/api/v1/status answers 200");
    assert_json_self_only_csp(&headers, "/api/v1/status");
    for key in ["\"indexed\"", "\"graph_revision\"", "\"node_count\"", "\"edge_count\""] {
        assert!(body.contains(key), "/api/v1/status carries {key}: {body}");
    }
    for absent in ["\"gate\"", "\"scan\"", "\"evolution\""] {
        assert!(
            !body.contains(absent),
            "/api/v1/status is the status projection alone — it must not carry {absent}: {body}",
        );
    }

    // The near miss: `/api/v1/statistics` is NOT this route. It answers the
    // telemetry read-model, whose `calls_by_tool` channel the status projection has
    // no field for — so a prefix match that swallowed it would be caught here.
    let resp = router.oneshot(get("/api/v1/statistics")).await.expect("route responds");
    let (code, stats_body, _) = body_string(resp).await;
    assert_eq!(code, StatusCode::OK, "/api/v1/statistics still answers 200");
    assert!(
        stats_body.contains("\"calls_by_tool\"") && !body.contains("\"calls_by_tool\""),
        "/api/v1/statistics and /api/v1/status are distinct read-models",
    );
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

/// A language-plugin registry fixture: two capability arms behind one trait,
/// through the same two helpers, plus a registry naming both types.
///
/// The `/api/v1` precedent route needs a repository that actually HAS a
/// precedent — a route test over the single-function default fixture would
/// assert serde field names against an empty answer and pass while proving
/// nothing (the defect review caught in the S-358 twin).
fn precedent_engine() -> (TempDir, Arc<Engine>) {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(
        repo,
        "src/plugin.rs",
        "pub trait LanguagePlugin {\n    fn extract(&self);\n}\n",
        "feat: the plugin trait",
    );
    commit(
        repo,
        "src/shared.rs",
        "pub fn parse_source() {}\npub fn emit_facts() {}\n",
        "feat: the shared helpers",
    );
    for (file, ty) in [("src/rustarm.rs", "RustArm"), ("src/pyarm.rs", "PyArm")] {
        commit(
            repo,
            file,
            &format!(
                "use crate::plugin::LanguagePlugin;\n\
                 use crate::shared::{{parse_source, emit_facts}};\n\n\
                 pub struct {ty};\n\n\
                 impl LanguagePlugin for {ty} {{\n\
                 \x20   fn extract(&self) {{\n\
                 \x20       parse_source();\n\
                 \x20       emit_facts();\n\
                 \x20   }}\n\
                 }}\n\n\
                 pub fn {init}_init() {{}}\n",
                init = file.trim_start_matches("src/").trim_end_matches(".rs")
            ),
            "feat: a capability arm",
        );
    }
    // A dispatcher CALLING each arm's init, so a file target yields MORE THAN
    // ONE precedent — without it every `?limit=` assertion below would hold
    // trivially against a one-element list and would pass even if the handler
    // never parsed `limit` at all. A module `use` list is deliberately not a
    // registration (see `is_registration_edge`), so the dispatcher must call.
    commit(
        repo,
        "src/registry.rs",
        "use crate::rustarm::rustarm_init;\n\
         use crate::pyarm::pyarm_init;\n\n\
         pub fn register_all() {\n\
         \x20   rustarm_init();\n\
         \x20   pyarm_init();\n\
         }\n",
        "feat: the registry",
    );
    // A pinned coordinate (S-703): the queried symbol names it.
    std::fs::create_dir_all(repo.join(".logos")).unwrap();
    std::fs::write(repo.join(".logos/config.toml"), "[project]\nid = \"example.com/fixture\"\n").unwrap();
    let engine = Arc::new(Engine::start(repo).expect("engine starts"));
    engine.index();
    (tmp, engine)
}

/// `GET /api/v1/precedent?target=<symbol|path>[&limit=<n>]` serializes the
/// [FR-NV-12] read-model (S-359, CR-114): the analogous nodes, the reason each
/// one is analogous, the stated notion and ranking rule, and the coverage limits.
///
/// Infallible at the surface ([NFR-CC-04]): a missing target, an unknown symbol
/// and a malformed `limit` are all honest `200`s carrying their own explanation,
/// never a `400`.
#[tokio::test]
async fn precedent_endpoint_serializes_the_read_model_with_its_reasons() {
    let (_tmp, engine) = precedent_engine();
    let router = web::router(engine);

    let path = "/api/v1/precedent?target=logos%20.%20example.com%2Ffixture%20.%20src/%60rustarm.rs%60/extract().";
    let resp = router.clone().oneshot(get(path)).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_json_self_only_csp(&headers, path);
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");

    // The route must be shown to COMPUTE, not merely to serialise field names:
    // the sibling arm comes back and says why it is analogous.
    assert_eq!(payload["target_kind"], "symbol", "{body}");
    let precedents = payload["precedents"].as_array().expect("precedents");
    assert_eq!(precedents.len(), 1, "{body}");
    assert_eq!(precedents[0]["file"], "src/pyarm.rs", "{body}");
    let facets: Vec<&str> = precedents[0]["reasons"]
        .as_array()
        .expect("reasons")
        .iter()
        .map(|r| r["facet"].as_str().unwrap())
        .collect();
    assert_eq!(facets, vec!["shared_supertype", "shared_callee"], "{body}");
    assert_eq!(precedents[0]["rank"]["facets"], 2, "{body}");
    assert!(
        !payload["notion"].as_str().unwrap_or_default().is_empty()
            && !payload["ranked_by"].as_str().unwrap_or_default().is_empty(),
        "the stated notion and ranking rule ride the payload (FR-NV-12 AC 1): {body}"
    );
    assert!(
        !payload["coverage"]["statement"].as_str().unwrap_or_default().is_empty(),
        "the coverage limits ride the payload (NFR-CC-04): {body}"
    );

    // A file target — the second half of the resolution rule — reaches the wire.
    let resp = router
        .clone()
        .oneshot(get("/api/v1/precedent?target=src/rustarm.rs"))
        .await
        .unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(payload["target_kind"], "file", "{body}");
    assert!(!payload["precedents"].as_array().unwrap().is_empty(), "{body}");

    // `?limit=` reaches the engine, and a malformed limit degrades to the
    // default rather than failing the request. Both halves are asserted against
    // a target with MORE THAN ONE precedent, so neither can hold trivially: if
    // the handler dropped `limit`, the capped case would return them all.
    let unbounded: serde_json::Value = {
        let resp = router
            .clone()
            .oneshot(get("/api/v1/precedent?target=src/rustarm.rs"))
            .await
            .unwrap();
        let (_status, body, _headers) = body_string(resp).await;
        serde_json::from_str(&body).expect("json")
    };
    let total = unbounded["total_found"].as_u64().expect("total_found");
    assert!(total > 1, "the fixture must offer a choice to cap: {unbounded}");

    let resp = router
        .clone()
        .oneshot(get("/api/v1/precedent?target=src/rustarm.rs&limit=1"))
        .await
        .unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(payload["precedents"].as_array().unwrap().len(), 1, "{body}");
    assert_eq!(payload["total_found"].as_u64(), Some(total), "{body}");
    assert_eq!(payload["elided"].as_u64(), Some(total - 1), "{body}");

    let resp = router
        .clone()
        .oneshot(get("/api/v1/precedent?target=src/rustarm.rs&limit=abc"))
        .await
        .unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "a malformed limit is not a 4xx: {body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(
        payload["precedents"].as_array().unwrap().len() as u64,
        total,
        "a malformed limit falls back to the default, it does not cap: {body}"
    );

    // An unknown target reaches the wire as an empty answer naming its reason.
    let resp = router
        .clone()
        .oneshot(get("/api/v1/precedent?target=no_such_thing"))
        .await
        .unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "an unknown target is not a 4xx");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(payload["empty_reason"]["code"], "target_unresolved", "{body}");

    // No target at all → an honest 200 that still STATES ITSELF. `precedents`
    // empty and `target_kind == "unresolved"` are both satisfied by a bare
    // `PrecedentResult::default()`, so asserting only those would certify the
    // very gap this checks for: the default carries no reason and no notion,
    // and [FR-NV-12] AC 4 makes an unexplained empty answer the one thing this
    // read-model may never be.
    let resp = router.oneshot(get("/api/v1/precedent")).await.unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "a targetless call is an honest 200");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(payload["precedents"].as_array().unwrap().is_empty(), "{body}");
    assert_eq!(payload["target_kind"], "unresolved", "{body}");
    assert_eq!(
        payload["empty_reason"]["code"], "target_unresolved",
        "an empty answer must name its reason (FR-NV-12 AC 4): {body}"
    );
    for stated in ["notion", "ranked_by"] {
        assert!(
            !payload[stated].as_str().unwrap_or_default().is_empty(),
            "the targetless answer must still state its {stated}: {body}"
        );
    }
    assert!(
        !payload["coverage"]["statement"].as_str().unwrap_or_default().is_empty(),
        "the targetless answer must still state its coverage limits: {body}"
    );
}

/// `GET /api/v1/impact-intersection?item=<id>=<sym>&…` serializes the
/// [FR-NV-11] read-model (S-358, CR-114): the per-item rows, the intersecting
/// pairs with their shared symbols, the safely-parallel pairs, and the coverage
/// limits. `item` REPEATS, which is the one thing this route does differently
/// from its neighbours — it reads the query as ordered pairs, not a map, so a
/// second `item` cannot silently overwrite the first.
///
/// Infallible at the surface ([NFR-CC-04]): no `item` at all is an honest `200`
/// empty read-model carrying its warning, never a `400`.
#[tokio::test]
async fn impact_intersection_endpoint_serializes_the_read_model_and_keeps_every_item() {
    let (_tmp, engine) = scanned_engine();
    let router = web::router(engine);

    let path = "/api/v1/impact-intersection?item=S-1=f&item=S-2=f";
    let resp = router.clone().oneshot(get(path)).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_json_self_only_csp(&headers, path);
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");

    // A repeated `item` param is KEPT — the reason this handler reads ordered
    // pairs rather than the map its neighbours use.
    assert_eq!(
        payload["items"].as_array().map(Vec::len),
        Some(2),
        "a repeated `item` param is kept, not overwritten: {body}"
    );
    // The route must be shown to COMPUTE, not merely to serialise field names:
    // both items declare `f`, so the pair collides on `f` and is attributed to
    // both. Asserting the keys exist would hold against an empty answer.
    let intersecting = payload["intersecting"].as_array().expect("intersecting");
    assert_eq!(intersecting.len(), 1, "{body}");
    assert_eq!(intersecting[0]["left"], "S-1");
    assert_eq!(intersecting[0]["right"], "S-2");
    let shared = intersecting[0]["shared"].as_array().expect("shared");
    assert!(
        shared.iter().any(|s| s["name"] == "f"),
        "the shared symbol is named: {body}"
    );
    assert_eq!(
        shared[0]["declared_by"],
        serde_json::json!(["S-1", "S-2"]),
        "both items declared it: {body}"
    );
    assert!(
        payload["safe_parallel"].as_array().unwrap().is_empty(),
        "a colliding pair is never also safely parallel: {body}"
    );
    assert!(
        !payload["coverage"]["statement"].as_str().unwrap_or_default().is_empty(),
        "the coverage limits ride the payload (NFR-CC-04): {body}"
    );

    // `?depth=` reaches the engine, and a malformed depth degrades to the
    // default rather than failing the request.
    for (query, expected) in [("&depth=1", 1), ("&depth=abc", 3)] {
        let resp = router
            .clone()
            .oneshot(get(&format!("{path}{query}")))
            .await
            .unwrap();
        let (status, body, _headers) = body_string(resp).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let payload: serde_json::Value = serde_json::from_str(&body).expect("json");
        assert_eq!(payload["depth"], expected, "{query}: {body}");
    }

    // An unknown symbol reaches the wire as a coverage limit, not an error.
    let resp = router
        .clone()
        .oneshot(get("/api/v1/impact-intersection?item=S-1=f&item=S-2=nope"))
        .await
        .unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "an unknown symbol is not a 4xx");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(payload["coverage"]["unresolved"][0]["item"], "S-2", "{body}");
    assert_eq!(payload["coverage"]["unresolved"][0]["symbol"], "nope", "{body}");
    assert_eq!(
        payload["coverage"]["items_without_resolved_symbols"],
        serde_json::json!(["S-2"]),
        "an item resting on nothing is named as such: {body}"
    );

    // No items at all → the honest empty default, not a 4xx.
    let resp = router.oneshot(get("/api/v1/impact-intersection")).await.unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "an itemless call is an honest 200");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(payload["items"].as_array().unwrap().is_empty(), "{body}");
    assert!(
        payload["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap_or_default().contains("at least two")),
        "the empty answer says why it is empty: {body}"
    );
}

/// `GET /api/v1/branch-overlap?ref=<r>&ref=<r>[&merge=<r>]` serializes the
/// [FR-NV-13] read-model (S-360, CR-114): the per-ref rows, the contended
/// symbols with the refs that modify them and the refs that do not, the merge
/// block, and the coverage limits. `ref` REPEATS, which is the one thing this
/// route does differently from its neighbours — it reads the query as ordered
/// pairs, not a map, so a second `ref` cannot silently overwrite the first.
///
/// The route must be shown to COMPUTE, not merely to serialise field names, so
/// the fixture grows two branches that genuinely edit the same function.
/// Infallible at the surface ([NFR-CC-04]): a ref that does not resolve is an
/// honest `200` carrying its warning, never a `400`.
#[tokio::test]
async fn branch_overlap_endpoint_serializes_the_read_model_and_keeps_every_ref() {
    let (tmp, engine) = scanned_engine();
    let repo = tmp.path();
    for (branch, body) in [
        ("topic", "pub fn f(x: i64) -> i64 { if x > 0 { x } else { 0 - x } }\n"),
        ("other", "pub fn f(x: i64) -> i64 { if x >= 0 { x } else { -x } }\n"),
    ] {
        sh_git(repo, &["checkout", "-q", "-b", branch, "main"]);
        commit(repo, "src/lib.rs", body, "branch edit");
        sh_git(repo, &["checkout", "-q", "main"]);
    }
    let router = web::router(engine);

    let path = "/api/v1/branch-overlap?ref=topic&ref=other";
    let resp = router.clone().oneshot(get(path)).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_json_self_only_csp(&headers, path);
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");

    // A repeated `ref` param is KEPT — the reason this handler reads ordered
    // pairs rather than the map its neighbours use.
    assert_eq!(
        payload["refs"].as_array().map(Vec::len),
        Some(2),
        "a repeated `ref` param is kept, not overwritten: {body}"
    );
    assert_eq!(payload["refs"][0]["ref"], "topic", "{body}");
    let contended = payload["contended"].as_array().expect("contended");
    // Two rows, and both are true: this fixture's `src/lib.rs` is a SINGLE line,
    // so the file's module node and the only function in it share one span and
    // neither encloses the other. The innermost-symbol rule that keeps a module
    // out of the answer in ordinary multi-line code has nothing to bite on here;
    // `logos-core/tests/branch_overlap.rs` pins that rule where it applies.
    assert_eq!(contended.len(), 2, "{body}");
    let f = contended
        .iter()
        .find(|row| row["name"] == "f")
        .unwrap_or_else(|| panic!("the edited function is contended: {body}"));
    assert_eq!(
        f["modified_by"],
        serde_json::json!(["topic", "other"]),
        "the refs are named, in the order supplied: {body}"
    );
    assert!(
        f["absent_from"].as_array().unwrap().is_empty(),
        "both refs touch it: {body}"
    );
    assert!(payload["merge"].is_null(), "no merge was stated: {body}");
    assert!(
        !payload["coverage"]["statement"].as_str().unwrap_or_default().is_empty(),
        "the coverage limits ride the payload (NFR-CC-04): {body}"
    );

    // `?merge=` reaches the engine: against `main` — which carries neither
    // branch's edit — `f` is reported as work the merge result does not have.
    let resp = router
        .clone()
        .oneshot(get(&format!("{path}&merge=main")))
        .await
        .unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(payload["merge"]["ref"], "main", "{body}");
    assert!(
        payload["merge"]["lost_symbols"]
            .as_array()
            .expect("lost_symbols")
            .iter()
            .any(|row| row["name"] == "f"),
        "{body}"
    );

    // A ref that does not resolve reaches the wire as a coverage limit.
    let resp = router
        .clone()
        .oneshot(get("/api/v1/branch-overlap?ref=topic&ref=nope"))
        .await
        .unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "an unknown ref is not a 4xx");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(
        payload["coverage"]["unresolved_refs"],
        serde_json::json!(["nope"]),
        "{body}"
    );

    // An EMPTY optional is an absent optional — `base=`/`merge=` are the normal
    // output of an HTML form, and reading them as stated values collapsed the
    // whole answer behind advice to pass the very flag that caused it.
    let resp = router
        .clone()
        .oneshot(get(&format!("{path}&base=&merge=")))
        .await
        .unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(
        payload["base_origin"], "the common ancestor (git merge-base) of the supplied refs",
        "{body}"
    );
    assert!(!payload["contended"].as_array().unwrap().is_empty(), "{body}");
    assert!(payload["merge"].is_null(), "{body}");
    assert!(payload["warnings"].as_array().unwrap().is_empty(), "{body}");

    // No `ref` at all is the honest empty default the handler doc promises,
    // not a 4xx — the one shape only this surface and MCP can even ask for.
    let resp = router.oneshot(get("/api/v1/branch-overlap")).await.unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "a refless call is an honest 200");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(payload["refs"].as_array().unwrap().is_empty(), "{body}");
    assert!(
        payload["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap_or_default().contains("no refs supplied")),
        "the empty answer says why it is empty: {body}"
    );
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
    // (`activity_by_day`, `calls_by_origin`) alongside the pre-existing fields,
    // plus the [FR-OB-11] attribution projections. The pre-existing keys are
    // asserted alongside the new ones deliberately: the cross-tab is additive and
    // the tab must keep working across the change.
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
        "\"calls_by_tool_origin\"",
        "\"calls_by_class\"",
        "\"attribution_coverage\"",
    ] {
        assert!(body.contains(key), "statistics carries {key}: {body}");
    }
    // The coverage limits reach the HTTP surface, not just the core read-model —
    // [S-306] renders them from here ([NFR-CC-04]).
    assert!(
        body.contains("\"raw_events_only\":true")
            && body.contains("\"legacy_null_origin_folds_into_main\":true"),
        "the attribution projections state their own limits: {body}"
    );
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

// ── Acceptance: the effective-chat slice, single-root (S-448) ────────────────

/// Single-root `GET /api/v1/config` carries the effective-chat slice and reaches
/// **no** `workspace` origin ([FR-WS-30], S-448 AC2/AC4, [ADR-52]) — even with the
/// served root beneath a workspace manifest and a chat-declaring `.logos/` that a
/// *discovery* would find. The handler passes no workspace root, so its body is
/// byte-for-byte the core read-model with none.
#[tokio::test]
async fn config_endpoint_single_root_reaches_no_workspace_origin() {
    let tmp = TempDir::new().unwrap();
    let above = tmp.path();
    std::fs::write(above.join("logos.workspace.toml"), "[workspace]\nname = \"w\"\n").unwrap();
    std::fs::create_dir_all(above.join(".logos")).unwrap();
    std::fs::write(above.join(".logos/config.toml"), "[chat]\nmodel = \"above/model\"\n").unwrap();
    let above_key = "sk-above-the-root-ab12";
    std::fs::write(above.join(".logos/secrets.toml"), format!("[chat]\napi_key = \"{above_key}\"\n"))
        .unwrap();
    let served = above.join("served");
    std::fs::create_dir_all(&served).unwrap();
    let engine = Arc::new(Engine::open(&served));

    let resp = web::router(Arc::clone(&engine)).oneshot(get("/api/v1/config")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "config-read answers 200");

    assert!(body.contains("\"effective_chat\":{"), "the slice is served: {body}");
    assert!(body.contains("\"policy_origin\":\"unset\""), "nothing above was inherited: {body}");
    assert!(body.contains("\"credential_origin\":\"unset\""), "no key above was inherited: {body}");
    assert!(!body.contains("\"workspace\""), "a single-root payload names no workspace origin: {body}");
    assert!(!body.contains("above/model") && !body.contains(above_key), "{body}");
    let core = serde_json::to_string(&engine.config_read(None).expect("core read")).unwrap();
    assert_eq!(body, core, "the single-root handler serves the core read-model with no workspace root");
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

/// FR-SC-14: `/api/v1/files` carries the findings column the CLI and MCP carry —
/// absent before any scan (no zeros), the same cell after one — and
/// `?with_findings` is the filter. The `hotspots` value is compared whole with
/// the engine payload both other surfaces serialise.
#[tokio::test]
async fn files_carries_the_findings_column_only_once_a_scan_is_stored() {
    let (tmp, engine) = scanned_engine();
    let router = web::router(Arc::clone(&engine));
    let files = |uri: &'static str| {
        let router = router.clone();
        async move {
            let (status, body, _h) = body_string(router.oneshot(get(uri)).await.unwrap()).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
            serde_json::from_str::<serde_json::Value>(&body).expect("valid JSON")
        }
    };

    for uri in ["/api/v1/files", "/api/v1/files?with_findings"] {
        let unscanned = files(uri).await;
        assert!(unscanned["hotspots"]["files"][0].get("findings").is_none(), "no scan, no column: {unscanned}");
        assert!(unscanned["hotspots"].get("with_findings").is_none(), "{unscanned}");
    }

    let report = tmp.path().join(".logos/lib.sarif");
    std::fs::write(
        &report,
        r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Semgrep OSS","semanticVersion":"1.85.0","rules":[]}},
        "results":[{"ruleId":"r","level":"error","message":{"text":"m"},"locations":[{"physicalLocation":
        {"artifactLocation":{"uri":"src/lib.rs"},"region":{"startLine":1}}}]}]}]}"#,
    )
    .unwrap();
    engine.security_ingest(&report).expect("the report ingests");

    for (uri, with_findings) in [("/api/v1/files", false), ("/api/v1/files?with_findings", true)] {
        let body = files(uri).await;
        let cell = &body["hotspots"]["files"][0]["findings"];
        assert_eq!((cell["count"].as_u64(), cell["highest"].as_str(), cell["stale"].as_u64()), (Some(1), Some("high"), Some(0)), "{body}");
        let engine_payload = serde_json::to_value(
            engine.latest_hotspots_with_findings(Some(50), false, false, with_findings).unwrap(),
        )
        .unwrap();
        assert_eq!(body["hotspots"], engine_payload, "web carries the engine payload: {uri}");
        assert_eq!(body["hotspots"]["with_findings"].as_bool().unwrap_or(false), with_findings);
    }
}

// ── Populated-graph: the graph/search endpoints reach real read-model fields ──

/// Over an indexed graph the `/api/v1/graph` snapshot carries real nodes (a `layer`
/// field) and `/api/v1/search` returns ranked hits — proving the endpoints reach
/// the accessors end-to-end, not merely parse-then-drop ([FR-UI-21], [NFR-RA-05]).
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

// ── Health: the three offender states over the real payload (S-499) ───────────

/// The captured real-payload fixture the SPA's Health tests render (`HealthView`
/// and `healthModel` Vitest suites import it). This test is its generator **and**
/// its guard: it serves `GET /api/v1/health` for three real stores and compares the
/// `scan.worst_offenders` object each returns with the committed file, so the UI's
/// fixture can never drift from what the route serializes. Regenerate with
/// `LOGOS_UPDATE_FIXTURES=1 cargo test -p web --test api_v1`.
///
/// Deliberately **not** `#[cfg(feature = "lang-rust")]`: the test build always
/// carries the Rust grammar (the `[dev-dependencies]` logos-core enables it), and
/// a gated guard compiles out of `gate.sh` and CI, which never pass that
/// feature to `web`. The offender payload changes whenever a later story changes
/// an offender list (Sprint 85's S-502 already did), so this guard has to run
/// everywhere.
const OFFENDERS_FIXTURE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/ui/src/views/health/fixtures/worst-offenders.real.json");

/// Three production functions nested 4, 6 and 5 levels deep (all at or over the
/// default `T_nest = 4`), declared shallowest first so the persisted order
/// (deepest first) differs from declaration order and from name order.
const NESTED_SRC: &str = "\
pub fn alpha_depth_four(x: i32) -> i32 {
    if x > 0 { if x > 1 { if x > 2 { if x > 3 { return x; } } } }
    0
}

pub fn beta_depth_six(x: i32) -> i32 {
    if x > 0 { if x > 1 { if x > 2 { if x > 3 { if x > 4 { if x > 5 { return x; } } } } } }
    0
}

pub fn gamma_depth_five(x: i32) -> i32 {
    if x > 0 { if x > 1 { if x > 2 { if x > 3 { if x > 4 { return x; } } } } }
    0
}
";

/// `scan.worst_offenders` of the served `/api/v1/health` for one project, scanned
/// first when `scan` is set.
async fn served_worst_offenders(src: &str, scan: bool) -> serde_json::Value {
    let dir = TempDir::new().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
    std::fs::write(dir.path().join("src/lib.rs"), src).expect("write fixture");
    let engine = Arc::new(Engine::start(dir.path()).expect("engine starts"));
    engine.index();
    if scan {
        engine.scan(false).expect("scan persists a metric snapshot");
    }
    let resp = web::router(engine).oneshot(get("/api/v1/health")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "health answers 200");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("health is JSON");
    payload["scan"]["worst_offenders"].clone()
}

/// A scanned fixture serves `recorded: true` with its three nesting offenders in
/// persisted order; a scanned clean fixture serves `recorded: true` with every list
/// empty; a store never scanned serves `recorded: false`. The three are the states
/// the Health drill-downs render, and the committed fixture holds them verbatim.
#[tokio::test]
async fn health_payload_distinguishes_recorded_recorded_empty_and_not_recorded() {
    let recorded = served_worst_offenders(NESTED_SRC, true).await;
    let recorded_empty =
        served_worst_offenders("pub fn api() {\n    helper();\n}\nfn helper() {}\n", true).await;
    let not_recorded = served_worst_offenders(NESTED_SRC, false).await;

    // The recorded flag is present in every state (key order is not asserted).
    assert_eq!(recorded["recorded"], true);
    let names: Vec<&str> = recorded["nesting"]
        .as_array()
        .expect("nesting is an array")
        .iter()
        .map(|o| o["name"].as_str().expect("offender name"))
        .collect();
    assert_eq!(
        names,
        ["beta_depth_six", "gamma_depth_five", "alpha_depth_four"],
        "the persisted order is deepest first: {recorded}",
    );
    let first = &recorded["nesting"][0];
    assert_eq!(first["file"], "src/lib.rs");
    assert_eq!(first["detail"], "nesting depth 6");

    // CR-209: the four further lists ride the same payload, same row shape.
    assert_eq!(recorded["equality"][0]["name"], "beta_depth_six");
    assert_eq!(recorded["equality"][0]["detail"], "complexity 7");
    assert!(recorded.get("unrecorded").is_none(), "elided on a snapshot this version wrote");

    assert_eq!(recorded_empty["recorded"], true, "a clean scan is recorded-empty");
    assert_eq!(not_recorded["recorded"], false, "a never-scanned store is not recorded");
    for dimension in [
        "nesting",
        "conciseness",
        "cohesion",
        "focus",
        "uniqueness",
        "acyclicity",
        "depth",
        "equality",
        "redundancy",
    ] {
        assert_eq!(recorded_empty[dimension], serde_json::json!([]), "{dimension} recorded-empty");
        assert_eq!(not_recorded[dimension], serde_json::json!([]), "{dimension} not recorded");
    }

    let captured = serde_json::json!({
        "recorded": recorded,
        "recordedEmpty": recorded_empty,
        "notRecorded": not_recorded,
    });
    let rendered = format!("{}\n", serde_json::to_string_pretty(&captured).expect("serialize"));
    if std::env::var_os("LOGOS_UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(Path::new(OFFENDERS_FIXTURE).parent().unwrap()).expect("mkdir");
        std::fs::write(OFFENDERS_FIXTURE, &rendered).expect("write fixture");
    }
    // Compared as parsed values: key order is a `serde_json` feature-unification
    // detail (`preserve_order`), not part of the contract this guards.
    let committed: serde_json::Value = std::fs::read_to_string(OFFENDERS_FIXTURE)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null);
    assert_eq!(
        committed, captured,
        "the SPA's real-payload fixture drifted from /api/v1/health — regenerate with \
         LOGOS_UPDATE_FIXTURES=1",
    );
}

/// `GET /api/v1/code-refs?ref=…` resolves a finalized chat answer's backticked
/// spans against the graph in ONE batched read (S-622, [FR-UI-47]): an existing
/// file and a unique symbol resolve with a seed; an ambiguous symbol and a
/// nonexistent path come back without one, so the client links exactly the first
/// two ([NFR-CC-04]). Each returned seed is then shown to open a NON-EMPTY Graph
/// snapshot through the very accessor the `/graph?seed=` link calls — a link the
/// Graph view could not seed would be a dead link, which is the fabrication this
/// endpoint exists to prevent.
///
/// [FR-UI-47]: ../../docs/specs/requirements/FR-UI-47.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
#[tokio::test]
async fn code_refs_link_an_existing_file_and_a_unique_symbol_and_nothing_else() {
    let (_tmp, engine) = precedent_engine();
    let router = web::router(engine);

    let path = "/api/v1/code-refs?ref=src/registry.rs:3&ref=register_all()&ref=extract&ref=src/missing.rs";
    let resp = router.clone().oneshot(get(path)).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_json_self_only_csp(&headers, path);
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");
    let refs = payload["refs"].as_array().expect("refs");
    let facts: Vec<(&str, &str, Option<&str>)> = refs
        .iter()
        .map(|r| (r["text"].as_str().unwrap(), r["outcome"].as_str().unwrap(), r["seed"].as_str()))
        .collect();
    assert_eq!(facts.len(), 4, "{body}");
    assert_eq!(facts[0], ("src/registry.rs:3", "file", Some("src/registry.rs")), "{body}");
    assert_eq!((facts[1].0, facts[1].1), ("register_all()", "symbol"), "{body}");
    let symbol_seed = facts[1].2.expect("a unique symbol carries its seed");
    assert!(symbol_seed.ends_with("register_all()."), "the seed is the canonical symbol: {body}");
    assert_eq!(facts[2], ("extract", "ambiguous", None), "two `extract`s, so no seed: {body}");
    assert_eq!(facts[3], ("src/missing.rs", "unresolved", None), "{body}");
    assert_eq!(payload["over_cap"], 0, "{body}");

    // The two seeds open a seeded Graph view: the symbol at the default tier, the
    // file at the file tier — exactly the two URLs `codeRefHref` builds.
    for graph in [
        format!("/api/v1/graph?seed={}", urlencode(symbol_seed)),
        "/api/v1/graph?seed=src/registry.rs&granularity=file".to_string(),
    ] {
        let resp = router.clone().oneshot(get(&graph)).await.unwrap();
        let (status, body, _headers) = body_string(resp).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let elements: serde_json::Value = serde_json::from_str(&body).expect("json");
        assert!(
            !elements["nodes"].as_array().expect("nodes").is_empty(),
            "{graph} opens a seeded, non-empty graph: {body}"
        );
    }
}

/// The candidate cap ([FR-UI-47]): spans are de-duplicated in arrival order and
/// at most `CODE_REF_CAP` (24) are resolved; the excess is counted, not looked
/// up, so a hand-built URL cannot buy unbounded lookups. No `ref` at all is an
/// honest empty `200`, never a `4xx` ([NFR-CC-04]).
#[tokio::test]
async fn code_refs_dedupe_and_cap_the_candidates_and_answer_an_empty_request() {
    let (_tmp, engine) = precedent_engine();
    let router = web::router(engine);

    let mut query: Vec<String> = (0..30).map(|i| format!("ref=name_{i}")).collect();
    query.insert(1, "ref=name_0".to_string());
    let path = format!("/api/v1/code-refs?{}", query.join("&"));
    let resp = router.clone().oneshot(get(&path)).await.unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");
    let refs = payload["refs"].as_array().expect("refs");
    assert_eq!(payload["cap"], 24, "{body}");
    assert_eq!(refs.len(), 24, "the cap bounds the lookups: {body}");
    assert_eq!(payload["over_cap"], 6, "30 distinct spans, 24 resolved: {body}");
    assert_eq!(refs[0]["text"], "name_0", "{body}");
    assert_eq!(refs[1]["text"], "name_1", "the duplicate `name_0` is resolved once: {body}");

    let resp = router.oneshot(get("/api/v1/code-refs")).await.unwrap();
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "no ref is not a 4xx: {body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(payload["refs"].as_array().unwrap().is_empty(), "{body}");
}

/// Percent-encode a query value (the canonical symbol carries spaces and
/// backticks).
fn urlencode(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// ── Security (S-670, [FR-SC-09]) ──────────────────────────────────────────────

/// Planted in the secret-class finding's message and snippet; must reach no
/// payload ([FR-SC-03]).
const PLANTED_SECRET: &str = "lgs_pl4nted_S3cr3t_web_q7Vx9Rz2Kw";

/// `src/lib.rs` for the security fixture: `find` encloses the SQL finding at
/// line 3, and `KEY` the secret at line 6.
const SECURITY_LIB_RS: &str = "/// Looks a user up.\npub fn find(id: &str) -> String {\n    format!(\"SELECT * FROM users WHERE id = {id}\")\n}\n\npub const KEY: &str = \"redacted-in-the-fixture\";\n";

/// A Semgrep report over [`SECURITY_LIB_RS`]: a CWE-89 finding (Injection under
/// both editions, at a different id in each) and a secret-class one, whose
/// message and snippet carry [`PLANTED_SECRET`].
fn security_sarif() -> String {
    format!(
        r#"{{"version":"2.1.0","runs":[{{"tool":{{"driver":{{"name":"Semgrep OSS","semanticVersion":"1.85.0","rules":[
{{"id":"rust.lang.security.formatted-sql","properties":{{"tags":["CWE-89: SQL Injection","security"]}}}},
{{"id":"generic.secrets.hardcoded-key","properties":{{"tags":["CWE-798: Hard-coded Credentials","security"]}}}}]}}}},
"results":[
{{"ruleId":"rust.lang.security.formatted-sql","level":"error","message":{{"text":"Formatted SQL query."}},
"locations":[{{"physicalLocation":{{"artifactLocation":{{"uri":"src/lib.rs"}},"region":{{"startLine":3,"snippet":{{"text":"format!(SELECT)"}}}}}}}}]}},
{{"ruleId":"generic.secrets.hardcoded-key","level":"error","message":{{"text":"Hard-coded key {PLANTED_SECRET}"}},
"locations":[{{"physicalLocation":{{"artifactLocation":{{"uri":"src/lib.rs"}},"region":{{"startLine":6,"snippet":{{"text":"{PLANTED_SECRET}"}}}}}}}}]}}]}}]}}"#
    )
}

/// A Rust repository with one Python script — so Bandit has a file to scan
/// (S-769) — and the report ingested, whose engine finds scanners only in a
/// stub directory holding `bandit` — installed, never run —
/// and consent in a temporary user directory, so no scanner on this machine is
/// found and the process environment is never changed.
#[cfg(unix)]
fn security_engine(rules_toml: Option<&str>) -> (TempDir, Arc<Engine>) {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().expect("temp root");
    let base = tmp.path().canonicalize().unwrap();
    let (repo, bin, user) = (base.join("project"), base.join("bin"), base.join("user"));
    for dir in [&repo, &bin, &user] {
        std::fs::create_dir_all(dir).unwrap();
    }
    sh_git(&repo, &["init", "-q", "-b", "main"]);
    if let Some(rules_toml) = rules_toml {
        std::fs::create_dir_all(repo.join(".logos")).unwrap();
        std::fs::write(repo.join(".logos/rules.toml"), rules_toml).unwrap();
    }
    commit(&repo, "src/lib.rs", SECURITY_LIB_RS, "seed");
    commit(&repo, "scripts/gen.py", "print(1)\n", "script");
    let stub = bin.join("bandit");
    std::fs::write(&stub, "#!/bin/sh\nprintf '%s\\n' 'bandit 1.7.10'\n").unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let report = base.join("semgrep.sarif");
    std::fs::write(&report, security_sarif()).unwrap();

    let env = logos_core::security::ScanEnvironment {
        user_dir: Some(user),
        search_path: Some(bin.into_os_string()),
        tools_dir: None,
    };
    let engine = Engine::start(&repo).expect("engine starts").with_scan_environment(env);
    engine.index();
    engine.security_ingest(&report).expect("the report ingests");
    (tmp, Arc::new(engine))
}

async fn security_payload(router: axum::Router) -> (String, serde_json::Value) {
    let (status, body, headers) = body_string(router.oneshot(get("/api/v1/security")).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK, "/api/v1/security answers 200: {body}");
    assert_json_self_only_csp(&headers, "/api/v1/security");
    let payload = serde_json::from_str(&body).expect("json");
    (body, payload)
}

/// The one scanner row named `name` in the status matrix.
fn scanner_row<'a>(page: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    page["status"]["scanners"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["scanner"] == name)
        .unwrap_or_else(|| panic!("a status row for {name}"))
}

/// `GET /api/v1/security` ([FR-SC-09]) over a fixture with findings, a
/// never-scanned scanner and an uncovered category: each absent state is in the
/// payload with what fixes it, every finding carries its graph context, both
/// OWASP editions classify the same findings, no snippet and no secret is
/// served, and the read writes nothing.
#[cfg(unix)]
#[tokio::test]
async fn security_page_states_findings_absent_scanners_and_uncovered_categories() {
    let (tmp, engine) = security_engine(None);
    let repo = tmp.path().canonicalize().unwrap().join("project");
    let router = web::router(engine);
    let history = repo.join(".logos/history.db");
    let before = snapshot_count(&history, "security_snapshots");
    let (body, payload) = security_payload(router.clone()).await;
    let page = &payload["security"];

    // The scanner matrix: one stored report, one installed scanner that never
    // ran, and a registered scanner that is not installed here.
    assert_eq!(scanner_row(page, "semgrep")["state"], "fresh");
    assert_eq!(scanner_row(page, "semgrep")["findings"], 2);
    assert_eq!(scanner_row(page, "bandit")["state"], "never_run");
    assert_eq!(scanner_row(page, "trivy")["state"], "not_installed");
    // FR-SC-26: the installed scanner's row states its coverage of the ignored
    // directories (the fixture ignores none); one not installed here states none.
    assert_eq!(scanner_row(page, "bandit")["ignored_paths"], "excluded");
    assert!(scanner_row(page, "trivy").get("ignored_paths").is_none());
    assert_eq!(page["findings"]["never_run"], serde_json::json!(["bandit"]));

    // Every stored finding, unfiltered, each in graph context (S-669).
    let findings = page["findings"]["findings"].as_array().unwrap();
    assert_eq!((page["findings"]["total"].as_u64(), findings.len()), (Some(2), 2));
    // The page's own count: both findings sit in files nothing touched, so both
    // count, and none is left out (S-760).
    assert_eq!(page["counts"]["counted"]["total"], 2, "{}", page["counts"]);
    assert_eq!(page["counts"]["excluded"], serde_json::json!({"not_fresh": 0, "suppressed": 0}));
    let sql = findings.iter().find(|f| f["rule_id"] == "rust.lang.security.formatted-sql").unwrap();
    assert_eq!(sql["context"]["enclosing"]["symbol"]["name"], "find");
    assert_eq!(sql["context"]["enclosing"]["confidence"], "resolved");
    // No route exists, so nothing can be ruled out: unknown, never unreachable.
    assert_eq!(sql["context"]["reachability"]["verdict"], "unknown");

    // Redaction: the secret-class message is the stored fixed text, and neither
    // the planted secret nor any snippet is served.
    let secret = findings.iter().find(|f| f["rule_id"] == "generic.secrets.hardcoded-key").unwrap();
    assert_eq!(secret["redacted"], true);
    assert!(!body.contains(PLANTED_SECRET), "the planted secret is served: {body}");
    assert!(!body.contains("snippet"), "a snippet is served: {body}");
    assert!(!body.contains("format!(SELECT)"), "a snippet's text is served: {body}");

    // Both editions over the same findings: the CWE-89 finding is Injection in
    // each, under each edition's own id.
    let editions = page["editions"].as_array().unwrap();
    let years: Vec<&str> = editions.iter().map(|e| e["edition"].as_str().unwrap()).collect();
    assert_eq!(years, ["2025", "2021"]);
    let category_of = |edition: &serde_json::Value| {
        edition["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["fingerprint"] == sql["fingerprint"])
            .map(|c| c["categories"].clone())
            .unwrap()
    };
    assert_eq!(category_of(&editions[0]), serde_json::json!(["A05:2025"]));
    assert_eq!(category_of(&editions[1]), serde_json::json!(["A03:2021"]));
    assert_eq!(editions[0]["findings"].as_array().unwrap().len(), 2, "every finding is classified");

    // Not covered: only Semgrep's report is stored and Bandit never ran, so the
    // supply-chain category no stored scanner can detect reads not covered —
    // never a count of zero.
    let supply_chain = editions[0]["categories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "A03:2025")
        .unwrap();
    assert_eq!(supply_chain["state"]["state"], "not_covered");
    assert!(supply_chain["state"].get("findings").is_none(), "not covered carries no count: {supply_chain}");

    // Without a `[security]` table there is no gate band.
    assert!(page["gate"].is_null(), "no table, no band: {}", page["gate"]);

    // Read-only: a second load stores nothing, and the route is GET-only.
    security_payload(router.clone()).await;
    assert_eq!(snapshot_count(&history, "security_snapshots"), before, "a GET stores no scan");
    let post = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/security")
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .unwrap();
    assert_eq!(router.oneshot(post).await.unwrap().status(), StatusCode::METHOD_NOT_ALLOWED);
}

/// With a `rules.toml [security]` table, the band is the verdict `check`
/// reaches on it ([FR-SC-10]). The fixture's findings are `high`, so a ratchet
/// at `critical` fails on none of them: a required scanner that never ran is
/// then the one reason the band fails, and a fresh one passes it.
#[cfg(unix)]
#[tokio::test]
async fn security_page_carries_the_gate_band_when_a_security_table_is_configured() {
    let table = |required: &str| format!("[security]\nrequired_scanners = [\"{required}\"]\nratchet_min_severity = \"critical\"\n");

    let (_tmp, engine) = security_engine(Some(&table("bandit")));
    let (_body, payload) = security_payload(web::router(engine)).await;
    let band = &payload["security"]["gate"];
    assert_eq!(band["violations"], serde_json::json!([]), "no rule fails on a high finding: {band}");
    assert_eq!(band["passed"], false, "a missing required scan alone fails the band: {band}");
    assert_eq!(band["required_scanners"], serde_json::json!(["bandit"]));
    assert_eq!(band["missing_scans"][0]["scanner"], "bandit");
    assert_eq!(band["missing_scans"][0]["state"], "never_run");
    assert_eq!(band["checked_rules"], 1, "the ratchet, and no cap: {band}");

    let (_tmp, engine) = security_engine(Some(&table("semgrep")));
    let (_body, payload) = security_payload(web::router(engine)).await;
    let band = &payload["security"]["gate"];
    assert_eq!((band["passed"].as_bool(), band["missing_scans"].as_array().map(Vec::len)), (Some(true), Some(0)), "{band}");
}

/// A same-origin `POST` carrying `intent` and a JSON `body` — what the
/// suppression dialog sends (S-672).
fn triage_post(path: &str, intent: &web::IntentToken, body: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::HOST, "127.0.0.1:4983")
        .header(header::ORIGIN, "http://127.0.0.1:4983")
        .header(header::CONTENT_TYPE, "application/json")
        .header(web::INTENT_HEADER, intent.as_str())
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Triage from the Security page ([FR-SC-12], S-672): a suppression posted with
/// a reason is written to `.logos/security-ignore.toml` and the page then shows
/// the finding suppressed **with that reason**; removing it restores the file
/// byte for byte. A blank reason is `422` and a malformed body `400`, each with
/// no write.
///
/// [FR-SC-12]: ../../docs/specs/requirements/FR-SC-12.md
#[cfg(unix)]
#[tokio::test]
async fn a_suppression_from_the_page_shows_its_reason_and_removing_it_restores_the_file() {
    let (tmp, engine) = security_engine(None);
    let repo = tmp.path().canonicalize().unwrap().join("project");
    let intent = web::IntentToken::generate();
    let router = web::router_with_intent(engine, intent.clone());
    let (_, payload) = security_payload(router.clone()).await;
    let sql = payload["security"]["findings"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["rule_id"] == "rust.lang.security.formatted-sql")
        .unwrap()
        .clone();
    let fingerprint = sql["fingerprint"].as_str().unwrap().to_string();
    assert!(sql["suppression"].is_null(), "unsuppressed before triage: {sql}");

    // A committed file already holding an unrelated entry, in canonical spelling.
    let ignore = repo.join(".logos/security-ignore.toml");
    let committed = format!("[[ignore]]\nfingerprint = \"{}\"\nreason = \"kept by hand\"\n", "f".repeat(64));
    std::fs::write(&ignore, &committed).unwrap();

    // Refusals write nothing.
    for (body, status) in [
        (format!(r#"{{"fingerprint":"{fingerprint}","reason":"   "}}"#), StatusCode::UNPROCESSABLE_ENTITY),
        (format!(r#"{{"fingerprint":"{fingerprint}"}}"#), StatusCode::BAD_REQUEST),
        (format!(r#"{{"fingerprint":"{fingerprint}","reason":"r","expires":"next week"}}"#), StatusCode::UNPROCESSABLE_ENTITY),
        // An unknown key is refused, never ignored.
        (format!(r#"{{"fingerprint":"{fingerprint}","reason":"r","scope":"all"}}"#), StatusCode::BAD_REQUEST),
    ] {
        let resp = router.clone().oneshot(triage_post("/api/v1/security/suppress", &intent, &body)).await.unwrap();
        let (got, text, _) = body_string(resp).await;
        assert_eq!(got, status, "{body}: {text}");
        assert!(text.contains("\"error\""), "the refusal says why: {text}");
        assert_eq!(std::fs::read_to_string(&ignore).unwrap(), committed, "{body} wrote nothing");
    }

    for body in [format!(r#"{{"fp":"{fingerprint}"}}"#), format!(r#"{{"fingerprint":"{fingerprint}","reason":"r"}}"#)] {
        let resp = router.clone().oneshot(triage_post("/api/v1/security/unsuppress", &intent, &body)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "unsuppress {body}");
        assert_eq!(std::fs::read_to_string(&ignore).unwrap(), committed, "unsuppress {body} wrote nothing");
    }

    let body = format!(r#"{{"fingerprint":"{fingerprint}","reason":"query is built from a constant","expires":"2999-12-31"}}"#);
    let resp = router.clone().oneshot(triage_post("/api/v1/security/suppress", &intent, &body)).await.unwrap();
    let (status, text, _) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let written: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(written["file"], ".logos/security-ignore.toml");
    assert_eq!(written["entries"], 2);
    assert_eq!(written["entry"]["reason"], "query is built from a constant");

    // The page labels the finding with the reason it was suppressed for.
    let (_, payload) = security_payload(router.clone()).await;
    let labelled = payload["security"]["findings"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["fingerprint"] == fingerprint.as_str())
        .unwrap()
        .clone();
    assert_eq!(
        labelled["suppression"],
        serde_json::json!({
            "state": "suppressed",
            "reason": "query is built from a constant",
            "expires": "2999-12-31",
            "by": {"fingerprint": fingerprint}
        })
    );

    let body = format!(r#"{{"fingerprint":"{fingerprint}"}}"#);
    let resp = router.clone().oneshot(triage_post("/api/v1/security/unsuppress", &intent, &body)).await.unwrap();
    let (status, text, _) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(std::fs::read(&ignore).unwrap(), committed.as_bytes(), "add-then-remove restores the file byte for byte");

    // Removing it again: nothing suppresses it, so it is refused.
    let resp = router.clone().oneshot(triage_post("/api/v1/security/unsuppress", &intent, &body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let (_, payload) = security_payload(router).await;
    let back = payload["security"]["findings"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["fingerprint"] == fingerprint.as_str())
        .unwrap()
        .clone();
    assert!(back["suppression"].is_null(), "the finding counts again: {back}");
}

/// "Everything under this path" from the Security page (S-770): a `path` entry
/// is written in its canonical spelling, labels every finding under it with
/// its selector, is listed with its count, and removing it restores the file
/// byte for byte. Both selectors, neither, or a whole-project glob is `422`
/// with no write.
#[cfg(unix)]
#[tokio::test]
async fn a_path_suppression_from_the_page_covers_the_directory_and_removing_it_restores_the_file() {
    let (tmp, engine) = security_engine(None);
    let repo = tmp.path().canonicalize().unwrap().join("project");
    let intent = web::IntentToken::generate();
    let router = web::router_with_intent(engine, intent.clone());
    let (_, payload) = security_payload(router.clone()).await;
    let sql = payload["security"]["findings"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["rule_id"] == "rust.lang.security.formatted-sql")
        .unwrap()
        .clone();
    let fingerprint = sql["fingerprint"].as_str().unwrap().to_string();
    let file = sql["path"].as_str().expect("the finding is bound to a file").to_string();
    let dir = file.rsplit_once('/').map_or(file.as_str(), |(dir, _)| dir).to_string();

    let ignore = repo.join(".logos/security-ignore.toml");
    let committed = format!("[[ignore]]\nfingerprint = \"{}\"\nreason = \"kept by hand\"\n", "f".repeat(64));
    std::fs::write(&ignore, &committed).unwrap();

    for body in [
        format!(r#"{{"fingerprint":"{fingerprint}","path":"{dir}","reason":"r"}}"#),
        r#"{"reason":"r"}"#.to_string(),
        r#"{"path":"**","reason":"r"}"#.to_string(),
        format!(r#"{{"fingerprint":"{fingerprint}","rule":"x","reason":"r"}}"#),
    ] {
        let resp = router.clone().oneshot(triage_post("/api/v1/security/suppress", &intent, &body)).await.unwrap();
        let (got, text, _) = body_string(resp).await;
        assert_eq!(got, StatusCode::UNPROCESSABLE_ENTITY, "{body}: {text}");
        assert_eq!(std::fs::read_to_string(&ignore).unwrap(), committed, "{body} wrote nothing");
    }

    let body = format!(r#"{{"path":"./{dir}/","reason":"generated code","expires":null}}"#);
    let resp = router.clone().oneshot(triage_post("/api/v1/security/suppress", &intent, &body)).await.unwrap();
    let (status, text, _) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let written: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(written["entry"]["path"], dir.as_str(), "written in the canonical spelling");
    assert!(
        std::fs::read_to_string(&ignore).unwrap().ends_with(&format!("[[ignore]]\npath = \"{dir}\"\nreason = \"generated code\"\n")),
        "the path entry follows the fingerprint entries"
    );

    let (_, payload) = security_payload(router.clone()).await;
    let findings = &payload["security"]["findings"];
    let under: Vec<&serde_json::Value> = findings["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["path"].as_str().is_some_and(|p| p.starts_with(&format!("{dir}/"))))
        .collect();
    assert!(!under.is_empty());
    for f in &under {
        assert_eq!(f["suppression"]["state"], "suppressed", "{f}");
        assert_eq!(f["suppression"]["by"], serde_json::json!({"path": dir}), "{f}");
    }
    let entry = findings["suppressions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["entry"]["path"] == dir.as_str())
        .unwrap()
        .clone();
    assert_eq!(entry["suppressed"], under.len(), "the entry counts what it suppresses: {entry}");
    assert_eq!(entry["orphaned"], false);
    assert_eq!(payload["security"]["status"]["suppressions"], findings["suppressions"], "status lists the same entries");

    // Unsuppress sends the selector the finding's label carries.
    let by = under[0]["suppression"]["by"].to_string();
    let resp = router.clone().oneshot(triage_post("/api/v1/security/unsuppress", &intent, &by)).await.unwrap();
    let (status, text, _) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(std::fs::read(&ignore).unwrap(), committed.as_bytes(), "add-then-remove restores the file byte for byte");
}
