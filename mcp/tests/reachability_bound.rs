//! MCP-boundary coverage for the S-294/CR-084 `workspace_reachability` payload
//! bound.
//!
//! The scoped, promotions-only projection itself is unit-tested in
//! `logos_core::federation::reach` and driven end-to-end through the `logos`
//! binary in `cli/tests/xservice_surface.rs`. What neither covers is the MCP
//! adapter's *own* default: `XserviceReachabilityParams.all` is an `Option<bool>`,
//! and the tool maps an omitted value to the promotions-only bound via
//! `p.all.unwrap_or(false)` (`mcp/src/server.rs`). That glue is distinct from the
//! CLI's clap `all: bool` default and is the load-bearing behaviour for the
//! size-limited MCP surface the filter exists for — a regression to
//! `unwrap_or(true)` would return the full ~500 KB per-repo-dead set by default
//! ([CR-084]) while every unit/CLI test still passed. This pins the default at the
//! live tool boundary.
//!
//! Rust-only fixture (private, uncalled functions are per-repo dead) — no bridge
//! edges are needed to exercise the dead-set suppression vs. `--all` distinction.
//!
//! [CR-084]: ../../docs/requests/CR-084-reachability-payload-filter.md

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

/// `core` member: an exported (live) function and a private, uncalled one the
/// annotation pass verdicts per-repo dead.
const CORE_RS: &str = r#"
pub fn used() -> i32 { 1 }

fn orphan() -> i32 { 41 + 1 }
"#;

/// `util` member: the same shape with distinct names, so a `--repo` scope has a
/// second member's dead callable to filter out.
const UTIL_RS: &str = r#"
pub fn helper() -> i32 { 2 }

fn unused() -> i32 { 7 }
"#;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write fixture");
}

/// Index a member into its own `.logos/logos.db`, then drop the engine so the
/// store is closed before the registry re-opens it (mirrors the logos-core
/// `xservice_reachability` integration harness).
fn index_member(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

fn member(name: &str, root: &Path) -> Member {
    Member {
        name: name.to_string(),
        root: root.to_path_buf(),
    }
}

fn registry(root: &Path, members: Vec<Member>) -> EngineRegistry<Engine> {
    let federation = Federation {
        name: "ws".to_string(),
        root: root.to_path_buf(),
        members,
        default: None,
        links: Vec::new(),
        governance: Default::default(),
    };
    EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy)
}

type Client = RunningService<RoleClient, ()>;

/// Boot a federated MCP server over `registry` and an in-process client (mirrors
/// the `hotspots_parity` harness, but on the federated backing so the
/// `xservice_*`/`workspace_*` tools are registered).
async fn boot(registry: EngineRegistry<Engine>) -> (Client, tokio::task::JoinHandle<()>) {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        if let Ok(running) = LogosMcp::federated(registry).serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_io).await.expect("client initialize");
    (client, server)
}

/// Call `workspace_reachability` with the given optional params and parse its JSON.
async fn call(client: &Client, repo: Option<&str>, all: Option<bool>) -> Value {
    let mut params = CallToolRequestParams::new("workspace_reachability");
    let mut args = Map::new();
    if let Some(r) = repo {
        args.insert("repo".into(), Value::from(r));
    }
    if let Some(a) = all {
        args.insert("all".into(), Value::from(a));
    }
    if !args.is_empty() {
        params = params.with_arguments(args);
    }
    let result = client.call_tool(params).await.expect("workspace_reachability call");
    assert_ne!(result.is_error, Some(true), "workspace_reachability must succeed");
    let text = result.content.first().unwrap().as_text().unwrap();
    serde_json::from_str(&text.text).expect("valid JSON")
}

/// S-294/CR-084 at the MCP boundary: an omitted `all` maps to the promotions-only
/// bound (dead set suppressed to `null`, not returned in full); `all: true`
/// returns the full per-repo-dead set; `repo` scopes it — every applied bound
/// stated on the wire ([NFR-CC-04]).
#[tokio::test]
async fn workspace_reachability_mcp_default_is_promotions_only_and_all_returns_full_set() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let core = root.join("core");
    let util = root.join("util");
    write(&core, "src/lib.rs", CORE_RS);
    write(&util, "src/lib.rs", UTIL_RS);
    index_member(&core);
    index_member(&util);

    let (client, server) = boot(registry(root, vec![member("core", &core), member("util", &util)])).await;

    // Default (`all` omitted): the adapter's `unwrap_or(false)` must yield the
    // promotions-only bound — dead suppressed to null, NOT the full set. This is
    // the exact line no unit/CLI test exercises.
    let default = call(&client, None, None).await;
    assert_eq!(
        default["scope"]["promotions_only"], true,
        "an omitted `all` maps to the promotions-only bound at the MCP boundary"
    );
    assert!(default["scope"]["repo"].is_null(), "no member scope by default");
    assert!(
        default["dead"].is_null(),
        "the per-repo-dead set is SUPPRESSED to null by default, not returned in full \
         (the ~500 KB payload CR-084 bounds): {}",
        default["dead"]
    );
    assert!(
        default["live_via_cross_service"].as_array().is_some(),
        "the promotions bucket is always carried: {}",
        default["live_via_cross_service"]
    );

    // `all: true`: the full per-repo-dead set, across BOTH members.
    let all = call(&client, None, Some(true)).await;
    assert_eq!(all["scope"]["promotions_only"], false, "--all states the full-set bound");
    let dead = all["dead"].as_array().expect("--all populates the dead set, never null");
    let names: Vec<&str> = dead.iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert!(
        names.contains(&"orphan") && names.contains(&"unused"),
        "both members' per-repo-dead callables are returned under --all: {names:?}"
    );

    // `repo` scopes the full set to one member — a filter, never a cap.
    let scoped = call(&client, Some("core"), Some(true)).await;
    assert_eq!(scoped["scope"]["repo"], "core", "the member scope is stated");
    let scoped_dead = scoped["dead"].as_array().expect("dead array under --all");
    assert!(
        scoped_dead.iter().all(|c| c["member"] == "core"),
        "the dead set is scoped to core: {scoped_dead:?}"
    );
    assert!(
        !scoped_dead.iter().any(|c| c["name"] == "unused"),
        "util's dead `unused` is filtered out by the scope, not counted: {scoped_dead:?}"
    );

    client.cancel().await.ok();
    server.abort();
}
