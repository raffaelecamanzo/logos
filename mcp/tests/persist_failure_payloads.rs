//! The MCP share of S-513 ([FR-EH-05](../../docs/specs/requirements/FR-EH-05.md)):
//! the `status` and `scan` tool payloads carry the count of files that failed
//! to persist and the stale files, exactly as the CLI's `--json` does — both
//! surfaces serialise the one `Engine` read-model ([NFR-CC-01]).
//!
//! The failure is produced through the runtime's debug-only fault seam, so the
//! file is gated on `debug_assertions`.
//!
//! [NFR-CC-01]: ../../docs/specs/requirements/NFR-CC-01.md
#![cfg(debug_assertions)]

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

type Client = RunningService<RoleClient, ()>;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Serve `engine` over an in-memory duplex and return the connected client.
async fn boot(engine: Engine) -> (Client, tokio::task::JoinHandle<()>) {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        if let Ok(running) = LogosMcp::new(engine).serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_io).await.expect("client initialize");
    (client, server)
}

async fn payload(client: &Client, tool: &'static str) -> Value {
    let result = client
        .call_tool(CallToolRequestParams::new(tool))
        .await
        .expect("tool call");
    assert_ne!(result.is_error, Some(true), "{tool} must succeed");
    let text = result.content.first().unwrap().as_text().unwrap();
    serde_json::from_str(&text.text).expect("valid JSON")
}

#[tokio::test]
async fn status_and_scan_payloads_carry_the_failed_and_stale_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    write(&root, "src/a.rs", "pub fn alpha() {}\n");
    write(&root, "src/b.rs", "pub fn beta() {}\n");

    // Index cleanly, then a sync whose edited b.rs fails: stale, last good
    // facts kept. The engine is dropped before the server boots, so the store
    // has one writer at a time.
    {
        let engine = Engine::start(&root).expect("engine");
        engine.index();
        write(&root, "src/b.rs", "pub fn beta_edited() {}\n");
        engine.runtime().unwrap().inject_persist_fault("src/b.rs");
        let synced = engine.sync(&[root.join("src/b.rs")]);
        assert_eq!(synced.files_failed, ["src/b.rs"], "the fault fired");
    }

    // The serving engine keeps the fault, so `scan`'s reconcile fails b.rs again.
    let engine = Engine::start(&root).expect("engine");
    engine.runtime().unwrap().inject_persist_fault("src/b.rs");
    let (client, server) = boot(engine).await;

    let status = payload(&client, "status").await;
    assert_eq!(status["persistence"]["failed_to_persist"], 1, "{status}");
    assert_eq!(status["persistence"]["stale_files"], serde_json::json!(["src/b.rs"]));

    let scan = payload(&client, "scan").await;
    assert_eq!(scan["persistence"]["failed_to_persist"], 1, "{scan}");
    assert_eq!(scan["persistence"]["stale_files"], serde_json::json!(["src/b.rs"]));

    client.cancel().await.expect("client shuts down");
    let _ = server.await;
}
