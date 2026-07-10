//! Offline carve-out fitness tests for the web surface (CR-012, ADR-27,
//! [NFR-SE-01], [FR-UI-01], [FR-UI-03], [BR-33], [UAT-UI-02]).
//!
//! These drive the real router in-process with `tower::ServiceExt::oneshot`
//! (no socket bound) and assert the loopback listener directly. They cover
//! [UAT-UI-02] steps 3 (loopback-only bind) and 4 (non-loopback `Host` → 403,
//! non-GET → 405), plus the self-only CSP on every response (BR-33).
//!
//! Since CR-078/[ADR-60] made `ui` the shipped default (S-287), the sandboxed
//! zero-egress session ([UAT-UI-02] step 2) is exercised here against the
//! **default (listen-only) build** by [`serve_ui_session_records_zero_egress`]:
//! a full dashboard session runs through the real router while a loopback
//! tripwire monitors for any outbound dial and the listener is asserted
//! loopback-only. It is the behavioral complement to the *structural* default-
//! tree no-egress-client scan in logos-core/tests/no_network_deps.rs — there is
//! no network *client* crate in this surface's graph, so it can only listen,
//! never dial. The `agents`-gated chat/wiki-generation surface has its own
//! zero-egress proof over the full orchestrated turn (web/tests/uat_ui_07.rs,
//! [UAT-UI-07]).

use std::sync::atomic::{AtomicUsize, Ordering};
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

/// A router over a throwaway (un-indexed) engine — enough to exercise the
/// substrate's guards and the index page's Engine bridge.
fn test_router() -> (TempDir, axum::Router) {
    let dir = TempDir::new().expect("temp dir");
    std::fs::create_dir_all(dir.path().join(".logos")).expect("pre-create .logos");
    let engine = Arc::new(Engine::open(dir.path()));
    let router = web::router(engine);
    (dir, router)
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .unwrap()
}

/// Step 3: the listener only ever binds a loopback address (ADR-27). `bind(0)`
/// takes an ephemeral port so the assertion is hermetic and never conflicts.
#[test]
fn listener_binds_loopback_only() {
    let listener = web::bind(0).expect("bind loopback ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    assert!(addr.ip().is_loopback(), "bound a non-loopback address: {addr}");
    assert_eq!(addr.ip(), std::net::Ipv4Addr::LOCALHOST);
}

/// [UAT-UI-02] step 2, promoted to the shipped default build (CR-078, ADR-60):
/// the default (listen-only) `serve --ui` read session serves cleanly over a
/// loopback-only listener.
///
/// The load-bearing egress guarantee is **structural**: the default build links no
/// HTTP *client* crate, enforced by `default_tree_denies_only_egress_client_crates`
/// in logos-core/tests/no_network_deps.rs — that is what proves the default
/// dashboard can only listen, never dial. This test is its *behavioral* companion:
/// it drives a real in-process session over the SPA shell and the config read-model
/// (the read path a browser first exercises against `serve --ui`) and asserts the
/// surface listener binds only loopback (ADR-27).
///
/// The loopback tripwire is a **narrow sentinel, not a general egress monitor**: an
/// in-process `oneshot` session opens no real sockets, and nothing here is wired to
/// dial the tripwire, so `connections == 0` documents only that the session makes
/// no accidental dial to a co-located loopback service. A regression that dialled an
/// *external* host would target its own endpoint (never this tripwire), and is
/// caught elsewhere: a new egress *client* crate trips the structural default-tree
/// scan, and the residual hyper-client / raw-socket case the denylist split cannot
/// see is defended by review and the read-only surface posture (a trade-off the
/// sprint-52 risk table accepts) — not by this assertion.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_ui_session_records_zero_egress() {
    // A loopback sentinel: its accept loop counts any connection dialled to its own
    // ephemeral address. Nothing in the listen-only build is wired to it, so a clean
    // run leaves the counter at zero; only an accidental dial to this exact loopback
    // address would register (see the docstring for what this does and does not cover).
    let tripwire = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback sentinel");
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&connections);
    tokio::spawn(async move {
        while tripwire.accept().await.is_ok() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    // Drive a real dashboard session over the throwaway (un-started) engine: the
    // SPA shell and the config read-model — the read path a browser first exercises
    // against `serve --ui`, and the two endpoints that serve without a live runtime.
    let (_dir, router) = test_router();
    for path in ["/", "/api/v1/config"] {
        let resp = router.clone().oneshot(get(path)).await.unwrap();
        assert!(
            resp.status().is_success(),
            "GET {path} serves during the dashboard session: {}",
            resp.status(),
        );
    }

    // The session made no dial to the loopback sentinel (its narrow scope) …
    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "the listen-only `serve --ui` session made no accidental dial to a loopback \
         service (UAT-UI-02); the no-egress-*client* guarantee is structural, in \
         logos-core/tests/no_network_deps.rs",
    );
    // … and the surface listener binds only loopback (the listen side of the seam).
    let bound = web::bind(0).expect("bind a loopback listener");
    assert!(
        bound.local_addr().unwrap().ip().is_loopback(),
        "the default `serve --ui` surface binds only a loopback address (ADR-27)",
    );
}

/// Step 5: a port conflict fails with an actionable error naming the `--port`
/// remedy (NFR-UX-02). Holding an ephemeral port and re-binding it is hermetic
/// (no fixed port) and reliable — two live listeners on one loopback port
/// collide with `AddrInUse` even though std sets `SO_REUSEADDR`.
#[test]
fn port_conflict_is_an_actionable_error() {
    let held = web::bind(0).expect("hold an ephemeral loopback port");
    let port = held.local_addr().expect("local addr").port();
    let err = web::bind(port).expect_err("re-binding the held port must fail").to_string();
    assert!(
        err.contains("already in use") && err.contains("--port"),
        "the conflict names the port and the --port remedy: {err}",
    );
}

/// The SPA shell at `/` renders through the submit-and-await Engine bridge and
/// carries the self-only CSP (FR-UI-03, BR-33) — the carve-out's per-response CSP
/// invariant over the one surviving HTML entry point, with no inline script and no
/// wildcard source. The shell's full contract is in `tests/spa_shell.rs`.
#[tokio::test]
async fn index_renders_and_carries_self_only_csp() {
    let (_dir, router) = test_router();
    let resp = router.oneshot(get("/")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let csp = resp
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .expect("every response carries a CSP header")
        .to_str()
        .unwrap()
        .to_string();
    assert!(csp.contains("default-src 'self'"), "self-only CSP: {csp}");
    assert!(!csp.contains('*'), "CSP allows no wildcard source: {csp}");

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Logos"), "renders the application shell");
    // The SPA bundle is CSP-clean by construction (FR-UI-22, ADR-44): external
    // hashed `<script src>` only, never an inline `<script>` block.
    assert!(!html.contains("<script>"), "no inline <script> block under the self-only CSP");
}

/// Step 4a: every non-GET request is answered 405 — and still carries the CSP.
#[tokio::test]
async fn non_get_methods_are_405() {
    for method in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
        let (_dir, router) = test_router();
        let req = Request::builder()
            .method(method.clone())
            .uri("/")
            .header(header::HOST, "127.0.0.1:4983")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} must be rejected 405",
        );
        assert!(
            resp.headers().contains_key(header::CONTENT_SECURITY_POLICY),
            "the 405 response still carries the CSP",
        );
    }
}

/// A non-GET to an unknown path is also 405 (the method guard runs before
/// routing, so the read-only posture holds for every path).
#[tokio::test]
async fn non_get_unknown_path_is_405() {
    let (_dir, router) = test_router();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/anything/at/all")
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
}

/// Step 4b: a request whose `Host` is not a loopback host is rejected 403
/// (DNS-rebinding defense, FR-UI-01).
#[tokio::test]
async fn non_loopback_host_is_403() {
    let (_dir, router) = test_router();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/")
        .header(header::HOST, "evil.example.com")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(
        resp.headers().contains_key(header::CONTENT_SECURITY_POLICY),
        "the 403 response still carries the CSP",
    );
}

/// A loopback `Host` is accepted (the guard does not over-block).
#[tokio::test]
async fn loopback_host_is_accepted() {
    for host in ["localhost:4983", "127.0.0.1", "[::1]:4983"] {
        let (_dir, router) = test_router();
        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .header(header::HOST, host)
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{host} should be accepted");
    }
}

// ── Mutating surface: enumerated POST routes + intent guard (ADR-31, NFR-SE-06,
// [UAT-UI-06]) ───────────────────────────────────────────────────────────────

/// A router over a **writable** root plus the session's valid intent token, so
/// the config-save bridge can perform a real validated atomic write (S-096) and
/// the tests can present the session token (and forge invalid ones).
fn mutating_router() -> (TempDir, axum::Router, IntentToken) {
    let dir = TempDir::new().expect("temp dir");
    std::fs::create_dir_all(dir.path().join(".logos")).expect("pre-create .logos");
    let intent = IntentToken::generate();
    let engine = Arc::new(Engine::open(dir.path()));
    let router = web::router_with_intent(engine, intent.clone());
    (dir, router, intent)
}

/// A `POST` to a config route, with optionally a same-origin `Origin` and an
/// intent-token header, carrying a urlencoded form body.
fn config_post(
    path: &str,
    intent: Option<&str>,
    origin: Option<&str>,
    body: &'static str,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(path)
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

/// The happy path: a same-origin `POST` carrying the valid intent token to the
/// enumerated save route is admitted, bridges to `config_write`, and the
/// validated candidate is written atomically — with the self-only CSP still
/// stamped on the mutating response. The submitted candidate is non-empty and
/// the written bytes are read back, so the test proves the payload actually
/// round-tripped through `config_write` (not a handler that wrote an empty file).
#[tokio::test]
async fn config_save_with_valid_intent_and_same_origin_writes() {
    let (dir, router, intent) = mutating_router();
    // A valid, non-default candidate: `max_file_size = 1048576` (urlencoded).
    let req = config_post(
        "/config/save",
        Some(intent.as_str()),
        Some("http://127.0.0.1:4983"),
        "file=config&content=max_file_size%20%3D%201048576",
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "a same-origin, intent-bearing save is accepted");
    assert!(
        resp.headers().contains_key(header::CONTENT_SECURITY_POLICY),
        "the mutating response still carries the self-only CSP",
    );
    let written = std::fs::read_to_string(dir.path().join(".logos/config.toml"))
        .expect("the validated candidate was written atomically");
    assert!(
        written.contains("max_file_size = 1048576"),
        "the submitted candidate round-tripped through config_write, not an empty write: {written:?}",
    );
}

/// S-169 / [FR-CF-06] / [NFR-SE-07]: the chat key write goes to the gitignored
/// `secrets.toml`, the response is masked (presence + last-4) and **never echoes
/// the raw key**, the key never lands in `config.toml`, and a subsequent
/// `GET /config` renders only the masked presence — the secret appears in no
/// response body or page source.
#[tokio::test]
async fn config_secret_write_persists_but_never_echoes_the_key() {
    const RAW_KEY: &str = "sk-or-v1-supersecret-9876";
    let (dir, router, intent) = mutating_router();

    // Write the key through the enumerated, guarded secret route.
    let req = config_post(
        "/config/secret",
        Some(intent.as_str()),
        Some("http://127.0.0.1:4983"),
        "api_key=sk-or-v1-supersecret-9876",
    );
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "a guarded secret write is accepted");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        !body.contains(RAW_KEY),
        "the write response must never echo the raw key (NFR-SE-07): {body}",
    );
    assert!(body.contains("9876"), "the masked response carries the last-4: {body}");

    // The key persists at rest in the gitignored secrets.toml…
    let secrets = std::fs::read_to_string(dir.path().join(".logos/secrets.toml")).unwrap();
    assert!(secrets.contains(RAW_KEY), "the key persists in secrets.toml");
    // …and never in the checked-in config.toml (absent here = certainly no key).
    assert!(
        !dir.path().join(".logos/config.toml").exists()
            || !std::fs::read_to_string(dir.path().join(".logos/config.toml"))
                .unwrap()
                .contains(RAW_KEY),
        "the key must never appear in config.toml",
    );

    // The SPA reads config through GET /api/v1/config; that read-model carries
    // only the masked presence (last-4) — never the raw key (NFR-SE-07).
    let page = router.oneshot(get("/api/v1/config")).await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let page_body = page.into_body().collect().await.unwrap().to_bytes();
    let page_body = String::from_utf8(page_body.to_vec()).unwrap();
    assert!(
        !page_body.contains(RAW_KEY),
        "the config read-model must never contain the raw key (NFR-SE-07)",
    );
    assert!(
        page_body.contains("9876"),
        "the read-model shows the masked presence (last-4)",
    );
}

/// The apply route admits a same-origin, intent-bearing `POST` and bridges to
/// `config_apply` (it is not 405/403 — the guards passed). On the throwaway
/// `Engine::open` engine the apply has no runtime to reconcile on, so it fails
/// loud as a `500` rather than fabricating a result — the engine's contract; a
/// successful apply over a started engine is the S-100/S-101 flow.
#[tokio::test]
async fn config_apply_route_admits_guarded_post() {
    let (_dir, router, intent) = mutating_router();
    let req = config_post(
        "/config/apply",
        Some(intent.as_str()),
        Some("http://127.0.0.1:4983"),
        "file=rules",
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "the apply route is reached (guards passed); a transient engine fails loud",
    );
}

/// A forged cross-origin write (browser-set `Origin` is the attacker's) is
/// rejected `403` even with a valid-looking token — and nothing is written.
#[tokio::test]
async fn cross_origin_config_save_is_403_with_no_write() {
    let (dir, router, intent) = mutating_router();
    let req = config_post(
        "/config/save",
        Some(intent.as_str()),
        Some("http://evil.example.com"),
        "file=config&content=",
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "a cross-origin write is rejected");
    assert!(
        !dir.path().join(".logos/config.toml").exists(),
        "a rejected write performs no file write",
    );
}

/// An intent-less write (no token header) is rejected `403` with no write, even
/// though it is same-origin — the per-session token is mandatory (NFR-SE-06).
#[tokio::test]
async fn config_save_without_intent_token_is_403_with_no_write() {
    let (dir, router, _intent) = mutating_router();
    let req =
        config_post("/config/save", None, Some("http://127.0.0.1:4983"), "file=config&content=");
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(!dir.path().join(".logos/config.toml").exists(), "no token ⇒ no write");
}

/// A wrong (forged) token of the right length is rejected `403` — the
/// constant-time compare returns no match — and nothing is written.
#[tokio::test]
async fn config_save_with_forged_token_is_403_with_no_write() {
    let (dir, router, _intent) = mutating_router();
    let forged = "deadbeef".repeat(8); // 64 hex chars, same shape, wrong value
    let req = config_post(
        "/config/save",
        Some(&forged),
        Some("http://127.0.0.1:4983"),
        "file=config&content=",
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(!dir.path().join(".logos/config.toml").exists(), "a forged token ⇒ no write");
}

/// S-169 / [NFR-SE-06] / [NFR-SE-07]: the `/config/secret` route is behind the
/// **same** intent + same-origin guards as `/config/save`. A cross-origin write,
/// a token-less write, and a forged-token write are each `403` and write nothing
/// to `secrets.toml` — the key cannot be set by an unguarded request.
#[tokio::test]
async fn config_secret_write_is_guarded_like_config_save() {
    let forged = "deadbeef".repeat(8); // 64 hex chars, right shape, wrong value
    let cases: [(&str, Option<&str>, Option<&str>); 3] = [
        // (label, intent token, origin)
        ("cross-origin", None, Some("http://evil.example.com")),
        ("no token", None, Some("http://127.0.0.1:4983")),
        ("forged token", Some(forged.as_str()), Some("http://127.0.0.1:4983")),
    ];
    for (label, token, origin) in cases {
        let (dir, router, intent) = mutating_router();
        // The cross-origin case carries the *valid* token but an attacker origin.
        let token = if label == "cross-origin" {
            Some(intent.as_str())
        } else {
            token
        };
        let req = config_post("/config/secret", token, origin, "api_key=sk-should-not-persist");
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "a {label} secret write must be rejected 403",
        );
        assert!(
            !dir.path().join(".logos/secrets.toml").exists(),
            "a rejected {label} secret write must persist no key",
        );
    }
}

/// A mutating `POST` with a valid token but **no** `Origin` header is rejected:
/// a same-origin proof is mandatory, and a mutating request without one cannot be
/// trusted as same-origin.
#[tokio::test]
async fn config_save_without_origin_is_403() {
    let (_dir, router, intent) = mutating_router();
    let req = config_post("/config/save", Some(intent.as_str()), None, "file=config&content=");
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// A `POST` to a non-enumerated path still returns `405`: the `POST` relaxation is
/// bounded to exactly the enumerated config/chat routes, so the SPA host (`/`), the
/// `/api/v1/*` read-models, and any unknown path stay GET-only.
#[tokio::test]
async fn post_to_non_config_route_is_405() {
    for path in ["/", "/api/v1/health", "/api/v1/graph", "/does-not-exist"] {
        let (_dir, router, intent) = mutating_router();
        let req = config_post(path, Some(intent.as_str()), Some("http://127.0.0.1:4983"), "x=1");
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "POST {path} (non-config) must be 405",
        );
    }
}

/// Under a listen-only `--features ui` build (no `agents`), the chat and
/// wiki-**generation** POST routes are never mounted and `method_guard` never
/// admits them, so a **well-formed** mutating POST (valid same-origin + intent
/// token, to rule out a mere CSRF `403`) is answered `405` — the dialing seam is
/// genuinely absent, not merely guarded (CR-078, ADR-60, S-286). This is the
/// complementary listen-only assertion to the `agents`-side positive coverage in
/// `chat_sse.rs` / `wiki_sse.rs`, and it locks the core runtime consequence of
/// the S-286 carve-out. Compiled only without `agents` (under `agents` these
/// routes are admitted, so a 405 assertion would be wrong).
#[cfg(not(feature = "agents"))]
#[tokio::test]
async fn chat_and_wiki_generation_posts_are_405_without_agents() {
    for path in [web::CHAT_POST_ROUTE, web::CHAT_CLEAR_ROUTE, web::WIKI_GENERATE_ROUTE] {
        let (_dir, router, intent) = mutating_router();
        // A fully valid mutating POST — same-origin + intent token — so the only
        // possible rejection is `method_guard`'s 405 (an unmounted, unadmitted
        // route), never `intent_guard`'s 403.
        let req = config_post(path, Some(intent.as_str()), Some("http://127.0.0.1:4983"), "q=hi");
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "POST {path} must be 405 in a listen-only (no-`agents`) build — the \
             chat/wiki-generation dialing seam is carved out (CR-078, ADR-60)",
        );
    }
}

/// An invalid candidate is rejected `422` with the typed validation message and
/// **no partial write** — the engine validates before the atomic swap (S-096,
/// NFR-RA-07); the file stays absent here.
#[tokio::test]
async fn config_save_invalid_candidate_is_422_with_no_write() {
    let (dir, router, intent) = mutating_router();
    // `zzz=1` is an unknown key — `#[serde(deny_unknown_fields)]` rejects it.
    let req = config_post(
        "/config/save",
        Some(intent.as_str()),
        Some("http://127.0.0.1:4983"),
        "file=config&content=zzz%3D1",
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "an invalid candidate is 422");
    assert!(
        !dir.path().join(".logos/config.toml").exists(),
        "a rejected candidate performs no partial write",
    );
}

/// A guarded `POST` with an unknown `file` value is a `400` (the guards passed,
/// but the body names no real policy file) — and writes nothing.
#[tokio::test]
async fn config_save_unknown_policy_file_is_400() {
    let (dir, router, intent) = mutating_router();
    let req = config_post(
        "/config/save",
        Some(intent.as_str()),
        Some("http://127.0.0.1:4983"),
        "file=bogus&content=",
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(!dir.path().join(".logos/config.toml").exists(), "an unknown file ⇒ no write");
}

/// A same-host but **different-port** `Origin` is rejected `403` — the
/// same-origin check binds the write to the exact origin (authority incl. port),
/// not merely "some loopback page". A no-port `Origin` against a ported `Host` is
/// likewise rejected. This is the realistic loopback CSRF case (another local dev
/// server on a different port).
#[tokio::test]
async fn config_save_with_port_mismatched_origin_is_403() {
    for origin in ["http://127.0.0.1:9999", "http://127.0.0.1"] {
        let (dir, router, intent) = mutating_router();
        let req = config_post("/config/save", Some(intent.as_str()), Some(origin), "file=config&content=");
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "origin {origin} must be rejected");
        assert!(!dir.path().join(".logos/config.toml").exists(), "no write on {origin}");
    }
}

/// A wrong-**length** token (the `matches()` length-mismatch fast path) is also
/// rejected `403` with no write — not just the same-length forged case.
#[tokio::test]
async fn config_save_with_wrong_length_token_is_403() {
    let (dir, router, _intent) = mutating_router();
    let req = config_post(
        "/config/save",
        Some("abcd1234"), // 8 chars, not 64
        Some("http://127.0.0.1:4983"),
        "file=config&content=",
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(!dir.path().join(".logos/config.toml").exists(), "a wrong-length token ⇒ no write");
}

/// The apply route shares the `parse_policy_file` guard: an unknown `file` value
/// (guards otherwise passed) is `400`.
#[tokio::test]
async fn config_apply_unknown_policy_file_is_400() {
    let (_dir, router, intent) = mutating_router();
    let req =
        config_post("/config/apply", Some(intent.as_str()), Some("http://127.0.0.1:4983"), "file=bogus");
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// [UAT-UI-06] step 3 / [BR-35] regression — the atomic no-partial-write guard
/// over a **pre-existing** file: an invalid edit is `422` (naming the fault) and
/// the file on disk is left **byte-identical**. The `..._invalid_candidate_...`
/// test above covers the absent-file case (nothing written); this proves the
/// stronger property the spec calls out — a real prior file is not even partially
/// overwritten by a rejected save (S-096 validate-before-atomic-swap, NFR-RA-07).
#[tokio::test]
async fn config_save_invalid_edit_leaves_a_preexisting_file_byte_identical() {
    let (dir, router, intent) = mutating_router();
    let path = dir.path().join(".logos/config.toml");
    // A valid file already on disk — the "before" the editor would be editing.
    std::fs::write(&path, "# keep-me marker\nmax_file_size = 4242\n").unwrap();
    let before = std::fs::read(&path).expect("read the pre-existing file");

    // An invalid candidate (unknown key) submitted same-origin with a valid token.
    let req = config_post(
        "/config/save",
        Some(intent.as_str()),
        Some("http://127.0.0.1:4983"),
        "file=config&content=zzz%3D1",
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "an invalid edit is 422");
    let msg = String::from_utf8(resp.into_body().collect().await.unwrap().to_bytes().to_vec())
        .unwrap();
    assert!(msg.contains("zzz"), "the 422 message names the offending key: {msg:?}");

    let after = std::fs::read(&path).expect("read the file after the rejected save");
    assert_eq!(
        before, after,
        "an invalid save leaves the pre-existing file byte-identical — no partial write (BR-35)",
    );
}

/// [UAT-UI-06] steps 4–5 / [BR-35]: a valid `rules.toml` save through the mutating
/// route stamps a provenance comment and the written contract **still parses** via
/// the standard load path (read back through a fresh `config_read`). The
/// confirmation gate itself is a same-origin client concern (the browser-level
/// UAT-UI-06 drive); here we lock the server-side provenance + reparse invariant.
#[tokio::test]
async fn config_save_rules_write_stamps_provenance_and_reparses() {
    let (dir, router, intent) = mutating_router();
    // A minimal valid rules contract (`[constraints]\nmax_cc = 10\n`, urlencoded).
    let req = config_post(
        "/config/save",
        Some(intent.as_str()),
        Some("http://127.0.0.1:4983"),
        "file=rules&content=%5Bconstraints%5D%0Amax_cc%20%3D%2010%0A",
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "a valid same-origin rules save is accepted");
    let body = String::from_utf8(resp.into_body().collect().await.unwrap().to_bytes().to_vec())
        .unwrap();
    assert!(body.contains("\"provenance_stamped\":true"), "the outcome reports the stamp: {body}");

    // The on-disk file carries the provenance comment …
    let written = std::fs::read_to_string(dir.path().join(".logos/rules.toml"))
        .expect("the rules file was written");
    assert!(
        written.contains("Written by the Logos web config editor"),
        "the written rules.toml carries the provenance comment: {written:?}",
    );
    // … and still parses via the standard load path (the read half of the seam).
    let read = Engine::open(dir.path()).config_read().expect("the stamped file reparses");
    assert!(read.rules.exists, "the reparsed rules view sees the written file");
}

/// An unknown **non-navigation** GET path is 404 (not 405 — GET is the allowed
/// method). The `get()` helper sends no `Accept`, so the SPA history fallback
/// (ADR-43) treats it as an asset/XHR miss and 404s rather than serving the shell;
/// the HTML-navigation case (`Accept: text/html` → shell) is asserted in
/// `tests/spa_shell.rs`. This keeps the carve-out's "unknown GET ⇒ 404" invariant
/// intact for everything that is not a browser navigation.
#[tokio::test]
async fn unknown_get_path_is_404() {
    let (_dir, router) = test_router();
    let resp = router.oneshot(get("/does-not-exist")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
