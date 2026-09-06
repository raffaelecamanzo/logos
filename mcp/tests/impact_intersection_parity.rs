//! CLI-vs-MCP payload-equality for the impact-set intersection query
//! (S-358 / [FR-NV-11], CR-114).
//!
//! Both surfaces delegate to the **one** `Engine::impact_intersection` `api`
//! entrypoint ([NFR-CC-01]) and serialise its read-model — the CLI via
//! `serde_json` in `--json` mode, the MCP twin via `Content::json`. This test
//! proves the wire payloads are byte-identical for the same repository state
//! **and the same `<id>=<symbol>` specs**, which is the half that could
//! plausibly drift: the spelling is parsed in `logos_core`, not in either
//! adapter, and this is what holds that seam in place.
//!
//! It is the fifth registration point of the pattern S-358 establishes for the
//! CR-114 query family — navigation-service body, CLI command with `--json`,
//! `/api/v1` route, MCP tool, and this parity guard. S-359 and S-360 follow it.
//!
//! [FR-NV-11]: ../../docs/specs/requirements/FR-NV-11.md
//! [NFR-CC-01]: ../../docs/specs/requirements/NFR-CC-01.md

use std::fs;
use std::path::Path;

use logos_core::Engine;
use mcp::LogosMcp;
use rmcp::{
    model::CallToolRequestParams,
    service::{RoleClient, RunningService},
    ServiceExt,
};
use serde_json::Value;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The Sprint 63 collision shape: three per-language capture arms routed
/// through one shared helper, plus an independent cluster.
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "src/http.rs", "pub fn http_client_crates() {}\n");
    for (file, arm) in [
        ("src/java.rs", "java_http_client_call"),
        ("src/typescript.rs", "typescript_http_client_call"),
    ] {
        write(
            tmp.path(),
            file,
            &format!(
                "use crate::http::http_client_crates;\n\n\
                 pub fn {arm}() {{\n    http_client_crates();\n}}\n"
            ),
        );
    }
    write(
        tmp.path(),
        "src/wiki.rs",
        "pub fn wiki_render() {\n    wiki_helper();\n}\npub fn wiki_helper() {}\n",
    );
    Engine::start(tmp.path()).expect("engine starts").index();
    tmp
}

type Client = RunningService<RoleClient, ()>;

async fn boot(root: &Path) -> (Client, tokio::task::JoinHandle<()>) {
    let engine = Engine::start(root).expect("engine start");
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        if let Ok(running) = LogosMcp::new(engine).serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_io).await.expect("client initialize");
    (client, server)
}

/// The CLI payload for `logos impact-intersection --item … --json` — exactly
/// what `Output::print` serializes (a fresh engine, dropped before the server
/// boots so the store has one writer at a time).
fn cli_payload(root: &Path, items: &[&str], depth: Option<usize>) -> Value {
    let specs: Vec<String> = items.iter().map(|s| s.to_string()).collect();
    let engine = Engine::start(root).expect("engine");
    let result = engine.impact_intersection(&specs, depth);
    serde_json::to_value(&result).expect("serialize")
}

/// The MCP `impact_intersection` tool payload for the same args.
async fn mcp_payload(client: &Client, items: &[&str], depth: Option<usize>) -> Value {
    let mut args = serde_json::Map::new();
    args.insert("items".into(), Value::from(items.to_vec()));
    if let Some(n) = depth {
        args.insert("depth".into(), Value::from(n));
    }
    let params = CallToolRequestParams::new("impact_intersection").with_arguments(args);
    let result = client
        .call_tool(params)
        .await
        .expect("impact_intersection tool call");
    assert_ne!(
        result.is_error,
        Some(true),
        "impact_intersection must succeed"
    );
    let text = result.content.first().unwrap().as_text().unwrap();
    serde_json::from_str(&text.text).expect("valid JSON")
}

#[tokio::test]
async fn cli_and_mcp_impact_intersection_payloads_are_identical() {
    let tmp = fixture();
    let root = tmp.path();
    let (client, server) = boot(root).await;

    // (1) The colliding set, at the default depth. Byte-identical payloads, and
    // the collision is actually reported (a test that compared two empty
    // results would pass while proving nothing).
    let colliding = [
        "S-341=java_http_client_call,http_client_crates",
        "S-343=typescript_http_client_call",
    ];
    let cli = cli_payload(root, &colliding, None);
    let mcp = mcp_payload(&client, &colliding, None).await;
    assert_eq!(
        cli, mcp,
        "CLI and MCP payloads must be byte-identical (FR-NV-11)"
    );
    let shared: Vec<&str> = mcp["intersecting"][0]["shared"]
        .as_array()
        .expect("the pair collides")
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(
        shared.contains(&"http_client_crates"),
        "the compared payload names the shared symbol: {shared:?}"
    );
    assert!(
        !mcp["coverage"]["statement"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "the coverage limits ride the wire payload (NFR-CC-04): {mcp}"
    );

    // (2) An explicit depth travels identically through both surfaces.
    assert_eq!(
        cli_payload(root, &colliding, Some(1)),
        mcp_payload(&client, &colliding, Some(1)).await,
        "an explicit depth must not diverge between surfaces"
    );

    // (3) The disjoint set — the `safe_parallel` half of the read-model — and
    // (4) a malformed spec, whose warning must reach BOTH surfaces identically:
    // the `<id>=<symbol>` parse lives in logos-core precisely so neither adapter
    // can invent a spelling of its own.
    let mixed = ["S-341=java_http_client_call", "S-999=wiki_render", "broken"];
    let cli = cli_payload(root, &mixed, None);
    let mcp = mcp_payload(&client, &mixed, None).await;
    assert_eq!(
        cli, mcp,
        "the disjoint + malformed-spec payload must be byte-identical"
    );
    assert_eq!(
        mcp["safe_parallel"].as_array().map(Vec::len),
        Some(1),
        "the independent cluster is safely parallel: {mcp}"
    );
    let warnings = mcp["warnings"].as_array().expect("warnings array");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap_or_default().contains("broken")),
        "a malformed spec is warned about on both surfaces: {warnings:?}"
    );

    drop(client);
    let _ = server.await;
}
