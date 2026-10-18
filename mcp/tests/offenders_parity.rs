//! CLI-vs-MCP parity for the uncapped offender listing (S-676, [FR-QM-19]):
//! `logos offenders --json` and the `offenders` tool.
//!
//! Both surfaces delegate to the one `Engine::offenders` entrypoint
//! ([NFR-CC-01]), through the core's own `OffendersFilter::parse`. This test
//! proves the two serialize the **same value** for one snapshot, filtered and
//! unfiltered, that each is byte-stable when asked twice, and that a bad
//! dimension is refused on the MCP side as invalid parameters.
//!
//! **What it does NOT cover.** `cli_payload` calls the `Engine` accessor the
//! CLI arm calls; it does not spawn the binary — `cli/tests/offenders_cli.rs`
//! runs the binary itself. The roster contract that `offenders` is
//! `logos offenders` is `cli/src/lib.rs::surface_parity`. The split follows
//! `bless_rules_parity.rs`.
//!
//! [FR-QM-19]: ../../docs/specs/requirements/FR-QM-19.md
//! [NFR-CC-01]: ../../docs/specs/requirements/NFR-CC-01.md

use std::fs;
use std::path::Path;

use logos_core::governance::OffendersFilter;
use logos_core::Engine;
use mcp::LogosMcp;
use rmcp::{
    model::{CallToolRequestParams, ErrorCode},
    service::{RoleClient, RunningService, ServiceError},
    ServiceExt,
};
use serde_json::{json, Value};

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// `name` nesting `depth` `if`s.
fn nested(name: &str, depth: usize) -> String {
    let open: String = (0..depth).map(|i| format!("if x > {i} {{\n")).collect();
    format!("pub fn {name}(x: i32) -> i32 {{\n{open}return x;\n{}0\n}}\n", "}\n".repeat(depth))
}

/// Twelve deep functions under `src/a` — past the display cap — and two in
/// `src/ab`, scanned once, so both surfaces read one snapshot with real
/// entries on both sides of the glob.
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let a: String = (0..12).map(|i| nested(&format!("a{i:02}"), 4 + i % 3)).collect();
    write(root, "src/lib.rs", "pub mod a;\npub mod ab;\n");
    write(root, "src/a/mod.rs", &a);
    write(root, "src/ab/mod.rs", &format!("{}{}", nested("ab0", 5), nested("ab1", 4)));
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    engine.scan(true).expect("scan runs");
    tmp
}

type Client = RunningService<RoleClient, ()>;

async fn boot(root: &Path) -> (Client, tokio::task::JoinHandle<()>) {
    let engine = Engine::start(root).expect("engine start");
    let (client_io, server_io) = tokio::io::duplex(256 * 1024);
    let server = tokio::spawn(async move {
        if let Ok(running) =
            LogosMcp::new(engine, &logos_ext::ExtensionHost::new()).serve(server_io).await
        {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_io).await.expect("client initialize");
    (client, server)
}

/// What the CLI arm prints for these tokens: the engine's listing, serialized.
fn cli_payload(root: &Path, path: Option<&str>, dimension: Option<&str>) -> String {
    let engine = Engine::start(root).expect("engine");
    let filter = OffendersFilter::parse(path, dimension).expect("valid filter");
    serde_json::to_string(&engine.offenders(&filter).expect("offenders reads")).unwrap()
}

async fn mcp_call(client: &Client, arguments: Value) -> Result<String, ServiceError> {
    let params = CallToolRequestParams::new("offenders")
        .with_arguments(arguments.as_object().cloned().unwrap_or_default());
    let result = client.call_tool(params).await?;
    assert_ne!(result.is_error, Some(true), "offenders must succeed: {result:?}");
    Ok(result.content.first().unwrap().as_text().unwrap().text.clone())
}

#[tokio::test]
async fn cli_and_mcp_list_the_same_offenders_byte_stably() {
    let tmp = fixture();
    let root = tmp.path();
    let cases: [(Option<&str>, Option<&str>, Value); 3] = [
        (None, None, json!({})),
        (Some("src/a/**"), Some("nesting"), json!({"path": "src/a/**", "dimension": "nesting"})),
        (None, Some("equality"), json!({"dimension": "equality"})),
    ];
    let cli: Vec<String> = cases.iter().map(|(p, d, _)| cli_payload(root, *p, *d)).collect();

    let (client, server) = boot(root).await;
    for ((_, _, arguments), cli) in cases.iter().zip(&cli) {
        let first = mcp_call(&client, arguments.clone()).await.expect("tool call");
        let second = mcp_call(&client, arguments.clone()).await.expect("tool call");
        assert_eq!(second, first, "the MCP payload is byte-stable: {arguments}");
        let (mcp, cli): (Value, Value) =
            (serde_json::from_str(&first).unwrap(), serde_json::from_str(cli).unwrap());
        assert_eq!(mcp, cli, "the same listing on both surfaces: {arguments}");
        assert_eq!(mcp["recorded"], true, "{mcp}");
    }
    assert_eq!(cli_payload(root, None, None), cli[0], "the CLI payload is byte-stable");

    let glob: Value = serde_json::from_str(&cli[1]).unwrap();
    let listed = glob["offenders"].as_array().unwrap();
    assert_eq!(listed.len(), 12, "all twelve under src/a, uncapped: {glob}");
    assert!(listed.iter().all(|o| o["file"].as_str().unwrap().starts_with("src/a/")), "{glob}");
    for field in ["node_id", "measure", "value", "threshold"] {
        assert!(listed.iter().all(|o| !o[field].is_null()), "{field} is carried: {glob}");
    }

    let refused = mcp_call(&client, json!({"dimension": "modularity"})).await;
    match refused {
        Err(ServiceError::McpError(error)) => {
            assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
            assert!(error.message.contains("modularity"), "the refusal names the token: {error:?}");
        }
        other => panic!("an unknown dimension is invalid params, got {other:?}"),
    }

    client.cancel().await.expect("client cancel");
    server.await.expect("server task");
}
