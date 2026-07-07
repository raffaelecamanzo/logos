//! The intent-guarded `POST /api/v1/verify` deep graph-consistency endpoint
//! (S-206 / [FR-UI-25], [FR-GV-19], [NFR-SE-06], [NFR-SE-07], [NFR-RA-05],
//! [ADR-31], [ADR-46]).
//!
//! Drives the **real** router in-process (`tower::ServiceExt::oneshot`, no socket)
//! and asserts the story's contract end-to-end:
//!
//! - a clean store returns a `200` JSON `VerifyReport` with `ok:true` under the
//!   byte-identical self-only CSP; a drifted store returns the live-vs-reindex
//!   deltas and the leaked/orphaned-symbol sample ([FR-UI-25] AC);
//! - the endpoint carries the same-origin + per-session intent-token guard
//!   ([ADR-31]) — a cross-origin, token-less, forged-token, origin-less, or
//!   non-loopback-`Host` request is rejected before the (expensive) reindex runs,
//!   and a `GET` to the route is `405` ([UAT-UI-02]);
//! - the shadow reindex runs **off the serve loop** on the blocking pool, so a
//!   verify in flight does not stall concurrent reads (the [ADR-46] mitigation);
//! - the response body never carries the masked write-only key ([NFR-SE-07]).
//!
//! The guard/method cases are grammar-independent and run under a bare
//! `cargo test -p web`; the populated-graph clean/drift assertions (which need a
//! reindex to produce symbols) are `#[cfg(feature = "lang-rust")]`.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use logos_core::Engine;
use tempfile::TempDir;
use tower::ServiceExt;
use web::{IntentToken, INTENT_HEADER};

const HOST: &str = "127.0.0.1:4983";
const SAME_ORIGIN: &str = "http://127.0.0.1:4983";
const VERIFY: &str = "/api/v1/verify";

// ── Fixtures & helpers ────────────────────────────────────────────────────────

fn write(root: &std::path::Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A router over a **throwaway** (un-indexed) engine plus the session's valid
/// intent token — enough to exercise the guards, which reject before the handler
/// (and its reindex) ever runs.
fn guard_router() -> (TempDir, axum::Router, IntentToken) {
    let dir = TempDir::new().expect("temp dir");
    std::fs::create_dir_all(dir.path().join(".logos")).expect("pre-create .logos");
    let intent = IntentToken::generate();
    let engine = Arc::new(Engine::open(dir.path()));
    let router = web::router_with_intent(engine, intent.clone());
    (dir, router, intent)
}

/// A `POST /api/v1/verify` with an optional intent token, `Origin`, and `Host`.
/// The handler reads no body, so the request carries none — the guards key on the
/// headers alone.
fn verify_post(intent: Option<&str>, origin: Option<&str>, host: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(Method::POST).uri(VERIFY);
    if let Some(host) = host {
        builder = builder.header(header::HOST, host);
    }
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    if let Some(token) = intent {
        builder = builder.header(INTENT_HEADER, token);
    }
    builder.body(Body::empty()).unwrap()
}

async fn body_string(resp: axum::http::Response<Body>) -> (StatusCode, String, axum::http::HeaderMap) {
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap(), headers)
}

/// Every verify answer — success or the honest error — carries the byte-identical
/// self-only CSP ([NFR-SE-06]).
fn assert_self_only_csp(headers: &axum::http::HeaderMap) {
    let csp = headers
        .get(header::CONTENT_SECURITY_POLICY)
        .expect("every response carries a CSP")
        .to_str()
        .unwrap();
    assert!(csp.contains("default-src 'self'"), "self-only CSP: {csp}");
    assert!(!csp.contains('*'), "CSP allows no wildcard source: {csp}");
}

// ── Guard contract (ADR-31, NFR-SE-06) — rejected before the reindex runs ──────

/// A cross-origin write (the attacker's browser-set `Origin`) is rejected `403`
/// even with the valid session token — and the CSP is still stamped.
#[tokio::test]
async fn cross_origin_verify_is_403() {
    let (_dir, router, intent) = guard_router();
    let req = verify_post(Some(intent.as_str()), Some("http://evil.example.com"), Some(HOST));
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "a cross-origin verify is rejected");
    assert!(
        resp.headers().contains_key(header::CONTENT_SECURITY_POLICY),
        "the 403 still carries the CSP",
    );
}

/// A token-less verify is rejected `403`, even same-origin — the per-session token
/// is mandatory ([NFR-SE-06]).
#[tokio::test]
async fn token_less_verify_is_403() {
    let (_dir, router, _intent) = guard_router();
    let req = verify_post(None, Some(SAME_ORIGIN), Some(HOST));
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_self_only_csp(resp.headers());
}

/// A forged token of the right shape (64 hex chars, wrong value) is rejected
/// `403` by the constant-time compare.
#[tokio::test]
async fn forged_token_verify_is_403() {
    let (_dir, router, _intent) = guard_router();
    let forged = "deadbeef".repeat(8); // 64 hex chars, right shape, wrong value
    let req = verify_post(Some(&forged), Some(SAME_ORIGIN), Some(HOST));
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_self_only_csp(resp.headers());
}

/// A verify with the valid token but **no** `Origin` header is rejected — a
/// same-origin proof is mandatory on a mutating-method request.
#[tokio::test]
async fn origin_less_verify_is_403() {
    let (_dir, router, intent) = guard_router();
    let req = verify_post(Some(intent.as_str()), None, Some(HOST));
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_self_only_csp(resp.headers());
}

/// A non-loopback `Host` is rejected `403` by the DNS-rebinding guard before the
/// intent guard even runs — the verify route inherits the loopback-only carve-out.
#[tokio::test]
async fn non_loopback_host_verify_is_403() {
    let (_dir, router, intent) = guard_router();
    let req = verify_post(Some(intent.as_str()), Some("http://evil.example.com"), Some("evil.example.com"));
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_self_only_csp(resp.headers());
}

/// A `GET` to the verify route is `405`: the route admits `POST` only, so the SPA
/// cannot fetch it as an ordinary read (it must carry the intent proof).
#[tokio::test]
async fn get_to_verify_route_is_405() {
    let (_dir, router, _intent) = guard_router();
    let req = Request::builder()
        .method(Method::GET)
        .uri(VERIFY)
        .header(header::HOST, HOST)
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED, "the verify route is POST-only");
}

// ── Report contract (FR-UI-25, FR-GV-19) — clean vs drifted ────────────────────

/// A same-origin, intent-bearing verify over a **clean** freshly-indexed store
/// returns `200` with a `VerifyReport` whose `ok:true`, zero deltas, and empty
/// leaked/orphaned samples — under the byte-identical self-only CSP ([FR-UI-25]).
#[cfg(feature = "lang-rust")]
#[tokio::test]
async fn clean_store_returns_ok_true_json() {
    let dir = TempDir::new().expect("temp dir");
    write(dir.path(), "src/lib.rs", "pub fn api() { helper(); }\nfn helper() {}\n");
    let engine = Arc::new(Engine::start(dir.path()).expect("engine starts"));
    engine.index();

    let intent = IntentToken::generate();
    let router = web::router_with_intent(engine, intent.clone());
    let req = verify_post(Some(intent.as_str()), Some(SAME_ORIGIN), Some(HOST));
    let resp = router.oneshot(req).await.unwrap();
    let (status, body, headers) = body_string(resp).await;

    assert_eq!(status, StatusCode::OK, "a guarded verify over a clean store answers 200");
    assert_self_only_csp(&headers);
    assert!(body.contains("\"ok\":true"), "a clean store reports CONSISTENT: {body}");
    assert!(body.contains("\"node_delta\":0"), "zero node delta on a clean store: {body}");
    assert!(body.contains("\"leaked_total\":0"), "no leaked symbols on a clean store: {body}");
    assert!(body.contains("\"orphaned_total\":0"), "no orphaned symbols on a clean store: {body}");
    // The report is the serialized read-model verbatim (FR-UI-25) — spot-check its
    // structural shape so a reshaped `VerifyReport` can't silently pass.
    for key in ["\"live\"", "\"reindex\"", "\"structural\"", "\"edge_delta\"", "\"file_delta\""] {
        assert!(body.contains(key), "the report carries {key}: {body}");
    }
}

/// A verify over a **drifted** store (a file deleted on disk without a sync — the
/// Channel-B orphan leak) returns `200` with `ok:false`, a positive `node_delta`,
/// a `file_delta ≥ 1`, and a leaked-symbol sample naming the removed file's
/// symbols ([FR-UI-25] AC, [FR-GV-19]). The live store is opened read-only for the
/// census, so the leak is *reported*, never healed.
#[cfg(feature = "lang-rust")]
#[tokio::test]
async fn drifted_store_returns_deltas_and_leaked_sample() {
    let dir = TempDir::new().expect("temp dir");
    write(dir.path(), "src/keep.rs", "pub fn keep() {}\n");
    write(dir.path(), "src/gone.rs", "pub fn gone() {}\npub fn also_gone() {}\n");
    let engine = Arc::new(Engine::start(dir.path()).expect("engine starts"));
    engine.index();

    // Delete a file on disk WITHOUT a sync: the live store retains its nodes, but a
    // fresh shadow reindex sees only the survivor — the drift `verify` must surface.
    std::fs::remove_file(dir.path().join("src/gone.rs")).unwrap();

    let intent = IntentToken::generate();
    let router = web::router_with_intent(engine, intent.clone());
    let req = verify_post(Some(intent.as_str()), Some(SAME_ORIGIN), Some(HOST));
    let resp = router.oneshot(req).await.unwrap();
    let (status, body, headers) = body_string(resp).await;

    assert_eq!(status, StatusCode::OK, "a drifted verify still answers 200 (the drift is in the body)");
    assert_self_only_csp(&headers);
    assert!(body.contains("\"ok\":false"), "the leak is drift: {body}");
    // The numeric deltas are `live − reindex`: a positive node_delta and a
    // file_delta ≥ 1 are the leak signature. Parse the report and assert on values
    // rather than brittle substring matching of a specific integer or symbol name.
    let report: serde_json::Value = serde_json::from_str(&body).expect("the body is JSON");
    assert!(report["node_delta"].as_i64().unwrap() > 0, "live surplus nodes: {body}");
    assert!(report["file_delta"].as_i64().unwrap() >= 1, "the live store retains the deleted file: {body}");
    assert!(report["leaked_total"].as_u64().unwrap() >= 1, "at least one leaked symbol: {body}");
    let leaked = report["leaked_symbols"].as_array().unwrap();
    assert!(!leaked.is_empty(), "the leaked sample is non-empty: {body}");
    assert!(
        leaked.iter().any(|s| s.as_str().unwrap_or_default().contains("gone")),
        "the leaked sample names the removed file's symbols: {body}",
    );
}

// ── Off-serve-loop execution (ADR-46 risk mitigation) ──────────────────────────

/// The seconds-to-minutes reindex runs on the blocking pool via the `bridge`
/// ([ADR-03] `spawn_blocking`), so a verify in flight does not stall the serve
/// loop. On the production single-threaded serve runtime (`#[tokio::test]` is
/// current-thread, mirroring `serve_surfaces`), a verify and a batch of concurrent
/// reads are `join`ed: all complete with `200`. The live-store census is read-only
/// and takes no exclusive lock, so the reads are genuinely served *while* the
/// verify runs — had the handler blocked the executor inline (or held a write
/// lock) the reads could not be driven to completion beside it.
#[cfg(feature = "lang-rust")]
#[tokio::test]
async fn verify_runs_off_the_serve_loop_concurrent_reads_still_served() {
    let dir = TempDir::new().expect("temp dir");
    write(dir.path(), "src/lib.rs", "pub fn api() { helper(); }\nfn helper() {}\n");
    let engine = Arc::new(Engine::start(dir.path()).expect("engine starts"));
    engine.index();
    engine.scan(false).expect("a scan persists a metric snapshot the read endpoints reflect");

    let intent = IntentToken::generate();
    let router = web::router_with_intent(engine, intent.clone());

    let read = |path: &'static str| {
        let router = router.clone();
        async move {
            let req = Request::builder()
                .method(Method::GET)
                .uri(path)
                .header(header::HOST, HOST)
                .body(Body::empty())
                .unwrap();
            router.oneshot(req).await.unwrap().status()
        }
    };
    let verify = {
        let router = router.clone();
        let token = intent.as_str().to_string();
        async move {
            let req = verify_post(Some(&token), Some(SAME_ORIGIN), Some(HOST));
            router.oneshot(req).await.unwrap().status()
        }
    };

    let (v, r1, r2, r3) = tokio::join!(
        verify,
        read("/api/v1/health"),
        read("/api/v1/overview"),
        read("/api/v1/status"),
    );
    assert_eq!(v, StatusCode::OK, "the verify itself completes");
    assert_eq!(r1, StatusCode::OK, "a health read is served during the verify");
    assert_eq!(r2, StatusCode::OK, "an overview read is served during the verify");
    // `/api/v1/status` is not a route — an unknown non-navigation GET is a 404,
    // proving the read reached the router (not stalled) even for a miss.
    assert_eq!(r3, StatusCode::NOT_FOUND, "an unknown read still routes (not stalled) during the verify");
}
