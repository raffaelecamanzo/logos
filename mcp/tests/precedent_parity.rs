//! CLI-vs-MCP payload-equality for the structural precedent query
//! (S-359 / [FR-NV-12], CR-114).
//!
//! Both surfaces delegate to the **one** `Engine::precedent` entrypoint
//! ([NFR-CC-01]) and serialise its read-model — the CLI via `serde_json` in
//! `--json` mode, the MCP twin via `Content::json`. This test proves the wire
//! payloads are byte-identical for the same repository state and the same
//! target, which is the half that could plausibly drift: the symbol-then-file
//! resolution rule lives in `logos_core`, not in either adapter, and this is
//! what holds that seam in place.
//!
//! **What it does NOT cover.** `cli_payload` calls the `Engine` accessor the CLI
//! arm calls; it does not spawn the binary, so argv plumbing — a dropped
//! `--limit`, a mis-declared positional — is invisible here. That half is
//! asserted through the real executable by
//! `cli/tests/cli_surface.rs::precedent_reports_the_sibling_arms_through_the_binary`.
//! The split follows `impact_intersection_parity.rs`, whose `CARGO_BIN_EXE_logos`
//! is not reachable from this crate either.
//!
//! It is the fifth registration point of the pattern S-358 established for the
//! CR-114 query family — navigation-service body, CLI command with `--json`,
//! `/api/v1` route, MCP tool, and this parity guard.
//!
//! [FR-NV-12]: ../../docs/specs/requirements/FR-NV-12.md
//! [NFR-CC-01]: ../../docs/specs/requirements/NFR-CC-01.md

use std::fs;
use std::path::Path;

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

/// Two sibling capability arms behind one trait and one shared pair of helpers
/// — the [FR-NV-12] AC 3 shape, minimal.
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    write(
        tmp.path(),
        "src/plugin.rs",
        "pub trait LanguagePlugin {\n    fn extract(&self);\n}\n",
    );
    write(
        tmp.path(),
        "src/shared.rs",
        "pub fn parse_source() {}\npub fn emit_facts() {}\n",
    );
    for ty in ["RustArm", "PythonArm"] {
        let file = format!("src/{}.rs", ty.to_lowercase());
        write(
            tmp.path(),
            &file,
            &format!(
                "use crate::plugin::LanguagePlugin;\n\
                 use crate::shared::{{parse_source, emit_facts}};\n\n\
                 pub struct {ty};\n\n\
                 impl LanguagePlugin for {ty} {{\n\
                 \x20   fn extract(&self) {{\n\
                 \x20       parse_source();\n\
                 \x20       emit_facts();\n\
                 \x20   }}\n\
                 }}\n\n\
                 pub fn {init}_init() {{}}\n",
                init = ty.to_lowercase()
            ),
        );
    }
    // A dispatcher CALLING each arm's init, so a FILE target yields more than
    // one precedent. Without it every payload here holds exactly one result and
    // an explicit `limit` is a no-op — the parity assertion would pass even if
    // the MCP tool dropped `p.limit` on the floor.
    write(
        tmp.path(),
        "src/registry.rs",
        "use crate::rustarm::rustarm_init;\n\
         use crate::pythonarm::pythonarm_init;\n\n\
         pub fn register_all() {\n\
         \x20   rustarm_init();\n\
         \x20   pythonarm_init();\n\
         }\n",
    );
    Engine::start(tmp.path()).expect("engine starts").index();
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

/// The CLI payload for `logos precedent <target> --json` — exactly what
/// `Output::print` serializes (a fresh engine, dropped before the server boots
/// so the store has one writer at a time).
fn cli_payload(root: &Path, target: &str, limit: Option<usize>) -> Value {
    let engine = Engine::start(root).expect("engine");
    let result = engine.precedent(target, limit);
    serde_json::to_value(&result).expect("serialize")
}

/// The MCP `precedent` tool payload for the same args.
async fn mcp_payload(client: &Client, target: &str, limit: Option<usize>) -> Value {
    let mut args = serde_json::Map::new();
    args.insert("target".into(), Value::from(target));
    if let Some(n) = limit {
        args.insert("limit".into(), Value::from(n));
    }
    let params = CallToolRequestParams::new("precedent").with_arguments(args);
    let result = client.call_tool(params).await.expect("precedent tool call");
    assert_ne!(result.is_error, Some(true), "precedent must succeed");
    let text = result.content.first().unwrap().as_text().unwrap();
    serde_json::from_str(&text.text).expect("valid JSON")
}

#[tokio::test]
async fn cli_and_mcp_precedent_payloads_are_identical() {
    let tmp = fixture();
    let root = tmp.path();
    let (client, server) = boot(root).await;

    // (1) A symbol target with a real sibling, at the default limit. The
    // payloads must be byte-identical AND non-trivial — two empty results
    // comparing equal would pass while proving nothing.
    let target = "logos . . . src/`rustarm.rs`/extract().";
    let cli = cli_payload(root, target, None);
    let mcp = mcp_payload(&client, target, None).await;
    assert_eq!(
        cli, mcp,
        "CLI and MCP payloads must be byte-identical (FR-NV-12)"
    );
    let precedents = mcp["precedents"].as_array().expect("precedents array");
    assert_eq!(
        precedents.len(),
        1,
        "the sibling arm is reported on the wire: {mcp}"
    );
    let reasons = precedents[0]["reasons"]
        .as_array()
        .expect("every result names why it is analogous");
    let facets: Vec<&str> = reasons
        .iter()
        .map(|r| r["facet"].as_str().unwrap())
        .collect();
    assert_eq!(facets, vec!["shared_supertype", "shared_callee"]);
    assert!(
        !mcp["notion"].as_str().unwrap_or_default().is_empty()
            && !mcp["ranked_by"].as_str().unwrap_or_default().is_empty(),
        "the stated notion and ranking rule ride the wire payload: {mcp}"
    );
    assert!(
        !mcp["coverage"]["statement"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "the coverage limits ride the wire payload (NFR-CC-04): {mcp}"
    );

    // (2) A file target — the second half of the resolution rule, which lives in
    // the core precisely so neither adapter can invent a spelling of its own.
    let file = "src/rustarm.rs";
    let unbounded = mcp_payload(&client, file, None).await;
    assert_eq!(unbounded["target_kind"], Value::from("file"));
    let total = unbounded["total_found"].as_u64().expect("total_found");
    assert!(total > 1, "the fixture must offer a choice to cap: {unbounded}");
    assert_eq!(
        cli_payload(root, file, None),
        unbounded,
        "a file target must not diverge between surfaces"
    );

    // (3) An explicit limit travels identically through both surfaces — asserted
    // against that multi-result target, so a dropped `p.limit` on the MCP side
    // would now show up as a longer list rather than comparing equal to itself.
    let capped = mcp_payload(&client, file, Some(1)).await;
    assert_eq!(
        capped["precedents"].as_array().map(Vec::len),
        Some(1),
        "the MCP tool must plumb `limit` through to the engine: {capped}"
    );
    assert_eq!(capped["elided"].as_u64(), Some(total - 1), "{capped}");
    assert_eq!(
        cli_payload(root, file, Some(1)),
        capped,
        "an explicit limit must not diverge between surfaces"
    );

    // (4) The empty answer, whose stated reason must reach BOTH surfaces
    // identically — that reason is the requirement's AC 4, not a nicety.
    let unknown = "no_such_thing_anywhere";
    let cli = cli_payload(root, unknown, None);
    let mcp = mcp_payload(&client, unknown, None).await;
    assert_eq!(
        cli, mcp,
        "the empty-with-a-reason payload must be byte-identical"
    );
    assert_eq!(mcp["empty_reason"]["code"], Value::from("target_unresolved"));

    drop(client);
    let _ = server.await;
}
