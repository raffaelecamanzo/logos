//! The MCP `status` tool reports the resolution figures a cold reindex reports,
//! over a synced store (S-598, [CR-195], [FR-RS-09], [FR-RS-04], [NFR-RA-06]).
//!
//! A capture-before-delete row ([ADR-10], `RefForm::Symbol`) is written when a
//! file is synced and duplicates a reference its source's own row records, so a
//! synced two-file Go module read calls and imports 1/1 → 2/2. The CLI and HTTP
//! halves of the agreement are `cli/tests/status_capture_rows.rs`; this crate
//! owns the protocol client, so the MCP half is here.
//!
//! The store is synced by one engine and then served by another, so the tool
//! answers over what a restarted server finds on disk.
//!
//! [CR-195]: ../../docs/requests/CR-195-status-resolution-figures-exclude-capture-before-delete-rows.md
//! [FR-RS-09]: ../../docs/specs/requirements/FR-RS-09.md
//! [FR-RS-04]: ../../docs/specs/requirements/FR-RS-04.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
//! [ADR-10]: ../../docs/specs/architecture/decisions/ADR-10.md

#![cfg(feature = "lang-all")]

use std::fs;
use std::path::Path;

use logos_core::model::RefForm;
use logos_core::Engine;
use mcp::LogosMcp;
use rmcp::{model::CallToolRequestParams, ServiceExt};
use serde_json::{json, Value};
use tempfile::TempDir;

const A_FILE: &str = "a/a.go";
const A_GO: &str = "package a\n\nfunc F() {}\n";
const B_GO: &str = "package b\n\nimport \"example.com/m/a\"\n\nfunc G() {\n\ta.F()\n}\n";

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The two-file Go module with `a/a.go` carrying `touch` ahead of its source.
fn module(touch: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "go.mod", "module example.com/m\n\ngo 1.22\n");
    write(tmp.path(), A_FILE, &format!("{touch}{A_GO}"));
    write(tmp.path(), "b/b.go", B_GO);
    tmp
}

/// The resolution figures of a status payload — the fields that must agree
/// between a synced store and a cold reindex.
fn figures(status: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for key in [
        "refs_total",
        "refs_resolved",
        "refs_unresolved",
        "resolution_coverage",
        "resolution_by_language",
    ] {
        out.insert(key.to_string(), status[key].clone());
    }
    Value::Object(out)
}

/// What the `status` tool answers over the project at `root`, served by a fresh
/// engine and an in-process client.
async fn status_tool(root: &Path) -> Value {
    let engine = Engine::start(root).expect("engine starts");
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        if let Ok(running) = LogosMcp::new(engine).serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_io).await.expect("client initialize");
    let result = client
        .call_tool(CallToolRequestParams::new("status"))
        .await
        .expect("status answers");
    assert_ne!(result.is_error, Some(true), "status reported a tool error");
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .expect("status returns JSON text");
    let value = serde_json::from_str(&text.text).expect("status content is JSON");
    drop(client);
    server.abort();
    value
}

#[tokio::test]
async fn a_synced_store_and_a_cold_reindex_report_the_same_figures_through_mcp() {
    let tmp = module("");
    {
        let engine = Engine::start(tmp.path()).expect("engine starts");
        engine.index();
        write(tmp.path(), A_FILE, &format!("// touched\n{A_GO}"));
        engine.sync(&[A_FILE.into()]);
        let captures = engine
            .runtime()
            .unwrap()
            .submit_read(|store| {
                Ok(store
                    .unresolved_refs()?
                    .iter()
                    .filter(|r| r.form == RefForm::Symbol)
                    .count())
            })
            .unwrap();
        assert!(captures > 0, "the sync wrote capture rows, or this test pins nothing");
    }
    let synced = figures(&status_tool(tmp.path()).await);

    let cold = module("// touched\n");
    Engine::start(cold.path()).expect("engine starts").index();
    assert_eq!(synced, figures(&status_tool(cold.path()).await));

    let go = &synced["resolution_by_language"][0];
    assert_eq!(go["language"], json!("go"));
    for class in ["calls", "imports"] {
        assert_eq!(
            (&go[class]["references"], &go[class]["bound"]),
            (&json!(1), &json!(1)),
            "{class}: {go}"
        );
    }
}
