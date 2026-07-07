//! [UAT-UI-06] acceptance scenario — config editing end-to-end over the mutating
//! web surface (S-101, [CR-025], [FR-UI-12], [FR-UI-13], [NFR-SE-06], [BR-35],
//! [ADR-31]).
//!
//! These drive the **real router** over a **started, indexed** `Engine` exactly
//! as `serve --ui` does — `tower::ServiceExt::oneshot` with no socket bound — so
//! the full edit→Save→(unchanged)→Apply→(reconciled/re-evaluated) flow is proven
//! through the HTTP boundary, not only at the engine seam. They complement:
//! - `tests/config_apply.rs` (logos-core) — the engine apply contract;
//! - `tests/carve_out.rs` — the per-guard mutating-surface fitness assertions
//!   (405/403, byte-identical no-partial-write, provenance reparse);
//! - `tests/config_view.rs` — the rendered editor view.
//!
//! Mapping to the [UAT-UI-06] steps:
//! - steps 2/6/7 — a `config.toml` save+Apply reconciles the graph; Save alone
//!   leaves it unchanged (`uat_ui_06_config_edit_save_then_apply_reconciles_the_graph`);
//! - steps 4/5/6 — a `rules.toml` save stamps provenance + reparses, and Apply
//!   re-evaluates the gate (`uat_ui_06_rules_edit_save_then_apply_re_evaluates_the_gate`);
//! - step 8 — a forged/cross-origin write and a non-config `POST` are rejected
//!   mid-flow with no effect (`uat_ui_06_forged_and_non_config_writes_are_rejected_during_the_flow`);
//! - step 9 — the listener binds loopback only and every mutating response carries
//!   the self-only CSP across the flow
//!   (`uat_ui_06_carve_out_holds_loopback_and_self_only_csp_through_the_flow`).
//!
//! Gated on `lang-rust`: the apply path indexes Rust fixtures, so a
//! `--no-default-features` build (which excludes the grammar) excludes this suite,
//! matching `logos-core/tests/config_apply.rs`.
#![cfg(feature = "lang-rust")]

use std::path::Path;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use logos_core::{Engine, Runtime};
use tempfile::TempDir;
use tower::ServiceExt;
use web::{IntentToken, INTENT_HEADER};

const SAME_ORIGIN: &str = "http://127.0.0.1:4983";

/// The contract that flags the upward domain→presentation dependency in the rules
/// fixture below (a layer-ordering + boundary violation). Mirrors the core apply
/// test's contract so the two suites assert the same governance behaviour.
const LAYERED_CONTRACT: &str = "\
[[layers]]
name  = \"domain\"
paths = [\"src/domain_*.rs\"]
order = 1

[[layers]]
name  = \"presentation\"
paths = [\"src/ui_*.rs\"]
order = 2

[[boundaries]]
from   = \"domain\"
to     = \"presentation\"
reason = \"the domain must not reach upward into presentation\"
";

/// Write `contents` to `<root>/<rel>`, creating parent directories.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(path, contents).expect("write fixture file");
}

/// The `lib.rs` → `util.rs` cross-file-call fixture: `alpha` calls
/// `crate::util::run`, which binds to `run` in `util.rs`.
fn write_cross_file_fixture(root: &Path) {
    write(root, "src/lib.rs", "pub fn alpha() {\n    crate::util::run();\n}\n");
    write(root, "src/util.rs", "pub fn run() {}\n");
}

/// Does the graph hold any node whose defining file is `rel`?
fn has_nodes_for_file(rt: &Runtime, rel: &str) -> bool {
    let rel = rel.to_string();
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .iter()
            .any(|n| n.file_path.as_deref() == Some(rel.as_str())))
    })
    .expect("read runs")
}

/// Percent-encode a string for an `application/x-www-form-urlencoded` value
/// (RFC-3986 unreserved set kept; everything else `%XX`). Avoids pulling a
/// url-encoding crate into the dev tree (NFR-SE-01: the dev-deps stay minimal).
fn form_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A `POST` to a config route with an optional same-origin `Origin` and intent
/// token, carrying a urlencoded form body (built from owned strings so the
/// multi-line rules contract can be encoded at runtime).
fn config_post(path: &str, intent: Option<&str>, origin: Option<&str>, body: String) -> Request<Body> {
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

/// Form body for a `file=<file>` + `content=<toml>` save.
fn save_body(file: &str, content: &str) -> String {
    format!("file={file}&content={}", form_value(content))
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// A router over a **started, indexed** engine rooted at `fixture`, plus the
/// session's valid intent token — the `serve --ui` shape the apply path needs (a
/// throwaway `Engine::open` engine has no runtime to reconcile on). Returns the
/// engine handle too so a test can read the graph directly before/after Apply.
fn started_router(
    fixture: impl FnOnce(&Path),
) -> (TempDir, Arc<Engine>, axum::Router, IntentToken) {
    let dir = TempDir::new().expect("temp root");
    fixture(dir.path());
    let engine = Arc::new(Engine::start(dir.path()).expect("engine starts"));
    engine.index();
    let intent = IntentToken::generate();
    let router = web::router_with_intent(Arc::clone(&engine), intent.clone());
    (dir, engine, router, intent)
}

// ── Steps 2/6/7: config.toml save+Apply reconciles; Save alone is inert ───────

/// A valid `config.toml` edit Saved through `POST /config/save` does **not** touch
/// the graph (Save runs no pipeline, [FR-UI-13]); the explicit `POST /config/apply`
/// reconciles the graph to the new admission policy ([FR-SY-07]) — the derived
/// graph changes only after Apply, never after Save alone.
#[tokio::test]
async fn uat_ui_06_config_edit_save_then_apply_reconciles_the_graph() {
    let (_dir, engine, router, intent) = started_router(write_cross_file_fixture);
    let rt = engine.runtime().expect("runtime present");
    assert!(has_nodes_for_file(rt, "src/util.rs"), "util.rs is indexed at the wide default");

    // Step 2: a valid narrowing edit (exclude util.rs) is Saved and accepted.
    let save = config_post(
        "/config/save",
        Some(intent.as_str()),
        Some(SAME_ORIGIN),
        save_body("config", "exclude = [\"src/util.rs\"]\n"),
    );
    let resp = router.clone().oneshot(save).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "the narrowing save is accepted");

    // Step 7: Save alone runs no pipeline — the graph is unchanged until Apply.
    assert!(
        has_nodes_for_file(rt, "src/util.rs"),
        "Save with no Apply leaves the derived graph unchanged (FR-UI-13)",
    );

    // Step 6: Apply reconciles the graph to the new admission policy.
    let apply = config_post(
        "/config/apply",
        Some(intent.as_str()),
        Some(SAME_ORIGIN),
        "file=config".to_string(),
    );
    let resp = router.clone().oneshot(apply).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "the apply succeeds over the started engine");
    let body = body_string(resp).await;
    assert!(body.contains("\"action\":\"reconciled\""), "a config apply reconciles: {body}");

    // The graph now reflects the new policy — only after Apply.
    assert!(
        !has_nodes_for_file(rt, "src/util.rs"),
        "the now-excluded file's nodes are purged after Apply (graph updated)",
    );
    assert!(
        has_nodes_for_file(rt, "src/lib.rs"),
        "the still-admitted caller is untouched by the reconcile",
    );
}

// ── Steps 4/5/6: rules.toml save (provenance + reparse) then Apply re-evaluates ─

/// A `rules.toml` edit Saved through the mutating route stamps a provenance comment
/// and the written contract still parses via the standard load path ([BR-35]); the
/// explicit Apply re-evaluates governance against the *unchanged* graph (no
/// reindex), so the gate reflects the new contract only after Apply ([FR-UI-13]).
#[tokio::test]
async fn uat_ui_06_rules_edit_save_then_apply_re_evaluates_the_gate() {
    let (dir, engine, router, intent) = started_router(|root| {
        // An upward dependency exists in code, but no rules.toml constrains it yet.
        write(
            root,
            "src/domain_core.rs",
            "use crate::ui_view::render;\n\npub fn compute() {\n    render();\n}\n",
        );
        write(root, "src/ui_view.rs", "pub fn render() {}\n");
    });

    // Baseline: a clean scan with no contract finds no violations.
    let baseline = engine.scan(false).expect("baseline scan");
    assert!(baseline.violations.is_empty(), "no contract yet ⇒ no violations");

    // Steps 4–5: Save the stricter contract; the engine stamps provenance.
    let save = config_post(
        "/config/save",
        Some(intent.as_str()),
        Some(SAME_ORIGIN),
        save_body("rules", LAYERED_CONTRACT),
    );
    let resp = router.clone().oneshot(save).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "a valid rules save is accepted");
    let body = body_string(resp).await;
    assert!(body.contains("\"provenance_stamped\":true"), "the save stamps provenance: {body}");

    // The written file carries the stamp and still parses via the load path.
    let written = std::fs::read_to_string(dir.path().join(".logos/rules.toml"))
        .expect("the rules file was written");
    assert!(
        written.contains("Written by the Logos web config editor"),
        "the written rules.toml carries the provenance comment: {written:?}",
    );
    assert!(
        Engine::open(dir.path()).config_read().expect("the stamped file reparses").rules.exists,
        "the stamped rules.toml reparses via the standard load path",
    );

    // Step 6: Apply re-evaluates governance against the unchanged graph (no reindex).
    let apply = config_post(
        "/config/apply",
        Some(intent.as_str()),
        Some(SAME_ORIGIN),
        "file=rules".to_string(),
    );
    let resp = router.clone().oneshot(apply).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "the rules apply succeeds");
    let body = body_string(resp).await;
    assert!(body.contains("\"action\":\"reevaluated\""), "a rules apply re-evaluates: {body}");

    // The gate now reflects the new contract — the upward dependency is flagged.
    let after = engine.scan(false).expect("post-apply scan");
    assert!(
        !after.violations.is_empty(),
        "the new contract's upward dependency is flagged after Apply (gate updated)",
    );
}

// ── Step 8: forged / non-config writes rejected mid-flow, with no effect ───────

/// During the edit+apply flow a cross-origin save (browser-set `Origin` is the
/// attacker's) is rejected `403` and a `POST` to a non-config route is `405` — the
/// mutating relaxation is bounded to the enumerated routes ([NFR-SE-06], [ADR-31]),
/// and neither touches the graph or writes a file.
#[tokio::test]
async fn uat_ui_06_forged_and_non_config_writes_are_rejected_during_the_flow() {
    let (dir, engine, router, intent) = started_router(write_cross_file_fixture);
    let rt = engine.runtime().expect("runtime present");

    // A cross-origin save carrying a valid token is rejected — no write, no apply.
    let forged = config_post(
        "/config/save",
        Some(intent.as_str()),
        Some("http://evil.example.com"),
        save_body("config", "exclude = [\"src/util.rs\"]\n"),
    );
    let resp = router.clone().oneshot(forged).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "a cross-origin write is rejected");

    // A POST to a non-config route is 405 — the relaxation is bounded.
    let non_config =
        config_post("/health", Some(intent.as_str()), Some(SAME_ORIGIN), "x=1".to_string());
    let resp = router.clone().oneshot(non_config).await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED, "a non-config POST is 405");

    // Neither rejected request had any effect: the graph is intact and the forged
    // save wrote no config.toml.
    assert!(has_nodes_for_file(rt, "src/util.rs"), "the rejected writes left the graph intact");
    assert!(
        !dir.path().join(".logos/config.toml").exists(),
        "a rejected cross-origin save performs no file write",
    );
}

// ── Step 9: loopback-only bind + self-only CSP across the edit+apply flow ──────

/// The carve-out invariant holds across the whole edit+apply flow ([ADR-31]): the
/// listener only ever binds a loopback address (no egress affordance), and **every**
/// mutating response carries the self-only CSP with no wildcard source ([BR-33]).
/// Zero outbound connection capability is additionally guaranteed structurally by
/// `logos-core/tests/no_network_deps.rs` (no network client in the surface graph),
/// so the surface can only ever listen on loopback, never dial.
#[tokio::test]
async fn uat_ui_06_carve_out_holds_loopback_and_self_only_csp_through_the_flow() {
    // The listener binds loopback only (ephemeral port ⇒ hermetic).
    let listener = web::bind(0).expect("bind a loopback ephemeral port");
    assert!(
        listener.local_addr().expect("local addr").ip().is_loopback(),
        "the web listener only ever binds a loopback address",
    );
    drop(listener);

    // Every mutating response in the flow carries the self-only CSP.
    let (_dir, _engine, router, intent) = started_router(write_cross_file_fixture);
    let flow = [
        ("/config/save", save_body("config", "exclude = [\"src/util.rs\"]\n")),
        ("/config/apply", "file=config".to_string()),
    ];
    for (path, body) in flow {
        let req = config_post(path, Some(intent.as_str()), Some(SAME_ORIGIN), body);
        let resp = router.clone().oneshot(req).await.unwrap();
        let csp = resp
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .unwrap_or_else(|| panic!("{path} response carries a CSP"))
            .to_str()
            .unwrap()
            .to_string();
        assert!(csp.contains("default-src 'self'"), "{path}: self-only CSP: {csp}");
        assert!(!csp.contains('*'), "{path}: CSP allows no wildcard source: {csp}");
    }
}
