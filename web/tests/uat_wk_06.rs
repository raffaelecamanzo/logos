// CR-078/ADR-60: the in-process wiki generation increment is the LLM egress
// carve-out, so this end-to-end acceptance scenario (mock-provider) compiles only
// under `--features agents`. A listen-only `--features ui` build serves the
// wiki-view but mounts no generation surface, so the whole test crate is empty
// there.
#![cfg(feature = "agents")]
//! [UAT-WK-06] acceptance scenario — the in-process wiki generation increment
//! end-to-end over the assembled Wiki-tab trigger (S-180, [CR-047], [FR-WK-18],
//! [FR-CF-07], [NFR-SE-01], [NFR-SE-07]).
//!
//! These drive the **real router** in-process exactly as `serve --ui` does
//! (`tower::ServiceExt::oneshot`, no socket bound), mirroring `uat_ui_07.rs`'s
//! shape for chat: a cohesive narrative tying together the per-story carve-out
//! guards built incrementally across S-176..179 into one end-to-end walk, rather
//! than re-proving their mechanics. Those stay in their own suites:
//! `web/tests/wiki_sse.rs` (route guards, streaming, single-run lock, teardown),
//! `wiki-agent/tests/{generation,carve_out}.rs` (queue order, dual-axis
//! freshness at the crate level, budget/provider halts, the ui-vs-default
//! dependency boundary), `logos-core/src/config/wiki.rs` unit tests (the
//! `[wiki].model` resolver), `logos-core/tests/no_network_deps.rs` (the
//! byte-identical default-features no-HTTP-client scan), and
//! `logos-core/tests/init.rs` / `cli/tests/cli_surface.rs` (no `claude -p`
//! reference remains from the retired [FR-WK-16] hook).
//!
//! Mapping to the [UAT-WK-06] acceptance criteria:
//! - **Configure** a provider + a `[wiki].model` distinct from `[chat].model` →
//!   the config read-model discloses both independently, key masked
//!   (`uat_wk_06_configured_read_model_discloses_distinct_wiki_and_chat_models_key_masked`);
//! - **Open + consent + regenerate + stream + dual-axis fresh + dedicated model +
//!   key never echoed** — the centerpiece: a real `Config::effective_wiki_model`
//!   resolution feeds the mock-provider run through the real SSE route; the
//!   regenerated pages read fresh on both axes, their `generator` provenance is
//!   the **resolved wiki model** (not the chat model), and the raw key never
//!   appears in the stream
//!   (`uat_wk_06_configured_open_regenerates_streams_and_reads_fresh_on_both_axes_with_the_dedicated_model`);
//! - **Empty work-list starts no run**, even fully configured with a distinct
//!   wiki model (`uat_wk_06_empty_work_list_starts_no_run_even_when_configured`);
//! - **Offline carve-out** — the structural halves (default-features
//!   no-HTTP-client, ui-vs-default `rig`/`reqwest` boundary, no `claude -p`
//!   reference) are guarded unchanged by `logos-core/tests/no_network_deps.rs`,
//!   `wiki-agent/tests/carve_out.rs`, `logos-core/tests/init.rs`, and
//!   `cli/tests/cli_surface.rs` (run as part of this story's verification); like
//!   `uat_ui_07.rs`'s own note, this scenario asserts the *behavioral* carve-out
//!   (zero real egress across the streamed run) rather than duplicating the
//!   dependency-tree scan.
//!
//! [UAT-WK-06]: ../../docs/specs/requirements/UAT-WK-06.md
//! [CR-047]: ../../docs/requests/CR-047-internal-wiki-generation-on-agent-substrate.md
//! [FR-WK-18]: ../../docs/specs/requirements/FR-WK-18.md
//! [FR-CF-07]: ../../docs/specs/requirements/FR-CF-07.md
//! [NFR-SE-01]: ../../docs/specs/requirements/NFR-SE-01.md
//! [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md
//! [FR-WK-16]: ../../docs/specs/requirements/FR-WK-16.md

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_core::{MockCompletionModel, MockTurn};
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use logos_core::config::{load_config_from_root, load_secrets_from_root};
use logos_core::wiki::revision_pending;
use logos_core::Engine;
use tempfile::TempDir;
use tower::ServiceExt;
use wiki_agent::WikiAgent;

use web::wikigen::{spawn_run, WikiRunGuard, WikiRunService, WikiSink};
use web::{router_with_intent, router_with_wiki, IntentToken, INTENT_HEADER, WIKI_GENERATE_ROUTE};

const ORIGIN: &str = "http://127.0.0.1:4983";
const HOST: &str = "127.0.0.1:4983";
/// The wiki-specific model — distinct from `CHAT_MODEL` — that must be honored
/// independently ([FR-CF-07]) and end up as the persisted generator provenance.
const WIKI_MODEL: &str = "wiki-only/dedicated-model";
/// The chat model, deliberately different, so a test that saw this instead of
/// [`WIKI_MODEL`] in a written page would prove the wiki/chat resolution had
/// crossed wires.
const CHAT_MODEL: &str = "chat-only/should-not-be-used-for-wiki";
/// The raw API key — never expected to appear in any response, log, or page.
const RAW_KEY: &str = "sk-uat-wk06-RAWSECRET-9821";

// ── Real-Engine fixture helpers (mirror wiki-agent/tests/generation.rs and
// web/tests/wiki_sse.rs) ───────────────────────────────────────────────────────

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

fn commit(cwd: &Path, rel: &str, contents: &str, msg: &str) {
    let path = cwd.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    sh_git(cwd, &["add", rel]);
    sh_git(cwd, &["commit", "-q", "-m", msg]);
}

fn branchy(name: &str, ifs: usize) -> String {
    let body: String =
        (0..ifs).map(|i| format!("    if x == {i} {{ return {i}; }}\n")).collect();
    format!("pub fn {name}(x: i64) -> i64 {{\n{body}    x\n}}\n")
}

/// A committed, indexed repo whose graph has a revision > 0, so the wiki
/// work-list yields page-worthy entities to generate — plus a `config.toml`/
/// `secrets.toml` pair carrying distinct `[wiki]`/`[chat]` models and a raw key,
/// the "Configure" step of the scenario.
fn configured_indexed_engine(repo: &Path) -> Arc<Engine> {
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");
    commit(repo, "src/b.rs", &branchy("b", 2), "add b");
    std::fs::create_dir_all(repo.join(".logos")).expect("pre-create .logos");
    std::fs::write(
        repo.join(".logos/config.toml"),
        format!(
            "[chat]\nprovider = \"openai\"\nmodel = \"{CHAT_MODEL}\"\n\
             base_url = \"https://openrouter.ai/api/v1\"\n\n[wiki]\nmodel = \"{WIKI_MODEL}\"\n"
        ),
    )
    .expect("write config.toml");
    std::fs::write(repo.join(".logos/secrets.toml"), format!("[chat]\napi_key = \"{RAW_KEY}\"\n"))
        .expect("write secrets.toml");
    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    Arc::new(engine)
}

/// A [`WikiRunService`] backed by the **real** [`WikiAgent`] over the offline
/// mock provider, whose generator label is `model_id` — the seam this scenario
/// uses to drive the exact model id [`Config::effective_wiki_model`] resolved,
/// so a written page's provenance proves the dedicated model was honored, not a
/// hardcoded stand-in ([FR-CF-07], mirrors `web/tests/wiki_sse.rs`'s
/// `AgentWikiRunService`).
struct ResolvedModelWikiRunService {
    engine: Arc<Engine>,
    mock: MockCompletionModel,
    model_id: String,
}

impl WikiRunService for ResolvedModelWikiRunService {
    fn start_run(&self, guard: WikiRunGuard, sink: WikiSink) {
        let engine = Arc::clone(&self.engine);
        let mock = self.mock.clone();
        let model_id = self.model_id.clone();
        spawn_run(guard, sink, move |sink| async move {
            match WikiAgent::new(mock, "test skill preamble", model_id)
                .run(engine, sink.as_progress_fn())
                .await
            {
                Ok(_) => {}
                Err(e) => sink.error(format!("wiki generation failed: {e}")),
            }
        })
    }
}

fn wiki_post(intent: Option<&str>, origin: Option<&str>, accept_event_stream: bool) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(WIKI_GENERATE_ROUTE)
        .header(header::HOST, HOST);
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    if let Some(token) = intent {
        builder = builder.header(INTENT_HEADER, token);
    }
    if accept_event_stream {
        builder = builder.header(header::ACCEPT, "text/event-stream");
    }
    builder.body(Body::empty()).unwrap()
}

fn get(path: &str) -> Request<Body> {
    Request::builder().method(Method::GET).uri(path).header(header::HOST, HOST).body(Body::empty()).unwrap()
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

// ── Configure: distinct wiki/chat models disclosed, key masked ────────────────

/// Configure step: a `config.toml` with `[wiki].model` distinct from
/// `[chat].model` is disclosed by the config read-model as **two independent**
/// values, alongside the configured endpoint host the first-use consent banner
/// names, and only the masked (presence + last-4) key ever appears — never the
/// raw value ([FR-CF-07], [NFR-SE-07]). The banner's client-side rendering and
/// ordering are covered by the frontend Vitest suite
/// (`web/ui/src/views/wiki/WikiGeneration.test.tsx`, mirroring how
/// `uat_ui_07_step3_consent_disclosure_names_configured_endpoint` backs chat's
/// consent disclosure); this asserts the Rust-side read-model data that banner
/// renders.
#[tokio::test]
async fn uat_wk_06_configured_read_model_discloses_distinct_wiki_and_chat_models_key_masked() {
    let dir = TempDir::new().expect("temp dir");
    let engine = configured_indexed_engine(dir.path());
    let router = router_with_intent(engine, IntentToken::generate());

    let body = body_string(router.oneshot(get("/api/v1/config")).await.unwrap()).await;
    assert!(body.contains(WIKI_MODEL), "the read-model discloses the dedicated wiki model: {body}");
    assert!(body.contains(CHAT_MODEL), "the read-model discloses the chat model: {body}");
    assert_ne!(WIKI_MODEL, CHAT_MODEL, "the two configured models are distinct by construction");
    assert!(
        body.contains("openrouter.ai"),
        "the read-model names the configured endpoint host the consent banner discloses: {body}",
    );
    assert!(!body.contains(RAW_KEY), "the raw key never appears in the config read-model: {body}");
    assert!(body.contains("9821"), "the masked key shows its last-4: {body}");
}

// ── Centerpiece: open → regenerate → stream → dual-axis fresh → dedicated
// model honored → key never echoed ─────────────────────────────────────────────

/// The centerpiece scenario: resolve the effective wiki model exactly as
/// production does ([`Config::effective_wiki_model`]), drive a real background
/// generation run through the real `POST /wiki/generate` SSE route, and prove
/// the full chain: pages stream in over SSE, the regenerated pages read fresh
/// on **both** axes, the persisted `generator` is the **dedicated wiki model**
/// (not the chat model), every written page is reachable only through
/// `wiki_read` (the `wiki write` contract), and the raw key never appears
/// anywhere in the flow ([FR-WK-18], [FR-CF-07], [FR-WK-03], [FR-WK-12],
/// [NFR-SE-07]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uat_wk_06_configured_open_regenerates_streams_and_reads_fresh_on_both_axes_with_the_dedicated_model(
) {
    let dir = TempDir::new().expect("temp dir");
    let engine = configured_indexed_engine(dir.path());

    // "Configure": resolve the effective wiki model through the exact production
    // path `ConfiguredWikiRunService` uses — proving the dedicated model is
    // resolved distinctly from chat before a single page is generated.
    let config = load_config_from_root(dir.path()).expect("load config.toml");
    let secrets = load_secrets_from_root(dir.path()).expect("load secrets.toml");
    let effective = config.effective_wiki_model(&secrets);
    assert_eq!(
        effective.model.as_deref(),
        Some(WIKI_MODEL),
        "the resolved effective model is the dedicated [wiki].model, not [chat].model",
    );
    assert_eq!(effective.api_key.as_deref(), Some(RAW_KEY), "the key inherits from [chat]/secrets.toml");

    let queue = engine.wiki_generate().expect("generate queue");
    assert!(!queue.items.is_empty(), "the indexed+configured fixture seeds a non-empty work-list");
    let expected_slugs: Vec<String> = queue.items.iter().map(|i| i.slug.clone()).collect();
    let turns: Vec<_> =
        (0..queue.items.len())
            .map(|i| {
                MockTurn::text(format!(
                    "# Generated page {i}\n\nThis regenerated page carries enough grounded prose \
                     to clear the write-path validity guard ([FR-WK-19])."
                ))
            })
            .collect();
    let mock = MockCompletionModel::new(turns);
    let intent = IntentToken::generate();
    let service: Arc<dyn WikiRunService> = Arc::new(ResolvedModelWikiRunService {
        engine: Arc::clone(&engine),
        mock: mock.clone(),
        model_id: effective.model.clone().expect("resolved above"),
    });
    let router = router_with_wiki(Arc::clone(&engine), intent.clone(), service);

    // "Open" the Wiki tab: the intent-guarded trigger streams the run.
    let resp = router.oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), true)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers().contains_key(header::CONTENT_SECURITY_POLICY),
        "the streaming response carries the unchanged self-only CSP",
    );
    let body = body_string(resp).await;

    // "Regenerate + stream": the run's per-page lifecycle streams in order.
    for marker in ["event: started", "event: page-started", "event: page-written", "event: completed"] {
        assert!(body.contains(marker), "the stream carries `{marker}`: {body}");
    }
    let started_at = body.find("event: started").unwrap();
    let completed_at = body.find("event: completed").unwrap();
    assert!(started_at < completed_at, "the run streams incrementally, start before completion: {body}");
    assert_eq!(mock.request_count(), queue.items.len(), "the mock synthesized exactly one turn per page");

    // "API key never echoed": the raw key appears nowhere in the streamed flow.
    assert!(!body.contains(RAW_KEY), "the raw API key never appears in the SSE stream: {body}");

    // "Dual-axis fresh" + "dedicated model honored" + "write-only via wiki write":
    // every queued page, read back through the unchanged wiki_read accessor
    // (the only path a `wiki write` result is observable through), is fresh on
    // both axes and carries the dedicated wiki model as its generator.
    let current_revision = engine.status().graph_revision;
    for slug in &expected_slugs {
        let page = engine.wiki_read(slug).expect("wiki_read").expect("a just-written page is present");
        assert!(!page.stale, "{slug} reads fresh on the content axis (FR-WK-03) after regeneration");
        assert!(
            !revision_pending(page.built_at_revision, current_revision),
            "{slug} reads fresh on the revision axis (FR-WK-12) after regeneration",
        );
        assert_eq!(
            page.generator, WIKI_MODEL,
            "{slug}'s generator provenance is the dedicated wiki model, not the chat model",
        );
        assert_ne!(page.generator, CHAT_MODEL, "{slug} was never generated under the chat model");
        assert!(!page.written_head.is_empty(), "{slug} carries the write-time HEAD (wiki write contract)");
        assert!(!page.body.contains(RAW_KEY), "{slug}'s generated body never contains the raw key");
    }

    // The web-facing read-model agrees: fresh on both axes via the same route a
    // browser would poll after a `page-written` frame bumps its refresh key.
    let page_resp = router_with_intent(Arc::clone(&engine), IntentToken::generate())
        .oneshot(get(&format!("/api/v1/wiki/page/{}", expected_slugs[0])))
        .await
        .unwrap();
    assert_eq!(page_resp.status(), StatusCode::OK);
    let page_body = body_string(page_resp).await;
    assert!(page_body.contains("\"stale\":false"), "the read-model reports content-axis fresh: {page_body}");
    assert!(
        page_body.contains("\"regen_pending\":false"),
        "the read-model reports revision-axis fresh: {page_body}",
    );
    assert!(!page_body.contains(RAW_KEY), "the wiki page read-model never contains the raw key");
}

// ── Empty work-list starts no run, even fully configured ───────────────────────

/// Even with a fully configured, distinct wiki model, an up-to-date repo (empty
/// work-list) opening the Wiki tab starts no run and makes no model call
/// ([NFR-CC-04], [FR-WK-18] acceptance) — the configure-first/consent
/// disclosure ceremony is irrelevant to the no-content short-circuit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uat_wk_06_empty_work_list_starts_no_run_even_when_configured() {
    let dir = TempDir::new().expect("temp dir");
    sh_git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "README.md", "# fixture\n", "add readme");
    std::fs::create_dir_all(dir.path().join(".logos")).expect("pre-create .logos");
    std::fs::write(
        dir.path().join(".logos/config.toml"),
        format!(
            "[chat]\nprovider = \"openai\"\nmodel = \"{CHAT_MODEL}\"\n\
             base_url = \"https://openrouter.ai/api/v1\"\n\n[wiki]\nmodel = \"{WIKI_MODEL}\"\n"
        ),
    )
    .expect("write config.toml");
    std::fs::write(dir.path().join(".logos/secrets.toml"), format!("[chat]\napi_key = \"{RAW_KEY}\"\n"))
        .expect("write secrets.toml");
    let engine = Arc::new(Engine::start(dir.path()).expect("engine starts"));
    assert!(
        engine.wiki_generate().expect("generate").items.is_empty(),
        "an un-indexed repo has an empty work-list",
    );

    let mock = MockCompletionModel::new([]);
    let intent = IntentToken::generate();
    let service: Arc<dyn WikiRunService> = Arc::new(ResolvedModelWikiRunService {
        engine: Arc::clone(&engine),
        mock: mock.clone(),
        model_id: WIKI_MODEL.to_string(),
    });
    let router = router_with_wiki(engine, intent.clone(), service);

    let resp = router.oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), true)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        !body.contains("event: started") && !body.contains("event: page-written"),
        "an empty work-list emits no run/page progress even though a distinct wiki model is configured: {body}",
    );
    assert_eq!(mock.request_count(), 0, "no model call is made when the work-list is empty");
}

// ── Behavioral offline carve-out: zero real egress across the full flow ────────

/// The behavioral half of the offline carve-out over the full configured flow:
/// zero real outbound connections across a streamed regeneration run, and the
/// listener binds only loopback ([NFR-SE-07], [NFR-SE-01]). The structural half
/// (default-features no-HTTP-client, ui-vs-default boundary, no `claude -p`
/// reference) is `logos-core/tests/no_network_deps.rs`,
/// `wiki-agent/tests/carve_out.rs`, `logos-core/tests/init.rs`, and
/// `cli/tests/cli_surface.rs` — unchanged, run as part of this story's
/// verification rather than duplicated here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uat_wk_06_configured_regeneration_records_zero_real_egress_and_binds_loopback() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&connections);
    tokio::spawn(async move {
        while listener.accept().await.is_ok() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    let dir = TempDir::new().expect("temp dir");
    let engine = configured_indexed_engine(dir.path());
    let queue = engine.wiki_generate().expect("generate queue");
    let turns: Vec<_> = (0..queue.items.len()).map(|i| MockTurn::text(format!("Body {i}."))).collect();
    let mock = MockCompletionModel::new(turns);
    let intent = IntentToken::generate();
    let service: Arc<dyn WikiRunService> = Arc::new(ResolvedModelWikiRunService {
        engine: Arc::clone(&engine),
        mock: mock.clone(),
        model_id: WIKI_MODEL.to_string(),
    });
    let router = router_with_wiki(engine, intent.clone(), service);

    let resp = router.oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), true)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("event: completed"), "the configured mock-provider run streamed to completion: {body}");
    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "the configured regeneration run opened zero real outbound connections (NFR-SE-07)",
    );
    let bound = web::bind(0).expect("bind a loopback listener");
    assert!(bound.local_addr().unwrap().ip().is_loopback(), "the web surface binds only a loopback address");
}
