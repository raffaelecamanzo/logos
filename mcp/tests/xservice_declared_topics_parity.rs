//! **S-490 at the MCP boundary: `xservice_declared_topics`, and the
//! `declared_topics` section of `workspace_status`, answer what the CLI arms
//! print** ([FR-WS-05], [FR-WS-36]).
//!
//! The MCP tool, over a live `call_tool` against members really indexed with
//! Java sources and Spring configuration, is compared whole-payload against the
//! thick-core read-model assembled exactly as `cli/src/xservice.rs`'s
//! `declared-topics` arm assembles it — this crate's established CLI-vs-MCP
//! parity shape (`xservice_build_deps_parity.rs`), since the mcp crate must not
//! depend on the cli crate ([ADR-01]). The CLI arm itself is driven through the
//! shipped binary in `cli/tests/xservice_declared_topics.rs`.
//!
//! **This is router-level parity.** That `logos serve --mcp` hands an agent
//! host this tool is proven separately, over the shipped binary, by
//! `cli/tests/serve_mcp_workspace.rs`: the single-root routing gap (Sprint 80,
//! impl decision 12) was closed by S-710, and nothing in this file claims the
//! served path.
//!
//! The fixture is not vacuous: `orders-api` publishes and declares `orders`
//! (observed), `ledger` declares it with no broker site (net-new) and `tools`
//! declares a value no broker site names (declared-only), so every part of the
//! wire compared is non-empty.
//!
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [FR-WS-36]: ../../docs/specs/requirements/FR-WS-36.md
//! [ADR-01]: ../../docs/specs/architecture/decisions/ADR-01.md
#![cfg(feature = "lang-all")]

use logos_core::federation::{query, xservice_declared_topics, DeclaredTopics};
use serde_json::{Map, Value};

#[path = "support/federated.rs"]
mod federated;

use federated::{boot, call, index_member, member, registry, write};

const PUBLISHER: &str = "\
package com.acme;
class OrderService {
    private KafkaTemplate<String, String> kafkaTemplate;

    public void emitOrder(String payload) {
        kafkaTemplate.send(\"orders\", payload);
    }
}
";

const MAIN: &str = "src/main/resources/application.yml";

#[tokio::test]
async fn the_mcp_twin_answers_the_cli_read_model_unscoped_and_under_repo() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let fixture = [
        ("orders-api", PUBLISHER, "outbound:\n  topic: orders\n"),
        ("ledger", "package com.acme;\nclass Plain {}\n", "ledger:\n  topic: orders\n"),
        ("tools", "package com.acme;\nclass Plain {}\n", "kafka:\n  topics: clickstream\n"),
    ];
    let mut members = Vec::new();
    for (name, java, yaml) in fixture {
        let dir = root.join(name);
        write(&dir, "src/main/java/com/acme/Source.java", java);
        write(&dir, MAIN, yaml);
        index_member(&dir);
        members.push(member(name, &dir));
    }

    // The CLI arm's body, verbatim minus the print (cli/src/xservice.rs).
    let cli = |repo: Option<&str>| {
        let reg = registry("acme", root, members.clone());
        serde_json::to_value(xservice_declared_topics(&DeclaredTopics::new().relation(&reg), repo))
            .expect("the read-model serializes")
    };

    let (client, server) = boot(registry("acme", root, members.clone())).await;
    let unscoped = call(&client, "xservice_declared_topics", Map::new()).await;
    let scoped = call(
        &client,
        "xservice_declared_topics",
        Map::from_iter([("repo".to_string(), Value::from("ledger"))]),
    )
    .await;
    let unknown = call(
        &client,
        "xservice_declared_topics",
        Map::from_iter([("repo".to_string(), Value::from("nope"))]),
    )
    .await;
    let status = call(&client, "workspace_status", Map::new()).await;

    // Guard the guard: equality over two empty answers would prove nothing.
    assert_eq!(unscoped["topics"][0]["topic"], "orders", "{unscoped}");
    assert_eq!(unscoped["topics"][0]["declarations"].as_array().map(Vec::len), Some(2), "{unscoped}");
    assert_eq!(unscoped["declared_only"][0]["member"], "tools", "{unscoped}");
    assert_eq!(unscoped["headline"]["declared_topic_pairs"], 2, "{unscoped}");
    assert_eq!(scoped["scope"], "ledger");
    assert_eq!(scoped["topics"][0]["declarations"][0]["status"], "net-new", "{scoped}");
    assert!(unknown["scope_note"].as_str().is_some_and(|n| n.starts_with("`nope` is not a member")), "{unknown}");

    assert_eq!(cli(None), unscoped, "CLI and MCP print one read-model");
    assert_eq!(cli(Some("ledger")), scoped, "both surfaces scope it identically");
    assert_eq!(cli(Some("nope")), unknown, "both state the same scope note");

    // `workspace_status` carries the section the CLI's does: the headline of
    // the same relation, after `topics`.
    let reg = registry("acme", root, members.clone());
    let expected = serde_json::to_value(&query::workspace_status(&reg).declared_topics).expect("serializes");
    assert_eq!(status["declared_topics"], expected, "one section on both surfaces");
    assert_eq!(status["declared_topics"], unscoped["headline"], "the status section is the answer's headline");

    client.cancel().await.ok();
    server.abort();
}
