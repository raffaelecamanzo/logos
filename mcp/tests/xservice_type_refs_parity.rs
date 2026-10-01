//! **S-474 at the MCP boundary: `xservice_type_refs`, and the type-reference
//! stitch in `xservice_callers` / `xservice_impact`, answer what the CLI arms
//! print** ([FR-WS-05], [FR-WS-35]).
//!
//! Each MCP tool, over a live `call_tool` against members really indexed with
//! Java sources and Maven poms, is compared whole-payload against the
//! thick-core read-model assembled exactly as `cli/src/xservice.rs`'s arms
//! assemble it — this crate's established CLI-vs-MCP parity shape
//! (`xservice_build_deps_parity.rs`), since the mcp crate must not depend on
//! the cli crate ([ADR-01]). The CLI arms themselves are driven through the
//! shipped binary in `cli/tests/xservice_type_refs.rs`.
//!
//! **This is router-level parity, and only that.** `logos serve --mcp` hands
//! the MCP loop the single-root engine even inside a workspace (Sprint 80,
//! impl decision 12), so no `xservice_*` tool — this one included — is
//! reachable by an agent host through `serve --mcp` today. The router below is
//! `LogosMcp::federated`, booted in process; nothing here claims the served
//! path.
//!
//! The fixture is not vacuous: `app` imports `lib`'s `Dto` (bound) and
//! `stray` imports it unrelated by the build (type-only), so the listing, the
//! headline and both stitched sections are non-empty on the wire compared.
//!
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [FR-WS-35]: ../../docs/specs/requirements/FR-WS-35.md
//! [ADR-01]: ../../docs/specs/architecture/decisions/ADR-01.md
#![cfg(feature = "lang-all")]

use logos_core::federation::{query, BuildDependencies, ContractBridge, TypeReferences};
use serde_json::{Map, Value};

#[path = "support/federated.rs"]
mod federated;

use federated::{boot, call, index_member, member, registry, write};

const DTO_NODE: &str = "logos . . . src/main/java/com/acme/lib/`Dto.java`/Dto#";

fn pom(artifact: &str, dependencies: &[&str]) -> String {
    let deps: String = dependencies
        .iter()
        .map(|d| format!("<dependency><groupId>com.acme</groupId><artifactId>{d}</artifactId></dependency>"))
        .collect();
    format!(
        "<project><groupId>com.acme</groupId><artifactId>{artifact}</artifactId>\
         <version>1</version><dependencies>{deps}</dependencies></project>"
    )
}

fn args(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs.iter().map(|(k, v)| (k.to_string(), Value::from(*v))).collect()
}

#[tokio::test]
async fn the_mcp_twins_answer_the_cli_read_models_with_the_type_reference_tier() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let files: [(&str, &str, String, &[&str]); 3] = [
        ("lib", "src/main/java/com/acme/lib/Dto.java", "package com.acme.lib;\n\npublic class Dto {}\n".into(), &[]),
        (
            "app",
            "src/main/java/com/acme/app/App.java",
            "package com.acme.app;\n\nimport com.acme.lib.Dto;\n\npublic class App {}\n".into(),
            &["lib"],
        ),
        (
            "stray",
            "src/main/java/com/acme/stray/Stray.java",
            "package com.acme.stray;\n\nimport com.acme.lib.Dto;\n\npublic class Stray {}\n".into(),
            &[],
        ),
    ];
    let mut members = Vec::new();
    for (name, path, java, deps) in &files {
        let dir = root.join(name);
        write(&dir, path, java);
        write(&dir, "pom.xml", &pom(name, deps));
        index_member(&dir);
        members.push(member(name, &dir));
    }

    // The CLI arms' bodies, verbatim minus the print (cli/src/xservice.rs).
    let cli = |tool: &str, repo: Option<&str>| {
        let reg = registry("acme", root, members.clone());
        let index = TypeReferences::new().index(&reg, &BuildDependencies::new());
        let (edges, residue) = query::reachability_inputs(&ContractBridge::new(), &reg);
        match tool {
            "xservice_type_refs" => serde_json::to_value(query::xservice_type_refs(&index, repo)),
            "xservice_callers" => serde_json::to_value(
                query::xservice_callers(&reg, &edges, &residue, DTO_NODE, None, None).with_type_references(&reg, &index),
            ),
            _ => serde_json::to_value(
                query::xservice_impact(&reg, &edges, &residue, DTO_NODE, None, None).with_type_references(&reg, &index),
            ),
        }
        .expect("the read-model serializes")
    };

    let (client, server) = boot(registry("acme", root, members.clone())).await;
    let unscoped = call(&client, "xservice_type_refs", Map::new()).await;
    let scoped = call(&client, "xservice_type_refs", args(&[("repo", "lib")])).await;
    let unknown = call(&client, "xservice_type_refs", args(&[("repo", "nope")])).await;
    let callers = call(&client, "xservice_callers", args(&[("symbol", DTO_NODE)])).await;
    let impact = call(&client, "xservice_impact", args(&[("symbol", DTO_NODE)])).await;

    // Guard the guard: equality over empty answers would prove nothing.
    assert_eq!(unscoped["headline"]["type_reference_pairs"], 1, "{unscoped}");
    assert_eq!(unscoped["providers"][0]["types"][0]["importers"][0]["member"], "app", "{unscoped}");
    assert_eq!(unscoped["headline"]["type_only"][0]["from"], "stray", "{unscoped}");
    assert_eq!(scoped["scope"], "lib");
    assert!(unknown["scope_note"].as_str().is_some_and(|n| n.contains("not in the workspace")), "{unknown}");
    for answer in [&callers, &impact] {
        assert_eq!(answer["via_type_reference"][0]["reached"], "via type reference", "{answer}");
    }

    assert_eq!(cli("xservice_type_refs", None), unscoped, "CLI and MCP print one read-model");
    assert_eq!(cli("xservice_type_refs", Some("lib")), scoped, "both surfaces scope it identically");
    assert_eq!(cli("xservice_type_refs", Some("nope")), unknown, "both state the same scope note");
    assert_eq!(cli("xservice_callers", None), callers, "callers stitches identically on both surfaces");
    assert_eq!(cli("xservice_impact", None), impact, "impact stitches identically on both surfaces");

    client.cancel().await.ok();
    server.abort();
}
