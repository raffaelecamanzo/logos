//! **S-464 at the MCP boundary: `xservice_build_deps` answers what the CLI arm
//! prints** ([FR-WS-05], [FR-WS-33]).
//!
//! The MCP tool, over a live `call_tool` against members really indexed with
//! Maven poms, is compared whole-payload against the thick-core read-model
//! assembled exactly as `cli/src/xservice.rs`'s `build-deps` arm assembles it —
//! this crate's established CLI-vs-MCP parity shape (`coverage_parity.rs`,
//! `xservice_residue_parity.rs`), since the mcp crate must not depend on the cli
//! crate ([ADR-01]). The CLI adapter's own arm is driven through the shipped
//! binary in `cli/tests/xservice_build_deps.rs`.
//!
//! The fixture is not vacuous: `adapter` builds against a parent and two
//! contexts' model libraries, so the rows, a declared scope and the
//! cross-context hint are all present on the wire being compared.
//!
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [FR-WS-33]: ../../docs/specs/requirements/FR-WS-33.md
//! [ADR-01]: ../../docs/specs/architecture/decisions/ADR-01.md

use logos_core::federation::{xservice_build_deps, BuildDependencies};
use serde_json::{Map, Value};

#[path = "support/federated.rs"]
mod federated;

use federated::{boot, call, index_member, member, registry, write};

const G: &str = "com.acme";

/// A pom producing `group:artifact`, with `(group, artifact, scope)` dependencies.
fn pom(group: &str, artifact: &str, parent: bool, deps: &[(&str, &str, Option<&str>)]) -> String {
    let parent = if parent {
        format!("<parent><groupId>{G}</groupId><artifactId>starter</artifactId><version>1</version></parent>")
    } else {
        String::new()
    };
    let deps: String = deps
        .iter()
        .map(|(g, a, s)| {
            let scope = s.map_or(String::new(), |s| format!("<scope>{s}</scope>"));
            format!("<dependency><groupId>{g}</groupId><artifactId>{a}</artifactId>{scope}</dependency>")
        })
        .collect();
    format!(
        "<project>{parent}<groupId>{group}</groupId><artifactId>{artifact}</artifactId>\
         <version>1</version><dependencies>{deps}</dependencies></project>"
    )
}

#[tokio::test]
async fn the_mcp_twin_answers_the_cli_read_model_unscoped_and_under_repo() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let archive = format!("{G}.archive");
    let mailbox = format!("{G}.mailbox");
    let poms = [
        ("starter", pom(G, "starter", false, &[])),
        ("archive-kafka-models", pom(&archive, "kafka-models", true, &[])),
        ("mailbox-kafka-models", pom(&mailbox, "kafka-models", true, &[])),
        (
            "adapter",
            pom(
                G,
                "adapter",
                true,
                &[(&archive, "kafka-models", None), (&mailbox, "kafka-models", Some("test"))],
            ),
        ),
    ];
    let mut members = Vec::new();
    for (name, xml) in &poms {
        let dir = root.join(name);
        write(&dir, "pom.xml", xml);
        write(&dir, "src/lib.rs", "pub fn f() {}\n");
        index_member(&dir);
        members.push(member(name, &dir));
    }

    // The CLI arm's body, verbatim minus the print (cli/src/xservice.rs).
    let cli = |repo: Option<&str>| {
        let reg = registry("acme", root, members.clone());
        serde_json::to_value(xservice_build_deps(&BuildDependencies::new().relation(&reg), repo))
            .expect("the read-model serializes")
    };

    let (client, server) = boot(registry("acme", root, members.clone())).await;
    let unscoped = call(&client, "xservice_build_deps", Map::new()).await;
    let scoped = call(
        &client,
        "xservice_build_deps",
        Map::from_iter([("repo".to_string(), Value::from("archive-kafka-models"))]),
    )
    .await;

    // Guard the guard: equality over two empty answers would prove nothing.
    assert_eq!(unscoped["members"].as_array().map(Vec::len), Some(4), "{unscoped}");
    assert_eq!(unscoped["cross_context"][0]["member"], "adapter", "{unscoped}");
    assert_eq!(unscoped["headline"]["build_dependency_pairs"]["pairs"], 5, "{unscoped}");
    assert_eq!(scoped["scope"], "archive-kafka-models");
    assert_eq!(scoped["members"].as_array().map(Vec::len), Some(1));
    assert_eq!(scoped["members"][0]["built_against_by"][0]["from"], "adapter");

    assert_eq!(cli(None), unscoped, "CLI and MCP print one read-model");
    assert_eq!(cli(Some("archive-kafka-models")), scoped, "both surfaces scope it identically");

    client.cancel().await.ok();
    server.abort();
}
