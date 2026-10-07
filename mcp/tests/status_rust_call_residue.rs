//! The MCP `status` tool carries the Rust row's `call_residue` (S-589,
//! [CR-188], [FR-RS-42], [FR-RS-09]): the unbound `Calls` rows as its
//! denominator, the count per reason and `unclassified`, with the readout's
//! internals left out. The CLI half is `cli/tests/status_rust_call_residue.rs`.
//!
//! [CR-188]: ../../docs/requests/CR-188-rust-receiver-typing.md
//! [FR-RS-42]: ../../docs/specs/requirements/FR-RS-42.md
//! [FR-RS-09]: ../../docs/specs/requirements/FR-RS-09.md

#![cfg(any(feature = "lang-rust", feature = "lang-all"))]

use std::fs;

use logos_core::Engine;
use mcp::LogosMcp;
use rmcp::{model::CallToolRequestParams, ServiceExt};
use serde_json::{json, Value};
use tempfile::TempDir;

/// `x.run()` binds. `s.len()` is on a std type, `x.absent()` names a method
/// `S` lacks, the chained `.run()` proves no receiver, and the bare `make()`
/// takes no receiver walk at all, so it is unclassified.
const LIB_RS: &str = "\
pub struct S;
impl S { pub fn run(&self) {} }
pub fn external(s: &String) { s.len(); }
pub fn missing(x: &S) { x.absent(); }
pub fn chain(x: &S) { x.run(); make().run(); }
";

#[tokio::test]
async fn the_status_tool_carries_the_rust_rows_call_residue() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("src/lib.rs"), LIB_RS).unwrap();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();

    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        if let Ok(running) = LogosMcp::new(engine).serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_io).await.expect("client initialize");
    let mut answers = Vec::new();
    // Twice over one engine: the second call is answered from the residue
    // memo (S-605, CR-201) and must state what the walk stated.
    for _ in 0..2 {
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
        answers.push(serde_json::from_str::<Value>(&text.text).expect("status content is JSON"));
    }
    drop(client);
    server.abort();
    assert_eq!(
        answers[0]["resolution_by_language"], answers[1]["resolution_by_language"],
        "a memo hit states the walk's figures"
    );
    let status = &answers[1];

    let rust = status["resolution_by_language"]
        .as_array()
        .expect("a row array")
        .iter()
        .find(|row| row["language"] == json!("rust"))
        .expect("a rust row");
    assert_eq!(
        rust["call_residue"],
        json!({
            "unbound": 4,
            "reasons": {
                "external-type": 1,
                "no-applicable-overload": 0,
                "no-receiver-evidence": 1,
                "overload-ambiguous": 0,
                "supertype-unreached": 1,
                "type-ambiguous": 0
            },
            "unclassified": 1,
            "scope": "repository"
        }),
        "{rust}"
    );
    let calls = &rust["calls"];
    assert_eq!(
        calls["references"].as_u64().unwrap() - calls["bound"].as_u64().unwrap(),
        4,
        "the denominator is the row's own unbound count: {calls}"
    );
}
