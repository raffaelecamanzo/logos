//! The federated MCP-boundary harness — the mcp crate's single source of truth
//! for "boot a real server over a real workspace and call a tool", shared by
//! every test that needs one.
//!
//! # Why shared, and not copied per test
//!
//! This crate already learnt that lesson once, for the tool roster: see
//! `support/roster.rs`, whose own docs record two sessions writing the same
//! hard-coded count and git auto-merging the identical edits without a conflict.
//! The harness below has the same shape and a worse failure mode — it is not one
//! number but six items (`write`, `index_member`, `member`, `registry`, `Client`,
//! `boot`), and a copy that drifts in *how it indexes* or *how large its duplex
//! buffer is* produces a test that passes for reasons the original does not
//! share. `mcp/tests/reachability_bound.rs` had the only copy until S-377 needed
//! a second; extracting it here is the alternative to a twin.
//!
//! Pulled in with `#[path = "support/federated.rs"] mod federated;`, which
//! compiles the module separately into each test binary — there is no shared
//! build unit and no compilation coupling, exactly as `support/roster.rs` is
//! used by three binaries today.
//!
//! Everything here is fixture plumbing: no assertions live in this file, so a
//! test that reads green never does so because of something written here.

#![allow(dead_code)] // each including binary uses a different subset

use std::path::{Path, PathBuf};

use logos_core::federation::{EngineRegistry, Federation, Member, RegistryMode};
use logos_core::Engine;
use mcp::LogosMcp;
use rmcp::{
    model::CallToolRequestParams,
    service::{RoleClient, RunningService},
    ServiceExt,
};
use serde_json::{Map, Value};

/// Write a fixture file, creating its parent directories.
pub fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write fixture");
}

/// Index a member repo into its own `.logos/logos.db`, then drop the engine so
/// the store is closed before the registry re-opens it (mirrors the logos-core
/// `xservice_*` integration harnesses).
pub fn index_member(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

pub fn member(name: &str, root: &Path) -> Member {
    Member {
        name: name.to_string(),
        root: root.to_path_buf(),
    }
}

/// A lazy registry over `members`, rooted at `root`.
///
/// [`RegistryMode::Lazy`] starts no engine, so a caller that only reads the tool
/// roster needs no member repo at all — pass an empty `members`.
pub fn registry(name: &str, root: &Path, members: Vec<Member>) -> EngineRegistry<Engine> {
    let federation = Federation {
        name: name.to_string(),
        root: root.to_path_buf(),
        members,
        default: None,
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
    };
    EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy)
}

pub type Client = RunningService<RoleClient, ()>;

/// Boot a **federated** MCP server over `registry` plus an in-process client, so
/// the `xservice_*` / `workspace_*` tools are registered and reachable over the
/// real protocol rather than through a direct method call.
///
/// The duplex buffer is 1 MiB: a `workspace_status` payload over a real workspace
/// is comfortably larger than the 64 KiB an earlier copy of this harness used,
/// and a short buffer stalls the response instead of failing an assertion.
pub async fn boot(registry: EngineRegistry<Engine>) -> (Client, tokio::task::JoinHandle<()>) {
    let (client_io, server_io) = tokio::io::duplex(1024 * 1024);
    let server = tokio::spawn(async move {
        if let Ok(running) = LogosMcp::federated(registry).serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_io).await.expect("client initialize");
    (client, server)
}

/// Call `tool` with `args` and parse its single text content as JSON, asserting
/// the call did not report an error.
pub async fn call(client: &Client, tool: &'static str, args: Map<String, Value>) -> Value {
    let mut params = CallToolRequestParams::new(tool);
    if !args.is_empty() {
        params = params.with_arguments(args);
    }
    let result = client.call_tool(params).await.unwrap_or_else(|e| panic!("{tool} call: {e}"));
    assert_ne!(result.is_error, Some(true), "{tool} must succeed");
    let text = result.content.first().expect("one content item").as_text().expect("text content");
    serde_json::from_str(&text.text).expect("valid JSON")
}
