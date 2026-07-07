//! The embedded SPA shell, its intent-`<meta>` bootstrap, the history fallback,
//! and legacy-HTML delegation (S-185, CR-049, [FR-UI-22], [FR-UI-21],
//! [NFR-SE-06], [ADR-43], [ADR-31]).
//!
//! These drive the real router in-process with `tower::ServiceExt::oneshot` (no
//! socket), proving the S-185 acceptance criteria end-to-end against actual
//! responses:
//! - the shell is served at `/` under the **byte-identical** self-only CSP, with
//!   the per-session intent token delivered as a `<meta name="logos-intent">` tag;
//! - the token read from that `<meta>` passes the same-origin/intent guard on a
//!   mutating `POST`, while a cross-origin or token-less one is rejected `403`;
//! - un-migrated tabs still render their legacy server-rendered HTML through the
//!   former `/overview` (and `/health`, …) paths now resolve to the SPA shell;
//! - an unmatched HTML navigation falls back to the shell (so a client-route
//!   refresh resolves), while a non-navigation miss stays an honest `404`;
//! - the served shell names no external origin.

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

/// A router over a throwaway (un-indexed) engine — enough to serve the static
/// shell and exercise the substrate's guards.
fn test_router() -> (TempDir, axum::Router) {
    let dir = TempDir::new().expect("temp dir");
    std::fs::create_dir_all(dir.path().join(".logos")).expect("pre-create .logos");
    let engine = Arc::new(Engine::open(dir.path()));
    (dir, web::router(engine))
}

/// A router over a **writable** root plus the session's valid intent token, so a
/// guarded `config_save` can perform a real write and the test can present the
/// token it reads back from the shell's `<meta>` tag.
fn mutating_router() -> (TempDir, axum::Router, IntentToken) {
    let dir = TempDir::new().expect("temp dir");
    std::fs::create_dir_all(dir.path().join(".logos")).expect("pre-create .logos");
    let intent = IntentToken::generate();
    let engine = Arc::new(Engine::open(dir.path()));
    let router = web::router_with_intent(engine, intent.clone());
    (dir, router, intent)
}

/// A browser-navigation GET: the loopback `Host` plus `Accept: text/html` (what
/// every top-level navigation sends), so the history fallback treats it as a
/// navigation rather than an asset/XHR miss.
fn navigate(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, "127.0.0.1:4983")
        .header(header::ACCEPT, "text/html,application/xhtml+xml")
        .body(Body::empty())
        .unwrap()
}

/// A non-navigation GET (no `Accept`) — an asset/XHR-shaped request.
fn fetch(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .unwrap()
}

/// A same-origin (or, with a foreign `origin`, cross-origin) mutating `POST`
/// carrying an optional intent-token header — mirrors the config-write contract.
fn config_post(intent: Option<&str>, origin: Option<&str>, body: &'static str) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/config/save")
        .header(header::HOST, "127.0.0.1:4983")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    if let Some(token) = intent {
        builder = builder.header(INTENT_HEADER, token);
    }
    builder.body(Body::from(body)).unwrap()
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Extract the `content` of the `<meta name="logos-intent">` tag from the served
/// shell — exactly what the SPA reads once at startup.
fn intent_meta(html: &str) -> Option<String> {
    let at = html.find("name=\"logos-intent\"")?;
    let rest = &html[at..];
    let start = rest.find("content=\"")? + "content=\"".len();
    let end = rest[start..].find('"')?;
    Some(rest[start..start + end].to_string())
}

/// AC1: the shell loads at `/` under the unchanged self-only CSP, carries the
/// intent token as a `<meta>` tag, and is the SPA shell (not a legacy
/// htmx-driven server-rendered page).
#[tokio::test]
async fn shell_serves_at_root_under_self_only_csp_with_intent_meta() {
    let (_dir, router) = test_router();
    let resp = router.oneshot(navigate("/")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "the shell is served at /");

    let csp = resp
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .expect("every response carries a CSP header")
        .to_str()
        .unwrap()
        .to_string();
    assert!(csp.contains("default-src 'self'"), "self-only CSP: {csp}");
    assert!(!csp.contains('*'), "the CSP allows no wildcard source: {csp}");

    let html = body_string(resp).await;
    assert!(html.contains("<meta name=\"logos-intent\""), "the shell carries the intent meta tag");
    assert!(html.contains("Logos"), "the shell document names the app");
    // The shell is the SPA bundle, not the legacy htmx-driven Dashboard.
    assert!(
        !html.contains("/assets/vendor/htmx.min.js"),
        "the SPA shell does not load the legacy htmx runtime",
    );
    assert!(!html.contains("<script>"), "no inline <script> under the self-only CSP");
}

/// AC1: the token delivered via the shell's `<meta>` tag is the live session
/// token and passes the same-origin + intent guard on a mutating `POST` — the
/// SPA's echo path proven end-to-end.
#[tokio::test]
async fn meta_token_passes_the_same_origin_intent_guard() {
    let (dir, router, intent) = mutating_router();

    // Read the token exactly as the SPA does: from the served shell's <meta>.
    let shell = router.clone().oneshot(navigate("/")).await.unwrap();
    let html = body_string(shell).await;
    let token = intent_meta(&html).expect("the shell carries a logos-intent meta token");
    assert_eq!(token, intent.as_str(), "the meta token is the live per-session token");

    // Echo it on a same-origin mutating POST — it must clear the guard and write.
    let req = config_post(
        Some(&token),
        Some("http://127.0.0.1:4983"),
        "file=config&content=max_file_size%20%3D%201048576",
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a same-origin POST carrying the <meta> intent token passes the guard",
    );
    let written = std::fs::read_to_string(dir.path().join(".logos/config.toml"))
        .expect("the validated candidate was written");
    assert!(written.contains("max_file_size = 1048576"), "the candidate round-tripped: {written:?}");
}

/// AC1 (negative): the same `<meta>` token from a **cross-origin** page is
/// rejected `403` (the same-origin proof is mandatory, NFR-SE-06).
#[tokio::test]
async fn meta_token_from_a_cross_origin_page_is_rejected() {
    let (dir, router, _intent) = mutating_router();
    let shell = router.clone().oneshot(navigate("/")).await.unwrap();
    let token = intent_meta(&body_string(shell).await).expect("meta token present");

    let req = config_post(
        Some(&token),
        Some("http://evil.example.com"),
        "file=config&content=max_file_size%20%3D%201048576",
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "a cross-origin write is 403 even with the token");
    assert!(
        !dir.path().join(".logos/config.toml").exists(),
        "a rejected write touches nothing",
    );
}

/// AC1 (negative): a token-less mutating `POST` is rejected `403` — the
/// per-session token is mandatory, so the `<meta>` delivery is load-bearing.
#[tokio::test]
async fn token_less_mutating_post_is_rejected() {
    let (dir, router, _intent) = mutating_router();
    let req = config_post(
        None,
        Some("http://127.0.0.1:4983"),
        "file=config&content=max_file_size%20%3D%201048576",
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "no token ⇒ 403");
    assert!(!dir.path().join(".logos/config.toml").exists(), "no write on a rejected POST");
}

/// FR-UI-22 / S-192 (decommission): the server-rendered view stack is gone — the
/// former view routes (`/overview`, `/health`, `/graph`, …) are now **client-side
/// SPA routes**, so an HTML navigation to any of them resolves to the SPA shell
/// (carrying the intent `<meta>`), never legacy server-rendered HTML. This is the
/// HTTP-layer proof that the dual rendering model collapsed to one (ADR-43).
#[tokio::test]
async fn former_view_routes_now_serve_the_spa_shell() {
    let (_dir, router) = test_router();
    for path in ["/overview", "/health", "/graph", "/files", "/config", "/wiki"] {
        let resp = router.clone().oneshot(navigate(path)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{path} resolves via the SPA history fallback");
        let html = body_string(resp).await;
        assert!(
            html.contains("<meta name=\"logos-intent\""),
            "{path} now serves the SPA shell, not legacy HTML",
        );
        assert!(
            !html.contains("/assets/vendor/htmx.min.js"),
            "{path} no longer serves the legacy htmx-driven view",
        );
    }
}

/// The SPA history fallback: an unmatched **HTML navigation** resolves to the
/// shell (so a client-route refresh works), while a non-navigation miss stays an
/// honest `404` (the carve-out's unknown-GET invariant for assets/XHR).
#[tokio::test]
async fn unknown_html_navigation_falls_back_to_shell_else_404() {
    let (_dir, router) = test_router();

    let nav = router.clone().oneshot(navigate("/graph/detail/some-symbol")).await.unwrap();
    assert_eq!(nav.status(), StatusCode::OK, "an HTML navigation falls back to the shell");
    let html = body_string(nav).await;
    assert!(html.contains("<meta name=\"logos-intent\""), "the fallback serves the SPA shell");

    let miss = router.oneshot(fetch("/graph/detail/some-symbol")).await.unwrap();
    assert_eq!(
        miss.status(),
        StatusCode::NOT_FOUND,
        "the same path without an HTML Accept is an asset/XHR miss ⇒ 404",
    );
}

/// AC3: the served shell names no external origin — it references only same-origin
/// assets, so no remote host is contacted (NFR-SE-01, FR-UI-22).
#[tokio::test]
async fn served_shell_names_no_external_origin() {
    let (_dir, router) = test_router();
    let html = body_string(router.oneshot(navigate("/")).await.unwrap()).await;
    assert!(!html.contains("http://"), "the shell names no http origin: {html}");
    assert!(!html.contains("https://"), "the shell names no https origin: {html}");
}

/// Every absolute same-origin resource the served shell references (`src="/…"` /
/// `href="/…"`) — the `/assets/*` module + stylesheet AND the **root-level**
/// `/theme-init.js` a Vite build copies from `public/`.
fn shell_absolute_refs(html: &str) -> Vec<String> {
    let mut refs = Vec::new();
    for attr in ["src=\"", "href=\""] {
        let mut from = 0;
        while let Some(rel) = html[from..].find(attr) {
            let start = from + rel + attr.len();
            let end = html[start..].find('"').map(|e| start + e).unwrap_or(html.len());
            let value = &html[start..end];
            if value.starts_with('/') && !refs.iter().any(|r| r == value) {
                refs.push(value.to_string());
            }
            from = end;
        }
    }
    refs
}

/// THE SEAM TEST (sprint-31 cross-story review finding #1): every absolute
/// same-origin resource the **served shell** references must resolve `200` through
/// the *real* router as a plain asset fetch (not an HTML navigation). This is the
/// seam S-184 (asset route table) + S-185 (history fallback) + S-193 (root-level
/// `theme-init.js` bootstrap) meet: the shell links `<script src="/theme-init.js">`
/// at the bundle ROOT, but the router only registers `/assets/*path`, so before the
/// fix a classic-script fetch (`Accept: */*`) of `/theme-init.js` fell through to a
/// `404` — silently disabling the no-flash theme bootstrap in a real release build.
///
/// Self-adjusting to whatever is embedded: over the committed Node-free placeholder
/// the shell references nothing absolute (the loop is empty and the test trivially
/// holds); over a real `npm run build` (CI, or a local build) it asserts the hashed
/// `/assets/*.js`/`*.css` AND `/theme-init.js` all serve `200`. The `fetch` (no
/// `text/html` Accept) is load-bearing: it proves each ref is genuinely served as an
/// asset, never masked by the shell's HTML-navigation fallback.
#[tokio::test]
async fn every_shell_referenced_asset_resolves_through_the_router() {
    let (_dir, router) = test_router();
    let html = body_string(router.clone().oneshot(navigate("/")).await.unwrap()).await;

    for path in shell_absolute_refs(&html) {
        let resp = router.clone().oneshot(fetch(&path)).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "the shell references `{path}` but the router does not serve it (a real-build \
             release would 404 this asset): {}",
            path,
        );
    }
}
