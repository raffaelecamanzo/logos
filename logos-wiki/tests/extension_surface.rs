//! The `wiki` extension as a binary composes it ([S-644]): what it adds on every
//! surface, and that a host without it adds none of it.
//!
//! Each case compares a host composing the extension with the public host that
//! composes nothing — the composition of a `logos` binary built without the `wiki`
//! feature — over the same project, so what differs is exactly the extension's
//! contribution: the MCP tools, the web read routes, the Dashboard's overview field,
//! the served manifest (the bytes the SPA mounts), the `[wiki]` claim and the
//! telemetry names.
//!
//! [S-644]: ../../docs/planning/journal.md#s-644-the-wiki-is-its-own-crate-and-a-registered-extension

use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use logos_core::federation::Backing;
use logos_core::observability::ToolClass;
use logos_core::Engine;
use logos_ext::{Extension, ExtensionHost};
use logos_wiki::{WikiEngine as _, WikiExtension};
use mcp::LogosMcp;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

/// The body `GET /api/v1/extensions` serves for a host composing the wiki,
/// committed where the SPA reads it (`web/ui/src/test/wiki.ts`, which the navigation
/// snapshot, the Dashboard and the Config suites mount the wiki from).
const SERVED_FIXTURE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../web/ui/src/extensions/fixtures/wiki.real.json");

fn wiki_host() -> ExtensionHost {
    ExtensionHost::new().register(WikiExtension::new()).expect("registers")
}

async fn get(host: &ExtensionHost, root: &Path, uri: &str) -> (StatusCode, Value) {
    let engine = Arc::new(Engine::start(root).expect("the engine starts"));
    let router = web::hosted_router(Arc::new(Backing::Single(engine)), web::IntentToken::generate(), host)
        .expect("the router builds");
    let response = router
        .oneshot(Request::get(uri).body(Body::empty()).expect("a request"))
        .await
        .expect("the router answers");
    let status = response.status();
    let bytes = response.into_body().collect().await.expect("a body").to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// The served manifest is, as values, the fixture the SPA mounts — so the wiki's
/// registration in the bundle (`views/wiki/extension.tsx`) is proven against what
/// the server says. Regenerate with
/// `LOGOS_UPDATE_FIXTURES=1 cargo test -p logos-wiki --test extension_surface`.
#[tokio::test]
async fn the_served_manifest_is_the_fixture_the_spa_mounts() {
    let dir = TempDir::new().unwrap();
    let (status, served) = get(&wiki_host(), dir.path(), "/api/v1/extensions").await;
    assert_eq!(status, StatusCode::OK);
    if std::env::var_os("LOGOS_UPDATE_FIXTURES").is_some() {
        let rendered = format!("{}\n", serde_json::to_string_pretty(&served).expect("serialize"));
        std::fs::write(SERVED_FIXTURE, rendered).expect("write the fixture");
    }
    let committed: Value = std::fs::read_to_string(SERVED_FIXTURE)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    assert_eq!(
        committed, served,
        "the SPA's wiki manifest fixture drifted from GET /api/v1/extensions — regenerate with \
         LOGOS_UPDATE_FIXTURES=1"
    );
}

/// The five wiki tools are served exactly while the extension is composed: the
/// public host's roster names no `wiki_*` tool, the wiki host's adds exactly the
/// five — under the names the `#[tool]` methods had.
#[test]
fn the_wiki_tools_are_served_only_while_the_extension_is_composed() {
    let dir = TempDir::new().unwrap();
    let names = |host: &ExtensionHost| -> Vec<String> {
        let engine = Engine::start(dir.path()).expect("the engine starts");
        let mut names: Vec<String> =
            LogosMcp::new(engine, host).list_tools().iter().map(|t| t.name.to_string()).collect();
        names.sort();
        names
    };
    let public = names(&ExtensionHost::new());
    let composed = names(&wiki_host());
    assert!(public.len() > 20, "the core roster was read: {public:?}");
    assert!(!public.iter().any(|n| n.starts_with("wiki")), "no wiki tool without the extension: {public:?}");
    let added: Vec<&String> = composed.iter().filter(|n| !public.contains(n)).collect();
    assert_eq!(added, ["wiki_materialize", "wiki_read", "wiki_search", "wiki_status", "wiki_write"]);
}

/// The wiki's read routes, its manifest entry and its Dashboard field exist exactly
/// while the extension is composed. Without it `/api/v1/wiki` is an honest `404` (an
/// unrouted API path, never the SPA shell), the extensions list names no wiki, and
/// the overview bundle carries no `overview_page` key — not even a `null` one.
#[tokio::test]
async fn the_wiki_routes_manifest_and_overview_field_exist_only_while_composed() {
    let dir = TempDir::new().unwrap();
    let public = ExtensionHost::new();
    let wiki = wiki_host();

    assert_eq!(get(&public, dir.path(), "/api/v1/wiki").await.0, StatusCode::NOT_FOUND);
    assert_eq!(get(&wiki, dir.path(), "/api/v1/wiki").await.0, StatusCode::OK);

    let (_, listed) = get(&public, dir.path(), "/api/v1/extensions").await;
    assert_eq!(listed["extensions"], serde_json::json!([]));
    let (_, listed) = get(&wiki, dir.path(), "/api/v1/extensions").await;
    assert_eq!(listed["extensions"][0]["id"], "wiki");
    assert_eq!(
        listed["extensions"][0]["ui"]["dashboard_cards"],
        serde_json::json!([{ "id": "project-overview", "label": "Project Overview" }])
    );

    let (status, overview) = get(&public, dir.path(), "/api/v1/overview").await;
    assert_eq!(status, StatusCode::OK);
    assert!(overview.get("status").is_some(), "the core bundle was read: {overview}");
    assert!(overview.get("overview_page").is_none(), "no wiki field without the extension: {overview}");
    let (status, overview) = get(&wiki, dir.path(), "/api/v1/overview").await;
    assert_eq!(status, StatusCode::OK);
    // `get`, not indexing: indexing a missing key also yields `Null`, which would let
    // an absent field pass for an honest `null` one.
    assert_eq!(
        overview.get("overview_page"),
        Some(&Value::Null),
        "composed, an unwritten page is an honest null — present, not absent"
    );
}

/// With the extension composed, the overview field is the Project Overview page as
/// `wiki_read` returns it — the payload the Dashboard card renders, unchanged from
/// when the core read it itself.
#[tokio::test]
async fn the_overview_field_is_the_project_overview_page() {
    let dir = TempDir::new().unwrap();
    let engine = Engine::start(dir.path()).expect("the engine starts");
    engine
        .wiki_write(
            "overview/project-overview",
            "Project Overview",
            "# Project Overview\n\nLogos is a local structural code-intelligence server for agents.\n",
            &[],
            "test",
        )
        .expect("write the page");
    let expected = serde_json::to_value(engine.wiki_read("overview/project-overview").expect("read"))
        .expect("serialise");
    drop(engine);
    let (_, overview) = get(&wiki_host(), dir.path(), "/api/v1/overview").await;
    assert_eq!(overview["overview_page"]["title"], "Project Overview");
    assert_eq!(overview["overview_page"]["body"], expected["body"]);
    assert_eq!(overview["overview_page"]["generator"], "test");
}

/// The extension owns `[wiki]`, validated as the core validated it, and reports
/// under the fourteen telemetry names the core's `Tool::Wiki*` variants had, with
/// the classes the core gave them ([FR-OB-09], [FR-OB-11]) — the stats read-model
/// sees the same names and classes it always did.
///
/// [FR-OB-09]: ../../docs/specs/requirements/FR-OB-09.md
/// [FR-OB-11]: ../../docs/specs/requirements/FR-OB-11.md
#[test]
fn it_owns_the_wiki_table_and_reports_under_the_wiki_telemetry_names() {
    let wiki = WikiExtension::new();
    assert_eq!(wiki.id(), "wiki");
    assert_eq!(wiki.config_tables(), ["wiki"]);
    assert!(wiki.secret_keys().is_empty(), "the wiki reuses the chat key: no secret of its own");
    let model = toml::Value::Table(toml::toml! { model = "m" });
    assert_eq!(wiki.validate_config_table("wiki", &model), Ok(()));
    let blank = toml::Value::Table(toml::toml! { model = " " });
    assert_eq!(
        wiki.validate_config_table("wiki", &blank),
        Err("wiki.model: must not be empty".to_string())
    );

    let tools: Vec<(&str, ToolClass)> = wiki.telemetry_tools().iter().map(|t| (t.name(), t.class())).collect();
    assert_eq!(
        tools,
        [
            ("wiki_write", ToolClass::EngineInternal),
            ("wiki_read", ToolClass::Navigation),
            ("wiki_delete", ToolClass::EngineInternal),
            ("wiki_pruned_log", ToolClass::EngineInternal),
            ("wiki_search", ToolClass::Navigation),
            ("wiki_status", ToolClass::ReadModel),
            ("wiki_generate", ToolClass::EngineInternal),
            ("wiki_native", ToolClass::EngineInternal),
            ("wiki_doc_category_present", ToolClass::EngineInternal),
            ("wiki_srs_mode", ToolClass::EngineInternal),
            ("wiki_guide_pages", ToolClass::EngineInternal),
            ("wiki_reconcile", ToolClass::EngineInternal),
            ("wiki_materialize", ToolClass::EngineInternal),
            ("wiki_skill_emit", ToolClass::EngineInternal),
        ]
    );
}

/// The `init -i` skill step arrives through the host ([`ExtensionHost::init_steps`]):
/// the wiki host carries it, the public host none — so both `init` paths (the plain
/// one and the workspace fallback, which pass the same `host.init_steps()`) get it
/// exactly while the wiki is composed.
#[test]
fn the_skill_step_arrives_through_the_host() {
    assert!(ExtensionHost::new().init_steps().is_empty());
    let dir = TempDir::new().unwrap();
    let options = logos_core::init::InitOptions { materialize_skill: true, ..Default::default() };
    let rows: Vec<String> = wiki_host()
        .init_steps()
        .iter()
        .flat_map(|step| step.run(dir.path(), &options).expect("the step runs"))
        .map(|row| row.target)
        .collect();
    assert_eq!(rows, [".agents/skills/logos-wiki"]);
}
