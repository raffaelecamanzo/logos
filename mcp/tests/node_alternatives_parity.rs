//! The MCP `node` answer names the alternatives a bare-name lookup passed over
//! (S-550 / [FR-NV-15]), byte-identical to what the CLI prints.
//!
//! Both surfaces delegate to the one `Engine::node` entrypoint ([NFR-CC-01]) and
//! serialise its read-model, so this guards the wire half: the `alternatives`
//! field survives the MCP adapter, and a lookup that passes over nothing keeps the
//! payload it always had (no `alternatives` key).
//!
//! `cli_payload` calls the `Engine` accessor the CLI arm calls; it does not spawn
//! the binary (`CARGO_BIN_EXE_logos` is not reachable from this crate — the split
//! `impact_intersection_parity.rs` documents).
//!
//! [FR-NV-15]: ../../docs/specs/requirements/FR-NV-15.md
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

/// A PSR-4-shaped PHP file where `Utils` is both the class and the file module.
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("src/Monolog/Utils.php");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        "<?php\n\nnamespace Monolog;\n\nfinal class Utils\n{\n    public static function canonicalize(string $path): string\n    {\n        return $path;\n    }\n}\n",
    )
    .unwrap();
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

/// The CLI payload for `logos node <symbol> --json` — what `Output::print`
/// serialises (a fresh engine, dropped before the server boots).
fn cli_payload(root: &Path, symbol: &str) -> Value {
    let engine = Engine::start(root).expect("engine");
    serde_json::to_value(engine.node(symbol, false)).expect("serialize")
}

/// The MCP `node` tool payload for the same symbol.
async fn mcp_payload(client: &Client, symbol: &str) -> Value {
    let mut args = serde_json::Map::new();
    args.insert("symbol".into(), Value::from(symbol));
    let result = client
        .call_tool(CallToolRequestParams::new("node").with_arguments(args))
        .await
        .expect("node tool call");
    assert_ne!(result.is_error, Some(true), "node must succeed");
    let text = result.content.first().unwrap().as_text().unwrap();
    serde_json::from_str(&text.text).expect("valid JSON")
}

#[tokio::test]
async fn the_mcp_node_answer_carries_the_alternatives_the_cli_prints() {
    let tmp = fixture();
    let root = tmp.path();
    let cli = cli_payload(root, "Utils");
    let (client, _server) = boot(root).await;

    let mcp = mcp_payload(&client, "Utils").await;
    assert_eq!(cli, mcp, "CLI and MCP payloads must be byte-identical (FR-NV-15)");
    assert_eq!(mcp["node"]["kind"], "class", "the lookup landed on the class: {mcp}");
    assert_eq!(
        mcp["alternatives"][0]["kind"], "module",
        "the module is named as an alternative: {mcp}"
    );
    assert_eq!(mcp["alternatives"].as_array().map(Vec::len), Some(1), "{mcp}");

    // A SCIP symbol is unchanged: no alternatives, and no key for them either.
    let module = mcp["alternatives"][0]["symbol"].as_str().expect("a symbol").to_string();
    let by_symbol = mcp_payload(&client, &module).await;
    assert_eq!(by_symbol["node"]["kind"], "module", "{by_symbol}");
    assert!(by_symbol.get("alternatives").is_none(), "{by_symbol}");
}
