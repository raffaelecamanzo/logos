//! The MCP `node`, `callers`, `callees` and `impact` answers to a symbol written
//! before symbols carried their repository coordinate (S-705, [FR-EX-36]): the
//! canonical answer plus a one-line note, byte-identical to what the CLI prints
//! — the old-form twin of `navigation_alternatives_parity.rs`.
//!
//! `cli_payload` calls the `Engine` accessor the CLI arm calls and serialises it
//! as `Output::print` does; it does not spawn the binary (`CARGO_BIN_EXE_logos`
//! is not reachable from this crate — `impact_intersection_parity.rs` says why).
//!
//! [FR-EX-36]: ../../docs/specs/requirements/FR-EX-36.md

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

/// The coordinate the fixture pins, so its symbols are the same on every machine.
const COORDINATE: &str = "example.com/fixture";

/// `run` is called by `alpha` and calls `gamma`.
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (rel, body) in [
        (".logos/config.toml", format!("[project]\nid = \"{COORDINATE}\"\n")),
        (
            "src/lib.rs",
            "pub fn alpha() {\n    run();\n}\npub fn run() {\n    gamma();\n}\npub fn gamma() {}\n".to_string(),
        ),
    ] {
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
    Engine::start(tmp.path()).expect("engine starts").index();
    tmp
}

type Client = RunningService<RoleClient, ()>;

async fn boot(root: &Path) -> (Client, tokio::task::JoinHandle<()>) {
    let engine = Engine::start(root).expect("engine start");
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        if let Ok(running) = LogosMcp::new(engine, &logos_ext::ExtensionHost::new()).serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_io).await.expect("client initialize");
    (client, server)
}

const TOOLS: [&str; 4] = ["node", "callers", "callees", "impact"];

/// The CLI payload for `logos <tool> <symbol> --json`.
fn cli_payload(root: &Path, tool: &str, symbol: &str) -> Value {
    let engine = Engine::start(root).expect("engine");
    match tool {
        "node" => serde_json::to_value(engine.node(symbol, false)),
        "callers" => serde_json::to_value(engine.callers(symbol, None)),
        "callees" => serde_json::to_value(engine.callees(symbol, None)),
        "impact" => serde_json::to_value(engine.impact(symbol, None)),
        other => panic!("not a navigation tool: {other}"),
    }
    .expect("serialize")
}

/// The MCP tool payload for the same lookup.
async fn mcp_payload(client: &Client, tool: &'static str, symbol: &str) -> Value {
    let mut args = serde_json::Map::new();
    args.insert("symbol".into(), Value::from(symbol));
    let result = client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(args))
        .await
        .unwrap_or_else(|err| panic!("{tool} tool call: {err}"));
    assert_ne!(result.is_error, Some(true), "{tool} must succeed");
    let text = result.content.first().unwrap().as_text().unwrap();
    serde_json::from_str(&text.text).expect("valid JSON")
}

/// A payload with its `query` echo and `warnings` lifted out.
fn answer(mut payload: Value) -> Value {
    let object = payload.as_object_mut().expect("an object");
    object.remove("query");
    object.remove("warnings");
    payload
}

#[tokio::test]
async fn an_old_form_symbol_answers_canonically_with_a_note_on_mcp_as_on_the_cli() {
    let tmp = fixture();
    let root = tmp.path();
    let tail = "src/`lib.rs`/run().";
    let old = format!("logos . . . {tail}");
    let canonical = format!("logos . {COORDINATE} . {tail}");
    let note = format!(
        "{old:?} carries no repository coordinate; resolved as {canonical:?}, the form this store holds"
    );
    let cli: Vec<Value> = TOOLS.iter().map(|tool| cli_payload(root, tool, &old)).collect();
    let (client, _server) = boot(root).await;

    for (tool, cli) in TOOLS.into_iter().zip(cli) {
        let mcp = mcp_payload(&client, tool, &old).await;
        assert_eq!(cli, mcp, "{tool}: CLI and MCP payloads must be byte-identical");
        assert_eq!(mcp["warnings"][0], note.as_str(), "{tool}: the note comes first: {mcp}");

        let by_canonical = mcp_payload(&client, tool, &canonical).await;
        assert_eq!(answer(mcp), answer(by_canonical.clone()), "{tool}: the canonical answer");
        assert!(
            !by_canonical["warnings"].to_string().contains("carries no repository coordinate"),
            "{tool}: a canonical symbol carries no note: {by_canonical}"
        );
    }
}
