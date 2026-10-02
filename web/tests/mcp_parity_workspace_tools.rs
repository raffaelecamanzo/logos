//! **The workspace tools answer their MCP twins** ([FR-WS-34], [NFR-PE-10]) —
//! HF-2, moved here from `agent-core/tests/addressed_workspace_tools.rs`.
//!
//! The agent tools of `agent-core` are compared, whole, with the MCP tools of
//! the `mcp` crate called over the real protocol (and, for the roster, which has
//! no MCP tool, with the read-model its HTTP route serves). That comparison needs
//! a surface crate, so it lives in a surface crate's tests: `web` already depends
//! on both `agent-core` and `mcp`, where `agent-core` dev-depending on `mcp`
//! was the one lower-crate → surface-crate edge in the workspace. The
//! assertions are the ones S-480 / S-484 wrote, unchanged; the harness is the
//! `mcp` crate's own boot-and-call one, included by path, over the same
//! two-member workspace `agent-core`'s own suite reads.
//!
//! [FR-WS-34]: ../../docs/specs/requirements/FR-WS-34.md
//! [NFR-PE-10]: ../../docs/specs/requirements/NFR-PE-10.md

#![cfg(feature = "agents")]

use agent_core::rig::tool::ToolSet;
use agent_core::{workspace_reading, workspace_toolset, xservice_toolset};
use logos_core::federation::query;
use serde_json::{json, Map, Value};

#[path = "../../mcp/tests/support/federated.rs"]
mod federated;
#[path = "../../agent-core/tests/support/workspace.rs"]
mod workspace;

use workspace::{args, Workspace};

// ── The workspace read-models answer their twins ────────────────────────────

/// Call a workspace tool through its toolset: its reading, lifted, and the
/// payload with the reading taken out — `shift_remove`, so the read-model's own
/// key order survives for the byte comparison below.
async fn workspace_call(set: &ToolSet, tool: &str, call: Value) -> (String, Value) {
    let out = set
        .call(tool, args(call))
        .await
        .unwrap_or_else(|e| panic!("{tool}: {e}"));
    let reading =
        workspace_reading(tool, &out).unwrap_or_else(|| panic!("{tool} carries a reading"));
    let mut payload: Value = serde_json::from_str(&out).expect("json");
    let object = payload.as_object_mut().expect("an object");
    assert_eq!(
        object.keys().next().map(String::as_str),
        Some("reading"),
        "{tool}: reading first"
    );
    object.shift_remove("reading");
    (reading, payload)
}

/// The two payloads serialize to the same bytes — not only the same data, which
/// `Value`'s order-insensitive equality would accept, but the same key order.
fn assert_same_payload(ours: &Value, theirs: &Value, what: &str) {
    assert_eq!(
        serde_json::to_string(ours).unwrap(),
        serde_json::to_string(theirs).unwrap(),
        "{what}: the tool's payload is its twin's, byte for byte"
    );
}

fn params(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// **S-484: each bridge-backed `xservice_*` tool names the members it read, as
/// its MCP twin does** — `member_reads` rides the tool's payload beside the
/// read-model, and the `reading` line the roster lifts is untouched by it.
#[tokio::test]
async fn each_bridge_backed_xservice_tool_names_the_members_it_read_as_its_twin_does() {
    let ws = Workspace::new();
    let set = xservice_toolset(ws.backing(false));
    let (client, server) = federated::boot(ws.registry(false)).await;

    for (tool, call) in [
        ("xservice_route_providers", json!({})),
        ("xservice_callers", json!({ "symbol": "shared" })),
        ("xservice_impact", json!({ "symbol": "shared", "repo": "api" })),
    ] {
        let out = set.call(tool, args(call.clone())).await.unwrap_or_else(|e| panic!("{tool}: {e}"));
        let ours: Value = serde_json::from_str(&out).expect("json");
        let theirs = federated::call(&client, tool, call.as_object().cloned().unwrap_or_default()).await;
        assert_eq!(ours["member_reads"], json!({ "read": ["api", "web"] }), "{tool}: {ours}");
        assert_eq!(ours["member_reads"], theirs["member_reads"], "{tool}: the twin names the same members");
        let reading = agent_core::xservice_reading(tool, &out).expect("a reading");
        assert!(!reading.contains("member_reads") && !reading.contains("read:"), "{tool}: {reading}");
    }

    client.cancel().await.ok();
    server.abort();
}

#[tokio::test]
async fn each_workspace_tool_answers_its_twins_payload() {
    let ws = Workspace::new();
    let xs = ws.backing(true);
    let set = workspace_toolset(xs.clone());
    let (client, server) = federated::boot(ws.registry(true)).await;

    // workspace_status
    let (reading, ours) = workspace_call(&set, "workspace_status", json!({})).await;
    let theirs = federated::call(&client, "workspace_status", Map::new()).await;
    assert_eq!(ours["members"].as_array().map(Vec::len), Some(2), "{ours}");
    assert_same_payload(&ours, &theirs, "workspace_status");
    // Every reading below is pinned whole: this layer's words literally, and the
    // core's own composed summaries interpolated from the payload beside them.
    let coverage = ours["coverage"]["resolved_edges_summary"].as_str().unwrap();
    let build = ours["build_dependency"]["summary"].as_str().unwrap();
    assert_eq!(
        reading,
        format!(
            "workspace_status \"shop\" — 2 member(s): 2 warm, 0 deferred, 0 degraded | store \
             open: 2 opened, 0 not attempted, 0 degraded | coverage: {coverage} | build \
             dependency (not a runtime coupling): {build}"
        )
    );

    // workspace_reachability, unscoped and scoped with the full dead set
    let (reading, ours) = workspace_call(&set, "workspace_reachability", json!({})).await;
    let theirs = federated::call(&client, "workspace_reachability", Map::new()).await;
    assert_same_payload(&ours, &theirs, "workspace_reachability");
    assert_eq!(
        reading,
        "workspace_reachability — ADVISORY: no callable live via cross-service; dead set \
         withheld (promotions only — pass all: true for it) | read 2 of 2 member(s) | seeded by \
         1 bridge invocation edge(s) beside a headline of 1 resolved"
    );
    let scoped = json!({ "repo": "api", "all": true });
    let (reading, ours) = workspace_call(&set, "workspace_reachability", scoped.clone()).await;
    let theirs = federated::call(
        &client,
        "workspace_reachability",
        params(&[("repo", json!("api")), ("all", json!(true))]),
    )
    .await;
    assert!(
        ours["dead"].is_array(),
        "the full dead set is present: {ours}"
    );
    assert_same_payload(&ours, &theirs, "workspace_reachability scoped");
    assert_eq!(ours["dead"].as_array().map(Vec::len), Some(1), "{ours}");
    assert_eq!(
        reading,
        "workspace_reachability for api — ADVISORY: no callable live via cross-service; 1 dead \
         app-wide | read 2 of 2 member(s) | seeded by 1 bridge invocation edge(s) beside a \
         headline of 1 resolved"
    );

    // A scope naming no member: the twin's payload, and a reading that says so
    // rather than reading the empty view as an empty answer for it.
    let ghost = json!({ "repo": "ghost", "all": true });
    let (reading, ours) = workspace_call(&set, "workspace_reachability", ghost).await;
    let theirs = federated::call(
        &client,
        "workspace_reachability",
        params(&[("repo", json!("ghost")), ("all", json!(true))]),
    )
    .await;
    assert_same_payload(&ours, &theirs, "workspace_reachability for a non-member");
    assert_eq!(
        reading,
        "workspace_reachability for ghost — NOT A MEMBER: \"ghost\" is not a workspace member, \
         so nothing was read for it and the empty view is not an absence; the members are: \
         api; web"
    );

    // workspace_check — the boundary rule is broken by web → api
    let (reading, ours) = workspace_call(&set, "workspace_check", json!({})).await;
    let theirs = federated::call(&client, "workspace_check", Map::new()).await;
    assert_eq!(
        ours["violations"].as_array().map(Vec::len),
        Some(1),
        "{ours}"
    );
    assert_same_payload(&ours, &theirs, "workspace_check");
    // The direction is pinned: the consumer `web` → the provider `api`.
    let violation = &ours["violations"][0];
    assert_eq!(
        reading,
        format!(
            "workspace_check \"shop\" — ADVISORY: 1 rule(s) over 1 cross-service binding(s), 1 \
             violation(s): {}: web:{} → api:{} [route]",
            violation["rule"].as_str().unwrap(),
            violation["from"]["symbol"].as_str().unwrap(),
            violation["to"]["symbol"].as_str().unwrap()
        )
    );

    // xservice_build_deps, unscoped and scoped
    let (reading, ours) = workspace_call(&set, "xservice_build_deps", json!({})).await;
    let theirs = federated::call(&client, "xservice_build_deps", Map::new()).await;
    assert_eq!(
        ours["headline"]["build_dependency_pairs"]["pairs"], 1,
        "{ours}"
    );
    assert_same_payload(&ours, &theirs, "xservice_build_deps");
    let build = ours["headline"]["summary"].as_str().unwrap().to_string();
    assert_eq!(
        reading,
        format!(
            "xservice_build_deps — BUILD DEPENDENCY, NOT A RUNTIME COUPLING: {build} | 2 member \
             row(s)"
        )
    );
    let (reading, ours) =
        workspace_call(&set, "xservice_build_deps", json!({ "repo": "web" })).await;
    let theirs = federated::call(
        &client,
        "xservice_build_deps",
        params(&[("repo", json!("web"))]),
    )
    .await;
    assert_eq!(ours["scope"], "web");
    assert_same_payload(&ours, &theirs, "xservice_build_deps scoped");
    assert_eq!(
        reading,
        format!(
            "xservice_build_deps for web — BUILD DEPENDENCY, NOT A RUNTIME COUPLING: {build} | 1 \
             member row(s)"
        )
    );
    // A scope naming no member reaches the core's `scope_note`, which the
    // reading carries rather than reading the empty rows as an answer.
    let (reading, ours) =
        workspace_call(&set, "xservice_build_deps", json!({ "repo": "ghost" })).await;
    let theirs = federated::call(
        &client,
        "xservice_build_deps",
        params(&[("repo", json!("ghost"))]),
    )
    .await;
    assert_same_payload(&ours, &theirs, "xservice_build_deps for a non-member");
    let note = ours["scope_note"]
        .as_str()
        .expect("a scope note for a non-member");
    assert_eq!(
        reading,
        format!(
            "xservice_build_deps for ghost — BUILD DEPENDENCY, NOT A RUNTIME COUPLING: {build} | 0 \
             member row(s) | {note}"
        )
    );

    // workspace_roster has no MCP tool: its twin is `GET /api/v1/workspace/roster`,
    // whose body is `query::workspace_roster` serialized.
    let (reading, ours) = workspace_call(&set, "workspace_roster", json!({})).await;
    let theirs = serde_json::to_value(query::workspace_roster(&ws.registry(true))).unwrap();
    assert_same_payload(&ours, &theirs, "workspace_roster");
    assert_eq!(reading, "workspace_roster \"shop\" — 2 member(s): api; web");

    client.cancel().await.ok();
    server.abort();
}

/// With no `[governance]` declared both surfaces answer "nothing checked": the
/// MCP twin with `null`, the tool with its reading alone and no report fields.
#[tokio::test]
async fn an_undeclared_rule_family_answers_nothing_on_both_surfaces() {
    let ws = Workspace::new();
    let set = workspace_toolset(ws.backing(false));
    let (client, server) = federated::boot(ws.registry(false)).await;
    let (reading, ours) = workspace_call(&set, "workspace_check", json!({})).await;
    let theirs = federated::call(&client, "workspace_check", Map::new()).await;
    assert_eq!(theirs, Value::Null);
    assert_eq!(ours, json!({}), "only the reading, no report");
    assert!(
        reading.contains("nothing was checked, which is not a pass"),
        "{reading}"
    );
    client.cancel().await.ok();
    server.abort();
}
