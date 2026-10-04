//! The MCP `callers`, `callees`, `impact` and `explore` answers name the
//! alternatives a bare-name lookup passed over (HF-1 / [FR-NV-15]), byte-identical
//! to what the CLI prints — the four-tool twin of `node_alternatives_parity.rs`.
//!
//! Every surface delegates to the one `Engine` accessor per tool ([NFR-CC-01]) and
//! serialises its read-model, so this guards the wire half: the `alternatives`
//! field survives the MCP adapter on each tool, and a lookup that passes over
//! nothing keeps the payload it always had (no `alternatives` key).
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

/// The four tools, each with the argument name its lookup text travels under.
const TOOLS: [(&str, &str); 4] = [
    ("callers", "symbol"),
    ("callees", "symbol"),
    ("impact", "symbol"),
    ("explore", "query"),
];

/// The CLI payload for `logos <tool> <text> --json` — what `Output::print`
/// serialises (a fresh engine, dropped before the server boots).
fn cli_payload(root: &Path, tool: &str, text: &str) -> Value {
    let engine = Engine::start(root).expect("engine");
    match tool {
        "callers" => serde_json::to_value(engine.callers(text, None)),
        "callees" => serde_json::to_value(engine.callees(text, None)),
        "impact" => serde_json::to_value(engine.impact(text, None)),
        "explore" => serde_json::to_value(engine.explore(text, None)),
        other => panic!("not a navigation tool: {other}"),
    }
    .expect("serialize")
}

/// The MCP tool payload for the same lookup.
async fn mcp_payload(client: &Client, tool: &'static str, arg: &str, text: &str) -> Value {
    let mut args = serde_json::Map::new();
    args.insert(arg.into(), Value::from(text));
    let result = client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(args))
        .await
        .unwrap_or_else(|err| panic!("{tool} tool call: {err}"));
    assert_ne!(result.is_error, Some(true), "{tool} must succeed");
    let text = result.content.first().unwrap().as_text().unwrap();
    serde_json::from_str(&text.text).expect("valid JSON")
}

/// The node a payload resolved to: `anchor` for `explore`, `resolved` otherwise.
fn resolved_kind<'a>(tool: &str, payload: &'a Value) -> &'a Value {
    let key = if tool == "explore" { "anchor" } else { "resolved" };
    &payload[key]["kind"]
}

#[tokio::test]
async fn the_mcp_answers_carry_the_alternatives_the_cli_prints() {
    let tmp = fixture();
    let root = tmp.path();
    let cli: Vec<Value> = TOOLS.iter().map(|(tool, _)| cli_payload(root, tool, "Utils")).collect();
    let (client, _server) = boot(root).await;

    for ((tool, arg), cli) in TOOLS.into_iter().zip(cli) {
        let mcp = mcp_payload(&client, tool, arg, "Utils").await;
        assert_eq!(cli, mcp, "{tool}: CLI and MCP payloads must be byte-identical (FR-NV-15)");
        assert_eq!(resolved_kind(tool, &mcp), "class", "{tool}: landed on the class: {mcp}");
        assert_eq!(mcp["alternatives"][0]["kind"], "module", "{tool}: {mcp}");
        assert_eq!(mcp["alternatives"].as_array().map(Vec::len), Some(1), "{tool}: {mcp}");

        // A SCIP symbol is unchanged: no alternatives, and no key for them either.
        let module = mcp["alternatives"][0]["symbol"].as_str().expect("a symbol").to_string();
        let by_symbol = mcp_payload(&client, tool, arg, &module).await;
        assert_eq!(resolved_kind(tool, &by_symbol), "module", "{tool}: {by_symbol}");
        assert!(by_symbol.get("alternatives").is_none(), "{tool}: {by_symbol}");
    }
}
