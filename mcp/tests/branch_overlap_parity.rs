//! CLI-vs-MCP payload-equality for the branch/merge symbol-overlap query
//! (S-360 / [FR-NV-13], CR-114).
//!
//! Both surfaces delegate to the **one** `Engine::branch_overlap` entrypoint
//! ([NFR-CC-01]) and serialise its read-model — the CLI via `serde_json` in
//! `--json` mode, the MCP twin via `Content::json`. This test proves the wire
//! payloads are byte-identical for the same repository state **and the same
//! refs**, which is the half that could plausibly drift: refs are handed to git
//! verbatim by the core, not interpreted by either adapter, and this is what
//! holds that seam in place.
//!
//! **What it does NOT cover.** `cli_payload` calls the `Engine` accessor the CLI
//! arm calls; it does not spawn the binary, so argv plumbing — a dropped
//! `--merge`, a mis-declared `--ref` arity — is invisible here. That half is
//! asserted through the real executable by
//! `cli/tests/cli_surface.rs::branch_overlap_reports_contention_and_loss_through_the_binary`.
//! The split follows `impact_intersection_parity.rs`, whose `CARGO_BIN_EXE_logos`
//! is likewise not reachable from this crate.
//!
//! It is the fifth registration point of the pattern S-358 established for the
//! CR-114 query family — navigation-service body, CLI command with `--json`,
//! `/api/v1` route, MCP tool, and this parity guard.
//!
//! [FR-NV-13]: ../../docs/specs/requirements/FR-NV-13.md
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
use serde_json::Value;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Run `git -C <root> <args…>` with a hermetic identity, asserting success.
fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.email=dev@logos",
            "-c",
            "user.name=Logos Dev",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Two branches that both edit the same function, plus a merge result that took
/// only one of them — so the compared payloads carry a real contention AND a
/// real loss, not two empty answers that would agree about nothing.
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let shared = |tail: &str| format!("pub fn shared() {{\n    let _ = 1;{tail}\n}}\n");
    write(root, "src/shared.rs", &shared(""));
    write(root, "src/solo.rs", "pub fn solo() {}\n");
    git(root, &["init", "-q"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "base"]);
    git(root, &["branch", "-M", "main"]);

    git(root, &["checkout", "-q", "-b", "left", "main"]);
    write(root, "src/shared.rs", &shared(" // left"));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "left"]);
    git(root, &["checkout", "-q", "main"]);

    git(root, &["checkout", "-q", "-b", "right", "main"]);
    write(root, "src/shared.rs", &shared(" // right"));
    write(root, "src/solo.rs", "pub fn solo() {\n    let _ = 2;\n}\n");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "right"]);

    // The merge result takes `left` only: `right`'s edit to `solo` is lost.
    git(root, &["checkout", "-q", "-b", "merged", "left"]);
    Engine::start(root).expect("engine starts").index();
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

/// The CLI payload for `logos branch-overlap --ref … --json` — exactly what
/// `Output::print` serializes (a fresh engine, dropped before the server boots
/// so the store has one writer at a time).
fn cli_payload(root: &Path, refs: &[&str], base: Option<&str>, merge: Option<&str>) -> Value {
    let names: Vec<String> = refs.iter().map(|s| (*s).to_string()).collect();
    let engine = Engine::start(root).expect("engine");
    let result = engine.branch_overlap(&names, base, merge);
    serde_json::to_value(&result).expect("serialize")
}

/// The MCP `branch_overlap` tool payload for the same args.
async fn mcp_payload(
    client: &Client,
    refs: &[&str],
    base: Option<&str>,
    merge: Option<&str>,
) -> Value {
    let mut args = serde_json::Map::new();
    args.insert("refs".into(), Value::from(refs.to_vec()));
    if let Some(base) = base {
        args.insert("base".into(), Value::from(base));
    }
    if let Some(merge) = merge {
        args.insert("merge".into(), Value::from(merge));
    }
    let params = CallToolRequestParams::new("branch_overlap").with_arguments(args);
    let result = client
        .call_tool(params)
        .await
        .expect("branch_overlap tool call");
    assert_ne!(result.is_error, Some(true), "branch_overlap must succeed");
    let text = result.content.first().unwrap().as_text().unwrap();
    serde_json::from_str(&text.text).expect("valid JSON")
}

#[tokio::test]
async fn cli_and_mcp_branch_overlap_payloads_are_identical() {
    let tmp = fixture();
    let root = tmp.path();
    let (client, server) = boot(root).await;

    // (1) The contending refs, with no merge stated. Byte-identical payloads,
    // and the contention is actually reported (a test that compared two empty
    // results would pass while proving nothing).
    let refs = ["left", "right"];
    let cli = cli_payload(root, &refs, None, None);
    let mcp = mcp_payload(&client, &refs, None, None).await;
    assert_eq!(
        cli, mcp,
        "CLI and MCP payloads must be byte-identical (FR-NV-13)"
    );
    let names: Vec<&str> = mcp["contended"]
        .as_array()
        .expect("contended")
        .iter()
        .map(|row| row["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"shared"),
        "the compared payload names the contended symbol: {names:?}"
    );
    assert!(
        !mcp["coverage"]["statement"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "the coverage limits ride the wire payload (NFR-CC-04): {mcp}"
    );

    // (2) An explicit base travels identically through both surfaces.
    assert_eq!(
        cli_payload(root, &refs, Some("main"), None),
        mcp_payload(&client, &refs, Some("main"), None).await,
        "an explicit base must not diverge between surfaces"
    );

    // (3) The merge half, and (4) a ref that does not resolve — whose warning
    // must reach BOTH surfaces identically: refs are handed to git by the core,
    // precisely so neither adapter can invent a resolution rule of its own.
    let mixed = ["right", "no-such-ref"];
    let cli = cli_payload(root, &mixed, None, Some("merged"));
    let mcp = mcp_payload(&client, &mixed, None, Some("merged")).await;
    assert_eq!(
        cli, mcp,
        "the merge + unresolved-ref payload must be byte-identical"
    );
    assert!(
        mcp["merge"]["lost_symbols"]
            .as_array()
            .expect("lost_symbols")
            .iter()
            .any(|row| row["name"] == "solo"),
        "`right`'s edit the merge never took is reported: {mcp}"
    );
    assert_eq!(
        mcp["coverage"]["unresolved_refs"],
        serde_json::json!(["no-such-ref"]),
        "an unresolvable ref is a coverage limit on both surfaces: {mcp}"
    );

    drop(client);
    let _ = server.await;
}
