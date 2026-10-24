//! CLI-vs-MCP parity for the branch delta (S-677, [FR-GV-24]):
//! `logos review --base --json` and the `review` tool.
//!
//! Both surfaces delegate to the one `Engine::review` entrypoint
//! ([NFR-CC-01]). This test proves the two serialize the **same value** for one
//! repository, that the MCP payload is byte-stable when asked twice, and that
//! an unresolvable base answers `measured: false` on the MCP side too. With
//! `branches` (S-757, [FR-GV-28]) the tool answers what `review --branches`
//! does, through `Engine::review_branches`.
//!
//! **What it does NOT cover.** `cli_payload` calls the `Engine` accessor the
//! CLI arm calls; it does not spawn the binary — `cli/tests/review_cli.rs`
//! runs the binary itself. The split follows `offenders_parity.rs`.
//!
//! [FR-GV-24]: ../../docs/specs/requirements/FR-GV-24.md
//! [NFR-CC-01]: ../../docs/specs/requirements/NFR-CC-01.md
//! [FR-GV-28]: ../../docs/specs/requirements/FR-GV-28.md

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

/// `main`'s `engine` module at `max_fan_out − 1`; `b1` and `b2` each add one
/// outgoing module edge to it.
fn branches_fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let engine_calling = |modules: &[&str]| -> String {
        let calls: String = modules.iter().map(|m| format!("    crate::{m}::work_{m}();\n")).collect();
        format!("pub fn run() {{\n{calls}}}\n")
    };
    git(root, &["init", "-q"]);
    write(root, ".gitignore", ".logos/\n");
    write(root, "src/lib.rs", "pub mod engine;\npub mod a;\npub mod b;\npub mod c;\npub mod d;\n");
    for m in ["a", "b", "c", "d"] {
        write(root, &format!("src/{m}.rs"), &format!("pub fn work_{m}() {{}}\n"));
    }
    write(root, "src/engine.rs", &engine_calling(&["a", "b"]));
    write(root, ".logos/rules.toml", "[constraints]\nmax_fan_out = 3\n");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "base"]);
    git(root, &["branch", "-M", "main"]);
    for (branch, extra) in [("b1", "c"), ("b2", "d")] {
        git(root, &["checkout", "-q", "-b", branch, "main"]);
        write(root, "src/engine.rs", &engine_calling(&["a", "b", extra]));
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", "one more edge"]);
    }
    git(root, &["checkout", "-q", "main"]);
    tmp
}

#[tokio::test]
async fn cli_and_mcp_return_the_same_branch_projection() {
    let tmp = branches_fixture();
    let root = tmp.path();
    let branches = ["b1".to_string(), "b2".to_string()];
    let cli = {
        let engine = Engine::start(root).expect("engine");
        serde_json::to_string(&engine.review_branches(Some("main"), &branches).expect("projection runs")).unwrap()
    };

    let (client, server) = boot(root).await;
    let arguments = json!({"base": "main", "branches": ["b1", "b2"]});
    let first = mcp_call(&client, arguments.clone()).await;
    assert_eq!(mcp_call(&client, arguments).await, first, "the MCP payload is byte-stable");
    assert_eq!(first, cli, "the same projection on both surfaces");
    let mcp: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(mcp["exceeded"][0]["rule"], "max_fan_out", "{mcp}");
    assert_eq!(mcp["exceeded"][0]["projected"], 4, "{mcp}");

    let empty: Value = serde_json::from_str(&mcp_call(&client, json!({"base": "main", "branches": []})).await).unwrap();
    assert!(empty.get("exceeded").is_none() && empty.get("offenders").is_some(), "no branches is the branch delta: {empty}");

    client.cancel().await.expect("client cancel");
    server.await.expect("server task");
}

/// `review` with `headroom: true` ([FR-GV-29], S-798) answers what
/// `logos review --headroom --json` does, through `Engine::review_headroom`,
/// and refuses the parameters that only mean something beside it.
///
/// [FR-GV-29]: ../../docs/specs/requirements/FR-GV-29.md
#[tokio::test]
async fn cli_and_mcp_return_the_same_headroom() {
    let tmp = branches_fixture();
    let root = tmp.path();
    let subjects = ["src/engine.rs".to_string(), "src/missing.rs".to_string()];
    let cli = {
        let engine = Engine::start(root).expect("engine");
        serde_json::to_string(&engine.review_headroom(Some("main"), &subjects, None).expect("headroom runs")).unwrap()
    };

    let (client, server) = boot(root).await;
    let arguments = json!({"base": "main", "headroom": true, "subjects": ["src/engine.rs", "src/missing.rs"]});
    let first = mcp_call(&client, arguments.clone()).await;
    assert_eq!(mcp_call(&client, arguments).await, first, "the MCP payload is byte-stable");
    assert_eq!(first, cli, "the same bytes on both surfaces");
    let mcp: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(mcp["measured"], true, "{mcp}");
    assert_eq!(mcp["rules"][0]["rule"], "max_fan_out", "{mcp}");
    assert_eq!(mcp["rules"][0]["subjects"][0]["headroom"], 1, "{mcp}");
    assert_eq!(mcp["unresolved"][0]["entry"], "src/missing.rs", "{mcp}");

    let near = json!({"base": "main", "headroom": true, "near": 0});
    let cli_near = {
        let engine = Engine::start(root).expect("engine");
        serde_json::to_string(&engine.review_headroom(Some("main"), &[], Some(0)).unwrap()).unwrap()
    };
    assert_eq!(mcp_call(&client, near).await, cli_near, "`near` reaches the engine");

    let missing: Value =
        serde_json::from_str(&mcp_call(&client, json!({"base": "no-such-ref", "headroom": true})).await).unwrap();
    assert_eq!(missing["measured"], false, "{missing}");

    for refused in [
        json!({"base": "main", "headroom": true, "branches": ["b1"]}),
        json!({"base": "main", "subjects": ["src/engine.rs"]}),
        json!({"base": "main", "near": 1}),
    ] {
        let params = CallToolRequestParams::new("review").with_arguments(refused.as_object().cloned().unwrap());
        assert!(client.call_tool(params).await.is_err(), "{refused} is refused as invalid parameters");
    }
    let off = json!({"base": "main", "headroom": false});
    let delta: Value = serde_json::from_str(&mcp_call(&client, off).await).unwrap();
    assert!(delta.get("rules").is_none() && delta.get("offenders").is_some(), "headroom: false is the delta: {delta}");

    client.cancel().await.expect("client cancel");
    server.await.expect("server task");
}
