//! The context-aware `serve --ui` web surface (S-249, [FR-WS-06], [ADR-52]):
//! the `/api/v1/workspace/*` cross-service fan-out, the single-root regression
//! (byte-identical when no manifest), the `--standalone` escape hatch, and the
//! warm-only-the-default-member startup policy ([NFR-PE-10]).
//!
//! Drives the **real** router in-process (`tower::ServiceExt::oneshot`, no socket)
//! over a two-member workspace fixture, exactly as the workspace SPA (S-250) will
//! consume it. The structural contract (workspace-mode `200`s, single-root `404`s,
//! `--standalone`, CSP, warm policy) needs no grammar; the cross-service *edge*
//! assertions (a resolved OpenAPI→axum route binding) are gated on `lang-all`.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt;
use logos_core::federation::{discover, Backing, EngineRegistry};
use logos_core::Engine;
use tempfile::TempDir;
use tower::ServiceExt;

// ── Fixtures ──────────────────────────────────────────────────────────────────

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

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write fixture");
}

/// A committed git repo — `discover` keeps only members that are distinct git
/// roots (FR-WS-01), so each member must be its own repository.
fn init_repo(dir: &Path, rel: &str, contents: &str) {
    std::fs::create_dir_all(dir).unwrap();
    sh_git(dir, &["init", "-q", "-b", "main"]);
    write(dir, rel, contents);
    sh_git(dir, &["add", "."]);
    sh_git(dir, &["commit", "-q", "-m", "init"]);
}

/// An OpenAPI spec whose `/users/{user_id}` `get` matches the axum `/users/{id}`
/// route (the `route_key` param-drift erasure) — one bound cross-service edge.
const OPENAPI_YAML: &str = "\
openapi: 3.0.3
info:
  title: User API
  version: 1.0.0
paths:
  /users/{user_id}:
    get:
      summary: Get a user
";

/// An axum app registering exactly one route, `GET /users/{id}`.
const AXUM_MAIN: &str = r#"
use axum::routing::get;
use axum::Router;
async fn get_user() {}
fn app() -> Router { Router::new().route("/users/{id}", get(get_user)) }
"#;

/// Build a two-member workspace: `api` (OpenAPI consumer) + `web` (axum provider),
/// each an indexed git repo, with the manifest at the parent naming `api` default.
fn workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    init_repo(&root.join("api"), "api/openapi.yaml", OPENAPI_YAML);
    init_repo(&root.join("web"), "src/main.rs", AXUM_MAIN);
    // Index each member so `workspace status` reports index freshness.
    Engine::start(root.join("api")).expect("api engine").index();
    Engine::start(root.join("web")).expect("web engine").index();
    std::fs::write(
        root.join("logos.workspace.toml"),
        "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"web\"]\ndefault = \"api\"\n",
    )
    .unwrap();
    tmp
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .unwrap()
}

async fn body_string(resp: Response<Body>) -> (StatusCode, String, axum::http::HeaderMap) {
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap(), headers)
}

/// The exact self-only CSP the surface stamps on every response — pinned
/// byte-for-byte (mirrors `api_v1.rs`'s `EXPECTED_CSP`) so a drift in *any*
/// directive on the workspace surface is caught, not just the `default-src`.
const EXPECTED_CSP: &str = "default-src 'self'; base-uri 'none'; form-action 'none'; \
                            frame-ancestors 'none'; object-src 'none'";

/// The self-only CSP must stay byte-identical on the workspace surface too.
fn assert_self_only_csp(headers: &axum::http::HeaderMap, path: &str) {
    let csp = headers
        .get(header::CONTENT_SECURITY_POLICY)
        .expect("every response carries a CSP")
        .to_str()
        .unwrap();
    assert_eq!(csp, EXPECTED_CSP, "{path} carries the byte-identical self-only CSP");
}

/// The full workspace read endpoint set the SPA fetches.
const WORKSPACE_ENDPOINTS: &[&str] = &[
    "/api/v1/workspace/status",
    "/api/v1/workspace/route-providers",
    "/api/v1/workspace/search?q=user",
    "/api/v1/workspace/callers?symbol=get_user",
    "/api/v1/workspace/impact?symbol=get_user",
];

fn ws_router(tmp: &TempDir) -> axum::Router {
    let federation = discover(tmp.path()).expect("discovery succeeds").expect("a workspace");
    let registry = EngineRegistry::<Engine>::new_serve_default(federation);
    web::workspace_router(registry).expect("the workspace router builds")
}

// ── AC1: workspace mode serves the fan-out; plain repo is unchanged ──────────

/// At a workspace parent the API serves workspace mode with the default member:
/// every `/api/v1/workspace/*` endpoint answers `200 application/json` under the
/// byte-identical self-only CSP ([FR-WS-06] AC1).
#[tokio::test]
async fn workspace_mode_serves_every_endpoint_under_self_only_csp() {
    let tmp = workspace();
    let router = ws_router(&tmp);
    for path in WORKSPACE_ENDPOINTS {
        let resp = router.clone().oneshot(get(path)).await.expect("route responds");
        let (status, body, headers) = body_string(resp).await;
        assert_eq!(status, StatusCode::OK, "{path} answers 200: {body}");
        assert_eq!(headers.get(header::CONTENT_TYPE).unwrap(), "application/json", "{path} is JSON");
        assert_self_only_csp(&headers, path);
    }
}

/// `workspace status` carries the workspace name and both repo-qualified members
/// with their index freshness — the coverage dashboard's data ([FR-WS-06] AC2).
#[tokio::test]
async fn workspace_status_reports_name_members_and_coverage() {
    let tmp = workspace();
    let router = ws_router(&tmp);
    let resp = router.oneshot(get("/api/v1/workspace/status")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["workspace"], "shop");
    let mut names: Vec<&str> =
        v["members"].as_array().unwrap().iter().map(|m| m["member"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(names, ["api", "web"], "both members are repo-qualified: {body}");
    // The 3-state coverage summary is always present (advisory tier, S-247).
    assert!(v["coverage"].get("bound_ratio").is_some(), "coverage summary present: {body}");
}

/// The cross-service read-models (service map, impact) are exposed to the frontend
/// ([FR-WS-06] AC2): every fan-out payload is repo-qualified, and impact carries
/// its seed + cross-service tiers.
#[tokio::test]
async fn workspace_impact_exposes_seed_and_cross_service_tiers() {
    let tmp = workspace();
    let router = ws_router(&tmp);
    let resp = router.oneshot(get("/api/v1/workspace/impact?symbol=get_user")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["seed"].is_array(), "impact carries the per-member seed tier: {body}");
    assert!(v["cross_service"].is_array(), "impact carries the cross-service tier: {body}");
}

/// Each parametrised workspace handler rejects a missing/empty required query
/// param with `400` before any fan-out (mirrors the single-root `search`/`node`
/// contract) — the error branch the happy-path loop never exercises.
#[tokio::test]
async fn workspace_handlers_require_their_query_param() {
    let tmp = workspace();
    let router = ws_router(&tmp);
    // `search` needs `q`; `callers`/`impact` need `symbol`. Empty counts as missing.
    for path in [
        "/api/v1/workspace/search",
        "/api/v1/workspace/search?q=",
        "/api/v1/workspace/callers",
        "/api/v1/workspace/callers?symbol=",
        "/api/v1/workspace/impact",
        "/api/v1/workspace/impact?symbol=%20",
    ] {
        let resp = router.clone().oneshot(get(path)).await.expect("route responds");
        let (status, body, _h) = body_string(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path} is 400 without its required param: {body}");
        assert!(body.contains("query parameter is required"), "{path} explains the missing param: {body}");
    }
}

/// `?repo=<member>` scopes the fan-out to that one member — asserted on `search`,
/// whose member set narrows without needing any cross-service edge (grammar-free):
/// unscoped fans over both members, `?repo=api` returns only `api`, and an unknown
/// repo surfaces as a single degraded per-member `error` (never a panic or a leak).
#[tokio::test]
async fn workspace_search_repo_scopes_the_fan_out() {
    let tmp = workspace();
    let router = ws_router(&tmp);

    let unscoped = router.clone().oneshot(get("/api/v1/workspace/search?q=user")).await.unwrap();
    let (_s, body, _h) = body_string(unscoped).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v.get("scope").is_none(), "no scope key when unscoped: {body}");
    let members: Vec<&str> =
        v["members"].as_array().unwrap().iter().map(|m| m["member"].as_str().unwrap()).collect();
    assert_eq!(members.len(), 2, "unscoped search fans over both members: {body}");

    let scoped = router.clone().oneshot(get("/api/v1/workspace/search?q=user&repo=api")).await.unwrap();
    let (_s, body, _h) = body_string(scoped).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["scope"], "api", "the applied scope is echoed: {body}");
    let members = v["members"].as_array().unwrap();
    assert_eq!(members.len(), 1, "scoped search fans over one member: {body}");
    assert_eq!(members[0]["member"], "api", "repo-qualified to the scoped member");

    // An unknown repo degrades to one per-member `error`, HTTP 200 (S-248 contract).
    let unknown = router.oneshot(get("/api/v1/workspace/search?q=user&repo=nope")).await.unwrap();
    let (status, body, _h) = body_string(unknown).await;
    assert_eq!(status, StatusCode::OK, "an unknown repo is not an HTTP error: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let members = v["members"].as_array().unwrap();
    assert_eq!(members.len(), 1);
    assert!(members[0]["error"].as_str().unwrap_or("").contains("no such workspace member"),
        "unknown repo surfaces as a degraded per-member error: {body}");
}

/// A plain repo with no manifest resolves `Backing::Single` through
/// `resolve_serve_backing` directly (the no-manifest branch), and a malformed
/// manifest makes it fail loud — the discovery decision, independent of the socket.
#[test]
fn resolve_serve_backing_single_for_a_plain_repo_and_fails_loud_on_a_bad_manifest() {
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path(), "src/lib.rs", "pub fn f() {}\n");
    let single = web::resolve_serve_backing(tmp.path(), false).expect("plain repo resolves");
    assert!(!single.is_federated(), "no manifest → single-root backing");
    assert!(single.as_single().is_some(), "the single-root engine is used");

    // A malformed manifest fails loud rather than silently degrading to single-root.
    std::fs::write(tmp.path().join("logos.workspace.toml"), "[workspace]\nname = \n").unwrap();
    assert!(
        web::resolve_serve_backing(tmp.path(), false).is_err(),
        "a malformed workspace manifest fails discovery loud"
    );
}

/// The cross-service *edge* is actually resolved (`lang-all`: OpenAPI + axum
/// grammars present): `route-providers` reports the one `api`→`web` binding, and
/// `--repo`/`?repo=` scopes it to the providing member ([FR-WS-06] AC2, service map).
#[cfg(feature = "lang-all")]
#[tokio::test]
async fn workspace_route_providers_report_the_resolved_binding() {
    let tmp = workspace();
    let router = ws_router(&tmp);

    let resp = router.clone().oneshot(get("/api/v1/workspace/route-providers")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let providers = v["providers"].as_array().expect("providers array");
    assert_eq!(providers.len(), 1, "one resolved cross-service route binding: {body}");
    assert_eq!(providers[0]["from"]["member"], "api", "consumer endpoint repo-qualified");
    assert_eq!(providers[0]["to"]["member"], "web", "provider endpoint repo-qualified");

    // `?repo=web` scopes to routes web provides → the one edge; `?repo=api` → none.
    let scoped = router.clone().oneshot(get("/api/v1/workspace/route-providers?repo=web")).await.unwrap();
    let (_s, body, _h) = body_string(scoped).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["scope"], "web");
    assert_eq!(v["providers"].as_array().unwrap().len(), 1);
    let scoped_api = router.oneshot(get("/api/v1/workspace/route-providers?repo=api")).await.unwrap();
    let (_s, body, _h) = body_string(scoped_api).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["providers"].as_array().unwrap().is_empty(), "api provides no routes: {body}");
}

// ── Single-root regression: the workspace surface is inert in a plain repo ────

/// In a plain single-root serve the `/api/v1/workspace/*` surface answers an honest
/// `404` (this is not a workspace) — the existing `/api/v1/*` responses are wholly
/// untouched ([FR-WS-06] AC1 byte-identity). The `404` still carries the self-only
/// CSP (the outer layer stamps every response).
#[tokio::test]
async fn single_root_workspace_endpoints_are_404() {
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path(), "src/lib.rs", "pub fn f() {}\n");
    let engine = Arc::new(Engine::start(tmp.path()).expect("engine starts"));
    let router = web::router(engine); // single-root router — no registry allocated

    for path in WORKSPACE_ENDPOINTS {
        let resp = router.clone().oneshot(get(path)).await.expect("route responds");
        let (status, body, headers) = body_string(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path} is 404 in single-root mode: {body}");
        assert!(body.contains("not a workspace"), "{path} explains why: {body}");
        assert_self_only_csp(&headers, path);
    }
}

// ── AC3: --standalone forces single-repo focus even under a manifest ─────────

/// `--standalone` forces the single-root focus even at a workspace parent: the
/// resolved backing is `Single`, never `Federated`, so discovery is bypassed
/// entirely ([FR-WS-06] AC3). Without it, the same parent resolves `Federated`.
#[test]
fn standalone_forces_single_root_even_under_a_manifest() {
    let tmp = workspace();

    let federated = web::resolve_serve_backing(tmp.path(), false).expect("resolves");
    assert!(federated.is_federated(), "a workspace parent resolves the federated backing");

    let standalone = web::resolve_serve_backing(tmp.path(), true).expect("resolves");
    assert!(
        !standalone.is_federated(),
        "--standalone forces single-root focus even with a manifest present"
    );
    assert!(standalone.as_single().is_some(), "the single-root engine is used");
}

// ── NFR-PE-10 / FR-WS-06: only the default member is warmed eagerly ──────────

/// At startup only the **default** member's engine is constructed eagerly; the
/// rest stay lazy until first touched ([NFR-PE-10], [FR-WS-06]). Asserted on the
/// registry the web router is built from, before any request fans out.
#[test]
fn only_the_default_member_is_warmed_at_startup() {
    let tmp = workspace();
    let federation = discover(tmp.path()).expect("discovery").expect("a workspace");
    let registry = EngineRegistry::<Engine>::new_serve_default(federation);
    assert_eq!(
        registry.resident_members(),
        ["api"],
        "only the declared default member (api) is warmed eagerly; web stays lazy"
    );
    // The backing built from it is federated (sanity: the router path uses this).
    let backing = Backing::Federated(registry);
    assert!(backing.is_federated());
}
