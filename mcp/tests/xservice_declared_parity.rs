//! **S-461 at the MCP boundary: `xservice_route_providers` carries the declared
//! relations the CLI arm prints** ([FR-WS-05], [FR-WS-31], [BR-57]).
//!
//! The MCP tool, over a live `call_tool` against members really indexed, is
//! compared whole-payload against the thick-core read-model assembled exactly as
//! `cli/src/xservice.rs`'s `route-providers` arm assembles it — this crate's
//! established CLI-vs-MCP parity shape (`xservice_build_deps_parity.rs`), since
//! the mcp crate must not depend on the cli crate ([ADR-01]). The CLI arm is
//! driven through the shipped binary in `cli/tests/xservice_declared.rs`.
//!
//! The fixture is not vacuous: `facade` vendors the PSS spec and its one
//! configuration-composed call binds it under the `/prov` its deploy overlay
//! commits, and `webmail` holds a copy of `mbx`'s own spec, so both relation
//! classes and a bound row are on the wire being compared.
//!
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [FR-WS-31]: ../../docs/specs/requirements/FR-WS-31.md
//! [BR-57]: ../../docs/specs/software-spec.md#327-workspace-federation
//! [ADR-01]: ../../docs/specs/architecture/decisions/ADR-01.md

use logos_core::federation::{cross_service_coverage, query, ContractBridge};
use serde_json::{Map, Value};

#[path = "support/federated.rs"]
mod federated;

use federated::{boot, call, index_member, member, registry, write};

const PSS_YAML: &str = "openapi: 3.0.3\ninfo:\n  title: PSS\n  version: 1.0.0\npaths:\n  \
/prov/domain/{d}/user/{u}:\n    get:\n      summary: user\n";

const MAILBOX_YAML: &str = "openapi: 3.0.3\ninfo:\n  title: Mailbox API\n  version: 1.0.0\npaths:\n  \
/folders/{id}:\n    get:\n      summary: folder\n";

const MBX_MAIN: &str = "use axum::routing::get;\nuse axum::Router;\n\nasync fn get_folder() {}\n\n\
fn app() -> Router {\n    Router::new().route(\"/folders/{id}\", get(get_folder))\n}\n";

const FACADE_CLIENT: &str = "use reqwest::Client;\n\npub async fn fetch_mailbox(client: Client) {\n    \
let _ = client.get(\"${pec-server.uri-get-mailbox}\").await;\n}\n";

#[tokio::test]
async fn the_mcp_twin_carries_the_declared_relations_the_cli_prints() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let files: [(&str, &[(&str, &str)]); 3] = [
        (
            "facade",
            &[
                ("api/pss.yaml", PSS_YAML),
                ("src/client.rs", FACADE_CLIENT),
                (
                    "src/main/resources/application.yml",
                    "pec-server:\n  uri-get-mailbox: /domain/{domain}/user/{user}\n  base-url: http://h\n",
                ),
                ("deploy-coll/values.yaml", "envFrom:\n  PECSERVER_BASEURL: 'https://pss.example/prov'\n"),
            ],
        ),
        ("mbx", &[("api/openapi.yaml", MAILBOX_YAML), ("src/main.rs", MBX_MAIN)]),
        ("webmail", &[("specs/mailbox.yaml", MAILBOX_YAML), ("src/lib.rs", "pub fn ui() {}\n")]),
    ];
    let mut members = Vec::new();
    for (name, contents) in files {
        let dir = root.join(name);
        for (rel, text) in contents {
            write(&dir, rel, text);
        }
        index_member(&dir);
        members.push(member(name, &dir));
    }

    // The CLI arm's body, verbatim minus the print (cli/src/xservice.rs).
    let cli = |repo: Option<&str>| {
        let reg = registry("pec", root, members.clone());
        let read = query::bridge_read(&ContractBridge::new(), &reg);
        let coverage = cross_service_coverage(&reg.answer());
        serde_json::to_value(query::xservice_route_providers(&read, repo).with_declared(coverage))
            .expect("the read-model serializes")
    };

    let (client, server) = boot(registry("pec", root, members.clone())).await;
    let unscoped = call(&client, "xservice_route_providers", Map::new()).await;
    let scoped = call(
        &client,
        "xservice_route_providers",
        Map::from_iter([("repo".to_string(), Value::from("mbx"))]),
    )
    .await;

    // Guard the guard: equality over two answers without the relations would
    // prove nothing about them.
    let headline = &unscoped["declared_contracts"]["headline"];
    assert_eq!((&headline["to_member"], &headline["to_external"]), (&1.into(), &1.into()), "{unscoped}");
    assert_eq!(unscoped["bound_external"]["rows"][0]["state"], "bound-external", "{unscoped}");
    assert_eq!(unscoped["bound_external"]["rows"][0]["base"]["path"], "/prov");
    assert_eq!(scoped["scope"], "mbx");
    assert!(scoped["declared_scope_note"].as_str().is_some_and(|n| n.contains("workspace-wide")));
    // The members the bridge read ride the MCP payload (S-484).
    assert!(
        unscoped["member_reads"]["read"].as_array().is_some_and(|read| read.len() == members.len()),
        "a cold first answer reads every member: {unscoped}"
    );

    assert_eq!(cli(None), unscoped, "CLI and MCP print one read-model");
    assert_eq!(cli(Some("mbx")), scoped, "both surfaces scope it identically");

    client.cancel().await.ok();
    server.abort();
}
