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

/// A second fixture, isolated from [`fixture`] so its extra hottest-of-all
/// whole test file never perturbs the existing ordering assertions: `hot.rs`
/// tops both axes among PRODUCTION files, but `tests.rs` — a whole test file
/// by path (bare-name convention, S-283) — is engineered even hotter, so the
/// optional production-scope filter (CR-076) has something real to drop.
fn fixture_with_test_file() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);

    let branchy = |name: &str, ifs: usize| -> String {
        let body: String = (0..ifs)
            .map(|i| format!("    if x == {i} {{ return {i}; }}\n"))
            .collect();
        format!("pub fn {name}(x: i64) -> i64 {{\n{body}    x\n}}\n")
    };

    // tests.rs: the hottest file overall, but a whole test file by path.
    for n in 0..5 {
        commit(
            repo,
            "src/tests.rs",
            &format!("{}// rev {n}\n", branchy("helper", 9)),
            &format!("tests v{n}"),
        );
    }
    // hot.rs: the hottest PRODUCTION file — leads once tests.rs is filtered out.
    for n in 0..3 {
        commit(
            repo,
            "src/hot.rs",
            &format!("{}// rev {n}\n", branchy("hot", 6)),
            &format!("hot v{n}"),
        );
    }
    // calm.rs: low on both axes.
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

/// The CLI payload for `logos hotspots [--limit N] [--production-scope]
/// --json` — exactly what `Output::print` serializes (a fresh engine, dropped
/// before the server boots so the store has one writer at a time).
fn cli_payload(root: &Path, limit: Option<usize>, production_scope: bool) -> Value {
    let engine = Engine::start(root).expect("engine");
    let report = engine
        .hotspots(limit, false, production_scope)
        .expect("hotspots");
    serde_json::to_value(&report).expect("serialize")
}

/// The MCP `hotspots` tool payload for the same args.
async fn mcp_payload(client: &Client, limit: Option<usize>, production_scope: bool) -> Value {
    let mut params = CallToolRequestParams::new("hotspots");
    let mut args = serde_json::Map::new();
    if let Some(n) = limit {
        args.insert("limit".into(), Value::from(n));
    }
    if production_scope {
        args.insert("production_scope".into(), Value::from(true));
    }
    if !args.is_empty() {
        params = params.with_arguments(args);
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
        .hotspots(None, false, false)
        .expect("warm-up");

    let (client, server) = boot(root).await;

    // (1) Unlimited: full ordering must be byte-identical, and the engineered
    // tie (tie_a / tie_b, equal on both axes) must resolve by path ascending —
    // proving the tie-break survives the CLI/MCP serialization round-trip.
    let cli_full = cli_payload(root, None, false);
    let mcp_full = mcp_payload(&client, None, false).await;
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
    let cli_lim = cli_payload(root, Some(2), false);
    let mcp_lim = mcp_payload(&client, Some(2), false).await;
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

/// CR-076: CLI and MCP agree on the optional production-scope filter — both
/// off (byte-identical to the whole-repo board) and on (the whole test file
/// is excluded on both surfaces identically).
#[tokio::test]
async fn cli_and_mcp_hotspot_payloads_are_identical_with_production_scope() {
    let tmp = fixture_with_test_file();
    let root = tmp.path();

    Engine::start(root)
        .expect("engine")
        .hotspots(None, false, false)
        .expect("warm-up");

    let (client, server) = boot(root).await;

    // Filter off: byte-identical on both surfaces, tests.rs still leads.
    let cli_off = cli_payload(root, None, false);
    let mcp_off = mcp_payload(&client, None, false).await;
    assert_eq!(
        cli_off, mcp_off,
        "production_scope=false payloads must be byte-identical"
    );
    assert_eq!(
        cli_off["files"][0]["path"].as_str(),
        Some("src/tests.rs"),
        "filter off: the whole-repo board is unchanged"
    );
    assert_eq!(cli_off["production_scope"].as_bool(), Some(false));

    // Filter on: byte-identical on both surfaces, tests.rs is gone, hot.rs leads.
    let cli_on = cli_payload(root, None, true);
    let mcp_on = mcp_payload(&client, None, true).await;
    assert_eq!(
        cli_on, mcp_on,
        "production_scope=true payloads must be byte-identical"
    );
    assert_eq!(cli_on["production_scope"].as_bool(), Some(true));
    let paths: Vec<&str> = cli_on["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(
        !paths.contains(&"src/tests.rs"),
        "the whole test file is excluded on both surfaces: {paths:?}"
    );
    assert_eq!(
        paths.first(),
        Some(&"src/hot.rs"),
        "the hottest PRODUCTION file leads the narrowed board: {paths:?}"
    );

    client.cancel().await.ok();
    server.abort();
}
