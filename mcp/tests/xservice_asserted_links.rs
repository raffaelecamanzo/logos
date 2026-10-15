//! Asserted `[[links]]` on the MCP surface (S-709, [FR-WS-41], [BR-75]).
//!
//! Over a real two-member workspace whose manifest declares one `http_call`
//! link — `web`'s runtime-composed call to `api`'s private cart handler — the
//! `xservice_callers` and `xservice_impact` tools carry the asserted edge under
//! `via_asserted_link` with `intake: "asserted"` and `reached: "(asserted)"`,
//! payload-identical to the read-model the CLI prints, and
//! `workspace_reachability` keeps the handler `live-via-asserted-link`.
//!
//! [FR-WS-41]: ../../docs/specs/requirements/FR-WS-41.md
//! [BR-75]: ../../docs/specs/software-spec.md#327-workspace-federation
#![cfg(feature = "lang-all")]

use logos_core::federation::{query, ContractBridge, Link, LinkRelation};
use serde_json::{Map, Value};

#[path = "support/federated.rs"]
mod federated;
use federated::{boot, call, index_member, member, registry_with_links, write};

const WEB_LIB: &str = "use reqwest::Client;\n\npub async fn fetch_cart(client: Client, url: String) {\n    \
                       let _ = client.get(url).await;\n}\n";

const API_LIB: &str = "async fn get_cart() {\n    load_cart();\n}\n\nfn load_cart() {}\n";

fn symbol(name: &str) -> String {
    format!("logos . . . src/`lib.rs`/{name}().")
}

fn link() -> Link {
    Link {
        relation: LinkRelation::HttpCall,
        from: "web::fetch_cart".to_string(),
        to: "api::get_cart".to_string(),
    }
}

#[tokio::test]
async fn the_mcp_tools_carry_an_asserted_link_labelled_as_the_cli_read_model_does() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let (web, api) = (root.join("web"), root.join("api"));
    write(&web, "src/lib.rs", WEB_LIB);
    write(&api, "src/lib.rs", API_LIB);
    index_member(&web);
    index_member(&api);
    let members = vec![member("web", &web), member("api", &api)];

    // The CLI arm's read-models, built as `cli/src/xservice.rs` builds them.
    let cli = {
        let registry = registry_with_links("shop", root, members.clone(), vec![link()]);
        let inputs = query::reachability_inputs(&ContractBridge::new(), &registry);
        [
            serde_json::to_value(query::xservice_callers(&registry, &inputs, &symbol("get_cart"), None, None)).unwrap(),
            serde_json::to_value(query::xservice_impact(&registry, &inputs, &symbol("fetch_cart"), None, None)).unwrap(),
        ]
    };

    let (client, server) = boot(registry_with_links("shop", root, members, vec![link()])).await;
    let args = |name: &str| Map::from_iter([("symbol".to_string(), Value::from(symbol(name)))]);
    let callers = call(&client, "xservice_callers", args("get_cart")).await;
    let impact = call(&client, "xservice_impact", args("fetch_cart")).await;

    let row = &callers["via_asserted_link"][0];
    assert_eq!((row["intake"].as_str(), row["reached"].as_str()), (Some("asserted"), Some("(asserted)")), "{callers}");
    assert_eq!(row["from"]["member"], "web");
    assert_eq!(callers["cross_service"], serde_json::json!([]), "never in the measured tier");
    let far = &impact["via_asserted_link"][0];
    assert_eq!((far["member"].as_str(), far["via"]["intake"].as_str()), (Some("api"), Some("asserted")), "{impact}");

    // The same read-model the CLI prints, member reads aside: the MCP server's
    // registry warmed differently, so which members it read is its own.
    let strip = |mut v: Value| {
        v.as_object_mut().unwrap().shift_remove("member_reads");
        v
    };
    assert_eq!(strip(cli[0].clone()), strip(callers), "callers: one read-model on both surfaces");
    assert_eq!(strip(cli[1].clone()), strip(impact), "impact: one read-model on both surfaces");

    let reach = call(&client, "workspace_reachability", Map::new()).await;
    let asserted: Vec<&str> = reach["live_via_asserted_link"]
        .as_array()
        .expect("the asserted promotions")
        .iter()
        .map(|claim| claim["name"].as_str().unwrap())
        .collect();
    assert_eq!(asserted, ["get_cart", "load_cart"], "{reach}");
    assert_eq!(reach["live_via_cross_service"], serde_json::json!([]));

    client.cancel().await.ok();
    server.abort();
}
