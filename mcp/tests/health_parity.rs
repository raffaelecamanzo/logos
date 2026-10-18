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
        if let Ok(running) = LogosMcp::new(engine, &logos_ext::ExtensionHost::new()).serve(server_io).await {
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

// ── S-505 / [FR-QM-17]: the `improvements` twin ──────────────────────────────
//
// The "where to start" list rides three surfaces over ONE read of the last
// snapshot: `GET /api/v1/health` (as `scan.where_to_start`), the MCP
// `improvements` tool and `logos --json improvements`. The CLI and MCP arms are
// compared here, where `Engine` and `LogosMcp` are both reachable; the web arm
// is pinned against the binary in `cli/tests/improvements_parity.rs`, the one
// crate that reaches the binary and the router. Each pair is checked where both
// of its surfaces are reachable (the `xservice_surface.rs` convention), and the
// pairs meet on `Engine::latest_health()` — the value the Health handler
// serialises — which this test also compares, so the chain closes.
//
// [FR-QM-17]: ../../docs/specs/requirements/FR-QM-17.md

/// Eight shallow functions and one nested six deep, scanned once: the deep one is
/// a ranked fix unit and a nesting outlier against the member's own distribution
/// (nine units, past the eight-unit floor), so the list holds real entries and
/// two empty read-models cannot pass by agreeing.
fn scanned_fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut source: String = (0..8)
        .map(|i| format!("pub fn shallow_{i}(x: i32) -> i32 {{\n    if x > {i} {{\n        return x;\n    }}\n    0\n}}\n"))
        .collect();
    let open: String = (0..6).map(|i| format!("if x > {i} {{\n")).collect();
    source.push_str(&format!("pub fn deep_one(x: i32) -> i32 {{\n{open}return x;\n{}0\n}}\n", "}\n".repeat(6)));
    write(tmp.path(), "src/lib.rs", &source);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    engine.scan(true).expect("scan runs");
    tmp
}

/// The CLI payload for `logos --json improvements`: the engine's list,
/// serialised exactly as `Output::print` does.
fn cli_improvements(root: &Path) -> Value {
    let engine = Engine::start(root).expect("engine");
    serde_json::to_value(engine.improvements().expect("improvements reads")).expect("serialize")
}

async fn mcp_improvements(client: &Client) -> String {
    let result = client
        .call_tool(CallToolRequestParams::new("improvements"))
        .await
        .expect("improvements tool call");
    assert_ne!(result.is_error, Some(true), "improvements must succeed: {result:?}");
    result.content.first().unwrap().as_text().unwrap().text.clone()
}

#[tokio::test]
async fn cli_and_mcp_improvements_payloads_are_identical_and_equal_the_health_lists() {
    let tmp = scanned_fixture();
    let root = tmp.path();

    let cli = cli_improvements(root);
    let health = {
        let engine = Engine::start(root).expect("engine");
        serde_json::to_value(engine.latest_health().expect("health").scan.where_to_start).unwrap()
    };
    let (client, server) = boot(root).await;
    let (first, second) = (mcp_improvements(&client).await, mcp_improvements(&client).await);
    assert_eq!(first, second, "the MCP payload is byte-stable on an unchanged snapshot");
    let mcp: Value = serde_json::from_str(&first).expect("valid JSON");

    // Non-trivial first: a recorded list with a ranked unit and a label.
    assert_eq!(cli["recorded"], Value::Bool(true), "{cli}");
    assert_eq!(cli["units"][0]["name"], "deep_one", "{cli}");
    assert!(cli["units"][0]["points"].as_i64().is_some_and(|p| p >= 1), "{cli}");
    assert!(cli["basis"].as_str().is_some_and(|b| b.starts_with("estimate")), "labelled an estimate: {cli}");
    assert!(
        cli["outliers"].as_array().unwrap().iter().any(|m| m["measure"] == "nesting" && m["labelled"] == 1),
        "{cli}"
    );

    assert_eq!(cli, mcp, "the CLI and MCP `improvements` payloads must be identical (NFR-CC-01)");
    assert_eq!(health, mcp, "and equal the list `GET /api/v1/health` carries as `scan.where_to_start`");

    client.cancel().await.expect("client shuts down");
    let _ = server.await;
}

/// A never-scanned store answers `recorded: false`, never an empty "clean" list
/// ([NFR-CC-04]) — on both surfaces.
///
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
#[tokio::test]
async fn a_never_scanned_store_reads_improvements_not_recorded_on_both_surfaces() {
    let tmp = fixture();
    let root = tmp.path();
    let cli = cli_improvements(root);
    let (client, server) = boot(root).await;
    let mcp: Value = serde_json::from_str(&mcp_improvements(&client).await).unwrap();
    assert_eq!(cli, serde_json::json!({"recorded": false, "units": [], "outliers": []}));
    assert_eq!(cli, mcp);
    client.cancel().await.expect("client shuts down");
    let _ = server.await;
}
