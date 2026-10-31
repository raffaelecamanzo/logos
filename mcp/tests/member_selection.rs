//! A read-only tool answers for the workspace member its `repo` names (S-710,
//! [FR-WS-42], [CR-225]), over the real protocol on a real two-member
//! workspace — plus a third member whose store will not open.
//!
//! - without `repo` the default member answers, and says so (`member`);
//! - with `repo` that member answers — `node` on a symbol only the non-default
//!   member holds is unknown without it and found with it;
//! - an unknown `repo` is refused listing the members, a degraded one with its
//!   reason, and any `repo` under a single-root server with "no workspace";
//! - a read-only extension tool takes `repo` the same way; a write one does not.
//!
//! [FR-WS-42]: ../../docs/specs/requirements/FR-WS-42.md
//! [CR-225]: ../../docs/requests/CR-225-per-member-single-root-tools.md

use std::path::{Path, PathBuf};
use std::sync::Arc;

use logos_core::Engine;
use logos_ext::mcp::McpTool;
use logos_ext::{Extension, ExtensionHost};
use mcp::LogosMcp;
use rmcp::model::{CallToolRequestParams, ErrorCode, Tool};
use rmcp::service::ServiceError;
use rmcp::ServiceExt;
use serde_json::{json, Map, Value};

#[path = "support/federated.rs"]
mod federated;
#[path = "support/roster.rs"]
mod roster;

use federated::{index_member, member, registry, write, Client};

/// A workspace of `svc-a` (the default: first in the manifest), `svc-b`, and
/// `svc-c`, whose store is not a database. Each healthy member holds a symbol
/// the other does not.
fn workspace() -> (tempfile::TempDir, Vec<logos_core::federation::Member>) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write(&root.join("svc-a"), "src/lib.rs", "pub fn alpha_only() -> u32 {\n    1\n}\n");
    write(&root.join("svc-b"), "src/lib.rs", "pub fn beta_only() -> u32 {\n    2\n}\n");
    write(&root.join("svc-c"), ".logos/logos.db", "this is not a SQLite database\n");
    index_member(&root.join("svc-a"));
    index_member(&root.join("svc-b"));
    let members = ["svc-a", "svc-b", "svc-c"].iter().map(|name| member(name, &root.join(name))).collect();
    (tmp, members)
}

/// Serve `server` to an in-process client over a 1 MiB duplex.
async fn connect(server: LogosMcp) -> Client {
    let (client_io, server_io) = tokio::io::duplex(1024 * 1024);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    ().serve(client_io).await.expect("client initialize")
}

async fn federated_client(members: Vec<logos_core::federation::Member>, root: &Path, host: &ExtensionHost) -> Client {
    connect(LogosMcp::federated(registry("w", root, members), host)).await
}

/// Call `tool`, returning its JSON payload, or the protocol error it was refused with.
async fn call(client: &Client, tool: &str, args: Value) -> Result<Value, ServiceError> {
    let Value::Object(args) = args else { panic!("test bug: arguments must be an object") };
    let params = CallToolRequestParams::new(tool.to_string()).with_arguments(args);
    let result = client.call_tool(params).await?;
    assert_ne!(result.is_error, Some(true), "{tool} reported a tool-execution error");
    let text = result.content.first().expect("one content item").as_text().expect("text content");
    Ok(serde_json::from_str(&text.text).expect("JSON content"))
}

/// The protocol error a refused call carries: its code and message.
async fn refusal(client: &Client, tool: &str, args: Value) -> (ErrorCode, String) {
    match call(client, tool, args).await {
        Err(ServiceError::McpError(error)) => (error.code, error.message.to_string()),
        other => panic!("{tool} was expected to be refused, got {other:?}"),
    }
}

/// The minimal arguments each listed read-only tool needs, plus `repo`.
fn listed_calls(repo: &str) -> Vec<(&'static str, Value)> {
    let base: Vec<(&'static str, Value)> = vec![
        ("search", json!({"query": "beta_only"})),
        ("context", json!({"task": "change beta_only"})),
        ("explore", json!({"query": "beta_only"})),
        ("node", json!({"symbol": "beta_only"})),
        ("callers", json!({"symbol": "beta_only"})),
        ("callees", json!({"symbol": "beta_only"})),
        ("impact", json!({"symbol": "beta_only"})),
        ("impact_intersection", json!({"items": ["x=beta_only"]})),
        ("precedent", json!({"target": "src/lib.rs"})),
        ("branch_overlap", json!({"refs": ["HEAD"]})),
        ("status", json!({})),
        ("health", json!({})),
        ("hotspots", json!({})),
        ("dsm", json!({})),
        ("evolution", json!({})),
        ("coverage_status", json!({})),
        ("security_status", json!({})),
        ("security_findings", json!({})),
        ("offenders", json!({})),
        ("review", json!({"base": "HEAD"})),
        ("improvements", json!({})),
    ];
    base.into_iter()
        .map(|(tool, mut args)| {
            args["repo"] = json!(repo);
            (tool, args)
        })
        .collect()
}

/// [CR-225] §6: a symbol only the non-default member holds is unknown without
/// `repo` and found with it — and each answer names the member it came from.
#[tokio::test]
async fn node_is_unknown_on_the_default_member_and_found_with_repo() {
    let (tmp, members) = workspace();
    let client = federated_client(members, tmp.path(), &ExtensionHost::new()).await;

    let unscoped = call(&client, "node", json!({"symbol": "beta_only"})).await.unwrap();
    assert_eq!(unscoped["member"], "svc-a", "the default member answers, and says so: {unscoped}");
    assert!(unscoped["node"].is_null(), "beta_only is not in the default member: {unscoped}");

    let scoped = call(&client, "node", json!({"symbol": "beta_only", "repo": "svc-b"})).await.unwrap();
    assert_eq!(scoped["member"], "svc-b");
    assert_eq!(scoped["node"]["name"], "beta_only", "found in svc-b: {scoped}");

    // The member stamp is first, and the answer's own fields follow unchanged.
    let first_key = scoped.as_object().unwrap().keys().next().cloned();
    assert_eq!(first_key.as_deref(), Some("member"));
    assert_eq!(scoped["query"], "beta_only");
}

/// Every listed read-only tool takes `repo` and answers for that member, naming
/// it — the quality reads included, not only navigation.
#[tokio::test]
async fn every_listed_read_only_tool_answers_for_the_member_repo_names() {
    let (tmp, members) = workspace();
    let client = federated_client(members, tmp.path(), &ExtensionHost::new()).await;
    let calls = listed_calls("svc-b");
    assert_eq!(calls.len(), roster::MEMBER_SELECTABLE_TOOLS.len(), "the test covers every listed tool");
    for (tool, args) in calls {
        assert!(roster::MEMBER_SELECTABLE_TOOLS.contains(&tool), "{tool} is a listed read-only tool");
        let answer = call(&client, tool, args).await.unwrap_or_else(|e| panic!("{tool}: {e}"));
        assert_eq!(answer["member"], "svc-b", "{tool} names the member it answered for: {answer}");
    }
    // `status` proves the routing, not just the label: svc-b's own graph.
    let status = call(&client, "status", json!({"repo": "svc-b"})).await.unwrap();
    let db = status["db_path"].as_str().unwrap_or_default();
    assert!(Path::new(db).starts_with(tmp.path().join("svc-b")), "svc-b's store answered: {db}");
}

/// The three refusals: an unknown member lists the members; a degraded member
/// carries its reason; neither falls back to the default member's answer.
#[tokio::test]
async fn an_unknown_repo_lists_the_members_and_a_degraded_one_gives_its_reason() {
    let (tmp, members) = workspace();
    let client = federated_client(members, tmp.path(), &ExtensionHost::new()).await;

    let (code, message) = refusal(&client, "node", json!({"symbol": "x", "repo": "svc-z"})).await;
    assert_eq!(code, ErrorCode::INVALID_PARAMS, "{message}");
    assert!(message.contains("no workspace member \"svc-z\""), "{message}");
    assert!(message.contains("the members are: svc-a, svc-b, svc-c"), "{message}");

    let (code, message) = refusal(&client, "status", json!({"repo": "svc-c"})).await;
    assert_eq!(code, ErrorCode::INTERNAL_ERROR, "a degraded member is not the caller's mistake: {message}");
    assert!(message.contains("workspace member \"svc-c\" is degraded: "), "{message}");
    assert!(message.contains("starting the engine for workspace member \"svc-c\""), "{message}");
}

/// A `repo` under a single-root server is refused, saying no workspace is
/// loaded; without it the answer carries no `member`.
#[tokio::test]
async fn a_single_root_server_refuses_repo_and_names_no_member() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write(tmp.path(), "src/lib.rs", "pub fn solo() {}\n");
    index_member(tmp.path());
    let client = connect(LogosMcp::new(Arc::new(Engine::start(tmp.path()).unwrap()), &ExtensionHost::new())).await;

    let answer = call(&client, "node", json!({"symbol": "solo"})).await.unwrap();
    assert!(answer.get("member").is_none(), "single-root names no member: {answer}");
    assert_eq!(answer["node"]["name"], "solo");

    let (code, message) = refusal(&client, "node", json!({"symbol": "solo", "repo": "svc-b"})).await;
    assert_eq!(code, ErrorCode::INVALID_PARAMS, "{message}");
    assert!(message.contains("no workspace is loaded"), "{message}");
}

/// A write tool takes no `repo`, acts on the default member and says which.
#[tokio::test]
async fn a_write_tool_acts_on_the_default_member_and_names_it() {
    let (tmp, members) = workspace();
    let client = federated_client(members, tmp.path(), &ExtensionHost::new()).await;
    let doctor = call(&client, "doctor", json!({})).await.unwrap();
    assert_eq!(doctor["member"], "svc-a", "{doctor}");
}

/// A `repo` given to a write or gate tool is refused, never ignored: a
/// `session_start` that named `svc-b` must not record `svc-a`'s baseline.
/// Every single-root tool outside the read-only set is checked, and nothing
/// runs — the refusal comes before the engine is touched.
#[tokio::test]
async fn every_write_and_gate_tool_refuses_repo_rather_than_acting_on_the_default_member() {
    let (tmp, members) = workspace();
    let server = LogosMcp::federated(registry("w", tmp.path(), members), &ExtensionHost::new());
    let write_tools: Vec<String> = server
        .list_tools()
        .into_iter()
        .map(|t| t.name.to_string())
        .filter(|name| {
            [roster::QUALITY_TOOLS, roster::COVERAGE_TOOLS, roster::SECURITY_TOOLS]
                .iter()
                .any(|group| group.contains(&name.as_str()))
        })
        .filter(|name| !roster::MEMBER_SELECTABLE_TOOLS.contains(&name.as_str()))
        .collect();
    assert_eq!(write_tools.len(), roster::SINGLE_ROOT - roster::MEMBER_SELECTABLE_TOOLS.len(), "{write_tools:?}");
    let client = connect(server).await;
    for tool in &write_tools {
        let mut args = json!({"repo": "svc-b"});
        if tool == "coverage_ingest" {
            args["report"] = json!("lcov.info");
        }
        if tool == "security_ingest" {
            args["report"] = json!("report.sarif");
        }
        if tool == "security_reconfirm" {
            args["fingerprint"] = json!("0".repeat(64));
        }
        let (code, message) = refusal(&client, tool, args).await;
        assert_eq!(code, ErrorCode::INVALID_PARAMS, "{tool}: {message}");
        assert!(message.contains("acts on the default member only"), "{tool}: {message}");
    }
    // `null` is "no repo": the tool runs, on the default member.
    let doctor = call(&client, "doctor", json!({"repo": null})).await.unwrap();
    assert_eq!(doctor["member"], "svc-a");
}

/// An extension with one read-only and one write tool, each echoing the root
/// of the engine it was handed — so the route, not a label, is what is tested.
struct Probe;

fn probe_tool(name: &'static str) -> Tool {
    let schema = json!({"type": "object", "properties": {"echo": {"type": "string"}}});
    let Value::Object(schema) = schema else { unreachable!("a JSON object") };
    Tool::new(name, "Echo the answering engine's root (test-only).", Arc::new(schema))
}

fn echo_root(engine: &Engine, arguments: Map<String, Value>) -> anyhow::Result<Value> {
    // A handler written without `repo` must never see it.
    anyhow::ensure!(!arguments.contains_key("repo"), "the adapter leaked repo to the handler");
    Ok(json!({"root": engine.root().display().to_string()}))
}

impl Extension for Probe {
    fn id(&self) -> &'static str {
        "probe"
    }

    fn mcp_tools(&self) -> Vec<McpTool> {
        vec![
            McpTool::read_only(probe_tool("probe_read"), echo_root),
            McpTool::new(probe_tool("probe_write"), echo_root),
        ]
    }
}

/// [FR-WS-42]: every read-only tool an extension registers takes `repo`, through
/// the registration seam; a tool registered without declaring itself read-only
/// does not, and acts on the default member.
#[tokio::test]
async fn a_read_only_extension_tool_takes_repo_and_a_write_one_does_not() {
    let host = ExtensionHost::new().register(Probe).expect("the probe registers");
    let (tmp, members) = workspace();
    let server = LogosMcp::federated(registry("w", tmp.path(), members), &host);
    let schema_of = |name: &str| {
        let tool = server.list_tools().into_iter().find(|t| t.name == name).expect("listed");
        tool.input_schema.get("properties").cloned().unwrap_or_default()
    };
    assert_eq!(
        schema_of("probe_read")["repo"]["description"],
        logos_ext::mcp::REPO_PARAM_DESCRIPTION
    );
    assert_eq!(schema_of("probe_read")["repo"]["type"], json!(["string", "null"]), "repo is optional");
    assert!(schema_of("probe_read").get("echo").is_some(), "the tool's own schema is kept");
    assert!(schema_of("probe_write").get("repo").is_none(), "a write tool takes no repo");
    let description_of = |name: &str| {
        let tool = server.list_tools().into_iter().find(|t| t.name == name).expect("listed");
        tool.description.as_deref().unwrap_or_default().to_string()
    };
    assert!(
        description_of("probe_write").ends_with(logos_ext::mcp::DEFAULT_MEMBER_DESCRIPTION),
        "a write tool says it acts on the default member"
    );
    assert!(!description_of("probe_read").contains("default member only"), "a read-only tool does not");

    let client = connect(server).await;
    let root_of = |answer: &Value| PathBuf::from(answer["root"].as_str().unwrap_or_default());
    let read = call(&client, "probe_read", json!({"repo": "svc-b"})).await.unwrap();
    assert_eq!(read["member"], "svc-b");
    assert_eq!(root_of(&read), tmp.path().join("svc-b"), "svc-b's engine answered: {read}");

    let unscoped = call(&client, "probe_read", json!({})).await.unwrap();
    assert_eq!(root_of(&unscoped), tmp.path().join("svc-a"), "the default member answered");

    let write = call(&client, "probe_write", json!({})).await.unwrap();
    assert_eq!(write["member"], "svc-a", "a write tool acts on the default member: {write}");

    // A write tool never follows a caller's `repo` to another member: it is
    // refused, and a `null` one is absent — the handler never sees either.
    let (code, message) = refusal(&client, "probe_write", json!({"repo": "svc-b"})).await;
    assert_eq!(code, ErrorCode::INVALID_PARAMS, "{message}");
    assert!(message.contains("acts on the default member only"), "{message}");
    let write = call(&client, "probe_write", json!({"repo": null})).await.unwrap();
    assert_eq!(root_of(&write), tmp.path().join("svc-a"), "{write}");
    // A `null` repo on a read-only tool is the default member, as the schema allows.
    let read = call(&client, "probe_read", json!({"repo": null})).await.unwrap();
    assert_eq!(read["member"], "svc-a", "{read}");

    let (code, message) = refusal(&client, "probe_read", json!({"repo": 7})).await;
    assert_eq!(code, ErrorCode::INVALID_PARAMS, "{message}");
    let (code, message) = refusal(&client, "probe_read", json!({"repo": "svc-z"})).await;
    assert_eq!(code, ErrorCode::INVALID_PARAMS, "{message}");
    assert!(message.contains("the members are: svc-a, svc-b, svc-c"), "{message}");
}
