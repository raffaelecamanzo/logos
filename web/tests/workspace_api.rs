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
use web::{IntentToken, INTENT_HEADER};
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

/// An OpenAPI spec whose sole operation, `/orphan`, matches no route anywhere in
/// the workspace — the near-degenerate CR-111/S-327 case: every cross-boundary
/// reference lands in the `no-provider-in-workspace` bucket, so the bound-ratio
/// denominator is zero.
const ORPHAN_OPENAPI_YAML: &str = "\
openapi: 3.0.3
info:
  title: Orphan API
  version: 1.0.0
paths:
  /orphan:
    get:
      summary: No provider anywhere in the workspace
";

/// Build a two-member workspace: `api` (an OpenAPI consumer built from `openapi`)
/// and `web` (the fixed axum provider), each an indexed git repo, with the
/// manifest at the parent naming `api` default.
fn workspace_with_openapi(openapi: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    init_repo(&root.join("api"), "api/openapi.yaml", openapi);
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

/// Build a two-member workspace: `api` (OpenAPI consumer) + `web` (axum provider),
/// each an indexed git repo, with the manifest at the parent naming `api` default.
fn workspace() -> TempDir {
    workspace_with_openapi(OPENAPI_YAML)
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
    "/api/v1/workspace/roster",
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

/// A workspace router plus the session's intent token — the seam the write-scope tests
/// need, since `intent_guard` (correctly) rejects a `POST` that does not echo the token
/// and [`web::workspace_router`] mints one the test cannot see.
fn ws_router_with_intent(tmp: &TempDir) -> (axum::Router, IntentToken) {
    let federation = discover(tmp.path()).expect("discovery succeeds").expect("a workspace");
    let registry = EngineRegistry::<Engine>::new_serve_default(federation);
    let intent = IntentToken::generate();
    let router = web::workspace_router_with_intent(registry, intent.clone())
        .expect("the workspace router builds");
    (router, intent)
}

/// Every file under `root` (for a failure message that shows where a write landed).
fn walk(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.display().to_string());
            }
        }
    }
    out.sort();
    out
}

/// An intent-guarded, same-origin form `POST` — the shape the Config tab sends.
fn post_form(path: &str, body: &'static str, intent: &IntentToken) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::HOST, "127.0.0.1:4983")
        .header(header::ORIGIN, "http://127.0.0.1:4983")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(INTENT_HEADER, intent.as_str())
        .body(Body::from(body))
        .unwrap()
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
    // Asserted on a field that is ALWAYS emitted: since S-326 the ratio is
    // `skip_serializing_if`, so its presence means "the denominator was non-zero",
    // not "the summary is here" — two different claims that this assertion used to
    // conflate.
    assert!(
        v["coverage"].get("members_total").is_some(),
        "coverage summary present: {body}"
    );
    assert_eq!(
        v["coverage"]["spec_conformance_ratio"], 1.0,
        "and this fixture DOES bind one reference, so the ratio is measured: {body}"
    );
    // CR-111 / FR-WS-05: the ratio never travels bare on the web serve surface
    // either — the identical `CrossServiceCoverage` type the CLI serializes and the
    // SPA's coverage panel reads carries the same two fields here (this fixture's
    // own small numbers: 1 of 1 measured, 0 excluded — the CR-111 pec-services
    // numbers, 6 of 7 / 899 excluded, are pinned verbatim at the core unit-test and
    // web-model/-view layers, where a 906-reference fixture is constructible).
    assert_eq!(v["coverage"]["spec_conformance_measured"], 1, "{body}");
    assert_eq!(
        v["coverage"]["spec_conformance_summary"],
        "1.000 (1 of 1 measured; 0 excluded as no-provider-in-workspace)",
        "{body}"
    );
    // S-323: the warm state rides the same rows the web surface already serves —
    // one read-model, so the shell sees exactly what `logos workspace status`
    // prints ([FR-WS-15]). `warming` is absent, never a fabricated 0 ([NFR-CC-04]).
    // The fixture indexes BOTH members, so the correct label is known — asserting
    // only `is_string()` would pass for `deferred`, `degraded`, or a bogus fifth
    // word, and this is the sole place the wire vocabulary is pinned against the
    // hand-maintained TS union `MemberWarmStateLabel` in `web/ui/src/api/types.ts`.
    for m in v["members"].as_array().unwrap() {
        assert_eq!(m["warm_state"], "warm", "both fixture members are indexed: {m}");
    }
    assert_eq!(v["warm_rollup"]["members"], 2, "the roll-up is served too: {body}");
    assert_eq!(v["warm_rollup"]["warm"], 2, "{body}");
    assert_eq!(v["warm_rollup"]["deferred"], 0, "{body}");
    assert_eq!(v["warm_rollup"]["degraded"], 0, "{body}");
    assert!(
        v["warm_rollup"].get("warming").is_none(),
        "no live warming signal ⇒ no `warming` key: {body}"
    );

    // S-326: the OPEN-state axis rides the same rows, on its own key, and the
    // degraded roll-up rides beside the warm one in one payload — not a second
    // member table ([FR-WS-16]). Both fixture members open, so the coverage
    // marker says the figures cover all of them ([NFR-CC-04]).
    for m in v["members"].as_array().unwrap() {
        assert_eq!(m["open_state"], "opened", "both fixture members open: {m}");
        assert!(
            m.get("degraded_reason").is_none(),
            "an opened member has no failure to report: {m}"
        );
    }
    assert_eq!(v["degraded_rollup"]["members"], 2, "{body}");
    assert_eq!(v["degraded_rollup"]["opened"], 2, "{body}");
    assert_eq!(v["degraded_rollup"]["not_attempted"], 0, "{body}");
    assert!(
        v["degraded_rollup"]["degraded_members"].as_array().unwrap().is_empty(),
        "nobody is degraded: {body}"
    );
    assert_eq!(v["degraded_rollup"]["covers_all_members"], true, "{body}");
    assert_eq!(v["coverage"]["members_read"], 2, "{body}");
    assert_eq!(v["coverage"]["members_total"], 2, "{body}");
    assert_eq!(v["coverage"]["covers_all_members"], true, "{body}");

    // S-377/CR-120: the intake split rides the same read-model, so the coverage
    // dashboard's data carries it without a second endpoint. Asserted here as well
    // as in the both-populations test below, so the field cannot be lost from the
    // surface's primary status assertion while a specialised test still passes.
    // This fixture is contract-surface only — its one bound reference is an OpenAPI
    // operation — and that is exactly the reference workspace's shape.
    assert_eq!(v["coverage"]["by_intake"]["contract_surface"]["bound"], 1, "{body}");
    assert_eq!(v["coverage"]["by_intake"]["invocation"]["bound"], 0, "{body}");
    for row in v["coverage"]["references"].as_array().unwrap() {
        assert_eq!(
            row["intake"], "contract-surface",
            "every row carries its intake on the web surface too: {row}"
        );
    }
}

/// CR-111 / S-327, over the web serve surface: a workspace whose only
/// cross-boundary reference has no provider anywhere reports a zero denominator —
/// `spec_conformance_ratio` absent — and the excluded count is STILL reported,
/// never suppressed alongside the absent ratio.
#[tokio::test]
async fn workspace_status_reports_the_excluded_count_when_the_ratio_is_absent() {
    let tmp = workspace_with_openapi(ORPHAN_OPENAPI_YAML);
    let router = ws_router(&tmp);
    let resp = router.oneshot(get("/api/v1/workspace/status")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert!(
        v["coverage"].get("spec_conformance_ratio").is_none(),
        "a zero denominator is absent, never a fabricated score: {body}"
    );
    assert_eq!(v["coverage"]["spec_conformance_measured"], 0, "{body}");
    assert_eq!(v["coverage"]["no_provider_in_workspace"], 1, "{body}");
    assert_eq!(
        v["coverage"]["spec_conformance_summary"],
        "0 of 0 measured; 1 excluded as no-provider-in-workspace",
        "the excluded count is reported even though the ratio itself is absent: {body}"
    );
}

/// **S-376/[CR-120]/[BR-51] over the web serve surface: the headline is a
/// resolved-edge count, and it never travels without its rate** ([FR-WS-05]'s
/// surface-parity rule).
///
/// Driven through the **real router** over the both-intakes fixture, following
/// S-372's and S-377's precedent: the coverage dashboard reads this endpoint, and
/// a projection introduced between the read-model and the response would pass
/// every core test while leaving the board showing a retired figure.
///
/// The fixture is the one shape where the two populations disagree usefully — the
/// OpenAPI operation binds AND the static client call binds — so `bound: 2` and
/// `resolved_cross_service_edges: 1` are different numbers here, and an
/// implementation that published the pooled count under the new name would fail.
///
/// [BR-51]: ../../docs/specs/software-spec.md#327-workspace-federation
/// [CR-120]: ../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
#[tokio::test]
async fn workspace_status_publishes_the_resolved_edge_headline_with_its_egress_rate() {
    let tmp = workspace_with_both_intakes();
    let router = ws_router(&tmp);
    let resp = router.oneshot(get("/api/v1/workspace/status")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let coverage = &v["coverage"];

    // AC1 on this surface: the retired key is gone under all three spellings.
    for retired in ["bound_ratio", "bound_ratio_measured", "bound_ratio_summary"] {
        assert!(
            coverage.get(retired).is_none(),
            "`{retired}` must be absent from the web payload (CR-120 AC1): {body}"
        );
    }

    // The headline: ONE resolved edge (the static client call), not the pooled
    // `bound: 2` — the OpenAPI operation's match is documentation conformance, not
    // a resolved call.
    assert_eq!(coverage["bound"], 2, "{body}");
    assert_eq!(
        coverage["resolved_cross_service_edges"], 1,
        "only the captured call site is a resolved cross-service edge: {body}"
    );
    // …and the rate beside it, over the invocation population's own denominator
    // (one bound call plus one recorded refusal, S-374).
    assert_eq!(coverage["egress_resolution"], 0.5, "{body}");
    assert_eq!(coverage["egress_resolution_measured"], 2, "{body}");
    assert_eq!(
        coverage["resolved_edges_summary"],
        "1 resolved cross-service edges; egress resolution 0.500 (1 of 2 egress sites resolved)",
        "the count and the rate arrive as one composed line, so a view cannot render \
         one without the other (BR-51): {body}"
    );
    // The spec-conformance ratio is still published beside it, still never bare.
    assert_eq!(coverage["spec_conformance_measured"], 3, "{body}");
    assert!(
        coverage["spec_conformance_summary"]
            .as_str()
            .is_some_and(|s| s.contains("of 3 measured")),
        "{body}"
    );
}

/// A `reqwest` client in the `api` member — the **invocation** intake population
/// this file's other fixtures do not have: `fetch_user` makes a static call that
/// binds `web`'s route (bound), `fetch_dynamic` composes its URL at runtime and is
/// refused, leaving one keyless ledger row (unbound, S-374).
const API_CLIENT: &str = r#"
use reqwest::Client;
pub async fn fetch_user(client: Client) { let _ = client.get("/users/{id}").await; }
pub async fn fetch_dynamic(client: Client, url: String) { let _ = client.get(url).await; }
"#;

/// [`workspace`] plus [`API_CLIENT`], so the coverage payload carries **both**
/// intake populations — which the OpenAPI-only fixtures cannot exercise.
fn workspace_with_both_intakes() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    init_repo(&root.join("api"), "api/openapi.yaml", OPENAPI_YAML);
    init_repo(&root.join("web"), "src/main.rs", AXUM_MAIN);
    write(&root.join("api"), "src/client.rs", API_CLIENT);
    Engine::start(root.join("api")).expect("api engine").index();
    Engine::start(root.join("web")).expect("web engine").index();
    std::fs::write(
        root.join("logos.workspace.toml"),
        "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"web\"]\ndefault = \"api\"\n",
    )
    .unwrap();
    tmp
}

/// **S-377/[CR-120] over the web serve surface: `intake` on every row and the
/// counts split by it** ([FR-WS-05]'s surface-parity rule).
///
/// The web surface serializes the identical `CrossServiceCoverage` the CLI and MCP
/// do, so this is asserted through the **real router** rather than by reasoning
/// from that fact: the coverage dashboard reads this endpoint, and a projection or
/// a `skip` introduced anywhere between the read-model and the response would pass
/// every core test while leaving the board blind — which is what the parity rule
/// exists to catch.
///
/// [CR-120]: ../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
#[tokio::test]
async fn workspace_status_reports_intake_on_every_row_and_splits_the_counts_by_it() {
    let tmp = workspace_with_both_intakes();
    let router = ws_router(&tmp);
    let resp = router.oneshot(get("/api/v1/workspace/status")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let coverage = &v["coverage"];
    let references = coverage["references"].as_array().expect("classified references");

    // Guard the guard: both populations must actually reach this surface, or every
    // assertion below is a claim about one of them.
    let mut intakes: Vec<&str> =
        references.iter().map(|r| r["intake"].as_str().unwrap_or("")).collect();
    intakes.sort_unstable();
    intakes.dedup();
    assert_eq!(
        intakes,
        ["contract-surface", "invocation"],
        "the fixture must serve both intake populations: {body}"
    );

    // Every row, in every state.
    for row in references {
        assert!(
            row["intake"].is_string(),
            "every coverage row carries `intake` on the web surface: {row}"
        );
    }

    // The split itself: the OpenAPI operation binds, and so does the static client
    // call — one binding per population, counted apart. That separation is the
    // whole point: on the reference workspace the second number is 0.
    let split = &coverage["by_intake"];
    assert_eq!(split["contract_surface"]["bound"], 1, "{body}");
    assert_eq!(split["invocation"]["bound"], 1, "{body}");
    assert_eq!(
        split["invocation"]["unbound"], 1,
        "the runtime-composed call's recorded refusal (S-374): {body}"
    );

    // And it sums to the headline, so the split can never under-report the four
    // counters the dashboard renders beside it.
    for field in ["bound", "ambiguous", "unbound", "no_provider_in_workspace"] {
        let summed = split["contract_surface"][field].as_u64().expect(field)
            + split["invocation"][field].as_u64().expect(field);
        assert_eq!(
            Some(summed),
            coverage[field].as_u64(),
            "`{field}`: the split must sum to the headline: {body}"
        );
    }
}

/// **The degraded shape over the web surface ([FR-WS-16]).**
///
/// The SPA declares `MemberStatus.open_state` / `degraded_cause` /
/// `degraded_reason` and `DegradedRollup` as its read-model contract, and every
/// other test of that shape runs through the CLI's serializer. This is the only
/// place the *web* payload is asserted to carry it — a handler that dropped the
/// flattened open state, or a roll-up that stopped naming members, would leave
/// the SPA's types describing a payload it no longer receives.
#[tokio::test]
async fn workspace_status_carries_the_degraded_shape_for_an_unopenable_member() {
    let tmp = workspace();
    // Break `web` the way the CLI suite does: a DIRECTORY where the store must
    // be, which no open can succeed against.
    let db = tmp.path().join("web").join(".logos").join("logos.db");
    std::fs::remove_file(&db).expect("clear the store file");
    std::fs::create_dir_all(&db).expect("a directory where the store must be");

    let router = ws_router(&tmp);
    let resp = router.oneshot(get("/api/v1/workspace/status")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "a degraded member is not an HTTP error: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();

    let web = v["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["member"] == "web")
        .expect("the broken member still has a row");
    assert_eq!(web["open_state"], "degraded", "{web}");
    assert_eq!(web["degraded_cause"], "store-obstructed", "{web}");
    assert!(
        web["degraded_reason"].as_str().is_some_and(|r| r.contains("regular file")),
        "the classified reason reaches the web payload: {web}"
    );
    assert!(
        web["degraded_diagnostic"]
            .as_str()
            .is_some_and(|d| d.contains("unable to open database file")),
        "and so does the verbatim diagnostic: {web}"
    );
    assert!(
        web["error"].as_str().is_some(),
        "the pre-existing `error` channel is untouched: {web}"
    );

    assert_eq!(
        v["degraded_rollup"]["degraded_members"].as_array().unwrap(),
        &vec![serde_json::Value::from("web")],
        "the roll-up NAMES it: {body}"
    );
    assert_eq!(v["degraded_rollup"]["opened"], 1, "{body}");
    assert_eq!(v["degraded_rollup"]["covers_all_members"], false, "{body}");
    assert_eq!(v["coverage"]["covers_all_members"], false, "{body}");
    assert_eq!(v["coverage"]["members_read"], 1, "{body}");
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
    let backing = Backing::Federated(Box::new(registry));
    assert!(backing.is_federated());
}

// ── S-250 / FR-UI-29: `?repo=` scopes every existing `/api/v1/*` view ─────────
//
// The workspace SPA's member selector rides one optional query param on the
// ordinary read-model endpoints (the `member::MemberEngine` extractor). These
// assert the three arms of that resolution: scoped → that member's engine,
// unknown → an honest 404, single-root → inert (byte-for-byte the pre-workspace
// response).

/// The member an `/api/v1/*` view answered from, read off the one figure that
/// needs no language grammar: the answering engine's own store path.
async fn health_db_path(router: &axum::Router, path: &str) -> String {
    let resp = router.clone().oneshot(get(path)).await.expect("route responds");
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{path} answers 200: {body}");
    assert_self_only_csp(&headers, path);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    v["status"]["db_path"].as_str().expect("health carries its store path").to_string()
}

/// `?repo=<member>` scopes an ordinary view to that member's engine, and an
/// unscoped request still answers from the warmed default — the selector's server
/// contract ([FR-UI-29] AC1). Asserted on `/api/v1/health`, whose `status.db_path`
/// names the answering member's store without needing any language grammar.
#[tokio::test]
async fn repo_param_scopes_an_existing_view_to_the_selected_member() {
    let tmp = workspace();
    let router = ws_router(&tmp);

    let default = health_db_path(&router, "/api/v1/health").await;
    let api = health_db_path(&router, "/api/v1/health?repo=api").await;
    let web = health_db_path(&router, "/api/v1/health?repo=web").await;

    assert_eq!(default, api, "an unscoped view answers from the warmed default member (api)");
    assert_ne!(api, web, "switching the member switches the answering engine");
    assert!(api.contains("/api/"), "?repo=api reads the api member's store: {api}");
    assert!(web.contains("/web/"), "?repo=web reads the web member's store: {web}");

    // A blank `?repo=` is "unscoped", never a member named "" (the SPA omits the
    // param in single-root mode, but a hand-built URL must not 404 on it).
    assert_eq!(health_db_path(&router, "/api/v1/health?repo=").await, default);
}

/// A `?repo=` naming a member this workspace does not have is an honest `404` —
/// a view is never quietly served a *different* member's figures ([NFR-RA-05]).
#[tokio::test]
async fn an_unknown_repo_is_an_honest_404_on_the_scoped_views() {
    let tmp = workspace();
    let router = ws_router(&tmp);
    for path in ["/api/v1/health", "/api/v1/overview", "/api/v1/coverage", "/api/v1/graph"] {
        let uri = format!("{path}?repo=nope");
        let resp = router.clone().oneshot(get(&uri)).await.expect("route responds");
        let (status, body, headers) = body_string(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri} is 404 for an unknown member: {body}");
        assert!(body.contains("no workspace member `nope`"), "{uri} names the member: {body}");
        assert_self_only_csp(&headers, &uri);
    }
}

/// In single-root mode `?repo=` is **inert**: the one engine IS the root, so the
/// response is what the unscoped request answers — the param never 404s a plain
/// repo ([FR-UI-29] AC4, [ADR-52]). The SPA renders no selector there and never
/// sends it; this is the server-side half of that guarantee.
#[tokio::test]
async fn single_root_ignores_the_repo_param() {
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path(), "src/lib.rs", "pub fn f() {}\n");
    let engine = Arc::new(Engine::start(tmp.path()).expect("engine starts"));
    let router = web::router(engine);

    let plain = health_db_path(&router, "/api/v1/health").await;
    let scoped = health_db_path(&router, "/api/v1/health?repo=anything").await;
    assert_eq!(plain, scoped, "a single-root serve answers the same engine, `?repo=` or not");
}

// ── S-250: the shell's boot probe, and the member scope on the WRITE seam ─────

/// `workspace roster` carries the manifest — name, default member, member names — and
/// is the endpoint the SPA shell probes on **every** page load ([FR-UI-29]).
#[tokio::test]
async fn workspace_roster_carries_the_manifest_name_default_and_members() {
    let tmp = workspace();
    let router = ws_router(&tmp);
    let resp = router.oneshot(get("/api/v1/workspace/roster")).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_self_only_csp(&headers, "/api/v1/workspace/roster");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["workspace"], "shop");
    // The default member is named, so the SPA opens on the member an UNSCOPED request
    // answers from, rather than guessing at the roster's first entry.
    assert_eq!(v["default"], "api");
    let members: Vec<&str> = v["members"].as_array().unwrap().iter().map(|m| m.as_str().unwrap()).collect();
    assert_eq!(members, ["api", "web"], "every member, in manifest order: {body}");
}

/// The roster starts **no** member engine ([NFR-PE-10]). The shell probes it on every
/// page load, so if it fanned out like `workspace status` does, merely opening the
/// dashboard would construct and watch every member's engine — undoing the
/// warm-only-the-default policy the federated serve exists to keep.
#[tokio::test]
async fn the_roster_probe_warms_no_member() {
    let tmp = workspace();
    let federation = discover(tmp.path()).expect("discovery").expect("a workspace");
    let registry = EngineRegistry::<Engine>::new_serve_default(federation);
    assert_eq!(registry.resident_members(), ["api"], "only the default member is warm at startup");

    // Read the roster straight off the registry the router would serve it from.
    let roster = logos_core::federation::query::workspace_roster(&registry);
    assert_eq!(roster.workspace, "shop");
    assert_eq!(roster.default.as_deref(), Some("api"));
    assert_eq!(roster.members, ["api", "web"]);
    // The decisive assertion: reading it warmed nothing new.
    assert_eq!(
        registry.resident_members(),
        ["api"],
        "the roster read must not construct any member engine (NFR-PE-10)"
    );
}

/// A **write** carries the member scope too ([FR-UI-29]). This is the load-bearing half:
/// the Config tab READS the selected member's policy, so its Save must write back to
/// THAT member — never over the workspace default's file.
#[tokio::test]
async fn a_scoped_config_write_targets_that_member_not_the_default() {
    let tmp = workspace();
    let (router, intent) = ws_router_with_intent(&tmp);
    let resp = router
        .clone()
        .oneshot(post_form("/config/save?repo=web", "file=rules&content=%23%20workspace%20policy%0A", &intent))
        .await
        .expect("route responds");
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "the scoped write is accepted: {body}");

    // The policy landed in the `web` member's store — and the default member (`api`) is
    // untouched. This is the whole point: the Config tab reads the SELECTED member, so a
    // save that drifted to the default would silently overwrite another repo's policy.
    let listing = walk(tmp.path());
    assert!(
        tmp.path().join("web/.logos/rules.toml").exists(),
        "the write targeted the scoped member; tree was: {listing:?}"
    );
    assert!(
        !tmp.path().join("api/.logos/rules.toml").exists(),
        "the default member's policy was NOT overwritten; tree was: {listing:?}"
    );
}

/// An unknown member on a **write** is a `404` — never a silent write to the default
/// member's policy file ([NFR-RA-05]).
#[tokio::test]
async fn an_unknown_repo_404s_a_write_rather_than_writing_the_default() {
    let tmp = workspace();
    let (router, intent) = ws_router_with_intent(&tmp);

    let resp = router
        .clone()
        .oneshot(post_form("/config/save?repo=nope", "file=rules&content=%23%20workspace%20policy%0A", &intent))
        .await
        .expect("route responds");
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "an unknown member is refused: {body}");
    assert!(body.contains("no workspace member `nope`"), "{body}");
    assert!(!tmp.path().join("api/.logos/rules.toml").exists(), "nothing was written anywhere");
    assert!(!tmp.path().join("web/.logos/rules.toml").exists(), "nothing was written anywhere");
}
