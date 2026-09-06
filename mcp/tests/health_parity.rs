//! CLI-vs-MCP payload-equality for the ARCHITECTURE `health` surface
//! (S-361 / [FR-CL-06], [FR-GV-18], [FR-GV-20]).
//!
//! Both surfaces delegate to the **one** `Engine::health` entrypoint
//! ([NFR-CC-01]) and serialise its read-model — the CLI via `serde_json` in
//! `--json` mode, the MCP twin via `Content::json`. This test proves the wire
//! payloads are byte-identical for the same repository state.
//!
//! It is the fifth registration point of the pattern S-358 established, arriving
//! late: `health` shipped as an MCP tool long before [FR-CL-06] gave it a CLI
//! twin, so the guard only became meaningful once there were two surfaces to
//! compare.
//!
//! # Why there is no `session_start`/`session_end` parity test
//!
//! The other two twins S-361 adds are deliberately excluded, and this is the
//! place a reader will look for them:
//!
//! - `session_start` **writes**. It upserts the project baseline
//!   (`governance::session_start` → `upsert_baseline`), so calling it once per
//!   surface compares two different repository states by construction — the
//!   second call's baseline is the first call's output.
//! - Its read-model is per-invocation regardless: `SessionInfo` carries
//!   `session_id` (the new snapshot's row id, which increments) and
//!   `started_at` (a wall-clock unix timestamp). Two invocations cannot be
//!   byte-equal even against a frozen tree.
//! - `session_end` is a pure read, but its verdict is *relative to whatever
//!   baseline exists*, so it inherits the same problem the moment it is
//!   sequenced after a `session_start`.
//!
//! Their CLI seam is asserted instead where it actually differs from the MCP
//! one — the exit-code projection — by
//! `cli/tests/cli_surface.rs::the_session_gate_pair_carries_a_baseline_across_
//! processes_and_exits_one_on_regression`, through the shipped binary.
//!
//! **What this file does NOT cover.** `cli_payload` calls the `Engine` accessor
//! the CLI arm calls; it does not spawn the binary, so argv plumbing — a dropped
//! `--no-reconcile`, an inverted flag — is invisible here. That half is asserted
//! through the real executable by
//! `cli/tests/cli_surface.rs::health_reports_the_architecture_read_model_through_
//! the_binary`. The split follows `impact_intersection_parity.rs`, whose
//! `CARGO_BIN_EXE_logos` is not reachable from this crate either.
//!
//! [FR-CL-06]: ../../docs/specs/requirements/FR-CL-06.md
//! [FR-GV-18]: ../../docs/specs/requirements/FR-GV-18.md
//! [FR-GV-20]: ../../docs/specs/requirements/FR-GV-20.md
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

/// A small indexed fixture — enough graph that the payload carries real counts
/// rather than zeros, so two empty read-models cannot pass by agreeing.
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "src/core.rs", "pub fn base() {}\n");
    write(
        tmp.path(),
        "src/mid.rs",
        "use crate::core::base;\npub fn mid() {\n    base();\n}\n",
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

/// The CLI payload for `logos health --json` — exactly what `Output::print`
/// serializes. The MCP tool hardcodes `e.health(true)`, which is what the CLI's
/// default (`--no-reconcile` absent → `!false`) resolves to, so `true` here is
/// the comparable call, not a convenience.
fn cli_payload(root: &Path) -> Value {
    let engine = Engine::start(root).expect("engine");
    let report = engine.health(true).expect("health");
    serde_json::to_value(&report).expect("serialize")
}

/// The MCP `health` tool payload.
async fn mcp_payload(client: &Client) -> Value {
    let result = client
        .call_tool(CallToolRequestParams::new("health"))
        .await
        .expect("health tool call");
    assert_ne!(result.is_error, Some(true), "health must succeed");
    let text = result.content.first().unwrap().as_text().unwrap();
    serde_json::from_str(&text.text).expect("valid JSON")
}

#[tokio::test]
async fn cli_and_mcp_health_payloads_are_identical() {
    let tmp = fixture();
    let root = tmp.path();

    // The CLI engine is dropped before the server boots, so the store has one
    // writer at a time (the `hotspots_parity.rs` convention).
    let cli = cli_payload(root);
    let (client, server) = boot(root).await;
    let mcp = mcp_payload(&client).await;

    // Non-trivial first: two zeroed read-models must not pass by agreeing.
    assert_eq!(cli["ok"], Value::Bool(true), "the fixture is healthy: {cli}");
    assert!(
        cli["nodes"].as_u64().is_some_and(|n| n > 0),
        "the fixture indexed something: {cli}"
    );
    assert!(
        cli["schema_version"].as_i64().is_some(),
        "the ARCHITECTURE read-model, not another tool's: {cli}"
    );

    assert_eq!(
        cli, mcp,
        "the CLI and MCP `health` payloads must be byte-identical (NFR-CC-01)"
    );

    client.cancel().await.expect("client shuts down");
    let _ = server.await;
}
