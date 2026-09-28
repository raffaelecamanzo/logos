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
use rusqlite::{Connection, OpenFlags};
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
    std::fs::write(root.join("logos.workspace.toml"), FIXTURE_MANIFEST).unwrap();
    tmp
}

/// The fixture's workspace manifest, verbatim — named so the manifest editor's
/// tests (S-430) can compare against, and post back, the exact bytes on disk.
const FIXTURE_MANIFEST: &str =
    "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"web\"]\ndefault = \"api\"\n";

/// The load fingerprint of a workspace tier with no `config.toml` yet — the BLAKE3
/// of the empty document, which is what an absent file reads as (S-451 T2). A
/// literal, because the static write-endpoint list below needs one; the read
/// returning exactly this is asserted by
/// `a_first_tier_save_over_a_file_created_since_load_is_a_409`.
const EMPTY_DOCUMENT_FINGERPRINT: &str =
    "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";

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
    // S-464 / [FR-WS-33]: the build-dependency relation — a GET like the rest,
    // so the `200`+CSP, single-root `404` and write-free loops all walk it.
    "/api/v1/workspace/build-deps",
    "/api/v1/workspace/search?q=user",
    "/api/v1/workspace/callers?symbol=get_user",
    "/api/v1/workspace/impact?symbol=get_user",
    // S-427 / [FR-WS-28]: the two read-models that shipped CLI-only. They are
    // ENUMERATED here rather than asserted ad hoc, because this list is what the
    // `200`+CSP loop, the single-root `404` loop and the write-free loop all walk
    // — a route absent from it is silently unguarded on all three.
    "/api/v1/workspace/reachability",
    "/api/v1/workspace/check",
    // S-429 / [FR-UI-37]: the `app`-scoped telemetry aggregate. Enumerated for
    // the same reason as the two above — the `200`+CSP loop, the single-root
    // `404` loop and the write-free loop all walk this list.
    "/api/v1/workspace/statistics",
    // S-450 / [FR-WS-30]: the workspace root read as a config root. A GET like
    // the rest, so the same three loops walk it — and the write-free loop is the
    // one that matters most here, because its two write twins below live beside it.
    "/api/v1/workspace/config",
    // S-430 / [FR-UI-38]: the manifest read the workspace Config editor loads. It
    // must be write-free and engine-free like every GET here; its save twin sits
    // in the write list below.
    "/api/v1/workspace/manifest",
];

/// The **mutating** workspace routes — S-450's two config-root writes
/// ([FR-WS-30]) and S-430's manifest save ([FR-UI-38]), all under [NFR-SE-06] —
/// with a well-formed body each. Kept apart from [`WORKSPACE_ENDPOINTS`] because every
/// loop over that list issues a `GET`, which these answer `405`; the route-table
/// guard reads the union of the two lists.
const WORKSPACE_WRITE_ENDPOINTS: &[(&str, &str)] = &[
    // S-451 T2: the save carries the load fingerprint — here the absent tier's,
    // [`EMPTY_DOCUMENT_FINGERPRINT`], since the fixture declares none.
    (
        "/api/v1/workspace/config/save",
        "content=%5Bchat%5D%0Amodel%20%3D%20%22ws%2Fsaved%22%0A&fingerprint=af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
    ),
    ("/api/v1/workspace/config/secret", "api_key=sk-workspace-written-key-wr42"),
    // S-430 / [FR-UI-38]: posts the fixture's own manifest back byte-identical,
    // so the well-formed save is the no-op `unchanged` — a `200` that writes
    // nothing whatever the fingerprint (identity is decided before staleness).
    (
        "/api/v1/workspace/manifest/save",
        "content=%5Bworkspace%5D%0Aname%20%3D%20%22shop%22%0Amembers%20%3D%20%5B%22api%22%2C%20%22web%22%5D%0Adefault%20%3D%20%22api%22%0A&fingerprint=never-loaded",
    ),
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
fn post_form(path: &str, body: impl Into<String>, intent: &IntentToken) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::HOST, "127.0.0.1:4983")
        .header(header::ORIGIN, "http://127.0.0.1:4983")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(INTENT_HEADER, intent.as_str())
        .body(Body::from(body.into()))
        .unwrap()
}

/// `application/x-www-form-urlencoded` for one field — every byte outside the
/// unreserved set percent-encoded, so a TOML document survives the trip intact.
fn form_field(name: &str, value: &str) -> String {
    let encoded: String = value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("{name}={encoded}")
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
        "1 resolved cross-service edge; egress resolution 0.500 (1 of 2 egress sites resolved)",
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

/// S-464 / [FR-WS-33]: `build-deps` serializes the read-model the CLI and MCP
/// print — per member, `builds_against` / `built_against_by` rows naming kind,
/// scope and artifact, beside the headline — and `?repo=` scopes the rows while
/// the denominator stays workspace-wide. The runtime `route-providers` answer
/// over the same workspace carries no build row ([BR-58]).
///
/// [FR-WS-33]: ../../docs/specs/requirements/FR-WS-33.md
/// [BR-58]: ../../docs/specs/software-spec.md#327-workspace-federation
#[cfg(feature = "lang-all")]
#[tokio::test]
async fn workspace_build_deps_reports_each_members_rows_and_repo_scopes() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let pom = |artifact: &str, dependency: &str| {
        format!(
            "<project><groupId>com.acme</groupId><artifactId>{artifact}</artifactId>\
             <version>1</version><dependencies>{dependency}</dependencies></project>"
        )
    };
    init_repo(&root.join("api"), "api/openapi.yaml", OPENAPI_YAML);
    write(
        &root.join("api"),
        "pom.xml",
        &pom(
            "api",
            "<dependency><groupId>com.acme</groupId><artifactId>web</artifactId>\
             <scope>test</scope></dependency>",
        ),
    );
    init_repo(&root.join("web"), "src/main.rs", AXUM_MAIN);
    write(&root.join("web"), "pom.xml", &pom("web", ""));
    Engine::start(root.join("api")).expect("api engine").index();
    Engine::start(root.join("web")).expect("web engine").index();
    std::fs::write(root.join("logos.workspace.toml"), FIXTURE_MANIFEST).unwrap();
    let router = ws_router(&tmp);

    let resp = router.clone().oneshot(get("/api/v1/workspace/build-deps")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["members"].as_array().unwrap().len(), 2, "{body}");
    let row = &v["members"][0]["builds_against"][0];
    assert_eq!(v["members"][0]["member"], "api");
    assert_eq!((&row["to"], &row["kind"], &row["scope"]), (&"web".into(), &"dependency".into(), &"test".into()));
    assert_eq!(row["artifact"], "com.acme:web");
    assert_eq!(v["members"][1]["built_against_by"][0]["from"], "api");
    assert_eq!(v["headline"]["build_dependency_pairs"]["pairs"], 1);
    assert!(v["headline"]["summary"].as_str().unwrap().contains("never a runtime coupling"));

    let scoped = router.clone().oneshot(get("/api/v1/workspace/build-deps?repo=web")).await.unwrap();
    let (_s, body, _h) = body_string(scoped).await;
    let w: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(w["scope"], "web");
    assert_eq!(w["members"].as_array().unwrap().len(), 1);
    assert_eq!(w["headline"], v["headline"], "the denominator is workspace-wide under a scope");

    let runtime = router.oneshot(get("/api/v1/workspace/route-providers")).await.unwrap();
    let (_s, body, _h) = body_string(runtime).await;
    assert!(!body.contains("com.acme"), "no build row among the runtime bindings: {body}");
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

// ── S-448 / FR-WS-30: the effective-chat slice under a federated backing ─────

/// The serialized config read-model up to — not including — its effective-chat
/// slice: every byte the Config editor reads and round-trips.
fn literal_prefix(payload: &str) -> &str {
    &payload[..payload.find(",\"effective_chat\":").expect("the payload carries the slice")]
}

/// Under a federated backing `GET /api/v1/config` resolves the slice against the
/// workspace root the backing already holds ([FR-WS-30], S-448 AC4): a member
/// that declares nothing inherits both halves (`workspace` origin), a member that
/// declares its own model keeps it (`member`) and inherits only the key — and in
/// both, every byte before the slice is the member's literal document, exactly
/// what a read with **no** workspace root serves (S-448 AC1). The raw key is never
/// on the wire; its last-4 is ([NFR-SE-07]).
#[tokio::test]
async fn config_endpoint_resolves_the_slice_against_the_backings_workspace_root() {
    let tmp = workspace();
    let ws_key = "sk-workspace-held-key-ws99";
    write(tmp.path(), ".logos/config.toml", "[chat]\nprovider = \"anthropic\"\nmodel = \"ws/model\"\n");
    write(tmp.path(), ".logos/secrets.toml", &format!("[chat]\napi_key = \"{ws_key}\"\n"));
    write(&tmp.path().join("web"), ".logos/config.toml", "[chat]\nmodel = \"web/own-model\"\n");
    let router = ws_router(&tmp);

    for (path, member, model, policy_origin) in [
        ("/api/v1/config", "api", "ws/model", "workspace"),
        ("/api/v1/config?repo=api", "api", "ws/model", "workspace"),
        ("/api/v1/config?repo=web", "web", "web/own-model", "member"),
    ] {
        let resp = router.clone().oneshot(get(path)).await.expect("route responds");
        let (status, body, headers) = body_string(resp).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        assert_self_only_csp(&headers, path);

        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let slice = &v["effective_chat"];
        assert_eq!(slice["policy"]["model"], model, "{path}: {body}");
        assert_eq!(slice["policy_origin"], policy_origin, "{path}: {body}");
        assert_eq!(slice["credential_origin"], "workspace", "{path}: {body}");
        assert_eq!(slice["credential"], serde_json::json!({"present": true, "last4": "ws99"}), "{path}");
        assert!(!body.contains(ws_key) && !body.contains("workspace-held"), "{path}: the raw key leaked");

        // The literal fields are the member's own — the inherited half is not in them.
        let alone = Engine::open(tmp.path().join(member)).config_read(None).expect("core read");
        let alone = serde_json::to_string(&alone).unwrap();
        assert_eq!(literal_prefix(&body), literal_prefix(&alone), "{path}: a literal field moved");
        assert!(!literal_prefix(&body).contains("ws/model"), "{path}: the inherited model leaked");
    }
}

/// The workspace root is the one the **backing** holds, not a member's parent
/// directory ([ADR-52]: taken, never discovered). With a nested member
/// (`services/web`) the two differ, so a handler that walked up from the member
/// would miss the root's `[chat]` and key entirely.
#[tokio::test]
async fn config_endpoint_resolves_a_nested_member_against_the_federation_root() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    init_repo(&root.join("api"), "api/openapi.yaml", OPENAPI_YAML);
    init_repo(&root.join("services/web"), "src/main.rs", AXUM_MAIN);
    std::fs::write(
        root.join("logos.workspace.toml"),
        "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"services/web\"]\ndefault = \"api\"\n",
    )
    .unwrap();
    write(root, ".logos/config.toml", "[chat]\nmodel = \"ws/model\"\n");
    write(root, ".logos/secrets.toml", "[chat]\napi_key = \"sk-federation-root-key-nr81\"\n");
    let router = ws_router(&tmp);

    let resp = router.oneshot(get("/api/v1/config?repo=services%2Fweb")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let slice = &v["effective_chat"];
    assert_eq!(slice["policy_origin"], "workspace", "the nested member inherits the root's policy: {body}");
    assert_eq!(slice["policy"]["model"], "ws/model", "{body}");
    assert_eq!(slice["credential_origin"], "workspace", "{body}");
    assert_eq!(slice["credential"]["last4"], "nr81", "{body}");
}

/// A fault at the workspace root is an honest `500` naming that file for a member
/// that inherits from it — never a silent `unset` ([NFR-RA-05]) — and it echoes no
/// key material ([NFR-SE-07]); a member declaring both halves never reads it and
/// still answers `200` ([FR-WS-30], S-448).
#[tokio::test]
async fn config_endpoint_fails_loud_on_a_faulty_workspace_tier_only_for_an_inheriting_member() {
    let tmp = workspace();
    write(tmp.path(), ".logos/secrets.toml", "[chat]\napi_key = sk-ws-unquoted-secret-qq44\n");
    write(&tmp.path().join("web"), ".logos/config.toml", "[chat]\nmodel = \"web/own-model\"\n");
    write(&tmp.path().join("web"), ".logos/secrets.toml", "[chat]\napi_key = \"sk-web-own-key-ww33\"\n");
    let router = ws_router(&tmp);

    let resp = router.clone().oneshot(get("/api/v1/config?repo=api")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "an inheriting member fails loud: {body}");
    assert!(body.contains(".logos/secrets.toml"), "the fault names the file: {body}");
    assert!(!body.contains("unquoted-secret-qq44"), "the malformed key is not echoed: {body}");

    let resp = router.oneshot(get("/api/v1/config?repo=web")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "a fully-declaring member never reads the workspace tier: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["effective_chat"]["credential_origin"], "member", "{body}");
}

/// HF-1 ([ADR-67] §2) on the wire: a member holding its own key but inheriting the
/// workspace policy is served the WORKSPACE credential — or none, when the workspace
/// holds no key — and `member_key_withheld` states the withheld key. The member's own
/// masked key still rides `chat_key` (its literal document), never the slice.
#[tokio::test]
async fn config_endpoint_withholds_a_member_key_from_an_inherited_workspace_policy() {
    let tmp = workspace();
    write(tmp.path(), ".logos/config.toml", "[chat]\nmodel = \"ws/model\"\n");
    write(tmp.path(), ".logos/secrets.toml", "[chat]\napi_key = \"sk-workspace-held-key-ws99\"\n");
    write(&tmp.path().join("api"), ".logos/secrets.toml", "[chat]\napi_key = \"sk-api-own-key-ap12\"\n");
    let router = ws_router(&tmp);

    let resp = router.clone().oneshot(get("/api/v1/config?repo=api")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let slice = &v["effective_chat"];
    assert_eq!(slice["policy_origin"], "workspace", "{body}");
    assert_eq!(slice["credential_origin"], "workspace", "{body}");
    assert_eq!(slice["credential"], serde_json::json!({"present": true, "last4": "ws99"}), "{body}");
    assert_eq!(slice["member_key_withheld"], true, "{body}");
    assert_eq!(v["chat_key"], serde_json::json!({"present": true, "last4": "ap12"}), "{body}");
    assert!(!body.contains("sk-api-own-key") && !body.contains("sk-workspace-held"), "{body}");

    std::fs::remove_file(tmp.path().join(".logos/secrets.toml")).unwrap();
    let resp = router.oneshot(get("/api/v1/config?repo=api")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let slice = &v["effective_chat"];
    assert_eq!(slice["credential_origin"], "unset", "the member key does not fill the gap: {body}");
    assert_eq!(slice["credential"], serde_json::json!({"present": false}), "{body}");
    assert_eq!(slice["member_key_withheld"], true, "{body}");
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

// ── S-429 / FR-UI-37: the `app`-scoped telemetry aggregate ───────────────────
//
// What these two assert is the **surface contract** T2 consumes: the denominator,
// the named unread members and their reasons, over the wire. The summing
// arithmetic itself — two members with telemetry, one unreadable, counts merged
// per tool/day/origin — is proven against real seeded stores in
// `logos_core::federation::telemetry`'s unit tests, which can build a migrated
// store through the product's own schema; a fixture here could only hand-write
// `CREATE TABLE`, which is a second copy of the migration ledger.

/// The aggregate states the population it summed over and **names** every member
/// it could not read, with the reason ([FR-UI-37], [NFR-CC-04]). Neither member of
/// this fixture has ever run telemetry, so `members_read` is `0` — the honest
/// awaiting-data signal — and a total is never presented as covering the roster.
#[tokio::test]
async fn workspace_statistics_states_its_denominator_and_names_every_unread_member() {
    let tmp = workspace();
    let router = ws_router(&tmp);
    let v = json_body(&router, "/api/v1/workspace/statistics").await;

    assert_eq!(v["workspace"], "shop");
    assert_eq!(v["window_days"], 7, "the FR-OB-04 default window");
    assert_eq!(v["members_total"], 2);
    assert_eq!(v["members_read"], 0, "no member has telemetry: {v}");
    assert_eq!(
        v["covers_all_members"], false,
        "the marker governing every figure below it: {v}"
    );
    let unread: Vec<(&str, &str)> = v["unread"]
        .as_array()
        .expect("unread is an array")
        .iter()
        .map(|u| (u["member"].as_str().unwrap(), u["reason"].as_str().unwrap()))
        .collect();
    assert_eq!(
        unread,
        [("api", "absent"), ("web", "absent")],
        "every member is named with its reason, in roster order: {v}"
    );
    assert!(
        v["unread"][0]["detail"].as_str().is_some_and(|d| !d.is_empty()),
        "each named member carries its diagnostic: {v}"
    );
    // Usage, not quality: no gate verdict, score or violation count is rolled up
    // here ([BR-56], [ADR-56]).
    for quality_key in ["signal", "score", "violations", "gate", "rules"] {
        assert!(
            v.get(quality_key).is_none(),
            "the usage aggregate carries no quality-signal element `{quality_key}`: {v}"
        );
    }
    // Latency percentiles are absent rather than averaged across members — a
    // summed percentile would be a fabricated figure ([NFR-CC-04]).
    for latency_key in ["latency_p50_ms", "latency_p95_ms", "latency_p99_ms"] {
        assert!(
            v.get(latency_key).is_none(),
            "percentiles must not be summed across members: {v}"
        );
    }

    // `?window=` is a documented part of this route's contract, pinned here
    // because without it the handler could ignore the query map entirely and
    // everything above would still pass. `window_days` is echoed even when
    // `members_read == 0`, so neither assertion needs a seeded store.
    let scoped = json_body(&router, "/api/v1/workspace/statistics?window=30").await;
    assert_eq!(scoped["window_days"], 30, "?window= scopes the trailing window: {scoped}");
    assert_eq!(
        scoped["attribution_coverage"]["requested_window_days"], 30,
        "and the coverage block echoes the window actually requested"
    );
    let lenient = json_body(&router, "/api/v1/workspace/statistics?window=banana").await;
    assert_eq!(
        lenient["window_days"], 7,
        "an unparseable window falls back to the FR-OB-04 default, the same lenient \
         query contract the other endpoints use: {lenient}"
    );
}

/// A member whose `telemetry.db` exists and is not a database is named
/// `unreadable`, distinctly from a member that simply never ran telemetry
/// ([FR-UI-37]). Collapsing the two would report a fault as routine.
#[tokio::test]
async fn an_unreadable_member_store_is_named_apart_from_an_absent_one() {
    let tmp = workspace();
    let broken = tmp.path().join("web").join(".logos");
    std::fs::create_dir_all(&broken).expect("member .logos");
    std::fs::write(broken.join("telemetry.db"), b"not a database").expect("broken store");

    let router = ws_router(&tmp);
    let v = json_body(&router, "/api/v1/workspace/statistics").await;

    let unread: Vec<(&str, &str)> = v["unread"]
        .as_array()
        .expect("unread is an array")
        .iter()
        .map(|u| (u["member"].as_str().unwrap(), u["reason"].as_str().unwrap()))
        .collect();
    assert_eq!(
        unread,
        [("api", "absent"), ("web", "unreadable")],
        "the two states are reported apart: {v}"
    );
    assert_eq!(v["members_read"], 0, "neither contributed: {v}");
}

/// **The binding criterion at the HTTP surface** ([FR-UI-37], [NFR-PE-10]):
/// serving the aggregate leaves the resident-engine set exactly where startup
/// left it. The 16-member connection-count form of the same claim lives in
/// `logos-core/tests/workspace_statistics_engine_free.rs`; this one proves the
/// route **the SPA actually calls** reaches the engine-free read and not
/// `Engine::stats`.
///
/// It therefore drives the real router. An earlier version called
/// `federation::workspace_statistics` directly off a registry while claiming this
/// in its doc comment — which re-tested the core function the other binary
/// already covers and asserted nothing whatever about the handler, leaving the
/// route free to fan out through engines behind a green test.
#[tokio::test]
async fn the_statistics_aggregate_warms_no_member() {
    let tmp = workspace();
    let federation = discover(tmp.path()).expect("discovery").expect("a workspace");
    let backing = Arc::new(Backing::Federated(Box::new(
        EngineRegistry::<Engine>::new_serve_default(federation),
    )));
    let router = web::router_for_backing(Arc::clone(&backing)).expect("the router builds");
    let before = {
        let registry = backing.as_federated().expect("the federated registry");
        assert_eq!(
            registry.resident_members(),
            ["api"],
            "only the default member is warm at startup"
        );
        (registry.engine_starts(), registry.live_read_connections())
    };

    let resp = router
        .oneshot(get("/api/v1/workspace/statistics"))
        .await
        .expect("route responds");
    let (status, body, _headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["members_total"], 2, "the request really answered: {body}");

    let registry = backing.as_federated().expect("the federated registry");
    assert_eq!(
        registry.resident_members(),
        ["api"],
        "serving the aggregate must not construct any member engine (NFR-PE-10)"
    );
    assert_eq!(
        (registry.engine_starts(), registry.live_read_connections()),
        before,
        "and must move neither the construction counter nor the connection count"
    );
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

// ── S-427 / FR-WS-28: reachability and governance over the HTTP read surface ──
//
// Two GETs serialising the `federation::reach` and `federation::governance`
// read-models that shipped CLI-only. The field-for-field CLI/HTTP agreement — the
// story's spine — is asserted in `cli/tests/xservice_surface.rs`, which is the one
// crate that can drive the real `logos` binary AND this router over the SAME
// workspace. What follows is the rest of the contract: the bound the default
// states, the honest empty, the named incomplete fan-out, and the surface hygiene.

/// The `[governance]` rule family the CLI suite declares, verbatim: `edge` (api)
/// must not call `core` (web), which the fixture's one bridge binding breaches.
/// Kept byte-identical to `cli/tests/xservice_surface.rs`'s `GOVERNANCE` so both
/// surfaces are asserted over the same declared policy.
const GOVERNANCE_RULES: &str = "
[[governance.service_layers]]
name = \"edge\"
members = [\"api\"]

[[governance.service_layers]]
name = \"core\"
members = [\"web\"]

[[governance.boundaries]]
from = \"edge\"
to = \"core\"
reason = \"edge services must not call core services directly\"
";

/// A member's gated verdict over its OWN `rules.toml`: whether it loaded one,
/// whether it passed, and how many violations it found. What ADR-56 says no
/// workspace governance — reported or saved — may ever move.
fn member_gate_verdict(root: &Path, member: &str) -> (bool, Option<bool>, usize) {
    let engine = Engine::start(root.join(member)).expect("member engine");
    let report = engine.check_rules(None, false).expect("the member evaluates its contract");
    (report.rules_present, report.passed, report.violations.len())
}

/// Append a `[governance]` section to the fixture's workspace manifest.
fn declare_rules(root: &Path, rules: &str) {
    let manifest = root.join("logos.workspace.toml");
    let existing = std::fs::read_to_string(&manifest).expect("the fixture wrote a manifest");
    std::fs::write(&manifest, format!("{existing}{rules}")).expect("append governance");
}

/// Break a member the way the CLI suite does: a **directory** where its store file
/// must be, which no open can succeed against.
fn obstruct_store(root: &Path, member: &str) {
    let db = root.join(member).join(".logos").join("logos.db");
    std::fs::remove_file(&db).expect("clear the store file");
    std::fs::create_dir_all(&db).expect("a directory where the store must be");
}

async fn json_body(router: &axum::Router, path: &str) -> serde_json::Value {
    let resp = router.clone().oneshot(get(path)).await.expect("route responds");
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{path} answers 200: {body}");
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("{path} is JSON: {e}\n{body}"))
}

/// **The bounded default, stated** ([FR-WS-12], [S-294]/[CR-084], [NFR-CC-04]).
///
/// Unbounded, this payload is the union of every member's per-repo dead set. The
/// HTTP default carries only the cross-service promotions and says so: `dead` is
/// `null` — *suppressed* — and never `[]`, which would read as a complete and
/// genuinely empty dead set. The view is labelled advisory and every claim rides
/// its coverage rider, so nothing here can be mistaken for a gate input.
#[tokio::test]
async fn reachability_default_is_promotions_only_states_its_bound_and_is_advisory() {
    let tmp = workspace();
    let router = ws_router(&tmp);
    let v = json_body(&router, "/api/v1/workspace/reachability").await;
    let view = &v["reachability"];

    assert_eq!(view["view"], "cross-service-union", "the labelled union view: {v}");
    assert_eq!(view["advisory"], true, "never a gate input (ADR-56): {v}");
    assert_eq!(view["scope"]["promotions_only"], true, "the default states its bound: {v}");
    assert!(view["scope"]["repo"].is_null(), "no member scope by default: {v}");
    assert!(
        view["dead"].is_null(),
        "the per-repo-dead set is SUPPRESSED to null, not emitted as [] that would read \
         as a complete, empty dead set (NFR-CC-04): {}",
        view["dead"]
    );
    assert!(
        view["live_via_cross_service"].as_array().is_some(),
        "the promotions bucket is always carried, never suppressed: {v}"
    );

    // The coverage rider, with real numbers: this fixture binds its one OpenAPI GET
    // to the axum route across the member boundary. Asserted on values that are
    // non-zero AND on the per-member tally below, so a field swap in
    // `CoverageRider::new` cannot pass as zero-vs-zero.
    let rider = &view["coverage"];
    assert_eq!(rider["bound"], 1, "the GET operation bound its cross-member route: {v}");
    assert_eq!(rider["ambiguous"], 0, "{v}");
    assert_eq!(rider["unbound"], 0, "{v}");
    assert_eq!(rider["spec_conformance_ratio"], 1.0, "{v}");
    assert_eq!(
        rider["spec_conformance_measured"], 1,
        "the ratio never travels without the denominator it was computed over (CR-111): {v}"
    );
    assert_eq!(rider["members_read"], 2, "{v}");
    assert_eq!(rider["members_total"], 2, "{v}");
    assert!(view["skipped_members"].as_array().unwrap().is_empty(), "{v}");

    // The promotion base is real: `web`'s `app` is dead in its own graph, so the
    // suppressed `dead` set above is suppressing something rather than nothing.
    let web_tally = view["members"]
        .as_array()
        .expect("members array")
        .iter()
        .find(|m| m["member"] == "web")
        .unwrap_or_else(|| panic!("web has a tally: {v}"));
    assert_eq!(web_tally["dead_per_repo"], 1, "web really does own a dead callable: {v}");
    assert_eq!(web_tally["dead_app_wide"], 1, "which this view leaves dead: {v}");

    // A complete fan-out says so, with the denominator beside it.
    assert_eq!(v["complete"], true, "every member opened: {v}");
    assert_eq!(v["degraded_rollup"]["members"], 2, "{v}");
    assert_eq!(v["degraded_rollup"]["opened"], 2, "{v}");
    assert!(
        v["degraded_rollup"]["degraded_members"].as_array().unwrap().is_empty(),
        "nothing degraded: {v}"
    );
}

/// `?all` lifts the promotions-only bound and `?repo=` scopes the view to one
/// member — both stated in `scope`, so the payload always names the filter it
/// applied ([CR-084]). `web` owns the dead `orphan`; an `api` scope filters it out.
#[tokio::test]
async fn reachability_all_and_repo_are_applied_and_echoed_in_the_scope() {
    let tmp = workspace();
    let router = ws_router(&tmp);

    let all = json_body(&router, "/api/v1/workspace/reachability?all").await;
    assert_eq!(all["reachability"]["scope"]["promotions_only"], false, "?all lifts it: {all}");
    let dead = all["reachability"]["dead"].as_array().expect("?all populates the dead set");
    // `app` is registered by nobody in this fixture, so web's own graph verdicts it
    // dead and no cross-service edge reaches it — dead app-wide too.
    assert!(
        dead.iter().any(|c| c["name"] == "app" && c["member"] == "web"),
        "web's unreferenced `app` is claimed dead app-wide: {dead:?}"
    );
    // Every claim carries the rider it rests on — it cannot be separated from it.
    let claim = dead.iter().find(|c| c["name"] == "app").expect("the app claim");
    assert_eq!(claim["verdict"], "dead", "{claim}");
    assert_eq!(&claim["coverage"], &all["reachability"]["coverage"], "{claim}");

    let scoped = json_body(&router, "/api/v1/workspace/reachability?all&repo=api").await;
    assert_eq!(scoped["reachability"]["scope"]["repo"], "api", "the scope is stated: {scoped}");
    for tally in scoped["reachability"]["members"].as_array().expect("members array") {
        assert_eq!(tally["member"], "api", "only the scoped member's tally: {scoped}");
    }
    assert!(
        !scoped["reachability"]["dead"]
            .as_array()
            .expect("?all populates the dead set")
            .iter()
            .any(|c| c["name"] == "app"),
        "`app` belongs to web, so an api scope filters it out: {scoped}"
    );
    // …and the scope is a FILTER, not a cap: scoping to the member that owns it
    // keeps it.
    let web_scoped = json_body(&router, "/api/v1/workspace/reachability?all&repo=web").await;
    assert!(
        web_scoped["reachability"]["dead"]
            .as_array()
            .expect("?all populates the dead set")
            .iter()
            .any(|c| c["name"] == "app" && c["member"] == "web"),
        "a web scope keeps web's own dead callable: {web_scoped}"
    );
    // A degraded member is workspace-wide context a scope must never hide.
    assert!(
        web_scoped["reachability"]["skipped_members"].as_array().is_some(),
        "the skipped-member list is always carried: {web_scoped}"
    );
}

/// **`?all` fails closed through the real route** ([NFR-CC-04], [CR-084]).
///
/// The bound this flag lifts is a size bound on a payload the CLI deliberately
/// suppresses by default, so every spelling of "no" must leave it in place. The
/// unit tests pin the reader; this pins the **handler** — that it reads `?all`
/// through the fail-closed reader and not through `wants_flag`, whose only
/// off-token is the literal `0`.
#[tokio::test]
async fn an_all_parameter_spelling_no_leaves_the_promotions_only_bound_in_place() {
    let tmp = workspace();
    let router = ws_router(&tmp);

    for off in ["?all=0", "?all=false", "?all=no", "?all=off", "?all=FALSE", "?all=nope"] {
        let v = json_body(&router, &format!("/api/v1/workspace/reachability{off}")).await;
        assert_eq!(
            v["reachability"]["scope"]["promotions_only"], true,
            "{off} must NOT lift the bound: {v}"
        );
        assert!(
            v["reachability"]["dead"].is_null(),
            "{off} must leave the dead set suppressed: {v}"
        );
    }

    // …and the opt-in still works, so this is a discrimination and not a disabling.
    for on in ["?all", "?all=1", "?all=true", "?all=yes"] {
        let v = json_body(&router, &format!("/api/v1/workspace/reachability{on}")).await;
        assert_eq!(
            v["reachability"]["scope"]["promotions_only"], false,
            "{on} must lift the bound: {v}"
        );
        assert!(v["reachability"]["dead"].as_array().is_some(), "{on}: {v}");
    }
}

/// **The honest empty** ([FR-WS-13], [ADR-56], [NFR-CC-04]): a workspace declaring
/// no rules produces no governance output at all — `null`, never a fabricated
/// zero-violation report that would read as a passing one.
#[tokio::test]
async fn governance_over_a_workspace_with_no_rules_is_null_not_a_passing_report() {
    let tmp = workspace();
    let router = ws_router(&tmp);
    let v = json_body(&router, "/api/v1/workspace/check").await;
    assert!(
        v["governance"].is_null(),
        "no declared rules ⇒ no report: {v}"
    );
    // And specifically NOT the shape a passing report would have.
    assert!(v["governance"].get("violations").is_none(), "{v}");
    assert!(v["governance"].get("rules_checked").is_none(), "{v}");
    assert_eq!(v["complete"], true, "the answer is still complete: {v}");
}

/// A declared rule evaluates over the bridge bindings and its breach is
/// **reported** — at `200`, with the member's own gated signal untouched. The
/// workspace tier is advisory: there is no exit code here to move, and no
/// member's `check` verdict moves either ([ADR-56]).
#[tokio::test]
async fn a_governance_violation_is_reported_at_200_and_moves_no_member_gate() {
    let tmp = workspace();
    // Give `web` a per-repo contract that genuinely FAILS, so "the member's gated
    // signal is unchanged" is a comparison of something rather than of two
    // nothings: every function has cyclomatic complexity >= 1, so `max_cc = 0`
    // always fires.
    std::fs::create_dir_all(tmp.path().join("web/.logos")).unwrap();
    std::fs::write(tmp.path().join("web/.logos/rules.toml"), "[constraints]\nmax_cc = 0\n").unwrap();

    let member_verdict = |tmp: &TempDir| member_gate_verdict(tmp.path(), "web");
    let before = member_verdict(&tmp);
    assert!(before.0, "the member loaded its own rules.toml");
    assert_eq!(before.1, Some(false), "and its gated verdict is a real FAIL, not an absent one");
    assert!(before.2 > 0, "with real violations");

    declare_rules(tmp.path(), GOVERNANCE_RULES);
    let router = ws_router(&tmp);
    let v = json_body(&router, "/api/v1/workspace/check").await;

    let report = &v["governance"];
    assert_eq!(report["workspace"], "shop", "{v}");
    assert_eq!(report["rules_checked"], 1, "{v}");
    assert_eq!(report["bindings_checked"], 1, "quantified over the one binding: {v}");
    let violations = report["violations"].as_array().expect("violations array");
    assert_eq!(violations.len(), 1, "the edge→core binding breaches the rule: {v}");
    assert_eq!(violations[0]["rule"], "workspace-boundary:edge->core", "{v}");
    assert_eq!(violations[0]["severity"], "error", "{v}");
    assert_eq!(violations[0]["from"]["member"], "api", "{v}");
    assert_eq!(violations[0]["to"]["member"], "web", "{v}");

    // The invariant: the member's real, failing gated signal is byte-identical.
    assert_eq!(member_verdict(&tmp), before, "no member's gated signal moved (ADR-56)");
}

/// **A malformed rule is a `500`, never a `null`** ([NFR-RA-05], [NFR-CC-04]).
///
/// This is the only behaviour that distinguishes `workspace_fan_try` from
/// `workspace_fan`, and it was the one path with no test: review agent 4 swallowed
/// the error (`.unwrap_or(None)`) and all 29 tests stayed green while a *failed*
/// check rendered as `governance: null`. That is the fourth-surface untruth this
/// sprint is removing elsewhere — to a consumer, `null` means "no policy declared"
/// (consumer assumption 3), so a workspace whose rules did not compile would read
/// as a workspace that declared none.
///
/// Both compile failures are asserted: an undeclared layer reference and a symbol
/// glob that will not parse. The self-only CSP is asserted on the `500` too — it is
/// the only `500` covered anywhere on this surface, and the outer layer's stamping
/// is otherwise only ever checked on `200`s and `404`s.
#[tokio::test]
async fn a_governance_rule_that_will_not_compile_is_a_500_and_never_an_honest_empty() {
    // Each of these fails `CompiledWorkspaceRules::compile` for a different reason.
    let cases = [
        (
            "an undeclared layer reference",
            "
[[governance.service_layers]]
name = \"edge\"
members = [\"api\"]

[[governance.boundaries]]
from = \"edge\"
to = \"ghost\"
",
            "undeclared service layer",
        ),
        (
            "a symbol glob that will not parse",
            "
[[governance.no_cross_service_callers]]
symbol = \"[\"
",
            "unclosed character class",
        ),
    ];

    for (what, rules, expected) in cases {
        let tmp = workspace();
        declare_rules(tmp.path(), rules);
        let router = ws_router(&tmp);
        let resp = router.oneshot(get("/api/v1/workspace/check")).await.expect("route responds");
        let (status, body, headers) = body_string(resp).await;

        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{what} must fail loud, not render as an honest empty: {body}"
        );
        let v: serde_json::Value = serde_json::from_str(&body).expect("the 500 body is JSON");
        assert!(
            v["error"].as_str().is_some_and(|e| e.contains(expected)),
            "{what}: the error chain names the cause: {body}"
        );
        // The decisive negative: it must not have taken the null branch.
        assert!(v.get("governance").is_none(), "{what} is not a governance answer: {body}");
        assert_self_only_csp(&headers, "/api/v1/workspace/check (500)");
    }
}

/// **A partial fan-out never presents itself as complete** ([FR-WS-16],
/// [NFR-CC-04]). The CLI states this in its exit code and a stderr notice; an HTTP
/// `200` has neither channel, so both payloads carry it — and they **name** the
/// member, because `covers_all_members == false` says *that* the workspace
/// degraded, not *where*.
#[tokio::test]
async fn an_unopenable_member_renders_both_answers_incomplete_and_names_it() {
    let tmp = workspace();
    declare_rules(tmp.path(), GOVERNANCE_RULES);
    obstruct_store(tmp.path(), "web");
    let router = ws_router(&tmp);

    // This pair, not WORKSPACE_ENDPOINTS: `complete` and `degraded_rollup` are the
    // AnswerCompleteness rider, which only the two read-models that carry it can
    // answer. A genuine semantic subset rather than a frozen list.
    for path in ["/api/v1/workspace/reachability", "/api/v1/workspace/check"] {
        let v = json_body(&router, path).await;
        assert_eq!(v["complete"], false, "{path} declares the answer incomplete: {v}");
        assert_eq!(
            v["degraded_rollup"]["degraded_members"].as_array().unwrap(),
            &vec![serde_json::Value::from("web")],
            "{path} NAMES the member it could not open: {v}"
        );
        assert_eq!(v["degraded_rollup"]["members"], 2, "{path} states the denominator: {v}");
        assert_eq!(v["degraded_rollup"]["covers_all_members"], false, "{path}: {v}");
    }
}

/// Under a **single-root** backing both routes answer the honest `404` the rest of
/// the fan-out answers, and the registry is never allocated — a plain repo pays
/// nothing for them ([ADR-52]).
///
/// The `404` itself is covered for the whole enumerated set by
/// [`single_root_workspace_endpoints_are_404`]; what this adds is the
/// **non-allocation**, asserted on the resolved backing rather than inferred from
/// the status code.
#[tokio::test]
async fn single_root_answers_404_for_both_routes_without_allocating_a_registry() {
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path(), "src/lib.rs", "pub fn f() {}\n");

    let backing = web::resolve_serve_backing(tmp.path(), false).expect("resolves");
    assert!(!backing.is_federated(), "a plain repo resolves the single-root backing");
    assert!(
        backing.as_federated().is_none(),
        "no member registry is allocated for a plain repo (ADR-52)"
    );

    let router = web::router_for_backing(Arc::new(backing)).expect("the router builds");
    for path in ["/api/v1/workspace/reachability", "/api/v1/workspace/check"] {
        let resp = router.clone().oneshot(get(path)).await.expect("route responds");
        let (status, body, headers) = body_string(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path} is 404 in single-root mode: {body}");
        assert!(body.contains("not a workspace"), "{path} explains why: {body}");
        assert_self_only_csp(&headers, path);
    }
}

/// A content digest of one member store — **every** user table, not a chosen few.
///
/// `count(*)` over a hand-picked table list is a proxy, and review agent 4 broke it:
/// an `INSERT OR REPLACE` into `project_metadata` on every `check` GET passed the
/// old three-table count comparison. The schema has ~29 tables, and a count is
/// blind in two directions at once — an insert into any table it does not watch,
/// and an in-place UPDATE of a row in one it does.
///
/// So this enumerates the store's own `sqlite_master` and digests each table's full
/// contents, which closes both directions: a new row, a changed row and a deleted
/// row all move the digest. `quote()` renders every column including NULLs and
/// BLOBs, and the ordering is fixed by `rowid` so the digest is stable across
/// re-reads of an unchanged store.
fn store_digest(db: &Path) -> Vec<(String, String)> {
    if !db.exists() {
        return Vec::new();
    }
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap_or_else(|e| panic!("open {} read-only: {e}", db.display()));
    let tables: Vec<String> = {
        let mut q = conn
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .expect("enumerate tables");
        let rows = q.query_map([], |r| r.get::<_, String>(0)).expect("table names");
        rows.map(|r| r.expect("a table name")).collect()
    };
    assert!(
        !tables.is_empty(),
        "{} declares no tables — the digest would be vacuous",
        db.display()
    );
    tables
        .into_iter()
        .map(|t| {
            // `quote(t.*)` is not valid SQLite, so build the column list explicitly.
            let cols: Vec<String> = {
                let mut q = conn
                    .prepare(&format!("SELECT name FROM pragma_table_info('{t}')"))
                    .expect("columns");
                let rows = q.query_map([], |r| r.get::<_, String>(0)).expect("column names");
                rows.map(|r| r.expect("a column name")).collect()
            };
            let quoted = cols
                .iter()
                .map(|c| format!("quote(\"{c}\")"))
                .collect::<Vec<_>>()
                .join("||','||");
            let sql = if quoted.is_empty() {
                format!("SELECT count(*) || ':' FROM \"{t}\"")
            } else {
                // Ordered by the RENDERED row text, not by `rowid`: the FTS shadow
                // tables have no rowid, and the rendered text is deterministic for
                // an unchanged table whatever the storage order.
                format!(
                    "SELECT count(*) || ':' || coalesce(group_concat(r, '|'), '') \
                     FROM (SELECT {quoted} AS r FROM \"{t}\" ORDER BY 1)"
                )
            };
            let digest: String = conn
                .query_row(&sql, [], |r| r.get(0))
                .unwrap_or_else(|e| panic!("digest {t}: {e}"));
            (t, digest)
        })
        .collect()
}

/// Both stores a member keeps: the graph store and the temporal history store. The
/// single-root guard this mirrors (`read_only_views.rs`) watches both, and the AC
/// says "every store's contents", so watching only `logos.db` would answer a
/// narrower question than the one asked.
fn member_digests(root: &Path, member: &str) -> Vec<(String, Vec<(String, String)>)> {
    ["logos.db", "history.db"]
        .into_iter()
        .map(|f| {
            let path = root.join(member).join(".logos").join(f);
            (format!("{member}/{f}"), store_digest(&path))
        })
        .collect()
}

/// **Write-free on read** ([FR-UI-03], [ADR-28]): loading every enumerated
/// workspace endpoint — once **and** repeatedly — leaves every member store's
/// contents unchanged.
///
/// Driven off [`WORKSPACE_ENDPOINTS`] rather than an ad-hoc list, so a route added
/// to the fan-out without being added there cannot exist. The comparison is a
/// full-content digest of every table of both of each member's stores, so an insert
/// into a table nobody thought to watch, and an in-place update that leaves the row
/// count alone, both fail it.
#[tokio::test]
async fn loading_every_workspace_endpoint_repeatedly_leaves_every_store_unchanged() {
    let tmp = workspace();
    declare_rules(tmp.path(), GOVERNANCE_RULES);
    let members = ["api", "web"];

    let before: Vec<_> = members.iter().map(|m| member_digests(tmp.path(), m)).collect();
    // The denominator: the graph store really does hold rows, so "unchanged" is a
    // statement about content and not about two empty stores.
    for (member, stores) in members.iter().zip(&before) {
        let (name, tables) = &stores[0];
        let nodes = tables
            .iter()
            .find(|(t, _)| t == "nodes")
            .unwrap_or_else(|| panic!("{name} has a nodes table"));
        assert!(
            !nodes.1.starts_with("0:"),
            "member {member} was indexed, so the digest has real content: {}",
            nodes.1
        );
    }

    let router = ws_router(&tmp);
    for path in WORKSPACE_ENDPOINTS {
        for _ in 0..2 {
            let resp = router.clone().oneshot(get(path)).await.expect("route responds");
            let (status, body, _h) = body_string(resp).await;
            assert_eq!(status, StatusCode::OK, "{path} answers 200: {body}");
        }
    }

    let after: Vec<_> = members.iter().map(|m| member_digests(tmp.path(), m)).collect();
    for (member, (b, a)) in members.iter().zip(before.iter().zip(&after)) {
        for ((bname, btables), (aname, atables)) in b.iter().zip(a) {
            assert_eq!(bname, aname, "the same stores are compared");
            // Compare table-by-table so a failure names the table that moved.
            let changed: Vec<&str> = btables
                .iter()
                .zip(atables)
                .filter(|((bt, bd), (at, ad))| bt == at && bd != ad)
                .map(|((t, _), _)| t.as_str())
                .collect();
            assert!(
                changed.is_empty() && btables.len() == atables.len(),
                "member {member}: a GET on the workspace fan-out changed {bname} \
                 (tables: {changed:?}) — ADR-28, FR-UI-03"
            );
        }
    }
}

/// **No widening of the resident-engine ceiling** ([NFR-PE-10]).
///
/// Measured on **two** instruments, because neither alone is enough:
/// `live_read_connections()` (residents × the per-member read pool — the unit
/// [NFR-PE-11]'s budget is written in) and `engine_starts()`. Core names the second
/// as *the* instrument for this claim: "A read-model that must not construct
/// engines of its own therefore asserts on this — `starts == walks × members` — not
/// on `resident_count()`" (`registry.rs`). It is strictly the sharper of the two,
/// because a handler that opens a member twice inside one fan-out, or thrashes it
/// through eviction, moves `starts` while leaving residency — and therefore the
/// connection count — exactly where it was.
///
/// Each route is measured on its **own fresh registry**, warm on the default member
/// only. Measuring them in sequence on one registry would compare each new route
/// against a ceiling the earlier ones had already raised, which is a comparison
/// that cannot fail.
///
/// # What this can fail on, and what it cannot
/// It fails on a handler that constructs more engines than the fan-out it joined —
/// an extra all-member walk, a repeated open, eviction thrash. It **cannot** see
/// cost paid outside this registry: a handler that built a whole second
/// `EngineRegistry` of its own would leak read connections per request and move
/// neither readout here, because both are per-instance counters on the registry the
/// router serves from. Nothing observable from `backing` can catch that, so this
/// test does not claim to — the guard against it is `workspace_read` being the one
/// seam that reaches a registry at all, and review. Said explicitly because an
/// earlier revision of this comment claimed the opposite, and a review agent
/// disproved it by leaking 24 connections per request past a green assertion.
#[tokio::test]
async fn neither_route_widens_the_resident_engine_ceiling_beyond_the_existing_fan_out() {
    let tmp = workspace();
    declare_rules(tmp.path(), GOVERNANCE_RULES);

    /// Serve `path` on a registry warm on the default member only, and return the
    /// live read-connection count and the engine-start count it leaves behind.
    async fn cost_of(tmp: &TempDir, path: &str) -> (usize, u64) {
        let federation = discover(tmp.path()).expect("discovery").expect("a workspace");
        let backing = Arc::new(Backing::Federated(Box::new(
            EngineRegistry::<Engine>::new_serve_default(federation),
        )));
        let router = web::router_for_backing(Arc::clone(&backing)).expect("the router builds");
        let resp = router.oneshot(get(path)).await.expect("route responds");
        assert_eq!(resp.status(), StatusCode::OK, "{path}");
        let registry = backing.as_federated().expect("the federated registry");
        (registry.live_read_connections(), registry.engine_starts())
    }

    // The baseline: what the widest route of the EXISTING fan-out already pays.
    let (status_conns, status_starts) = cost_of(&tmp, "/api/v1/workspace/status").await;
    assert!(status_conns > 0, "the existing fan-out really does hold read connections");
    assert!(status_starts > 0, "and really does construct member engines");

    // Driven off WORKSPACE_ENDPOINTS rather than a frozen pair. The list this loop
    // used to carry was written when those were the two newest routes; a route
    // added later then sits silently outside the one test named for the ceiling it
    // is meant to hold. `status` is the baseline itself and `roster` has its own
    // dedicated test, but neither needs excluding — both trivially satisfy
    // `<= status`, and excluding them would reintroduce exactly the
    // hand-maintained list this replaces.
    for path in WORKSPACE_ENDPOINTS {
        let (conns, starts) = cost_of(&tmp, path).await;
        assert!(
            conns <= status_conns,
            "{path} holds {conns} read connections where the existing fan-out pays \
             {status_conns} — the resident-engine ceiling widened (NFR-PE-10)"
        );
        assert!(
            starts <= status_starts,
            "{path} started {starts} member engines where the existing fan-out starts \
             {status_starts} — it constructs engines the fan-out it joined does not \
             (NFR-PE-10)"
        );
    }
}

/// **The enumeration is itself guarded** ([FR-UI-21] AC, [FR-UI-03]).
///
/// [`WORKSPACE_ENDPOINTS`] is what the `200`+CSP loop, the single-root `404` loop
/// and the write-free loop all walk, so a route missing from it is unguarded on all
/// three — and silently, which is the failure mode an enumerated list has instead of
/// a wildcard. The `POST` routes live in [`WORKSPACE_WRITE_ENDPOINTS`] instead
/// (those loops issue `GET`s), and are walked by the S-450 tests. This asserts the
/// union of the two lists and the router's own route table name exactly the same
/// set, read out of `src/lib.rs` at compile time. Set equality in both
/// directions, so it needs no count to keep up to date.
///
/// # Two blind spots a line-based scan had, both closed
/// The first version split each *line* on `.route("`, which two real patterns in
/// the same file defeat. A review agent proved the first by adding a route whose
/// call rustfmt wraps across four lines: invisible to the scan, so the guard passed
/// over an unenumerated route. The second is already live here — `VERIFY_POST_ROUTE`
/// registers a path by **constant**, not by inline literal, so a future
/// `/api/v1/workspace/*` GET written that way would also slip through.
///
/// So the scan runs over the whole source as one string (formatting-independent),
/// and a second assertion requires that no `/api/v1/workspace/` path is declared as
/// a `const`. The `!declared.is_empty()` check remains, but note what it is and is
/// not: it catches a *total* parse failure, not a partial one — the second
/// assertion is what covers the const form.
#[test]
fn the_enumerated_endpoint_list_is_exactly_the_routers_workspace_route_table() {
    const ROUTER_SOURCE: &str = include_str!("../src/lib.rs");

    // Whole-source scan: `.route(` followed by a string literal, wherever the line
    // breaks fall.
    let mut declared: Vec<&str> = ROUTER_SOURCE
        .split(".route(")
        .skip(1)
        .filter_map(|rest| rest.trim_start().strip_prefix('"'))
        .filter_map(|rest| rest.split('"').next())
        .filter(|path| path.starts_with("/api/v1/workspace/"))
        .collect();
    declared.sort_unstable();
    declared.dedup();

    let mut enumerated: Vec<&str> = WORKSPACE_ENDPOINTS
        .iter()
        .map(|e| e.split('?').next().expect("a path before any query string"))
        .chain(WORKSPACE_WRITE_ENDPOINTS.iter().map(|(path, _)| *path))
        .collect();
    enumerated.sort_unstable();
    enumerated.dedup();

    assert!(
        !declared.is_empty(),
        "the route-table scan found nothing — it stopped matching src/lib.rs"
    );
    assert_eq!(
        enumerated, declared,
        "WORKSPACE_ENDPOINTS + WORKSPACE_WRITE_ENDPOINTS and the router's \
         /api/v1/workspace/* route table have drifted: a route in the table but not \
         the lists is unguarded by every loop that walks them"
    );

    // The const form the inline scan cannot see. `VERIFY_POST_ROUTE` is the live
    // precedent for it in this very file, so this is a pattern already in use.
    let const_declared: Vec<&str> = ROUTER_SOURCE
        .lines()
        .filter(|l| l.contains("const ") && l.contains(": &str"))
        .filter_map(|l| l.split('"').nth(1))
        .filter(|path| path.starts_with("/api/v1/workspace/"))
        .collect();
    assert!(
        const_declared.is_empty(),
        "a /api/v1/workspace/* path is declared as a const ({const_declared:?}), which \
         the route-table scan above cannot see. Either inline the literal at its \
         `.route(` call or teach this guard to resolve the constant — do not leave \
         the route enumerable only by hand."
    );
}

// ── S-450 / FR-WS-30: the workspace root is a config root ─────────────────────
//
// One read and two writes in the `/api/v1/workspace/*` family, calling the SAME
// validate-before-write writers a member's Config tab calls, pointed at the
// federation root the backing already holds. No engine is constructed there
// ([ADR-40]'s exception) and no apply route exists at this scope.

/// A single-root serve answers the config read and every workspace write — the
/// S-430 manifest save included — with the family's own not-a-workspace `404`,
/// and the writes write nothing ([FR-WS-30], [ADR-52]). The `GET` is also walked
/// by `single_root_workspace_endpoints_are_404`; the `POST`s can only be walked
/// here, with a valid intent token, so the `404` is the handler's and not a
/// guard's `403`/`405`.
#[tokio::test]
async fn single_root_answers_the_workspace_config_routes_with_the_not_a_workspace_refusal() {
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path(), "src/lib.rs", "pub fn f() {}\n");
    let intent = IntentToken::generate();
    let engine = Arc::new(Engine::start(tmp.path()).expect("engine starts"));
    let router = web::router_with_intent(engine, intent.clone());
    let before = walk(tmp.path());

    // The family's standard refusal, byte for byte: what an existing fan-out
    // route answers in the same single-root serve.
    let resp = router.clone().oneshot(get("/api/v1/workspace/statistics")).await.unwrap();
    let (status, standard, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{standard}");
    assert!(api_error(&standard).starts_with("not a workspace:"), "{standard}");

    let resp = router.clone().oneshot(get("/api/v1/workspace/config")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body, standard, "the read refuses in the family's own words");

    for (path, form) in WORKSPACE_WRITE_ENDPOINTS {
        let resp = router.clone().oneshot(post_form(path, *form, &intent)).await.unwrap();
        let (status, body, headers) = body_string(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path} is refused in single-root mode: {body}");
        assert_eq!(body, standard, "{path} refuses in the family's own words");
        assert_self_only_csp(&headers, path);
    }
    assert_eq!(walk(tmp.path()), before, "a refused workspace write wrote nothing");
}

/// **Every workspace write is registered in the enumerated config-write allow-list**
/// ([NFR-SE-06]). The method guard matches on exact path equality, so a route
/// missing from [`web::CONFIG_POST_ROUTES`] is `405` before it routes: this reds
/// on the list, on the handler being reachable through the whole guard stack, and
/// on the match staying exact (a near-miss path one character off is `405`).
///
/// The generic direction — *any* `POST`-mounted route, added later and not
/// listed — is `every_post_mounted_route_is_admitted_by_the_method_guard` in
/// `src/lib.rs`, which can reach the guard's private predicate.
#[tokio::test]
async fn every_workspace_write_is_in_the_config_write_allow_list_and_nothing_near_them_is() {
    let tmp = workspace();
    let (router, intent) = ws_router_with_intent(&tmp);
    for (path, form) in WORKSPACE_WRITE_ENDPOINTS {
        assert!(
            web::CONFIG_POST_ROUTES.contains(path),
            "{path} is not in CONFIG_POST_ROUTES, so the method guard refuses it"
        );
        let resp = router.clone().oneshot(post_form(path, *form, &intent)).await.unwrap();
        let (status, body, _h) = body_string(resp).await;
        assert_eq!(status, StatusCode::OK, "{path} reaches its handler: {body}");

        for near in [format!("{path}/"), format!("{path}x"), path.to_uppercase()] {
            let resp = router.clone().oneshot(post_form(&near, *form, &intent)).await.unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "POST {near} is not an enumerated route and must stay 405"
            );
        }
    }
}

/// The writes carry the same same-origin + intent proof as every mutating route
/// ([NFR-SE-06]): a cross-origin write and a tokenless write are `403` and leave
/// the workspace root untouched.
#[tokio::test]
async fn workspace_writes_are_refused_without_the_intent_proof() {
    let tmp = workspace();
    let (router, intent) = ws_router_with_intent(&tmp);
    let before = walk(tmp.path());
    for (path, form) in WORKSPACE_WRITE_ENDPOINTS {
        for (origin, token) in [
            ("http://evil.example", Some(intent.as_str())),
            ("http://127.0.0.1:4983", None),
        ] {
            let mut req = Request::builder()
                .method(Method::POST)
                .uri(*path)
                .header(header::HOST, "127.0.0.1:4983")
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            if let Some(token) = token {
                req = req.header(INTENT_HEADER, token);
            }
            let resp = router.clone().oneshot(req.body(Body::from(*form)).unwrap()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{path} from {origin}, token {token:?}");
        }
    }
    assert_eq!(walk(tmp.path()), before, "a forged workspace write wrote nothing");
}

/// A save → read round-trip at the workspace root persists through the existing
/// writers ([FR-WS-30], [NFR-RA-07]): the literal document reads back byte for
/// byte, the credential reads back masked only ([NFR-SE-07]) and sits on disk at
/// mode **0o600**, and a member that declares nothing now inherits both halves
/// from exactly this tier — the one the seam reads.
///
/// And the [ADR-40] fault this exception exists to avoid: after the full cycle
/// the workspace root holds **no graph store** — `.logos/` carries the two files
/// written and nothing else — and a re-discovery still names the same two members,
/// so the `.logos/` the save created is never admitted as one ([FR-WS-01]).
#[tokio::test]
async fn a_workspace_save_then_read_round_trips_and_leaves_no_graph_store_at_the_root() {
    let tmp = workspace();
    let root = tmp.path();
    let raw_key = "sk-workspace-round-trip-key-rt77";
    let document = "# the estate's one chat policy\n[chat]\nprovider = \"anthropic\"\nmodel = \"ws/round-trip\"\n";
    let (router, intent) = ws_router_with_intent(&tmp);
    let mut bodies = Vec::new();

    let resp = router
        .clone()
        .oneshot(post_form(
            "/api/v1/workspace/config/save",
            format!("{}&fingerprint={EMPTY_DOCUMENT_FINGERPRINT}", form_field("content", document)),
            &intent,
        ))
        .await
        .unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["path"], ".logos/config.toml", "{body}");
    assert_eq!(v["outcome"], "written", "{body}");
    bodies.push(body);

    let resp = router
        .clone()
        .oneshot(post_form("/api/v1/workspace/config/secret", form_field("api_key", raw_key), &intent))
        .await
        .unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["chat_key"], serde_json::json!({"present": true, "last4": "rt77"}), "{body}");
    bodies.push(body);

    // On disk: the literal bytes, and the credential owner-only.
    assert_eq!(std::fs::read_to_string(root.join(".logos/config.toml")).unwrap(), document);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(root.join(".logos/secrets.toml")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the workspace credential is owner-only, got {:o}", mode & 0o777);
    }

    // Read back through the workspace route: the literal document, the masked key.
    let resp = router.clone().oneshot(get("/api/v1/workspace/config")).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_self_only_csp(&headers, "/api/v1/workspace/config");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["config"]["content"], document, "{body}");
    assert_eq!(v["config"]["parsed"]["chat"]["model"], "ws/round-trip", "{body}");
    assert_eq!(v["chat_key"], serde_json::json!({"present": true, "last4": "rt77"}), "{body}");
    // The workspace root has no tier above it, so nothing here is inherited.
    assert_eq!(v["effective_chat"]["policy_origin"], "member", "{body}");
    assert_eq!(v["effective_chat"]["credential_origin"], "member", "{body}");
    bodies.push(body);

    // …and the member that declares nothing inherits both halves from this tier.
    let resp = router.clone().oneshot(get("/api/v1/config?repo=web")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["effective_chat"]["policy"]["model"], "ws/round-trip", "{body}");
    assert_eq!(v["effective_chat"]["policy_origin"], "workspace", "{body}");
    assert_eq!(v["effective_chat"]["credential_origin"], "workspace", "{body}");
    bodies.push(body);

    for body in &bodies {
        assert!(!body.contains(raw_key) && !body.contains("round-trip-key"), "the raw key leaked: {body}");
    }

    // ADR-40: no engine was constructed at the workspace root. A graph store is
    // the evidence one leaves behind, so `.logos/` holds exactly the two files
    // written and the managed `.gitignore` the writers keep beside them.
    let mut at_root: Vec<String> = std::fs::read_dir(root.join(".logos"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    at_root.sort();
    assert_eq!(
        at_root,
        [".gitignore", "config.toml", "secrets.toml"],
        "the workspace root carries only what was written"
    );
    assert!(!root.join(".logos/logos.db").exists(), "no graph store at the workspace root");

    // FR-WS-01, as a regression guard: the member set is unchanged after the
    // save. (A `.logos/` holding only these files is not admissible anyway — the
    // exclusion itself is proven by the core fixture
    // `a_logos_dir_at_the_workspace_root_is_never_a_member`.)
    let federation = discover(root).expect("discovery succeeds").expect("a workspace");
    let names: Vec<&str> = federation.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["api", "web"], "the member set is unchanged by a workspace-root .logos/");
}

/// The `/api/v1` family's error body: a JSON object whose `error` is a string —
/// the shape the workspace-tier routes promise their consumer, unlike the member
/// `/config/*` routes' plain text. Panics (naming the body) on any other shape.
fn api_error(body: &str) -> String {
    let v: serde_json::Value =
        serde_json::from_str(body).unwrap_or_else(|e| panic!("the error body is JSON ({e}): {body}"));
    v["error"]
        .as_str()
        .unwrap_or_else(|| panic!("the error body carries an `error` string: {body}"))
        .to_string()
}

/// A refused save leaves its target **byte-identical** ([NFR-RA-07]): an invalid
/// document is a `422` over the existing workspace file, and a credential write
/// over an unparsable store is a `422` that neither overwrites it nor echoes the
/// key it was handed ([NFR-SE-07]).
#[tokio::test]
async fn a_refused_workspace_save_leaves_the_target_byte_identical() {
    let tmp = workspace();
    let root = tmp.path();
    let config = "[chat]\nmodel = \"ws/kept\"\n";
    let secrets = "[chat]\napi_key = sk-unquoted-stays-put\n";
    write(root, ".logos/config.toml", config);
    write(root, ".logos/secrets.toml", secrets);
    let (router, intent) = ws_router_with_intent(&tmp);

    let resp = router
        .clone()
        .oneshot(post_form(
            "/api/v1/workspace/config/save",
            format!("{}&fingerprint=any-load", form_field("content", "[chat]\nmodle = \"typo\"\n")),
            &intent,
        ))
        .await
        .unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "an invalid document is the client's fault: {body}");
    assert!(api_error(&body).contains("modle"), "the refusal names the offending key: {body}");
    assert_eq!(std::fs::read_to_string(root.join(".logos/config.toml")).unwrap(), config);

    let resp = router
        .clone()
        .oneshot(post_form("/api/v1/workspace/config/secret", "api_key=sk-handed-in-key-hk12", &intent))
        .await
        .unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(!body.contains("handed-in-key") && !body.contains("unquoted-stays-put"), "no key echoed: {body}");
    assert!(!api_error(&body).is_empty(), "the refusal is the family's JSON error: {body}");
    assert_eq!(std::fs::read_to_string(root.join(".logos/secrets.toml")).unwrap(), secrets);
}

/// The write route is the **repair path** for the fault S-448 keeps fail-loud: a
/// broken workspace-root `config.toml` makes every inheriting member's config read
/// a `500`, and a save of a valid document over it succeeds — the writer
/// validates the NEW content, never the old — after which the member reads again.
///
/// The workspace read itself is NOT fail-loud (S-451 T2): it delivers the broken
/// document with its fingerprint and a `null` parse, which is what the save is
/// then made against.
#[tokio::test]
async fn a_workspace_save_repairs_a_broken_workspace_file_the_member_read_fails_on() {
    let tmp = workspace();
    write(tmp.path(), ".logos/config.toml", "[chat]\nmodle = \"ws/typo\"\n");
    let (router, intent) = ws_router_with_intent(&tmp);

    let resp = router.clone().oneshot(get("/api/v1/config?repo=web")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR, "the broken tier fails the inheriting member loud");
    // …while the workspace read delivers the broken file for repair — never a
    // defaulted read-model standing in for it.
    let v = json_body(&router, "/api/v1/workspace/config").await;
    assert!(v["config"]["parsed"].is_null(), "{v}");
    assert!(v["config"]["error"].as_str().is_some_and(|e| e.contains("line 2, column 1")), "{v}");
    let fingerprint = v["config"]["fingerprint"].as_str().unwrap().to_string();

    let (status, body, _h) = save_tier(&router, &intent, "[chat]\nmodel = \"ws/repaired\"\n", &fingerprint).await;
    assert_eq!(status, StatusCode::OK, "the save validates the new document, not the broken one: {body}");

    let resp = router.clone().oneshot(get("/api/v1/config?repo=web")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "the member reads again: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["effective_chat"]["policy"]["model"], "ws/repaired", "{body}");

    let v = json_body(&router, "/api/v1/workspace/config").await;
    assert_eq!(v["config"]["parsed"]["chat"]["model"], "ws/repaired", "the workspace read parses again: {v}");
}

/// The workspace tier carries the chat policy and its credential, and nothing
/// else: `file=rules` — or any `file` other than `config` — is refused `400` and
/// writes nothing there, because
/// workspace governance is declared in the manifest ([FR-WS-13]) and a rules file
/// at the root would be read by nothing.
#[tokio::test]
async fn the_workspace_save_accepts_the_config_document_only() {
    let tmp = workspace();
    let (router, intent) = ws_router_with_intent(&tmp);

    let resp = router
        .clone()
        .oneshot(post_form("/api/v1/workspace/config/save", "file=rules&content=%23%20rules%0A", &intent))
        .await
        .unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(api_error(&body).contains("file=config"), "the refusal says what is accepted: {body}");
    assert!(!tmp.path().join(".logos/rules.toml").exists(), "no rules file at the workspace root");
    assert!(!tmp.path().join(".logos").exists(), "a refused save creates nothing");

    // An allow-list, not a deny-list of `rules`: any other spelling — including
    // the near miss `Config` — is refused too, and writes nothing.
    for other in ["bogus", "Config", ""] {
        let resp = router
            .clone()
            .oneshot(post_form(
                "/api/v1/workspace/config/save",
                format!("file={other}&{}", form_field("content", "[chat]\nmodel = \"ws/m\"\n")),
                &intent,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "`file={other}` is refused");
        assert!(!tmp.path().join(".logos").exists(), "`file={other}` wrote nothing");
    }

    let resp = router
        .oneshot(post_form(
            "/api/v1/workspace/config/save",
            format!(
                "file=config&{}&fingerprint={EMPTY_DOCUMENT_FINGERPRINT}",
                form_field("content", "[chat]\nmodel = \"ws/m\"\n")
            ),
            &intent,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "`file=config` is the one document accepted");
}

/// No apply/reconcile route exists at this scope ([ADR-40]): applying a config
/// runs the pipeline, which needs an engine, which is exactly what the workspace
/// root must never get. A well-formed `POST` there is `405` — unlisted, unrouted.
#[tokio::test]
async fn there_is_no_workspace_apply_route() {
    let tmp = workspace();
    let (router, intent) = ws_router_with_intent(&tmp);
    assert!(!web::CONFIG_POST_ROUTES.iter().any(|r| r.starts_with("/api/v1/workspace/") && r.contains("apply")));
    let resp = router
        .oneshot(post_form("/api/v1/workspace/config/apply", "file=config", &intent))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
}

/// The workspace credential is kept out of version control **whoever authored
/// the manifest** ([NFR-SE-07], [FR-WS-30]). A hand-written manifest at a root
/// that is itself a git working tree never ran `logos init --workspace`, so no
/// managed root `.gitignore` exists — and a credential written through the
/// route must still be ignored, by the `.logos/.gitignore` the writer keeps
/// beside it. Asserted through git's own verdict, not the file's text.
#[tokio::test]
async fn a_credential_written_under_a_hand_written_manifest_at_a_tracked_root_is_ignored() {
    let tmp = workspace();
    let root = tmp.path();
    sh_git(root, &["init", "-q", "-b", "main"]);
    assert!(!root.join(".gitignore").exists(), "no enablement ran, so no managed root ignore");
    let (router, intent) = ws_router_with_intent(&tmp);

    let resp = router
        .oneshot(post_form("/api/v1/workspace/config/secret", "api_key=sk-hand-written-root-hw05", &intent))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(root.join(".logos/secrets.toml").is_file(), "the credential was written");

    let ignored = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "core.excludesFile=/dev/null", "check-ignore", "-q", ".logos/secrets.toml"])
        .status()
        .expect("git is on PATH")
        .success();
    assert!(ignored, "git ignores the workspace credential");
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "core.excludesFile=/dev/null", "status", "--porcelain", "--untracked-files=all", "--", ".logos"])
        .output()
        .expect("git is on PATH");
    let status = String::from_utf8_lossy(&status.stdout);
    assert!(!status.contains("secrets.toml"), "`git add -A` would not pick the key up: {status}");
}

// ── S-430 / FR-UI-38: the workspace manifest is an editable document ──────────
//
// One read and one intent-guarded save over `logos.workspace.toml` itself, through
// core's whole-manifest write path. The contract asserted here is what makes it
// safe to put a file that governs N repositories behind a Save button: it
// validates before it writes, writes nothing for a no-op, refuses to clobber an
// edit it never saw, and touches no member.

/// Every file under `<root>/<member>/.logos/` with its size and mtime — the
/// evidence a reindex, a gate run or a store write leaves behind. A member store
/// that is merely *opened* read-only moves none of these.
fn member_logos_stat(root: &Path, member: &str) -> Vec<(String, u64, std::time::SystemTime)> {
    let dir = root.join(member).join(".logos");
    let mut out: Vec<_> = walk(&dir)
        .into_iter()
        .map(|rel| {
            let meta = std::fs::metadata(dir.join(&rel)).expect("a walked file has metadata");
            (format!("{member}/.logos/{rel}"), meta.len(), meta.modified().expect("an mtime"))
        })
        .collect();
    out.sort();
    out
}

/// Save `content` against `fingerprint` through the intent-guarded route.
async fn save_manifest(
    router: &axum::Router,
    intent: &IntentToken,
    content: &str,
    fingerprint: &str,
) -> (StatusCode, String, axum::http::HeaderMap) {
    let body = format!("{}&{}", form_field("content", content), form_field("fingerprint", fingerprint));
    let resp = router
        .clone()
        .oneshot(post_form("/api/v1/workspace/manifest/save", body, intent))
        .await
        .expect("route responds");
    body_string(resp).await
}

/// The read is the literal document, the fingerprint of its exact bytes — the one
/// a save must post back — and the parse verdict. The fingerprint is core's own
/// function over the bytes on disk, not a figure this test re-derives.
#[tokio::test]
async fn the_manifest_read_is_the_literal_document_and_the_fingerprint_a_save_posts_back() {
    let tmp = workspace();
    let router = ws_router(&tmp);
    let resp = router.clone().oneshot(get("/api/v1/workspace/manifest")).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_self_only_csp(&headers, "/api/v1/workspace/manifest");

    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["path"], "logos.workspace.toml", "{body}");
    assert_eq!(v["content"], FIXTURE_MANIFEST, "the literal bytes, never a re-serialisation: {body}");
    assert_eq!(
        v["fingerprint"],
        logos_core::federation::manifest::fingerprint(FIXTURE_MANIFEST.as_bytes()),
        "{body}"
    );
    assert_eq!(v["parsed"]["workspace"]["name"], "shop", "{body}");
    assert!(v["error"].is_null(), "{body}");
    assert_eq!(v["governance_in_effect"], true, "the serve loaded exactly this file: {body}");
}

/// A manifest broken on disk while the serve runs is still delivered — with the
/// parser's message — so the editor can repair it; it is not a `500`.
#[tokio::test]
async fn a_manifest_broken_on_disk_is_delivered_with_its_fault_for_repair() {
    let tmp = workspace();
    // The serve started over a valid manifest; the break happens under it.
    let (router, intent) = ws_router_with_intent(&tmp);
    let broken = format!("{FIXTURE_MANIFEST}bogus_key = 1\n");
    std::fs::write(tmp.path().join("logos.workspace.toml"), &broken).unwrap();

    let v = json_body(&router, "/api/v1/workspace/manifest").await;
    assert_eq!(v["content"], broken.as_str(), "{v}");
    assert!(v["parsed"].is_null(), "{v}");
    assert!(v["error"].as_str().is_some_and(|e| e.contains("bogus_key")), "{v}");
    assert_eq!(v["governance_in_effect"], false, "an unparsable file governs nothing: {v}");

    let fingerprint = v["fingerprint"].as_str().unwrap();
    let (status, body, _h) = save_manifest(&router, &intent, FIXTURE_MANIFEST, fingerprint).await;
    assert_eq!(status, StatusCode::OK, "the save is the repair path: {body}");
    assert_eq!(std::fs::read_to_string(tmp.path().join("logos.workspace.toml")).unwrap(), FIXTURE_MANIFEST);
}

/// **A governance save is advisory, and touches no member** ([ADR-56],
/// [FR-UI-38]). A real `[governance]` family is saved through the intent-guarded
/// route: the manifest holds exactly the posted bytes, every file under every
/// member's `.logos/` keeps its size and mtime (no reindex, no gate output, no
/// store write), both stores' full contents are unchanged, and each member's gated
/// verdict over its own rules — a real FAIL for `web` — is identical before and
/// after.
///
/// It also pins the one thing the save does NOT do: the running serve keeps the
/// rules it started with, so the read now says `governance_in_effect: false` and
/// the view can state that the findings beside it predate the save.
#[tokio::test]
async fn a_governance_save_writes_the_manifest_verbatim_and_moves_no_member() {
    let tmp = workspace();
    let root = tmp.path();
    // `max_cc = 0` always fires, so "unmoved" compares a real FAIL, not two absences.
    std::fs::create_dir_all(root.join("web/.logos")).unwrap();
    std::fs::write(root.join("web/.logos/rules.toml"), "[constraints]\nmax_cc = 0\n").unwrap();
    let verdict_before = [member_gate_verdict(root, "api"), member_gate_verdict(root, "web")];
    assert_eq!(verdict_before[1].1, Some(false), "web's gated verdict is a real FAIL");

    let (router, intent) = ws_router_with_intent(&tmp);
    let loaded = json_body(&router, "/api/v1/workspace/manifest").await;
    let fingerprint = loaded["fingerprint"].as_str().expect("a fingerprint").to_string();

    // Digest first, stat second — and the reverse after the save. Opening a WAL
    // store even read-only touches its `-shm`, so a digest taken between the two
    // stats would be the thing that moved it.
    let digests_before = [member_digests(root, "api"), member_digests(root, "web")];
    let stat_before = [member_logos_stat(root, "api"), member_logos_stat(root, "web")];
    assert!(
        stat_before.iter().all(|files| files.iter().any(|(p, ..)| p.ends_with("logos.db"))),
        "both members were indexed, so the stat comparison watches real stores: {stat_before:?}"
    );

    let candidate = format!("{FIXTURE_MANIFEST}{GOVERNANCE_RULES}");
    let (status, body, headers) = save_manifest(&router, &intent, &candidate, &fingerprint).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_self_only_csp(&headers, "/api/v1/workspace/manifest/save");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["outcome"], "written", "{body}");
    assert_eq!(
        v["fingerprint"],
        logos_core::federation::manifest::fingerprint(candidate.as_bytes()),
        "{body}"
    );
    assert_eq!(std::fs::read_to_string(root.join("logos.workspace.toml")).unwrap(), candidate);

    assert_eq!(
        [member_logos_stat(root, "api"), member_logos_stat(root, "web")],
        stat_before,
        "no member's .logos/ moved: no reindex, no gate output, no store write"
    );
    assert_eq!(
        [member_digests(root, "api"), member_digests(root, "web")],
        digests_before,
        "no member store's contents changed"
    );

    let reread = json_body(&router, "/api/v1/workspace/manifest").await;
    assert_eq!(reread["content"], candidate.as_str(), "{reread}");
    assert_eq!(reread["fingerprint"], v["fingerprint"], "{reread}");
    assert_eq!(
        reread["governance_in_effect"], false,
        "the serve still evaluates the rules it started with: {reread}"
    );

    assert_eq!(
        [member_gate_verdict(root, "api"), member_gate_verdict(root, "web")],
        verdict_before,
        "no member's gated signal moved (ADR-56)"
    );
}

/// **No silent clobber.** The manifest is edited on disk after the editor loaded
/// it; a save against the stale fingerprint is a `409` carrying what is on disk
/// now, and writes nothing. Re-saving against the conflict's own fingerprint is
/// the explicit overwrite the user chose.
#[tokio::test]
async fn a_save_against_a_manifest_changed_since_load_is_a_409_that_writes_nothing() {
    let tmp = workspace();
    let path = tmp.path().join("logos.workspace.toml");
    let (router, intent) = ws_router_with_intent(&tmp);
    let loaded = json_body(&router, "/api/v1/workspace/manifest").await;
    let fingerprint = loaded["fingerprint"].as_str().unwrap().to_string();

    let by_hand = format!("# edited in a terminal while the tab was open\n{FIXTURE_MANIFEST}");
    std::fs::write(&path, &by_hand).unwrap();

    let mine = FIXTURE_MANIFEST.replace("default = \"api\"", "default = \"web\"");
    let (status, body, headers) = save_manifest(&router, &intent, &mine, &fingerprint).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_self_only_csp(&headers, "/api/v1/workspace/manifest/save");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["outcome"], "conflict", "{body}");
    assert_eq!(v["loaded_fingerprint"], fingerprint.as_str(), "{body}");
    assert_eq!(v["disk_content"], by_hand.as_str(), "{body}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), by_hand, "the hand edit survives");

    let disk = v["disk_fingerprint"].as_str().unwrap();
    let (status, body, _h) = save_manifest(&router, &intent, &mine, disk).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), mine, "the explicit overwrite");
}

/// **Validate, then write.** A candidate the parser rejects is a `422` naming the
/// fault and the manifest is byte-identical; a save with no fingerprint is a `400`
/// that is not attempted; a byte-identical save is `unchanged` and writes nothing
/// (mtime unmoved).
#[tokio::test]
async fn a_rejected_fingerprintless_or_identical_manifest_save_writes_nothing() {
    let tmp = workspace();
    let path = tmp.path().join("logos.workspace.toml");
    let (router, intent) = ws_router_with_intent(&tmp);
    let loaded = json_body(&router, "/api/v1/workspace/manifest").await;
    let fingerprint = loaded["fingerprint"].as_str().unwrap().to_string();
    let past = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(past).unwrap();
    let unmoved = |why: &str| {
        assert_eq!(std::fs::read_to_string(&path).unwrap(), FIXTURE_MANIFEST, "{why}: byte-identical");
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), past, "{why}: not rewritten");
    };

    let typo = FIXTURE_MANIFEST.replace("members", "membrs");
    let (status, body, _h) = save_manifest(&router, &intent, &typo, &fingerprint).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(api_error(&body).contains("membrs"), "the refusal names the offending key: {body}");
    unmoved("a rejected candidate");

    let resp = router
        .clone()
        .oneshot(post_form(
            "/api/v1/workspace/manifest/save",
            form_field("content", "[workspace]\nname = \"other\"\n"),
            &intent,
        ))
        .await
        .unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(api_error(&body).contains("fingerprint"), "{body}");
    unmoved("a fingerprint-less save");

    let (status, body, _h) = save_manifest(&router, &intent, FIXTURE_MANIFEST, &fingerprint).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["outcome"], "unchanged", "{body}");
    unmoved("an identical save");
}

/// **`governance_in_effect` compares the rules, not their presence.** The serve
/// starts WITH rules: a save that edits something else leaves it `true`, and a
/// save that edits only one rule's `reason` turns it `false` — the view must not
/// present findings over the old rule as the verdict on the new one ([NFR-CC-04]).
#[tokio::test]
async fn governance_in_effect_tracks_an_edit_to_one_rule_when_the_serve_started_with_rules() {
    let tmp = workspace();
    declare_rules(tmp.path(), GOVERNANCE_RULES);
    let (router, intent) = ws_router_with_intent(&tmp);
    let loaded = json_body(&router, "/api/v1/workspace/manifest").await;
    assert_eq!(loaded["governance_in_effect"], true, "{loaded}");
    let content = loaded["content"].as_str().unwrap().to_string();

    let elsewhere = content.replace("default = \"api\"", "default = \"web\"");
    let (status, body, _h) =
        save_manifest(&router, &intent, &elsewhere, loaded["fingerprint"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after = json_body(&router, "/api/v1/workspace/manifest").await;
    assert_eq!(after["governance_in_effect"], true, "a non-governance edit leaves the rules in effect: {after}");

    let reworded = elsewhere.replace(
        "edge services must not call core services directly",
        "edge goes through the gateway",
    );
    assert_ne!(reworded, elsewhere, "the fixture declares the reason being edited");
    let (status, body, _h) =
        save_manifest(&router, &intent, &reworded, after["fingerprint"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let reread = json_body(&router, "/api/v1/workspace/manifest").await;
    assert_eq!(reread["governance_in_effect"], false, "one reworded rule is a different family: {reread}");
}

/// A manifest removed while the tab was open answers the save `500` — the fault is
/// the server's disk, not the edit — and is not recreated.
#[tokio::test]
async fn a_save_over_a_manifest_removed_since_load_is_a_500_that_recreates_nothing() {
    let tmp = workspace();
    let path = tmp.path().join("logos.workspace.toml");
    let (router, intent) = ws_router_with_intent(&tmp);
    let loaded = json_body(&router, "/api/v1/workspace/manifest").await;
    std::fs::remove_file(&path).unwrap();

    let (status, body, _h) =
        save_manifest(&router, &intent, FIXTURE_MANIFEST, loaded["fingerprint"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(!api_error(&body).is_empty(), "{body}");
    assert!(!path.exists(), "the manifest was not recreated");
}

// ── The workspace chat tier as a group of the app-level Config view (S-451) ───
//
// The view's second group saves `<workspace-root>/.logos/config.toml` and the
// credential beside it through the S-450 routes. What S-451 adds to their
// contract is the property the manifest group already carries: the save reaches
// no member.

/// **A workspace-tier save moves no member** ([FR-UI-38], [FR-WS-30]). The two
/// writes the group makes — the `[chat]`/`[wiki]` document and the credential —
/// land at the workspace root, and every file under every member's `.logos/`
/// keeps its size and mtime (no reindex, no gate output, no store write) while
/// both stores' full contents are unchanged. Each member's gated verdict is
/// identical before and after.
///
/// The stat is compared BEFORE any member read: resolving a member's config
/// opens its engine, and that is a read the save did not make. Only then is the
/// save shown to have reached the members the way the ADR-67 tier promises —
/// through resolution, with the member's own files untouched.
#[tokio::test]
async fn a_workspace_tier_save_moves_no_member_and_reindexes_nothing() {
    let tmp = workspace();
    let root = tmp.path();
    // `max_cc = 0` always fires, so "unmoved" compares a real FAIL, not two absences.
    std::fs::create_dir_all(root.join("web/.logos")).unwrap();
    std::fs::write(root.join("web/.logos/rules.toml"), "[constraints]\nmax_cc = 0\n").unwrap();
    let verdict_before = [member_gate_verdict(root, "api"), member_gate_verdict(root, "web")];
    assert_eq!(verdict_before[1].1, Some(false), "web's gated verdict is a real FAIL");

    let (router, intent) = ws_router_with_intent(&tmp);
    // The group's own load, as the view makes it, before anything is watched.
    let loaded = json_body(&router, "/api/v1/workspace/config").await;
    assert_eq!(loaded["config"]["exists"], false, "the tier starts undeclared: {loaded}");

    // Digest first, stat second — and the reverse after the save (see
    // `a_governance_save_writes_the_manifest_verbatim_and_moves_no_member`).
    let digests_before = [member_digests(root, "api"), member_digests(root, "web")];
    let stat_before = [member_logos_stat(root, "api"), member_logos_stat(root, "web")];
    assert!(
        stat_before.iter().all(|files| files.iter().any(|(p, ..)| p.ends_with("logos.db"))),
        "both members were indexed, so the stat comparison watches real stores: {stat_before:?}"
    );

    let document = "[chat]\nprovider = \"anthropic\"\nmodel = \"ws/tier-group\"\n\n[wiki]\nmodel = \"ws/tier-wiki\"\n";
    let resp = router
        .clone()
        .oneshot(post_form(
            "/api/v1/workspace/config/save",
            format!(
                "file=config&{}&{}",
                form_field("content", document),
                form_field("fingerprint", loaded["config"]["fingerprint"].as_str().unwrap())
            ),
            &intent,
        ))
        .await
        .unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let resp = router
        .clone()
        .oneshot(post_form(
            "/api/v1/workspace/config/secret",
            form_field("api_key", "sk-workspace-tier-group-tg51"),
            &intent,
        ))
        .await
        .unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(std::fs::read_to_string(root.join(".logos/config.toml")).unwrap(), document);

    assert_eq!(
        [member_logos_stat(root, "api"), member_logos_stat(root, "web")],
        stat_before,
        "no member's .logos/ moved: no reindex, no gate output, no store write"
    );
    assert_eq!(
        [member_digests(root, "api"), member_digests(root, "web")],
        digests_before,
        "no member store's contents changed"
    );
    for member in ["api", "web"] {
        assert!(
            !root.join(member).join(".logos/config.toml").exists()
                && !root.join(member).join(".logos/secrets.toml").exists(),
            "the tier was not written into {member}'s .logos/"
        );
    }

    // The save reached the members through the tier, not through their files.
    let v = json_body(&router, "/api/v1/config?repo=web").await;
    assert_eq!(v["effective_chat"]["policy"]["model"], "ws/tier-group", "{v}");
    assert_eq!(v["effective_chat"]["policy_origin"], "workspace", "{v}");
    assert_eq!(v["effective_chat"]["credential_origin"], "workspace", "{v}");
    assert_eq!(v["config"]["exists"], false, "web's own document is still undeclared: {v}");

    assert_eq!(
        [member_gate_verdict(root, "api"), member_gate_verdict(root, "web")],
        verdict_before,
        "no member's gated signal moved"
    );
}

// ── S-451 T2: the tier read is the repair path; the tier save refuses a clobber ─
//
// FR-UI-38's Statement applies the manifest's two write properties to the
// workspace tier as well (CR-145): a broken `<workspace-root>/.logos/config.toml`
// is delivered for repair rather than refused, and a save made against a file
// that changed on disk since the load is refused rather than written over it. Both
// mirror S-430's manifest route; neither adds a route or a guard.

/// Save `content` to the workspace tier against `fingerprint`.
async fn save_tier(
    router: &axum::Router,
    intent: &IntentToken,
    content: &str,
    fingerprint: &str,
) -> (StatusCode, String, axum::http::HeaderMap) {
    let body = format!(
        "file=config&{}&{}",
        form_field("content", content),
        form_field("fingerprint", fingerprint)
    );
    let resp = router
        .clone()
        .oneshot(post_form("/api/v1/workspace/config/save", body, intent))
        .await
        .expect("route responds");
    body_string(resp).await
}

/// **A broken tier file opens for repair** ([FR-UI-38], [NFR-RA-05]). The read
/// is a `200` carrying the literal document, the fingerprint of its bytes and a
/// `null` parse, with a fault naming the file and the position only — never the
/// offending line or the key it rejects — and a save of a valid document against
/// that fingerprint repairs it, after which the read parses again.
#[tokio::test]
async fn a_broken_tier_config_is_read_for_repair_and_a_save_against_its_fingerprint_repairs_it() {
    let tmp = workspace();
    let broken = "[chat]\nmodel = \"ws/kept\"\nmodle = \"ws/typo\"\n";
    write(tmp.path(), ".logos/config.toml", broken);
    let (router, intent) = ws_router_with_intent(&tmp);

    let resp = router.clone().oneshot(get("/api/v1/workspace/config")).await.unwrap();
    let (status, body, headers) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "a broken tier file is delivered, not refused: {body}");
    assert_self_only_csp(&headers, "/api/v1/workspace/config");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["config"]["content"], broken, "the literal bytes: {body}");
    assert_eq!(v["config"]["exists"], true, "{body}");
    assert!(v["config"]["parsed"].is_null(), "no parse stands in for the broken file: {body}");
    assert_eq!(
        v["config"]["fingerprint"],
        logos_core::federation::manifest::fingerprint(broken.as_bytes()),
        "{body}"
    );
    let error = v["config"]["error"].as_str().unwrap_or_else(|| panic!("the fault is stated: {body}"));
    assert!(error.contains(".logos/config.toml"), "the fault names the file: {error}");
    assert!(error.contains("line 3, column 1"), "the fault names the position: {error}");
    assert!(!error.contains("modle") && !error.contains("ws/typo"), "no snippet of the file: {error}");
    // The credential half is readable, so it is stated as it is.
    assert_eq!(v["chat_key"], serde_json::json!({"present": false}), "{body}");

    let fingerprint = v["config"]["fingerprint"].as_str().unwrap().to_string();
    let repaired = "[chat]\nmodel = \"ws/repaired\"\n";
    let (status, body, _h) = save_tier(&router, &intent, repaired, &fingerprint).await;
    assert_eq!(status, StatusCode::OK, "the save is the repair path: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["outcome"], "written", "{body}");
    assert_eq!(
        v["fingerprint"],
        logos_core::federation::manifest::fingerprint(repaired.as_bytes()),
        "the fingerprint the editor holds from now on: {body}"
    );
    assert_eq!(std::fs::read_to_string(tmp.path().join(".logos/config.toml")).unwrap(), repaired);

    let v = json_body(&router, "/api/v1/workspace/config").await;
    assert_eq!(v["config"]["parsed"]["chat"]["model"], "ws/repaired", "{v}");
    assert!(v["config"]["error"].is_null(), "{v}");
}

/// **A broken credential store is named, never shown** ([NFR-SE-07]). Its parse
/// fault can quote the offending line, and here that line IS the key: the read is
/// still a `200`, the key half is `null` with a fault naming the file and the
/// position, and no fragment of the file — not the key, not its line — appears
/// anywhere in the body. The policy half is unaffected and still editable.
#[tokio::test]
async fn a_broken_tier_secret_store_is_reported_by_file_and_position_and_never_echoed() {
    let tmp = workspace();
    let key = "sk-unquoted-broken-line-bk07";
    let secrets = format!("[chat]\napi_key = {key}\n");
    let policy = "[chat]\nmodel = \"ws/policy\"\n";
    write(tmp.path(), ".logos/secrets.toml", &secrets);
    write(tmp.path(), ".logos/config.toml", policy);
    let router = ws_router(&tmp);

    let resp = router.clone().oneshot(get("/api/v1/workspace/config")).await.unwrap();
    let (status, body, _h) = body_string(resp).await;
    assert_eq!(status, StatusCode::OK, "a broken store does not fail the tier read: {body}");
    for fragment in [key, "unquoted", "bk07", "api_key = ", secrets.lines().nth(1).unwrap()] {
        assert!(!body.contains(fragment), "`{fragment}` of secrets.toml reached the body: {body}");
    }
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["chat_key"].is_null(), "no key state is invented for an unreadable store: {body}");
    let error = v["chat_key_error"].as_str().unwrap_or_else(|| panic!("the fault is stated: {body}"));
    assert!(error.contains(".logos/secrets.toml"), "the fault names the file: {error}");
    assert!(error.contains("line 2, column"), "the fault names the position: {error}");
    assert_eq!(v["config"]["parsed"]["chat"]["model"], "ws/policy", "the policy half is unaffected: {body}");
    assert!(v["config"]["error"].is_null(), "{body}");
}

/// **No silent clobber** ([FR-UI-38], CR-145). A hand edit made while the tab is
/// open moves the fingerprint; a save against the load's is a `409` carrying the
/// document on disk now, and the file is byte-identical. Re-saving against the
/// conflict's own fingerprint is the explicit overwrite.
#[tokio::test]
async fn a_tier_save_against_a_file_changed_since_load_is_a_409_that_writes_nothing() {
    let tmp = workspace();
    let path = tmp.path().join(".logos/config.toml");
    write(tmp.path(), ".logos/config.toml", "[chat]\nmodel = \"ws/loaded\"\n");
    let (router, intent) = ws_router_with_intent(&tmp);
    let loaded = json_body(&router, "/api/v1/workspace/config").await;
    let fingerprint = loaded["config"]["fingerprint"].as_str().unwrap().to_string();

    let by_hand = "# edited in a terminal while the tab was open\n[chat]\nmodel = \"ws/by-hand\"\n";
    std::fs::write(&path, by_hand).unwrap();

    let mine = "[chat]\nmodel = \"ws/mine\"\n";
    let (status, body, headers) = save_tier(&router, &intent, mine, &fingerprint).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_self_only_csp(&headers, "/api/v1/workspace/config/save");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["outcome"], "conflict", "{body}");
    assert_eq!(v["loaded_fingerprint"], fingerprint.as_str(), "{body}");
    assert_eq!(v["disk_content"], by_hand, "{body}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), by_hand, "the hand edit survives byte-identical");

    let disk = v["disk_fingerprint"].as_str().unwrap();
    let (status, body, _h) = save_tier(&router, &intent, mine, disk).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), mine, "the explicit overwrite");
}

/// A tier created on disk while the tab showed it absent is a conflict too: the
/// absent file's fingerprint is the empty document's, and a hand-created file
/// moves it.
#[tokio::test]
async fn a_first_tier_save_over_a_file_created_since_load_is_a_409() {
    let tmp = workspace();
    let (router, intent) = ws_router_with_intent(&tmp);
    let loaded = json_body(&router, "/api/v1/workspace/config").await;
    assert_eq!(loaded["config"]["exists"], false, "{loaded}");
    let fingerprint = loaded["config"]["fingerprint"].as_str().unwrap().to_string();
    assert_eq!(fingerprint, EMPTY_DOCUMENT_FINGERPRINT, "an absent tier is the empty document");

    let by_hand = "[chat]\nmodel = \"ws/created-by-hand\"\n";
    write(tmp.path(), ".logos/config.toml", by_hand);
    let (status, body, _h) = save_tier(&router, &intent, "[chat]\nmodel = \"ws/mine\"\n", &fingerprint).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(std::fs::read_to_string(tmp.path().join(".logos/config.toml")).unwrap(), by_hand);
}

/// **The fingerprint is required, and identity writes nothing.** A save with no
/// fingerprint is a `400` that is not attempted — the file byte-identical and not
/// rewritten; a candidate byte-identical to disk is `unchanged` and not rewritten;
/// a rejected candidate is still a `422` that leaves it byte-identical.
#[tokio::test]
async fn a_fingerprintless_rejected_or_identical_tier_save_writes_nothing() {
    let tmp = workspace();
    let path = tmp.path().join(".logos/config.toml");
    let on_disk = "[chat]\nmodel = \"ws/kept\"\n";
    write(tmp.path(), ".logos/config.toml", on_disk);
    let (router, intent) = ws_router_with_intent(&tmp);
    let loaded = json_body(&router, "/api/v1/workspace/config").await;
    let fingerprint = loaded["config"]["fingerprint"].as_str().unwrap().to_string();
    let past = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(past).unwrap();
    let unmoved = |why: &str| {
        assert_eq!(std::fs::read_to_string(&path).unwrap(), on_disk, "{why}: byte-identical");
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), past, "{why}: not rewritten");
    };

    for form in [
        format!("file=config&{}", form_field("content", "[chat]\nmodel = \"ws/other\"\n")),
        format!("file=config&{}&fingerprint=%20", form_field("content", "[chat]\nmodel = \"ws/other\"\n")),
    ] {
        let resp = router
            .clone()
            .oneshot(post_form("/api/v1/workspace/config/save", form, &intent))
            .await
            .unwrap();
        let (status, body, _h) = body_string(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(api_error(&body).contains("fingerprint"), "the refusal says what is missing: {body}");
        unmoved("a fingerprint-less save");
    }

    let (status, body, _h) = save_tier(&router, &intent, "[chat]\nmodle = \"typo\"\n", &fingerprint).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    unmoved("a rejected candidate");

    let (status, body, _h) = save_tier(&router, &intent, on_disk, "some-other-load").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["outcome"], "unchanged", "identity is decided before staleness: {body}");
    assert_eq!(v["fingerprint"], fingerprint.as_str(), "{body}");
    unmoved("an identical save");
}

/// **Validation precedes identity** (S-451 T2). Posting the broken document back
/// unchanged — the first thing a user does who opens the repair editor and
/// clicks Save — is a `422`, not an `unchanged` `200` that would report success
/// over a file every inheriting member still fails on; nothing is rewritten.
#[tokio::test]
async fn re_saving_the_identical_broken_tier_document_is_a_422_not_unchanged() {
    let tmp = workspace();
    let path = tmp.path().join(".logos/config.toml");
    let broken = "[chat]\nmodle = \"ws/typo\"\n";
    write(tmp.path(), ".logos/config.toml", broken);
    let (router, intent) = ws_router_with_intent(&tmp);
    let loaded = json_body(&router, "/api/v1/workspace/config").await;
    let fingerprint = loaded["config"]["fingerprint"].as_str().unwrap().to_string();
    let past = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(past).unwrap();

    let (status, body, _h) = save_tier(&router, &intent, broken, &fingerprint).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), broken, "byte-identical");
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), past, "not rewritten");
}

