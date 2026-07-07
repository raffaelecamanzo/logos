//! CLI-vs-MCP payload-equality for the coverage evidence-tier surfaces (S-051,
//! CR-007, [FR-CV-06], [FR-CV-07]).
//!
//! Both surfaces delegate to the **one** `api` entrypoint per command
//! (`Engine::coverage_status` / `Engine::hotspots`, [NFR-CC-01]) and serialise its
//! read-model — the CLI via `serde_json`, the MCP twin via `Content::json`. This
//! proves the wire payloads are byte-identical for the same store state over a
//! real git-history + indexed + coverage-ingested fixture. (The full 25-tool
//! `logos:*` listing is asserted in `protocol.rs`.)

use std::path::Path;
use std::process::Command;

use logos_core::Engine;
use mcp::LogosMcp;
use rmcp::{
    model::CallToolRequestParams,
    service::{RoleClient, RunningService},
    ServiceExt,
};
use serde_json::{Map, Value};

fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(out.status.success(), "git {args:?} failed");
}

fn commit(cwd: &Path, rel: &str, contents: &str, msg: &str) {
    let path = cwd.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    sh_git(cwd, &["add", rel]);
    sh_git(cwd, &["commit", "-q", "-m", msg]);
}

fn branchy(name: &str, ifs: usize) -> String {
    let body: String = (0..ifs)
        .map(|i| format!("    if x == {i} {{ return {i}; }}\n"))
        .collect();
    format!("pub fn {name}(x: i64) -> i64 {{\n{body}    x\n}}\n")
}

/// An indexed git fixture with two churny, complex files — one covered, one not —
/// and a coverage snapshot already ingested.
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    for n in 0..3 {
        commit(
            repo,
            "src/hot.rs",
            &format!("{}// rev {n}\n", branchy("hot", 6)),
            &format!("hot v{n}"),
        );
    }
    for n in 0..2 {
        commit(
            repo,
            "src/tested.rs",
            &format!("{}// rev {n}\n", branchy("tested", 4)),
            &format!("tested v{n}"),
        );
    }

    {
        // Ingest coverage through a transient engine, then drop it so the server
        // owns the store with a single writer at a time.
        let engine = Engine::start(repo).expect("engine starts");
        engine.index();
        let lines: String = (1..=5).map(|n| format!("DA:{n},1\n")).collect();
        let report = repo.join("coverage.info");
        std::fs::write(
            &report,
            format!("TN:suite\nSF:src/tested.rs\n{lines}end_of_record\n"),
        )
        .unwrap();
        engine
            .coverage_ingest(&report, None)
            .expect("coverage ingest");
    }
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

/// The JSON payload an MCP tool returns (the `Content::json` text re-parsed).
async fn mcp_payload(
    client: &Client,
    tool: &'static str,
    args: Option<Map<String, Value>>,
) -> Value {
    let mut params = CallToolRequestParams::new(tool);
    if let Some(map) = args {
        params = params.with_arguments(map);
    }
    let result = client.call_tool(params).await.expect("tool call");
    assert_ne!(result.is_error, Some(true), "{tool} must succeed");
    let text = result.content.first().unwrap().as_text().unwrap();
    serde_json::from_str(&text.text).expect("valid JSON")
}

#[tokio::test]
async fn cli_and_mcp_coverage_status_payloads_are_identical() {
    let tmp = fixture();
    let root = tmp.path();

    let cli = {
        let engine = Engine::start(root).expect("engine");
        serde_json::to_value(engine.coverage_status().expect("coverage status")).unwrap()
    };

    let (client, server) = boot(root).await;
    let mcp = mcp_payload(&client, "coverage_status", None).await;
    assert_eq!(
        cli, mcp,
        "CLI and MCP coverage_status payloads must be byte-identical (FR-CV-06)"
    );
    // Provenance + freshness are present on a populated payload.
    assert!(
        mcp["head_sha"].is_string(),
        "HEAD provenance present: {mcp}"
    );
    assert_eq!(mcp["fresh_files"].as_u64(), Some(1));
    // The overall line-coverage aggregate rides the payload in parity: the lone
    // fresh file (src/tested.rs) has 5/5 lines covered → 100% (FR-CV-06, CR-021).
    assert_eq!(mcp["overall_coverage_bp"].as_i64(), Some(10_000));

    client.cancel().await.ok();
    server.abort();
}

#[tokio::test]
async fn cli_and_mcp_untested_hotspots_payloads_are_identical() {
    let tmp = fixture();
    let root = tmp.path();

    // Warm the lazy mine so neither compared call carries the first-mine notice.
    Engine::start(root)
        .expect("engine")
        .hotspots(None, true)
        .expect("warm-up");

    let cli = {
        let engine = Engine::start(root).expect("engine");
        serde_json::to_value(engine.hotspots(None, true).expect("hotspots")).unwrap()
    };

    let (client, server) = boot(root).await;
    let args = Map::from_iter([("untested".to_string(), Value::Bool(true))]);
    let mcp = mcp_payload(&client, "hotspots", Some(args)).await;
    assert_eq!(
        cli, mcp,
        "CLI and MCP --untested hotspot payloads must be byte-identical (FR-CV-07)"
    );

    // The uncovered hotspot leads; the fresh-covered file is filtered out.
    let paths: Vec<&str> = mcp["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths.first(),
        Some(&"src/hot.rs"),
        "uncovered hotspot leads: {paths:?}"
    );
    assert!(
        !paths.contains(&"src/tested.rs"),
        "fresh-covered file excluded under --untested: {paths:?}"
    );
    assert_eq!(mcp["coverage_basis"], "coverage");

    client.cancel().await.ok();
    server.abort();
}

#[tokio::test]
async fn mcp_coverage_ingest_returns_the_ingest_summary() {
    // A fresh indexed git repo with NO prior ingest, so the MCP `coverage_ingest`
    // tool exercises a SUCCESSFUL write path (protocol.rs only covers its error
    // path) and serialises the `IngestSummary` read-model via the same `api`
    // chokepoint the CLI uses (FR-CV-06, NFR-CC-01).
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    sh_git(root, &["init", "-q", "-b", "main"]);
    commit(root, "src/lib.rs", "pub fn a() -> i64 { 0 }\n", "lib");
    let report = root.join("coverage.info");
    std::fs::write(&report, "TN:suite\nSF:src/lib.rs\nDA:1,1\nend_of_record\n").unwrap();
    Engine::start(root).expect("engine").index();

    let (client, server) = boot(root).await;
    let args = Map::from_iter([(
        "report".to_string(),
        Value::String(report.to_string_lossy().into_owned()),
    )]);
    let mcp = mcp_payload(&client, "coverage_ingest", Some(args)).await;

    // The summary read-model (not an error) came back with the expected shape.
    assert_eq!(
        mcp["format"], "lcov",
        "auto-detected the LCOV format: {mcp}"
    );
    assert_eq!(mcp["matched_files"].as_u64(), Some(1), "src/lib.rs bound");
    assert!(
        mcp["snapshot_id"].as_i64().is_some(),
        "a snapshot was opened"
    );
    assert_eq!(
        mcp["already_ingested"], false,
        "a fresh ingest, not a no-op"
    );
    assert!(mcp["head_sha"].as_str().is_some_and(|s| !s.is_empty()));

    client.cancel().await.ok();
    server.abort();
}
