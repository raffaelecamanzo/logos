//! An extension's web routes are mounted through the host, behind the whole
//! guard stack, and only while the extension is active (S-639, [FR-DS-04],
//! [ADR-74], [ADR-75]).
//!
//! The stub extension (`logos_ext::testing`) contributes a `GET` and a `POST`
//! route. Registered, the `GET` answers like a core read and the `POST` is
//! admitted by the method guard and then gated by the intent guard exactly as the
//! config writes are ([NFR-SE-06]). Unregistered, or registered behind a
//! feature the gate does not enable, neither path exists and a `POST` to the
//! write route is refused `405` before any handler runs.
//!
//! [FR-DS-04]: ../../docs/specs/requirements/FR-DS-04.md
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
//! [ADR-75]: ../../docs/specs/architecture/decisions/ADR-75.md
//! [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use logos_core::federation::Backing;
use logos_core::Engine;
use logos_ext::testing::{
    StubExtension, TestGate, STUB_FEATURE, STUB_GET_ROUTE, STUB_OVERVIEW_FIELD, STUB_POST_ROUTE,
};
use logos_ext::ExtensionHost;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;
use web::{IntentToken, INTENT_HEADER};

const HOST: &str = "127.0.0.1:4983";
const ORIGIN: &str = "http://127.0.0.1:4983";

fn router(host: &ExtensionHost) -> (TempDir, Router, IntentToken) {
    let dir = TempDir::new().expect("temp dir");
    let engine = Arc::new(Engine::start(dir.path()).expect("engine starts"));
    let intent = IntentToken::generate();
    let router = web::hosted_router(Arc::new(Backing::Single(engine)), intent.clone(), host)
        .expect("the router builds");
    (dir, router, intent)
}

fn get(path: &str) -> Request<Body> {
    Request::get(path).header(header::HOST, HOST).body(Body::empty()).expect("a request")
}

/// A `POST` from the dashboard's own origin, carrying `intent` when given.
fn post(path: &str, intent: Option<&IntentToken>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN);
    if let Some(token) = intent {
        builder = builder.header(INTENT_HEADER, token.as_str());
    }
    builder.body(Body::empty()).expect("a request")
}

async fn send(router: &Router, request: Request<Body>) -> (StatusCode, Option<String>, Vec<u8>) {
    let response = router.clone().oneshot(request).await.expect("the router answers");
    let status = response.status();
    let csp = response
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .map(|v| v.to_str().expect("ASCII").to_string());
    let body = response.into_body().collect().await.expect("a body").to_bytes().to_vec();
    (status, csp, body)
}

fn json(body: &[u8]) -> Value {
    serde_json::from_slice(body).expect("a JSON body")
}

#[tokio::test]
async fn a_registered_extension_get_route_answers_behind_the_guard_stack() {
    let host = ExtensionHost::new().register(StubExtension::new()).expect("registers");
    let (_dir, router, _) = router(&host);

    let (status, csp, body) = send(&router, get(STUB_GET_ROUTE)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body)["extension"], "stub");
    assert!(csp.is_some_and(|c| c.contains("default-src 'self'")), "the CSP layer covers it");

    // The loopback-host guard covers it too: a rebinding Host is refused.
    let rebinding = Request::get(STUB_GET_ROUTE)
        .header(header::HOST, "evil.example.com")
        .body(Body::empty())
        .expect("a request");
    assert_eq!(send(&router, rebinding).await.0, StatusCode::FORBIDDEN);

    // A `POST` to the read route is not on the allow-list.
    assert_eq!(send(&router, post(STUB_GET_ROUTE, None)).await.0, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn a_registered_extension_post_route_is_admitted_by_the_method_and_intent_guards() {
    let stub = StubExtension::new();
    let host = ExtensionHost::new().register(stub.clone()).expect("registers");
    let (_dir, router, intent) = router(&host);

    // Admitted by the method guard, then refused by the intent guard without the
    // session's token — no handler ran.
    assert_eq!(send(&router, post(STUB_POST_ROUTE, None)).await.0, StatusCode::FORBIDDEN);
    let forged = IntentToken::generate();
    assert_eq!(send(&router, post(STUB_POST_ROUTE, Some(&forged))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(stub.posts(), 0, "a refused POST reaches no handler");

    // Same-origin with the session's token: the handler runs.
    let (status, csp, body) = send(&router, post(STUB_POST_ROUTE, Some(&intent))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body)["posts"], 1);
    assert!(csp.is_some(), "the CSP layer stamps the write's response");
    assert_eq!(stub.posts(), 1);

    // The allow-list entry is the route, not a prefix of it.
    for near_miss in ["/api/v1/stub/actions", "/api/v1/stub/action/x", "/api/v1/stub/actio"] {
        let (status, ..) = send(&router, post(near_miss, Some(&intent))).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{near_miss} is not admitted");
    }
    assert_eq!(stub.posts(), 1);
}

#[tokio::test]
async fn an_unregistered_or_disabled_extension_mounts_no_route() {
    let hosts = [
        ExtensionHost::new(),
        ExtensionHost::new().register(StubExtension::gated(STUB_FEATURE)).expect("registers"),
        ExtensionHost::new()
            .register(StubExtension::gated(STUB_FEATURE))
            .expect("registers")
            .with_gate(TestGate::enabling(&["another-feature"])),
    ];
    for host in hosts {
        let (_dir, router, intent) = router(&host);
        let (status, ..) = send(&router, get(STUB_GET_ROUTE)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{host:?}: no GET route");
        let (status, ..) = send(&router, post(STUB_POST_ROUTE, Some(&intent))).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{host:?}: the POST is not admitted");
    }

    // The same stub with its feature enabled: both routes are there, so the
    // refusals above are the gate's doing.
    let enabled = ExtensionHost::new()
        .register(StubExtension::gated(STUB_FEATURE))
        .expect("registers")
        .with_gate(TestGate::enabling(&[STUB_FEATURE]));
    let (_dir, router, intent) = router(&enabled);
    assert_eq!(send(&router, get(STUB_GET_ROUTE)).await.0, StatusCode::OK);
    assert_eq!(send(&router, post(STUB_POST_ROUTE, Some(&intent))).await.0, StatusCode::OK);
}

/// The overview carries an extension's field exactly while it is active: an
/// unregistered or disabled extension adds none, the same stub enabled adds its
/// field, read over the member's engine.
#[tokio::test]
async fn an_unregistered_or_disabled_extension_adds_no_overview_field() {
    let hosts = [
        ExtensionHost::new(),
        ExtensionHost::new().register(StubExtension::gated(STUB_FEATURE)).expect("registers"),
        ExtensionHost::new()
            .register(StubExtension::gated(STUB_FEATURE))
            .expect("registers")
            .with_gate(TestGate::enabling(&["another-feature"])),
    ];
    for host in hosts {
        let (_dir, router, _) = router(&host);
        let (status, _, body) = send(&router, get("/api/v1/overview")).await;
        assert_eq!(status, StatusCode::OK, "{host:?}");
        let overview = json(&body);
        assert!(overview.get("status").is_some(), "{host:?}: the core bundle was read");
        assert!(overview.get(STUB_OVERVIEW_FIELD).is_none(), "{host:?}: no stub field");
    }

    let enabled = ExtensionHost::new()
        .register(StubExtension::gated(STUB_FEATURE))
        .expect("registers")
        .with_gate(TestGate::enabling(&[STUB_FEATURE]));
    let (dir, router, _) = router(&enabled);
    let (status, _, body) = send(&router, get("/api/v1/overview")).await;
    assert_eq!(status, StatusCode::OK);
    let overview = json(&body);
    assert_eq!(overview[STUB_OVERVIEW_FIELD]["extension"], "stub");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    let read_root = std::fs::canonicalize(overview[STUB_OVERVIEW_FIELD]["root"].as_str().expect("a root"))
        .expect("canonical read root");
    assert_eq!(read_root, root, "read over the member's engine");
}

/// An extension that mounts a route on a core path.
struct Shadow;

impl logos_ext::Extension for Shadow {
    fn id(&self) -> &'static str {
        "shadow"
    }

    fn web_routes(&self, _context: &logos_ext::web::WebContext) -> Vec<logos_ext::web::WebRoute> {
        async fn impostor() -> &'static str {
            "impostor"
        }
        vec![logos_ext::web::WebRoute::get("/api/v1/status", impostor, ())]
    }
}

/// An extension route can never silently shadow a core one: an overlapping path
/// fails the router build, loudly, before anything is served.
#[test]
fn an_extension_route_on_a_core_path_fails_the_router_build() {
    let host = ExtensionHost::new().register(Shadow).expect("registers");
    let dir = TempDir::new().expect("temp dir");
    let engine = Arc::new(Engine::start(dir.path()).expect("engine starts"));
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        web::hosted_router(Arc::new(Backing::Single(engine)), IntentToken::generate(), &host)
    }));
    assert!(built.is_err(), "mounting over `/api/v1/status` must not build");
}

/// An extension whose `POST` routes take parameters — a whole segment, and one
/// inside a segment, both of which axum routes.
struct Parameterised;

impl logos_ext::Extension for Parameterised {
    fn id(&self) -> &'static str {
        "parameterised"
    }

    fn web_routes(&self, _context: &logos_ext::web::WebContext) -> Vec<logos_ext::web::WebRoute> {
        async fn handled() -> &'static str {
            "handled"
        }
        vec![
            logos_ext::web::WebRoute::post("/api/v1/probe/:id/delete", handled, ()),
            logos_ext::web::WebRoute::post("/api/v1/probe/item-:id", handled, ()),
        ]
    }
}

/// The method guard admits an extension `POST` exactly where the router routes
/// it: every path axum matches to a declared `POST` reaches the handler, and a
/// path it matches to nothing — a near miss — is refused before any handler.
#[tokio::test]
async fn an_extension_post_is_admitted_exactly_where_the_router_routes_it() {
    let host = ExtensionHost::new().register(Parameterised).expect("registers");
    let (_dir, router, intent) = router(&host);
    for routed in ["/api/v1/probe/7/delete", "/api/v1/probe/abc/delete", "/api/v1/probe/item-7"] {
        let (status, _, body) = send(&router, post(routed, Some(&intent))).await;
        assert_eq!((status, body.as_slice()), (StatusCode::OK, &b"handled"[..]), "{routed}");
    }
    for near_miss in ["/api/v1/probe/7/delete/x", "/api/v1/probe/7", "/api/v1/probe/item", "/api/v1/probe/7/deletes"] {
        let (status, ..) = send(&router, post(near_miss, Some(&intent))).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{near_miss} is not admitted");
    }
}

/// An extension that answers both methods on one path — the shape the `wiki`
/// extension's `/wiki/generate` has (a `WebRoute::shell` beside its
/// `WebRoute::post`, S-644), as the core's `get(spa_shell).post(wiki_generate)`
/// had before it.
struct BothMethods;

impl logos_ext::Extension for BothMethods {
    fn id(&self) -> &'static str {
        "both-methods"
    }

    fn web_routes(&self, _context: &logos_ext::web::WebContext) -> Vec<logos_ext::web::WebRoute> {
        async fn read() -> &'static str {
            "read"
        }
        async fn write() -> &'static str {
            "write"
        }
        vec![
            logos_ext::web::WebRoute::get("/api/v1/both", read, ()),
            logos_ext::web::WebRoute::post("/api/v1/both", write, ()),
        ]
    }
}

/// A `GET` and a `POST` on one path are merged into one route: each method
/// reaches its own handler, and the `POST` is still admitted and intent-gated.
#[tokio::test]
async fn a_get_and_a_post_on_one_extension_path_each_reach_their_handler() {
    let host = ExtensionHost::new().register(BothMethods).expect("registers");
    let (_dir, router, intent) = router(&host);
    let (status, _, body) = send(&router, get("/api/v1/both")).await;
    assert_eq!((status, body.as_slice()), (StatusCode::OK, &b"read"[..]));
    let (status, _, body) = send(&router, post("/api/v1/both", Some(&intent))).await;
    assert_eq!((status, body.as_slice()), (StatusCode::OK, &b"write"[..]));
    assert_eq!(send(&router, post("/api/v1/both", None)).await.0, StatusCode::FORBIDDEN);
}
