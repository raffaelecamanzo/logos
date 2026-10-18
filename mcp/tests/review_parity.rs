//! CLI-vs-MCP parity for the branch delta (S-677, [FR-GV-24]):
//! `logos review --base --json` and the `review` tool.
//!
//! Both surfaces delegate to the one `Engine::review` entrypoint
//! ([NFR-CC-01]). This test proves the two serialize the **same value** for one
//! repository, that the MCP payload is byte-stable when asked twice, and that
//! an unresolvable base answers `measured: false` on the MCP side too.
//!
//! **What it does NOT cover.** `cli_payload` calls the `Engine` accessor the
//! CLI arm calls; it does not spawn the binary — `cli/tests/review_cli.rs`
//! runs the binary itself. The split follows `offenders_parity.rs`.
//!
//! [FR-GV-24]: ../../docs/specs/requirements/FR-GV-24.md
//! [NFR-CC-01]: ../../docs/specs/requirements/NFR-CC-01.md

use std::fs;
use std::path::Path;
use std::process::Command;

use logos_core::Engine;
use mcp::LogosMcp;
use rmcp::{
    model::CallToolRequestParams,
    service::{RoleClient, RunningService},
    ServiceExt,
};
use serde_json::{json, Value};

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// `git -C <root> <args…>` hermetically, asserting success.
fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c", "user.email=dev@logos", "-c", "user.name=Logos Dev",
            "-c", "commit.gpgsign=false", "-c", "core.hooksPath=",
            "-c", "maintenance.auto=false", "-c", "gc.auto=0",
        ])
        .args(args)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// `name` nesting `depth` `if`s.
fn nested(name: &str, depth: usize) -> String {
    let open: String = (0..depth).map(|i| format!("if x > {i} {{\n")).collect();
    format!("pub fn {name}(x: i32) -> i32 {{\n{open}return x;\n{}0\n}}\n", "}\n".repeat(depth))
}

/// `main` with a 4-deep function, scanned; `feature` adds a 6-deep one.
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    git(root, &["init", "-q"]);
    write(root, ".gitignore", ".logos/\n");
    write(root, "src/lib.rs", &nested("alpha", 4));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "base"]);
    git(root, &["branch", "-M", "main"]);
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    engine.scan(true).expect("scan records the merge-base snapshot");
    git(root, &["checkout", "-q", "-b", "feature"]);
    write(root, "src/lib.rs", &format!("{}{}", nested("alpha", 4), nested("beta", 6)));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "deep"]);
    engine.index();
    tmp
}

type Client = RunningService<RoleClient, ()>;

async fn boot(root: &Path) -> (Client, tokio::task::JoinHandle<()>) {
    let engine = Engine::start(root).expect("engine start");
    let (client_io, server_io) = tokio::io::duplex(256 * 1024);
    let server = tokio::spawn(async move {
        if let Ok(running) =
            LogosMcp::new(engine, &logos_ext::ExtensionHost::new()).serve(server_io).await
        {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_io).await.expect("client initialize");
    (client, server)
}

/// What the CLI arm prints for `base`: the engine's review, serialized.
fn cli_payload(root: &Path, base: &str) -> String {
    let engine = Engine::start(root).expect("engine");
    serde_json::to_string(&engine.review(Some(base)).expect("review runs")).unwrap()
}

async fn mcp_call(client: &Client, arguments: Value) -> String {
    let params = CallToolRequestParams::new("review")
        .with_arguments(arguments.as_object().cloned().unwrap_or_default());
    let result = client.call_tool(params).await.expect("tool call");
    assert_ne!(result.is_error, Some(true), "review must succeed: {result:?}");
    result.content.first().unwrap().as_text().unwrap().text.clone()
}

#[tokio::test]
async fn cli_and_mcp_return_the_same_review_byte_stably() {
    let tmp = fixture();
    let root = tmp.path();
    let cli = cli_payload(root, "main");

    let (client, server) = boot(root).await;
    let first = mcp_call(&client, json!({"base": "main"})).await;
    let second = mcp_call(&client, json!({"base": "main"})).await;
    assert_eq!(second, first, "the MCP payload is byte-stable");
    let (mcp, cli): (Value, Value) = (serde_json::from_str(&first).unwrap(), serde_json::from_str(&cli).unwrap());
    assert_eq!(mcp, cli, "the same review on both surfaces");
    assert_eq!(mcp["before_state"], "recorded-snapshot", "{mcp}");
    assert!(
        mcp["offenders"].as_array().unwrap().iter().any(|o| o["name"] == "beta" && o["status"] == "new"),
        "{mcp}"
    );

    let missing: Value = serde_json::from_str(&mcp_call(&client, json!({"base": "no-such-ref"})).await).unwrap();
    assert_eq!(missing["measured"], false, "{missing}");

    client.cancel().await.expect("client cancel");
    server.await.expect("server task");
}
