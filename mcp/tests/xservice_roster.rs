//! The `Backing::Single | Federated` roster invariant (S-248, [FR-WS-05],
//! [ADR-52]).
//!
//! The load-bearing guarantee: introducing the federated backing and the
//! `xservice_*` cross-service tools must leave the single-root tool roster
//! **byte-for-byte unchanged** — same 27 tools, same schemas, no `repo`
//! dimension. Federation only ever *adds* tools, and only when a workspace is
//! present. This drives the roster directly through [`LogosMcp::list_tools`]
//! (engine-free: the router is built from the static tool attrs, independent of
//! the backing's engines).

use std::collections::BTreeMap;

use logos_core::federation::{EngineRegistry, Federation, RegistryMode};
use logos_core::Engine;
use mcp::LogosMcp;

/// The single-root server over a throwaway (unindexed) engine — `list_tools`
/// reads the static router, so no index is needed.
fn single_tools() -> Vec<rmcp::model::Tool> {
    let tmp = tempfile::tempdir().expect("tempdir");
    LogosMcp::new(Engine::open(tmp.path())).list_tools()
}

/// The federated server over an (empty, never-warmed) member registry — Lazy
/// mode starts no engine, so no member repo is needed to read the roster.
fn federated_tools() -> Vec<rmcp::model::Tool> {
    let federation = Federation {
        name: "w".to_string(),
        root: "/ws".into(),
        members: Vec::new(),
        default: None,
        links: Vec::new(),
    };
    LogosMcp::federated(EngineRegistry::new(federation, RegistryMode::Lazy)).list_tools()
}

/// Serialise a tool to its full JSON wire form (name + description + schema) —
/// the byte-identity comparison unit.
fn wire(tool: &rmcp::model::Tool) -> serde_json::Value {
    serde_json::to_value(tool).expect("a tool serialises")
}

fn names(tools: &[rmcp::model::Tool]) -> Vec<String> {
    tools.iter().map(|t| t.name.to_string()).collect()
}

/// The single-root roster is exactly today's 27 tools, and none of them carries
/// a `repo` parameter — the single-root wire contract is unchanged (FR-WS-05).
#[test]
fn single_root_roster_is_the_unchanged_27_with_no_repo_dimension() {
    let single = single_tools();
    assert_eq!(
        single.len(),
        27,
        "single-root backing registers exactly the 27 tools (FR-MC-01): {:?}",
        names(&single)
    );

    for tool in &single {
        assert!(
            !tool.name.starts_with("xservice_") && tool.name != "workspace_status",
            "no cross-service tool leaks into the single-root roster: {}",
            tool.name
        );
        let schema = serde_json::to_string(&tool.input_schema).expect("schema serialises");
        assert!(
            !schema.contains("\"repo\""),
            "single-root tool {} must not gain a repo param (byte-identity): {schema}",
            tool.name
        );
    }
}

/// The 27 single-root tools appear **byte-identical** under the federated
/// backing, which adds exactly the 5 cross-service tools on top (FR-WS-05).
#[test]
fn federated_backing_adds_xservice_without_touching_the_single_roster() {
    let single = single_tools();
    let federated = federated_tools();

    let federated_by_name: BTreeMap<String, serde_json::Value> =
        federated.iter().map(|t| (t.name.to_string(), wire(t))).collect();

    // Every single-root tool is present under federation with an identical wire
    // contract — same schema, same description, byte-for-byte.
    for tool in &single {
        let name = tool.name.to_string();
        assert_eq!(
            federated_by_name.get(&name),
            Some(&wire(tool)),
            "shared tool {name} must be byte-identical under both backings",
        );
    }

    assert_eq!(
        federated.len(),
        27 + 5,
        "federated backing is the 27 single tools + the 5 xservice tools: {:?}",
        names(&federated)
    );

    let added: Vec<String> = federated
        .iter()
        .map(|t| t.name.to_string())
        .filter(|name| !single.iter().any(|s| s.name.as_ref() == name))
        .collect();
    // `list_all` sorts by name, so the added set is alphabetical.
    assert_eq!(
        added,
        vec![
            "workspace_status",
            "xservice_callers",
            "xservice_impact",
            "xservice_route_providers",
            "xservice_search",
        ],
        "federation adds exactly the FR-WS-05 cross-service tools",
    );
}
