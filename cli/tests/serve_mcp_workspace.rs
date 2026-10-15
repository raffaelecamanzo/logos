//! The shipped `logos serve --mcp` serves a discovered workspace as one: a
//! read-only tool's `repo` reaches the member it names, and the cross-service
//! tools are listed (S-710, [FR-WS-42], [FR-WS-05]).
//!
//! The in-process MCP suites build the federated server directly, so they would
//! stay green if the serve path handed MCP only the default member's engine —
//! the wiring before S-710. This drives the real binary over stdio on a
//! two-member workspace, so that regression has somewhere to fail.
//!
//! Compiled only with `ui`: the slim build serves MCP through
//! `mcp::serve_stdio`, which serves a single root by design.
//!
//! [FR-WS-42]: ../../docs/specs/requirements/FR-WS-42.md
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md

#![cfg(feature = "ui")]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write fixture");
}

/// A member repository: `git init`, so discovery admits it, then `logos index`.
fn member(ws: &Path, name: &str, source: &str) {
    let root = ws.join(name);
    write(&root, "src/lib.rs", source);
    let git = Command::new("git")
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0", "init", "-q"])
        .arg(&root)
        .env("GIT_CEILING_DIRECTORIES", ws)
        .status()
        .expect("git runs");
    assert!(git.success(), "git init {name}");
    let index = Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(&root)
        .arg("index")
        .env("GIT_CEILING_DIRECTORIES", ws)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("logos index runs");
    assert!(index.success(), "logos index {name}");
}

/// The next response carrying `id`, read off the drained stdout channel with a
/// bound — a silent server fails the test instead of hanging it.
fn response(rx: &Receiver<Value>, id: u64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let value = rx.recv_timeout(left).unwrap_or_else(|_| panic!("no response {id} within 60s"));
        if value["id"].as_u64() == Some(id) {
            return value;
        }
    }
}

/// The JSON payload of a successful `tools/call` response.
fn payload(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool result: {response}"));
    serde_json::from_str(text).expect("JSON content")
}

#[test]
fn serve_mcp_at_a_workspace_root_answers_for_the_member_repo_names() {
    let tmp = TempDir::new().expect("tempdir");
    let ws = tmp.path();
    write(ws, "logos.workspace.toml", "[workspace]\nname = \"w\"\nmembers = [\"svc-a\", \"svc-b\"]\n");
    member(ws, "svc-a", "pub fn alpha_only() -> u32 {\n    1\n}\n");
    member(ws, "svc-b", "pub fn beta_only() -> u32 {\n    2\n}\n");

    let mut child = Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(ws)
        .args(["serve", "--mcp"])
        .env("GIT_CEILING_DIRECTORIES", ws)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn logos serve --mcp");
    let mut stdin = child.stdin.take().expect("stdin");
    // Drain stdout on a thread: a full pipe would block the server and fake a hang.
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
            if tx.send(value).is_err() {
                break;
            }
        }
    });

    let call = |id: u64, name: &str, arguments: Value| {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
    };
    for message in [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "s-710", "version": "0"}}
        }),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        call(3, "node", json!({"symbol": "beta_only"})),
        call(4, "node", json!({"symbol": "beta_only", "repo": "svc-b"})),
    ] {
        writeln!(stdin, "{message}").expect("write request");
    }
    stdin.flush().expect("flush");

    response(&rx, 1);
    let tools = response(&rx, 2);
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(names.contains(&"xservice_search"), "the workspace server lists the cross-service tools: {names:?}");

    let unscoped = payload(&response(&rx, 3));
    assert_eq!(unscoped["member"], "svc-a", "the default member answers, and says so: {unscoped}");
    assert!(unscoped["node"].is_null(), "beta_only is not in the default member: {unscoped}");

    let scoped = payload(&response(&rx, 4));
    assert_eq!(scoped["member"], "svc-b", "{scoped}");
    assert_eq!(scoped["node"]["name"], "beta_only", "found in the member repo names: {scoped}");

    // A host disconnect winds the server down; bounded, so a hang fails here.
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.try_wait().expect("try_wait").is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("logos serve --mcp did not exit after stdin closed");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
