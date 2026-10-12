//! `GET /api/v1/extensions` — the server states which extensions are active, with
//! their UI manifests (S-641, [FR-UI-51], CR-213).
//!
//! The SPA will read it on boot to decide what to mount ([S-642]); it is a read of the
//! host the router was composed with, so it reaches no engine and records nothing.
//! It sits behind the whole guard stack like any core read: `GET`-only (the method
//! guard), loopback-only (the host guard) and same-origin (no cross-origin grant is
//! ever added, and the self-only CSP stamps the response).
//!
//! [FR-UI-51]: ../../docs/specs/requirements/FR-UI-51.md
//! [S-642]: ../../docs/planning/journal.md#s-642-the-spa-mounts-navigation-views-and-config-panels-from-registered-extensions

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use logos_core::federation::Backing;
use logos_core::Engine;
use logos_ext::testing::{StubExtension, TestGate, STUB_FEATURE};
use logos_ext::ExtensionHost;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;
use web::{IntentToken, INTENT_HEADER};

const HOST: &str = "127.0.0.1:4983";
const ORIGIN: &str = "http://127.0.0.1:4983";
const PATH: &str = "/api/v1/extensions";

fn router(host: &ExtensionHost) -> (TempDir, Router, IntentToken) {
    let dir = TempDir::new().expect("temp dir");
    let engine = Arc::new(Engine::start(dir.path()).expect("engine starts"));
    let intent = IntentToken::generate();
    let router = web::hosted_router(Arc::new(Backing::Single(engine)), intent.clone(), host)
        .expect("the router builds");
    (dir, router, intent)
}

fn get() -> Request<Body> {
    Request::get(PATH).header(header::HOST, HOST).body(Body::empty()).expect("a request")
}

async fn read(router: &Router, request: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let response = router.clone().oneshot(request).await.expect("the router answers");
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.expect("a body").to_bytes().to_vec();
    (status, headers, body)
}

async fn listed(host: &ExtensionHost) -> Value {
    let (_dir, router, _) = router(host);
    let (status, _, body) = read(&router, get()).await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice(&body).expect("a JSON body")
}

/// With no extension the list is empty, and the list is the whole body.
#[tokio::test]
async fn with_no_extension_the_list_is_empty() {
    assert_eq!(listed(&ExtensionHost::new()).await, json!({ "extensions": [] }));
}

/// A registered stub is listed with its UI manifest in the shape the SPA's
/// registries read.
#[tokio::test]
async fn a_registered_extension_is_listed_with_its_manifest() {
    let host = ExtensionHost::new().register(StubExtension::new()).expect("registers");
    assert_eq!(
        listed(&host).await,
        json!({
            "extensions": [{
                "id": "stub",
                "ui": {
                    "nav": [{
                        "id": "stub",
                        "label": "Stub",
                        "path": "/stub",
                        "group": "A",
                        "scope": "member"
                    }],
                    "views": ["/stub"],
                    "config_panels": [{ "id": "stub", "label": "Stub", "table": "stub" }],
                    "dashboard_cards": [{ "id": "stub-card", "label": "Stub" }]
                }
            }]
        })
    );
}

/// An active extension that contributes no UI is still listed — the SPA learns it
/// is active — with a `null` manifest.
#[tokio::test]
async fn an_active_extension_without_a_manifest_is_listed_with_a_null_one() {
    struct Headless;
    impl logos_ext::Extension for Headless {
        fn id(&self) -> &'static str {
            "headless"
        }
    }
    let host = ExtensionHost::new().register(Headless).expect("registers");
    assert_eq!(
        listed(&host).await,
        json!({ "extensions": [{ "id": "headless", "ui": null }] })
    );
}

/// Only *active* extensions are listed: a gated extension whose feature is not
/// enabled is absent, the same stub with its feature enabled is there — so the
/// absence is the gate's doing.
#[tokio::test]
async fn only_active_extensions_are_listed() {
    let disabled = ExtensionHost::new()
        .register(StubExtension::gated(STUB_FEATURE))
        .expect("registers")
        .with_gate(TestGate::enabling(&[]));
    assert_eq!(listed(&disabled).await, json!({ "extensions": [] }));

    let ungated = ExtensionHost::new().register(StubExtension::gated(STUB_FEATURE)).expect("registers");
    assert_eq!(listed(&ungated).await, json!({ "extensions": [] }));

    let enabled = ExtensionHost::new()
        .register(StubExtension::gated(STUB_FEATURE))
        .expect("registers")
        .with_gate(TestGate::enabling(&[STUB_FEATURE]));
    let body = listed(&enabled).await;
    assert_eq!(body["extensions"][0]["id"], "stub");
}

/// Several extensions are listed in registration order, with a withheld one
/// dropped from between two active ones — the order the SPA mounts navigation in.
#[tokio::test]
async fn several_extensions_are_listed_in_registration_order_and_a_withheld_one_is_dropped() {
    struct Named(&'static str, Option<&'static str>);
    impl logos_ext::Extension for Named {
        fn id(&self) -> &'static str {
            self.0
        }
        fn required_feature(&self) -> Option<&'static str> {
            self.1
        }
    }
    let host = ExtensionHost::new()
        .register(Named("alpha", None))
        .and_then(|h| h.register(Named("disabled", Some(STUB_FEATURE))))
        .and_then(|h| h.register(StubExtension::new()))
        .and_then(|h| h.register(Named("omega", Some("enabled"))))
        .expect("registers")
        .with_gate(TestGate::enabling(&["enabled"]));
    let body = listed(&host).await;
    let ids: Vec<&str> =
        body["extensions"].as_array().expect("a list").iter().map(|e| e["id"].as_str().expect("an id")).collect();
    assert_eq!(ids, ["alpha", "stub", "omega"]);
    assert_eq!(body["extensions"][0]["ui"], Value::Null);
    assert_eq!(body["extensions"][1]["ui"]["views"], json!(["/stub"]));
}

/// The list follows the gate on every request: the gate is asked anew per read,
/// so a feature it enables or withdraws at run time shows up (or goes) without
/// rebuilding the router.
#[tokio::test]
async fn the_list_follows_the_gate_on_every_request() {
    use logos_ext::FeatureGate;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Enables the stub's feature exactly while the test's switch is on.
    struct Switch(Arc<AtomicBool>);
    impl FeatureGate for Switch {
        fn enabled(&self) -> BTreeSet<String> {
            if self.0.load(Ordering::SeqCst) {
                BTreeSet::from([STUB_FEATURE.to_string()])
            } else {
                BTreeSet::new()
            }
        }
    }

    let on = Arc::new(AtomicBool::new(false));
    let host = ExtensionHost::new()
        .register(StubExtension::gated(STUB_FEATURE))
        .expect("registers")
        .with_gate(Switch(Arc::clone(&on)));
    let (_dir, router, _) = router(&host);
    for enabled in [false, true, false, true] {
        on.store(enabled, Ordering::SeqCst);
        let (status, _, body) = read(&router, get()).await;
        assert_eq!(status, StatusCode::OK);
        let body: Value = serde_json::from_slice(&body).expect("a JSON body");
        let ids: Vec<&str> =
            body["extensions"].as_array().expect("a list").iter().filter_map(|e| e["id"].as_str()).collect();
        let expected: &[&str] = if enabled { &["stub"] } else { &[] };
        assert_eq!(ids, expected, "with the feature {}: {body}", if enabled { "on" } else { "off" });
    }
}

/// `GET`-only: the method guard refuses every other method before routing, even a
/// same-origin `POST` carrying the session's intent token.
#[tokio::test]
async fn the_endpoint_is_get_only() {
    let host = ExtensionHost::new().register(StubExtension::new()).expect("registers");
    let (_dir, router, intent) = router(&host);
    // The route exists (a bare 405 from an unmounted path would prove nothing).
    assert_eq!(read(&router, get()).await.0, StatusCode::OK);
    for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
        let request = Request::builder()
            .method(method.clone())
            .uri(PATH)
            .header(header::HOST, HOST)
            .header(header::ORIGIN, ORIGIN)
            .header(INTENT_HEADER, intent.as_str())
            .body(Body::empty())
            .expect("a request");
        assert_eq!(read(&router, request).await.0, StatusCode::METHOD_NOT_ALLOWED, "{method}");
    }
}

/// Loopback-only: a request whose `Host` is not loopback (DNS rebinding) is
/// refused by the host guard, with the loopback names admitted.
#[tokio::test]
async fn the_endpoint_is_loopback_only() {
    let (_dir, router, _) = router(&ExtensionHost::new());
    let rebinding = Request::get(PATH).header(header::HOST, "evil.example.com").body(Body::empty()).expect("a request");
    assert_eq!(read(&router, rebinding).await.0, StatusCode::FORBIDDEN);
    for host in ["localhost:4983", "127.0.0.1:4983", "[::1]:4983"] {
        let request = Request::get(PATH).header(header::HOST, host).body(Body::empty()).expect("a request");
        assert_eq!(read(&router, request).await.0, StatusCode::OK, "{host}");
    }
}

/// Same-origin: the response grants no cross-origin read and carries the
/// unchanged self-only CSP.
#[tokio::test]
async fn the_endpoint_is_same_origin() {
    let (_dir, router, _) = router(&ExtensionHost::new());
    let cross_origin = Request::get(PATH)
        .header(header::HOST, HOST)
        .header(header::ORIGIN, "http://evil.example.com")
        .body(Body::empty())
        .expect("a request");
    let (status, headers, _) = read(&router, cross_origin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none(),
        "no cross-origin grant: {headers:?}"
    );
    let csp = headers.get(header::CONTENT_SECURITY_POLICY).expect("the CSP layer covers it");
    assert!(csp.to_str().expect("ASCII").contains("default-src 'self'"));
}

/// The served-body fixture the SPA's registry tests mount
/// (`web/ui/src/extensions/wire.test.ts` imports it). This test is its generator
/// **and** its guard, like the Health offenders fixture in `api_v1.rs`: the body
/// `GET /api/v1/extensions` serialises and the wire types `extensionsClient.ts`
/// declares are two hand-written halves of one contract ([S-641] → [S-642]), and
/// only a body both sides read can tell when one of them moves. Regenerate with
/// `LOGOS_UPDATE_FIXTURES=1 cargo test -p web --test extensions_endpoint`.
///
/// [S-641]: ../../docs/planning/journal.md#s-641-the-server-states-its-active-extensions-and-the-spa-bundle-is-an-injected-asset-provider
/// [S-642]: ../../docs/planning/journal.md#s-642-the-spa-mounts-navigation-views-and-config-panels-from-registered-extensions
const SERVED_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/ui/src/extensions/fixtures/extensions.real.json");

/// The served body carries every manifest field the SPA reads — both nav scopes,
/// a workspace replacement, a view without a nav entry, a config panel, a
/// Dashboard card (S-644) — plus an active extension without a manifest, and it
/// is byte-for-byte the committed fixture the SPA mounts.
#[tokio::test]
async fn the_served_body_is_the_fixture_the_spa_registry_mounts() {
    use logos_ext::ui::{UiConfigPanel, UiDashboardCard, UiManifest, UiNavGroup, UiNavItem, UiNavScope};

    struct Wire;
    impl logos_ext::Extension for Wire {
        fn id(&self) -> &'static str {
            "wire-probe"
        }
        fn ui_manifest(&self) -> Option<UiManifest> {
            Some(UiManifest {
                nav: vec![
                    UiNavItem {
                        id: "wire-probe",
                        label: "Wire probe",
                        path: "/wire-probe",
                        group: UiNavGroup::B,
                        scope: UiNavScope::Member,
                        replaced_in_workspace_by: Some("wire-probe-workspace"),
                    },
                    UiNavItem {
                        id: "wire-probe-workspace",
                        label: "Wire probe (workspace)",
                        path: "/wire-probe-workspace",
                        group: UiNavGroup::C,
                        scope: UiNavScope::App,
                        replaced_in_workspace_by: None,
                    },
                ],
                views: vec!["/wire-probe", "/wire-probe-workspace", "/wire-probe-detail"],
                config_panels: vec![UiConfigPanel { id: "wire-probe-config", label: "Wire probe", table: "wire_probe" }],
                dashboard_cards: vec![UiDashboardCard { id: "wire-probe-card", label: "Wire probe" }],
            })
        }
    }
    struct Headless;
    impl logos_ext::Extension for Headless {
        fn id(&self) -> &'static str {
            "wire-headless"
        }
    }

    let host = ExtensionHost::new().register(Wire).and_then(|h| h.register(Headless)).expect("registers");
    let served = listed(&host).await;
    let rendered = format!("{}\n", serde_json::to_string_pretty(&served).expect("serialize"));
    if std::env::var_os("LOGOS_UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(std::path::Path::new(SERVED_FIXTURE).parent().expect("a parent")).expect("mkdir");
        std::fs::write(SERVED_FIXTURE, &rendered).expect("write fixture");
    }
    // Compared as parsed values: key order is a `serde_json` feature-unification
    // detail (`preserve_order`), not part of the contract this guards.
    let committed: Value = std::fs::read_to_string(SERVED_FIXTURE)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    assert_eq!(
        committed, served,
        "the SPA's served-body fixture drifted from GET /api/v1/extensions — regenerate with \
         LOGOS_UPDATE_FIXTURES=1"
    );
}
