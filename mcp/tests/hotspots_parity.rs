//! CLI-vs-MCP payload-equality for the temporal `hotspots` surface (S-048,
//! [FR-GH-06], [UAT-GH-03]).
//!
//! Both surfaces delegate to the **one** `Engine::hotspots` `api` entrypoint
//! ([NFR-CC-01]) and serialise its read-model — the CLI via `serde_json` in
//! `--json` mode, the MCP twin via `Content::json`. This test proves the wire
//! payloads are byte-identical for the same repository state, over a real
//! git-history + indexed fixture.

use std::path::Path;
use std::process::Command;

use logos_core::Engine;
use mcp::LogosMcp;
use rmcp::{
    model::CallToolRequestParams,
    service::{RoleClient, RunningService},
    ServiceExt,
};
use serde_json::Value;

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

/// An indexed git fixture with a churny, complex file.
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);

    // A branchy function with `ifs` decision points (drives per-function CC).
    let branchy = |name: &str, ifs: usize| -> String {
        let body: String = (0..ifs)
            .map(|i| format!("    if x == {i} {{ return {i}; }}\n"))
            .collect();
        format!("pub fn {name}(x: i64) -> i64 {{\n{body}    x\n}}\n")
    };

    // hot.rs: top on both axes (3 commits, 6 branches) → the engineered #1.
    for n in 0..3 {
        commit(
            repo,
            "src/hot.rs",
            &format!("{}// rev {n}\n", branchy("hot", 6)),
            &format!("hot v{n}"),
        );
    }
    // tie_a.rs / tie_b.rs: IDENTICAL on both axes (2 commits, 3 branches) → a
    // genuine score tie, so the path-ascending tie-break is exercised through
    // the full CLI/MCP serialization round-trip (UAT-GH-03).
    for name in ["tie_b", "tie_a"] {
        for n in 0..2 {
            commit(
                repo,
                &format!("src/{name}.rs"),
                &format!("{}// rev {n}\n", branchy(name, 3)),
                &format!("{name} v{n}"),
            );
        }
    }
    // calm.rs: low on both axes (1 commit, trivial) → ranks last.
    commit(repo, "src/calm.rs", "pub fn calm() -> i64 { 0 }\n", "calm");

    Engine::start(repo).expect("engine starts").index();
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

/// The CLI payload for `logos hotspots [--limit N] --json` — exactly what
/// `Output::print` serializes (a fresh engine, dropped before the server boots
/// so the store has one writer at a time).
fn cli_payload(root: &Path, limit: Option<usize>) -> Value {
    let engine = Engine::start(root).expect("engine");
    let report = engine.hotspots(limit, false).expect("hotspots");
    serde_json::to_value(&report).expect("serialize")
}

/// The MCP `hotspots` tool payload for the same args.
async fn mcp_payload(client: &Client, limit: Option<usize>) -> Value {
    let mut params = CallToolRequestParams::new("hotspots");
    if let Some(n) = limit {
        params = params.with_arguments(serde_json::Map::from_iter([(
            "limit".into(),
            Value::from(n),
        )]));
    }
    let result = client.call_tool(params).await.expect("hotspots tool call");
    assert_ne!(result.is_error, Some(true), "hotspots must succeed");
    let text = result.content.first().unwrap().as_text().unwrap();
    serde_json::from_str(&text.text).expect("valid JSON")
}

#[tokio::test]
async fn cli_and_mcp_hotspot_payloads_are_identical_with_limit_and_tie_break() {
    let tmp = fixture();
    let root = tmp.path();

    // Warm the lazy mine first, so neither compared call carries the one-time
    // first-mine notice (the surfaces still match — this isolates state drift).
    Engine::start(root)
        .expect("engine")
        .hotspots(None, false)
        .expect("warm-up");

    let (client, server) = boot(root).await;

    // (1) Unlimited: full ordering must be byte-identical, and the engineered
    // tie (tie_a / tie_b, equal on both axes) must resolve by path ascending —
    // proving the tie-break survives the CLI/MCP serialization round-trip.
    let cli_full = cli_payload(root, None);
    let mcp_full = mcp_payload(&client, None).await;
    assert_eq!(
        cli_full, mcp_full,
        "CLI and MCP full payloads must be byte-identical (FR-GH-06)"
    );
    let paths: Vec<&str> = mcp_full["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    let a = paths
        .iter()
        .position(|p| *p == "src/tie_a.rs")
        .expect("tie_a");
    let b = paths
        .iter()
        .position(|p| *p == "src/tie_b.rs")
        .expect("tie_b");
    assert!(a < b, "a score tie breaks by path ascending: {paths:?}");
    assert_eq!(paths.first(), Some(&"src/hot.rs"), "engineered #1 leads");

    // (2) Limited: `--limit 2` must truncate the four-file board identically on
    // both surfaces, while `ranked_files` preserves the true total.
    let cli_lim = cli_payload(root, Some(2));
    let mcp_lim = mcp_payload(&client, Some(2)).await;
    assert_eq!(cli_lim, mcp_lim, "limited payloads must be byte-identical");
    assert_eq!(
        mcp_lim["files"].as_array().unwrap().len(),
        2,
        "--limit caps"
    );
    assert_eq!(
        mcp_lim["ranked_files"].as_u64(),
        Some(4),
        "ranked_files preserves the pre-limit total ({mcp_lim})"
    );

    client.cancel().await.ok();
    server.abort();
}
