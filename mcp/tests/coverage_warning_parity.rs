//! The MCP `scan` carries the [FR-QM-18] coverage warning the CLI `scan --json`
//! does (S-523): both serialize the one `Engine::scan` result, whose warning
//! comes from the shared thin-graph helper.
//!
//! **What it does NOT cover.** The CLI binary's own exit codes —
//! `cli/tests/cli_surface.rs` runs it — and the gate / session-start / readout
//! surfaces, which `logos-core/tests/metrics.rs` checks against the helper.
//!
//! [FR-QM-18]: ../../docs/specs/requirements/FR-QM-18.md

use std::fs;
use std::path::Path;

use logos_core::Engine;
use mcp::LogosMcp;
use rmcp::{
    model::CallToolRequestParams,
    service::{RoleClient, RunningService},
    ServiceExt,
};
use serde_json::{json, Value};

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
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

/// The `coverage:` entries of a `scan` payload's warnings.
fn coverage(payload: &Value) -> Vec<String> {
    payload["warnings"]
        .as_array()
        .expect("a warnings array")
        .iter()
        .filter_map(|w| w.as_str().filter(|w| w.starts_with("coverage: ")).map(str::to_owned))
        .collect()
}

/// A C project — C binds within a file only — scanned through the MCP `scan`
/// tool carries exactly the one coverage warning the shared helper computes,
/// naming `c` and its share.
#[tokio::test]
async fn the_mcp_scan_carries_the_helpers_one_coverage_warning() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write(root, "src/core/core.c", "int helper(int x) { return x + 1; }\nint step(int x) { return helper(x); }\n");
    write(root, "src/net/net.c", "int step(int x);\nint send(int x) { return step(x); }\n");
    let expected = {
        let engine = Engine::start(root).expect("engine starts");
        engine.index();
        engine
            .runtime()
            .expect("runtime")
            .submit_read(|store| logos_core::governance::thin_graph::ThinGraph::read(store))
            .expect("the helper reads")
            .warnings()
    };
    assert_eq!(expected.len(), 1, "{expected:?}");

    let (client, server) = boot(root).await;
    let params = CallToolRequestParams::new("scan")
        .with_arguments(json!({ "no_reconcile": true }).as_object().cloned().unwrap_or_default());
    let result = client.call_tool(params).await.expect("scan answers");
    assert_ne!(result.is_error, Some(true), "scan must succeed: {result:?}");
    let text = &result.content.first().unwrap().as_text().unwrap().text;
    let payload: Value = serde_json::from_str(text).expect("a JSON payload");
    assert_eq!(coverage(&payload), expected, "the MCP scan carries the helper's text, once");
    assert!(expected[0].starts_with("coverage: c is 100% of production callables"), "{}", expected[0]);

    client.cancel().await.expect("client cancel");
    server.await.expect("server task");
}
