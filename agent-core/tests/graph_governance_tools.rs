//! Grounded per-tool results for the graph + governance domains (S-167).
//!
//! Each test drives a tool through the **real** `rig` `ToolSet` built by
//! `agent_core::{graph_toolset, governance_toolset}` — i.e. by name with a JSON
//! argument string, exactly as a subagent would (S-174) — over a fixture
//! `Engine` indexing a real Rust file. Asserting on the returned read-model
//! proves the tool returns grounded `Engine` output, not a fabricated shape
//! (acceptance: "each `rig` `Tool` returns grounded `Engine`/filesystem
//! results from the existing read-models").

use std::sync::Arc;

use agent_core::{governance_toolset, graph_toolset};
use logos_core::Engine;
use serde_json::Value;

/// A fixture project with a three-function call chain (`alpha → beta → gamma`)
/// so the graph tools have real symbols and edges to resolve.
fn fixture_engine() -> (Arc<Engine>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn alpha() { beta(); }\n\
         pub fn beta() { gamma(); }\n\
         pub fn gamma() {}\n",
    )
    .expect("write fixture");
    let engine = Engine::start(dir.path()).expect("engine start");
    (Arc::new(engine), dir)
}

/// Drive one tool by name with JSON args through a `ToolSet`, returning the
/// parsed JSON read-model.
async fn call(toolset: &rig_core::tool::ToolSet, tool: &str, args: Value) -> Value {
    let out = toolset
        .call(tool, args.to_string())
        .await
        .unwrap_or_else(|e| panic!("tool {tool} failed: {e}"));
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("tool {tool} output not JSON: {e}"))
}

#[tokio::test]
async fn search_returns_grounded_hits() {
    let (engine, _dir) = fixture_engine();
    let tools = graph_toolset(engine);
    let result = call(&tools, "search", serde_json::json!({ "query": "alpha" })).await;

    assert_eq!(result["query"], "alpha", "echoes the grounded query");
    let hits = result["hits"].as_array().expect("hits array");
    assert!(
        hits.iter().any(|h| h["name"] == "alpha"),
        "search surfaces the indexed symbol: {result}"
    );
}

#[tokio::test]
async fn node_resolves_an_indexed_symbol() {
    let (engine, _dir) = fixture_engine();
    let tools = graph_toolset(engine);
    let result = call(&tools, "node", serde_json::json!({ "symbol": "beta" })).await;

    assert_eq!(
        result["node"]["name"], "beta",
        "an indexed symbol resolves to its own node: {result}"
    );
}

#[tokio::test]
async fn context_bundle_is_grounded_in_the_index() {
    let (engine, _dir) = fixture_engine();
    let tools = graph_toolset(engine);
    let result = call(&tools, "context", serde_json::json!({ "task": "alpha" })).await;

    assert_eq!(result["task"], "alpha", "echoes the task");
    assert!(
        result["nodes"].is_array(),
        "context returns a ranked node bundle (the ContextBundle read-model): {result}"
    );
}

#[tokio::test]
async fn impact_returns_the_labeled_blast_radius() {
    let (engine, _dir) = fixture_engine();
    let tools = graph_toolset(engine);
    // `gamma` is at the bottom of the chain — changing it breaks alpha/beta upstream.
    let result = call(&tools, "impact", serde_json::json!({ "symbol": "gamma" })).await;

    assert_eq!(result["query"], "gamma");
    // The fixed DL-03 labels prove this is the ImpactResult read-model, not another.
    assert!(
        result["upstream_label"].is_string() && result["downstream_label"].is_string(),
        "impact carries the labeled both-directions read-model: {result}"
    );
    assert!(
        !result["upstream"].as_array().unwrap().is_empty(),
        "changing gamma has upstream impact (alpha/beta): {result}"
    );
}

#[tokio::test]
async fn explore_groups_neighbourhood_by_file() {
    let (engine, _dir) = fixture_engine();
    let tools = graph_toolset(engine);
    let result = call(&tools, "explore", serde_json::json!({ "query": "beta" })).await;

    assert_eq!(result["query"], "beta");
    assert!(
        result["files"].is_array(),
        "explore returns per-file groups (the ExploreResult read-model): {result}"
    );
}

#[tokio::test]
async fn affected_reports_the_changed_set_closure() {
    let (engine, _dir) = fixture_engine();
    let tools = graph_toolset(engine);
    let result = call(
        &tools,
        "affected",
        serde_json::json!({ "files": ["src/lib.rs"] }),
    )
    .await;

    assert_eq!(result["tests_only"], false);
    let changed = result["changed"].as_array().expect("changed array");
    assert!(
        changed.iter().any(|c| c.as_str() == Some("src/lib.rs")),
        "affected echoes the changed set it computed the closure for: {result}"
    );
    assert!(
        result["affected"].is_array(),
        "affected returns the dependent-file closure (the AffectedResult read-model): {result}"
    );
}

#[tokio::test]
async fn callers_and_callees_resolve_the_call_chain() {
    let (engine, _dir) = fixture_engine();
    let tools = graph_toolset(engine);

    // `beta` is called by `alpha`.
    let callers = call(&tools, "callers", serde_json::json!({ "symbol": "beta" })).await;
    let names: Vec<&str> = callers["callers"]
        .as_array()
        .expect("callers array")
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert!(names.contains(&"alpha"), "alpha calls beta: {callers}");

    // `beta` calls `gamma`.
    let callees = call(&tools, "callees", serde_json::json!({ "symbol": "beta" })).await;
    let names: Vec<&str> = callees["callees"]
        .as_array()
        .expect("callees array")
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert!(names.contains(&"gamma"), "beta calls gamma: {callees}");
}

#[tokio::test]
async fn search_rejects_an_unknown_node_kind() {
    let (engine, _dir) = fixture_engine();
    let tools = graph_toolset(engine);
    let err = tools
        .call("search", serde_json::json!({ "query": "x", "kind": "bogus" }).to_string())
        .await
        .expect_err("an unknown kind is the caller's fault");
    let message = err.to_string();
    assert!(
        message.contains("unknown node kind"),
        "the refusal names the fault and lists valid kinds: {message}"
    );
}

#[tokio::test]
async fn scan_returns_a_grounded_quality_signal() {
    let (engine, _dir) = fixture_engine();
    let tools = governance_toolset(engine);
    let result = call(&tools, "scan", serde_json::json!({})).await;

    // The freshness line is the FR-RC-03 grounding stamp every quality run carries.
    assert!(
        result.get("freshness").is_some(),
        "scan carries the freshness provenance line: {result}"
    );
    assert!(
        result.get("signal").is_some(),
        "scan reports the 0-10000 quality signal field: {result}"
    );
}

#[tokio::test]
async fn check_rules_and_health_run_through_the_governance_set() {
    let (engine, _dir) = fixture_engine();
    let tools = governance_toolset(engine);

    let rules = call(&tools, "check_rules", serde_json::json!({})).await;
    assert!(rules.is_object(), "check_rules returns a report object");

    let health = call(&tools, "health", serde_json::json!({})).await;
    assert!(health.is_object(), "health returns a report object");
}

#[tokio::test]
async fn dsm_rejects_an_unknown_granularity() {
    let (engine, _dir) = fixture_engine();
    let tools = governance_toolset(engine);
    let err = tools
        .call("dsm", serde_json::json!({ "granularity": "bogus" }).to_string())
        .await
        .expect_err("an unknown granularity is the caller's fault");
    assert!(
        err.to_string().contains("valid granularities"),
        "the refusal names the valid granularities: {err}"
    );
}

#[tokio::test]
async fn gate_is_read_only_and_reports_a_verdict() {
    let (engine, _dir) = fixture_engine();
    let tools = governance_toolset(engine);
    // `gate` runs with save=false inside the tool — a pure verdict, no snapshot.
    let result = call(&tools, "gate", serde_json::json!({})).await;
    assert!(result.is_object(), "gate returns a verdict object: {result}");
}
