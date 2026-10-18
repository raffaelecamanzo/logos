//! CLI-vs-MCP parity for the ratchet's bless command (S-507, [FR-GV-23]):
//! `logos check --bless` and the `bless_rules` tool.
//!
//! Both surfaces delegate to the one `Engine::bless_rules` entrypoint
//! ([NFR-CC-01]). This test proves the two write the **same bytes** to
//! `.logos/rules-baseline.json` and return the same report, so an agent and a
//! human blessing the same tree produce the same reviewable diff.
//!
//! Two fields are excluded from the payload comparison because they describe
//! the run, not the verdict: `ran_at` (the run's timestamp) and `freshness`
//! (what this run's reconcile touched — the first run indexes, the second does
//! not).
//!
//! **What it does NOT cover.** `cli_payload` calls the `Engine` accessor the
//! CLI arm calls; it does not spawn the binary. The flag-to-method plumbing is
//! the `Commands::Check { bless, .. }` arm in `cli/src/dispatch.rs`, and the
//! roster contract that `bless_rules` is `logos check --bless` is
//! `cli/src/lib.rs::surface_parity`. The split follows
//! `branch_overlap_parity.rs`.
//!
//! [FR-GV-23]: ../../docs/specs/requirements/FR-GV-23.md
//! [NFR-CC-01]: ../../docs/specs/requirements/NFR-CC-01.md

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::Engine;
use mcp::LogosMcp;
use rmcp::{
    model::CallToolRequestParams,
    service::{RoleClient, RunningService},
    ServiceExt,
};
use serde_json::Value;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Two functions over `max_cc = 2` under ratchet, so the blessed set carries
/// real entries — two empty files would agree about nothing.
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write(
        root,
        ".logos/rules.toml",
        "[constraints]\nmax_cc = { max = 2, ratchet = true }\n",
    );
    let branchy = |name: &str, branches: usize| {
        let ifs: String = (0..branches)
            .map(|i| format!("    if x > {i} {{\n        y += 1;\n    }}\n"))
            .collect();
        format!("pub fn {name}(x: i32) -> i32 {{\n    let mut y = 0;\n{ifs}    y\n}}\n")
    };
    write(root, "src/lib.rs", &format!("{}{}", branchy("f", 3), branchy("g", 4)));
    Engine::start(root).expect("engine starts").index();
    tmp
}

fn blessed_file(root: &Path) -> PathBuf {
    root.join(".logos").join("rules-baseline.json")
}

/// The verdict half of a report: everything but the run's own timestamp and
/// freshness line.
fn verdict(mut report: Value) -> Value {
    let map = report.as_object_mut().expect("a report object");
    map.remove("ran_at");
    map.remove("freshness");
    report
}

type Client = RunningService<RoleClient, ()>;

async fn boot(root: &Path) -> (Client, tokio::task::JoinHandle<()>) {
    let engine = Engine::start(root).expect("engine start");
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
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

#[tokio::test]
async fn cli_bless_and_mcp_bless_rules_write_the_same_bytes_and_report_the_same_verdict() {
    let tmp = fixture();
    let root = tmp.path();

    // The CLI arm: `logos check --bless --json` serialises this report.
    let cli = {
        let engine = Engine::start(root).expect("engine");
        let report = engine.bless_rules(None, true).expect("bless runs");
        serde_json::to_value(&report).expect("serialize")
    };
    let cli_bytes = fs::read(blessed_file(root)).expect("the CLI bless wrote the file");
    fs::remove_file(blessed_file(root)).unwrap();

    let (client, server) = boot(root).await;
    let params = CallToolRequestParams::new("bless_rules").with_arguments(serde_json::Map::new());
    let result = client.call_tool(params).await.expect("bless_rules tool call");
    assert_ne!(result.is_error, Some(true), "bless_rules must succeed");
    let text = result.content.first().unwrap().as_text().unwrap();
    let mcp: Value = serde_json::from_str(&text.text).expect("valid JSON");
    let mcp_bytes = fs::read(blessed_file(root)).expect("the MCP bless wrote the file");

    assert_eq!(mcp_bytes, cli_bytes, "both surfaces bless byte-identically");
    assert_eq!(verdict(mcp.clone()), verdict(cli), "same report from both surfaces");
    assert_eq!(mcp["ratchet"]["written"], 2, "the compared payload carries real entries: {mcp}");
    assert_eq!(mcp["passed"], true, "blessed debt passes: {mcp}");

    client.cancel().await.expect("client cancel");
    server.await.expect("server task");
}
