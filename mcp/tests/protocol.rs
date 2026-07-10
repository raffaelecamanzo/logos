//! Protocol-level acceptance tests for the Logos MCP surface (S-017).
//!
//! An in-process rmcp client drives the real `LogosMcp` handler over a tokio
//! duplex pipe — the same JSON-RPC framing as stdio, without spawning a
//! process (the process-level invariants are covered by `stdout_safety.rs`).
//!
//! Coverage: tool registration/namespacing (FR-MC-01, UAT-MC-01), thin
//! delegation outputs (FR-MC-02), server-instructions (FR-MC-03, UAT-MC-03),
//! structured errors + server-stays-alive (FR-MC-06, NFR-RA-12, UAT-MC-04),
//! and the deferred `stats` tool (FR-MC-05).

use logos_core::Engine;
use mcp::LogosMcp;
use rmcp::{
    model::{CallToolRequestParams, ErrorCode},
    service::{RoleClient, RunningService, ServiceError},
    ServiceExt,
};
use serde_json::{json, Value};

/// The 8 navigation tools wired to `Engine` methods (FR-NV-01..07).
const NAV_TOOLS: [&str; 8] = [
    "search", "context", "explore", "node", "callers", "callees", "impact", "status",
];

/// The 10 quality tools wired to the governance engine (S-020, FR-MC-01;
/// `doctor` added by S-204/CR-052, `verify` by S-205/CR-052, FR-GV-18/FR-GV-19;
/// the static test-gap tool removed by S-289/CR-079).
const QUALITY_TOOLS: [&str; 10] = [
    "scan",
    "health",
    "doctor",
    "verify",
    "session_start",
    "session_end",
    "rescan",
    "check_rules",
    "evolution",
    "dsm",
];

/// The 1 temporal tool wired to the history engine (S-048, CR-006, FR-GH-06).
const TEMPORAL_TOOLS: [&str; 1] = ["hotspots"];

/// The 3 coverage tools wired to the evidence store (S-051, CR-007, FR-CV-06/07;
/// `coverage_refresh` added by S-140/CR-036, FR-CV-10).
const COVERAGE_TOOLS: [&str; 3] = ["coverage_ingest", "coverage_status", "coverage_refresh"];

/// The 5 wiki twins wired to the wiki store (S-053, CR-008, FR-WK-09;
/// `wiki_materialize` added by S-263/CR-062, FR-WK-20). `wiki delete`/`wiki
/// skill` are CLI-only — destructive/install ops off the agent surface.
const WIKI_TOOLS: [&str; 5] = [
    "wiki_write",
    "wiki_read",
    "wiki_search",
    "wiki_status",
    "wiki_materialize",
];

type Client = RunningService<RoleClient, ()>;

/// Boot a server over a fresh temp project and connect an in-process client.
///
/// Returns the client, the server task handle (to observe teardown), and the
/// temp dir guard (dropped last so `.logos/` outlives the engine).
async fn connect() -> (Client, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = Engine::start(dir.path()).expect("engine start");
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        if let Ok(running) = LogosMcp::new(engine).serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_io).await.expect("client initialize");
    (client, server, dir)
}

async fn call(client: &Client, tool: &'static str, args: Value) -> Result<Value, ServiceError> {
    let request = match args {
        Value::Object(map) => CallToolRequestParams::new(tool).with_arguments(map),
        Value::Null => CallToolRequestParams::new(tool),
        other => panic!("test bug: tool arguments must be an object, got {other}"),
    };
    let result = client.call_tool(request).await?;
    assert_ne!(
        result.is_error,
        Some(true),
        "logos:{tool} reported a tool-execution error"
    );
    let content = result
        .content
        .first()
        .unwrap_or_else(|| panic!("logos:{tool} returned no content"));
    let text = content
        .as_text()
        .unwrap_or_else(|| panic!("logos:{tool} content is not JSON text"));
    Ok(serde_json::from_str(&text.text)
        .unwrap_or_else(|e| panic!("logos:{tool} content is not valid JSON: {e}")))
}

/// Unwrap the structured MCP error a call returned (panics on success —
/// these assertions are the FR-MC-06 contract).
fn mcp_error(result: Result<Value, ServiceError>, tool: &str) -> rmcp::model::ErrorData {
    match result {
        Err(ServiceError::McpError(data)) => data,
        Err(other) => panic!("logos:{tool} failed with a non-MCP error: {other}"),
        Ok(value) => panic!("logos:{tool} unexpectedly succeeded: {value}"),
    }
}

// ── FR-MC-01 / FR-MC-05 / UAT-MC-01: registration and namespacing ─────────

#[tokio::test]
async fn all_twenty_seven_tools_register_with_bare_names() {
    let (client, _server, _dir) = connect().await;

    let tools = client.list_all_tools().await.expect("tools/list");
    let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    names.sort_unstable();

    let mut expected: Vec<&str> = NAV_TOOLS
        .iter()
        .chain(QUALITY_TOOLS.iter())
        .chain(TEMPORAL_TOOLS.iter())
        .chain(COVERAGE_TOOLS.iter())
        .chain(WIKI_TOOLS.iter())
        .copied()
        .collect();
    expected.sort_unstable();
    assert_eq!(
        names, expected,
        "exactly the 8 navigation + 10 quality + 1 temporal + 3 coverage + 5 wiki tools must register (FR-MC-01)"
    );

    // Namespacing is the HOST's job, derived from the server identity: names
    // stay bare — no baked-in `codegraph_`/`sentrux_`/`logos:` prefix
    // (FR-MC-01) and no charset-violating `:` (MCP tool-name SEP).
    for name in &names {
        assert!(
            !name.contains(':') && !name.starts_with("codegraph") && !name.starts_with("sentrux"),
            "tool {name:?} must be a bare name — the host renders logos:{name}"
        );
    }

    // The namespace authority: the server identifies as `logos`.
    let info = client.peer_info().expect("server info");
    assert_eq!(info.server_info.name, "logos");

    // FR-MC-05: the MCP `stats` tool is deferred (CLI-only for v1).
    assert!(
        !names.contains(&"stats"),
        "no logos:stats MCP tool may register in v1 (FR-MC-05)"
    );
}

// ── FR-MC-03 / NFR-UX-04 / UAT-MC-03: server-instructions ─────────────────

#[tokio::test]
async fn server_instructions_contain_all_three_steers() {
    let (client, _server, _dir) = connect().await;

    let info = client.peer_info().expect("server info");
    let instructions = info
        .instructions
        .as_deref()
        .expect("server-instructions must be served (FR-MC-03)")
        .to_lowercase();

    // Steer 1: prefer graph tools over raw reads.
    assert!(
        instructions.contains("graph tools over raw"),
        "instructions must steer graph-first usage"
    );
    // Steer 2: the session-gate protocol, all four beats.
    for token in ["session_start", "session_end", "check_rules", "stop"] {
        assert!(
            instructions.contains(token),
            "instructions must spell out the session-gate protocol ({token})"
        );
    }
    // Steer 3: status (index) vs health (architecture) disambiguation.
    assert!(
        instructions.contains("status") && instructions.contains("health"),
        "instructions must disambiguate status vs health"
    );
}

// ── FR-MC-02 / UAT-MC-01: the 8 navigation tools delegate and answer ──────

#[tokio::test]
async fn navigation_tools_return_their_read_models() {
    let (client, _server, _dir) = connect().await;

    // Each call must yield the tool's typed read-model as JSON — the empty
    // project answers with empty-but-well-formed results (FR-NV-09 posture).
    // The expected field is UNIQUE to each tool's read-model (no `query`/
    // shared fields), so a cross-tool dispatch swap — the primary bug class
    // in a pure dispatch layer — fails loudly (FR-MC-02).
    let calls: [(&'static str, Value, &str); 8] = [
        ("search", json!({"query": "anything"}), "hits"),
        ("context", json!({"task": "find the entrypoint"}), "hops"),
        ("explore", json!({"query": "anything"}), "total_files"),
        ("node", json!({"symbol": "does::not::Exist"}), "node"),
        ("callers", json!({"symbol": "does::not::Exist"}), "callers"),
        ("callees", json!({"symbol": "does::not::Exist"}), "callees"),
        (
            "impact",
            json!({"symbol": "does::not::Exist"}),
            "upstream_label",
        ),
        ("status", Value::Null, "freshness"),
    ];
    for (tool, args, expected_field) in calls {
        let value = call(&client, tool, args).await.expect(tool);
        assert!(
            value.get(expected_field).is_some(),
            "logos:{tool} read-model must carry its {expected_field:?} field, got {value}"
        );
    }
}

#[tokio::test]
async fn search_accepts_a_valid_kind_filter() {
    let (client, _server, _dir) = connect().await;

    let value = call(
        &client,
        "search",
        json!({"query": "x", "kind": "function", "limit": 5}),
    )
    .await
    .expect("search with kind filter");
    assert_eq!(value["query"], "x");
}

// ── FR-MC-01 / S-020: the 9 quality tools answer live read-models ─────────

#[tokio::test]
async fn quality_tools_return_live_read_models() {
    let (client, _server, _dir) = connect().await;

    // Each call must yield the tool's typed read-model as JSON — a live
    // (non-stub) response even on an empty project, where the empty-graph
    // honesty posture applies (signal "n/a"/null, ADR-12). As with the
    // navigation sweep, the expected field is UNIQUE per read-model so a
    // dispatch swap fails loudly (FR-MC-02). `scan`/`rescan` share the
    // ScanResult shape by design (rescan replays the last scan).
    let calls: [(&'static str, Value, &str); 8] = [
        ("scan", json!({}), "metrics"),
        ("rescan", Value::Null, "violations"),
        ("check_rules", json!({}), "checked_rules"),
        ("evolution", json!({"limit": 5}), "snapshots"),
        ("dsm", json!({"granularity": "file"}), "matrix"),
        ("health", Value::Null, "schema_version"),
        ("session_start", Value::Null, "session_id"),
        ("session_end", Value::Null, "epsilon"),
    ];
    for (tool, args, expected_field) in calls {
        let value = call(&client, tool, args).await.expect(tool);
        assert!(
            value.get(expected_field).is_some(),
            "logos:{tool} read-model must carry its {expected_field:?} field, got {value}"
        );
    }

    // The server survived all eight live evaluations (NFR-RA-12).
    call(&client, "status", Value::Null)
        .await
        .expect("server still alive");
}

// ── FR-CV-06 / S-051: the coverage tools answer live read-models ───────────

#[tokio::test]
async fn coverage_tools_return_live_read_models() {
    let (client, _server, _dir) = connect().await;

    // On an empty temp project (no coverage ingested), `coverage_status` reports
    // n/a + a notice and still answers a typed read-model (FR-CV-06). The unique
    // `freshness_bp` field proves the dispatch landed on the right read-model.
    let status = call(&client, "coverage_status", Value::Null)
        .await
        .expect("coverage_status");
    assert!(
        status.get("freshness_bp").is_some() && status.get("notice").is_some(),
        "coverage_status carries its read-model + n/a notice, got {status}"
    );
    assert_eq!(
        status["total_files"].as_u64(),
        Some(0),
        "no coverage ingested → zero covered files"
    );

    // `coverage_ingest` with a bogus path fails loud as a structured error (the
    // report is unreadable) — never a crash; the server survives (FR-MC-06).
    let err = mcp_error(
        call(
            &client,
            "coverage_ingest",
            json!({"report": "definitely-not-here.lcov"}),
        )
        .await,
        "coverage_ingest",
    );
    assert_ne!(err.message.len(), 0, "structured error carries a message");

    // `coverage_refresh` (CR-036/FR-CV-10) dispatches to its own tool: on the
    // empty temp project no `[coverage_ingest].refresh_cmd` is configured, so it
    // returns a structured error naming the missing config — proving the tool is
    // wired (not just registered) and the server survives (FR-MC-06).
    let refresh_err = mcp_error(
        call(&client, "coverage_refresh", Value::Null).await,
        "coverage_refresh",
    );
    assert!(
        refresh_err.message.contains("refresh_cmd"),
        "coverage_refresh error names the missing config, got {:?}",
        refresh_err.message
    );

    call(&client, "status", Value::Null)
        .await
        .expect("server still alive after coverage tool errors");
}

#[tokio::test]
async fn quality_results_carry_the_freshness_line() {
    let (client, _server, _dir) = connect().await;

    // Every aggregate evaluation stamps the FR-RC-03 freshness line
    // (ADR-11); the empty temp project is outside git → "no-git".
    let value = call(&client, "scan", json!({})).await.expect("scan");
    let freshness = value["freshness"].as_str().expect("freshness is a string");
    assert!(
        freshness.contains("reconciled") && freshness.contains("unresolved refs"),
        "FR-RC-03 freshness line, got {freshness:?}"
    );

    // The FR-RC-04 escape hatch marks the result assumed-fresh.
    let value = call(&client, "scan", json!({"no_reconcile": true}))
        .await
        .expect("scan --no-reconcile");
    let freshness = value["freshness"].as_str().expect("freshness is a string");
    assert!(
        freshness.contains("assumed-fresh"),
        "FR-RC-04 assumed-fresh marker, got {freshness:?}"
    );
}

#[tokio::test]
async fn dsm_rejects_an_unknown_granularity() {
    let (client, _server, _dir) = connect().await;

    let err = mcp_error(
        call(&client, "dsm", json!({"granularity": "bogus"})).await,
        "dsm",
    );
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(err.message.contains("bogus"), "error names the bad token");
}

// ── FR-MC-06 / NFR-RA-12 / UAT-MC-04: faults are structured, never fatal ──

#[tokio::test]
async fn missing_required_argument_is_invalid_params_and_server_survives() {
    let (client, _server, _dir) = connect().await;

    let err = mcp_error(call(&client, "search", json!({})).await, "search");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);

    call(&client, "status", Value::Null)
        .await
        .expect("server still alive");
}

#[tokio::test]
async fn unknown_kind_filter_is_invalid_params_naming_the_valid_set() {
    let (client, _server, _dir) = connect().await;

    let err = mcp_error(
        call(&client, "search", json!({"query": "x", "kind": "bogus"})).await,
        "search",
    );
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(err.message.contains("bogus"), "error names the bad token");
}

#[tokio::test]
async fn unknown_tool_is_a_structured_error_and_server_survives() {
    let (client, _server, _dir) = connect().await;

    let err = mcp_error(
        call(&client, "definitely_not_a_tool", Value::Null).await,
        "?",
    );
    assert_ne!(err.message.len(), 0, "structured error carries a message");

    call(&client, "status", Value::Null)
        .await
        .expect("server still alive");
}

// ── ADR-14 / FR-EH-01: correctness faults carry the severity tag ──────────

/// A correctness fault reaching the MCP boundary is tagged `severity:
/// "correctness"` in its structured `data` payload — the same classification
/// the CLI projects onto exit code 3 (ADR-14). The tag is read from the core's
/// `error::classify`, never re-decided at the surface, so the two surfaces stay
/// consistent (FR-EH-02). A bogus coverage report is a fail-loud Correctness
/// condition; the server survives (FR-MC-06).
#[tokio::test]
async fn a_correctness_fault_is_tagged_severity_correctness() {
    let (client, _server, _dir) = connect().await;

    let err = mcp_error(
        call(
            &client,
            "coverage_ingest",
            json!({"report": "definitely-not-here.lcov"}),
        )
        .await,
        "coverage_ingest",
    );
    let data = err
        .data
        .as_ref()
        .expect("structured errors carry a data payload (ADR-14)");
    assert_eq!(
        data["severity"], "correctness",
        "a fail-loud fault is tagged correctness, got {data}"
    );
    assert_eq!(data["tool"], "coverage_ingest", "the tag names the tool");

    call(&client, "status", Value::Null)
        .await
        .expect("server still alive after a correctness fault");
}

// ── FR-MC-06 / NFR-RA-12: host disconnect tears the server down ───────────

#[tokio::test]
async fn host_disconnect_completes_the_server_task() {
    let (client, server, _dir) = connect().await;

    call(&client, "status", Value::Null)
        .await
        .expect("warm-up call");

    // Dropping the client closes its end of the pipe — the host vanished.
    drop(client);

    tokio::time::timeout(std::time::Duration::from_secs(10), server)
        .await
        .expect("server task must finish promptly after the host disconnects")
        .expect("server task must not panic");
}
