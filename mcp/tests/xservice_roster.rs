//! The `Backing::Single | Federated` roster invariant (S-248, [FR-WS-05],
//! [ADR-52]).
//!
//! The load-bearing guarantee: introducing the federated backing and the
//! `xservice_*` cross-service tools must leave the single-root tool roster
//! **byte-for-byte unchanged** — same tools, same schemas, no `repo`
//! dimension. Federation only ever *adds* tools, and only when a workspace is
//! present. This drives the roster directly through [`LogosMcp::list_tools`]
//! (engine-free: the router is built from the static tool attrs, independent of
//! the backing's engines).

use std::collections::BTreeMap;

use logos_core::federation::{EngineRegistry, Federation, RegistryMode};
use logos_core::Engine;
use mcp::LogosMcp;

/// The shipped tool roster, shared with `protocol.rs` and `stdout_safety.rs` —
/// every count below derives from it, so a roster change is one edit and cannot
/// leave a guard behind (S-361; see the module's own docs).
#[path = "support/roster.rs"]
mod roster;

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
        governance: Default::default(),
        warm_concurrency: None,
        member_kinds: Default::default(),
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

/// The single-root roster is exactly the set declared in `tests/support/roster.rs`,
/// and none of them carries a `repo` parameter — the single-root wire contract is
/// unchanged (FR-WS-05).
///
/// S-358/CR-114 raised the roster 27→28 for `impact_intersection` ([FR-NV-11]),
/// S-359/CR-114 28→29 for `precedent` ([FR-NV-12]), and S-360/CR-114 29→30 for
/// `branch_overlap` ([FR-NV-13]): each is one more single-root navigation tool,
/// delegating to one `Engine` method like every other. The count is the guard's
/// whole point — a tool added to the federated roster only, or one that silently
/// gained a `repo` dimension, must fail here.
///
/// [FR-NV-11]: ../../docs/specs/requirements/FR-NV-11.md
/// [FR-NV-12]: ../../docs/specs/requirements/FR-NV-12.md
#[test]
fn single_root_roster_is_the_declared_set_with_no_repo_dimension() {
    let single = single_tools();
    assert_eq!(
        single.len(),
        roster::SINGLE_ROOT,
        "single-root backing registers exactly the {} declared tools (FR-MC-01): {:?}",
        roster::SINGLE_ROOT,
        names(&single)
    );

    for tool in &single {
        assert!(
            !tool.name.starts_with("xservice_") && !tool.name.starts_with("workspace_"),
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

/// The single-root tools appear **byte-identical** under the federated
/// backing, which adds exactly the declared cross-service tools on top
/// (FR-WS-05, plus S-257's `workspace_reachability` union view, FR-WS-12,
/// S-258's `workspace_check` governance tool, FR-WS-13, S-464's
/// `xservice_build_deps`, FR-WS-33, and S-474's `xservice_type_refs`, FR-WS-35).
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
        roster::FEDERATED,
        "federated backing is the {} single tools + the {} cross-service tools: {:?}",
        roster::SINGLE_ROOT,
        roster::XSERVICE_TOOLS.len(),
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
        roster::XSERVICE_TOOLS,
        "federation adds exactly the FR-WS-05 query tools + the \
         FR-WS-12 app-wide reachability union view + the FR-WS-13 governance tool",
    );
}

/// The workspace governance tool is federation-only and **advisory**: it never
/// appears under the single-root backing, so a repo with no workspace cannot even
/// reach the workspace rule family — the per-repo gate is untouched by
/// construction ([FR-WS-13], [ADR-56]).
#[test]
fn workspace_check_is_federated_only_and_never_a_gate_tool() {
    let single = single_tools();
    assert!(
        !single.iter().any(|t| t.name == "workspace_check"),
        "workspace_check must not exist without a workspace manifest",
    );

    let federated = federated_tools();
    let check = federated
        .iter()
        .find(|t| t.name == "workspace_check")
        .expect("the federated backing registers workspace_check");
    let description = check.description.as_deref().unwrap_or_default();
    assert!(
        description.contains("ADVISORY") && description.contains("never alters"),
        "the tool description states it cannot move a member's gate: {description}",
    );
}

/// The `xservice_route_providers` description names the [FR-WS-10] relation
/// set, states the `broker-topic` fan-out (never a sole provider), and names
/// the [S-410]-era `intake`/`from_value`/`to_value` fields — the payload the
/// pre-S-410 wording no longer described. It also pins out the retired claim
/// that a `route` consumer endpoint binds exactly one provider, which [S-420]
/// falsified in the same sprint.
///
/// [S-420]: ../../docs/planning/journal.md#s-420-the-bridge-keys-an-http-consumer-on-its-committed-target
/// [FR-WS-10]: ../../docs/specs/requirements/FR-WS-10.md
/// [S-410]: ../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
#[test]
fn xservice_route_providers_description_names_relations_fan_out_and_provenance_fields() {
    let federated = federated_tools();
    let tool = federated
        .iter()
        .find(|t| t.name == "xservice_route_providers")
        .expect("the federated backing registers xservice_route_providers");
    let description = tool.description.as_deref().unwrap_or_default();

    // Backtick-quoted, not a bare substring: "route" alone also matches the
    // pre-S-410 wording ("resolved route bindings", "provider route", "scopes
    // to routes"), so a bare check would not actually prove this relation is
    // named as one of the three.
    for relation in ["`route`", "`grpc-call`", "`broker-topic`"] {
        assert!(
            description.contains(relation),
            "the description names the {relation} relation: {description}",
        );
    }
    assert!(
        description.contains("FANS OUT"),
        "the description states the broker relation fans out rather than binding a sole provider: {description}",
    );
    for field in ["intake", "from_value", "to_value"] {
        assert!(
            description.contains(field),
            "the description names the {field} field: {description}",
        );
    }

    // The `route` arm no longer binds one provider per CONSUMER ENDPOINT, and the
    // description must not say it does. S-420 keys a configuration-composed HTTP
    // target on EVERY committed composition (`bridge::identify`'s `keyed`), and
    // `match_indexed` emits one edge per composition that binds its own sole
    // provider - so one call site can appear several times in `providers`, naming
    // several members. The pre-S-420 clause told a consumer the opposite, which
    // reads as a licence to de-duplicate `providers` on the consumer endpoint and
    // silently drop edges. Asserted as an ABSENCE as well as a presence: the
    // absence is what actually pins the retired claim out of the payload's
    // description, and it survives a rewording of the replacement sentence.
    assert!(
        !description.contains("bind to exactly one provider"),
        "the description no longer claims the route/grpc arms bind one provider per consumer endpoint: {description}",
    );
    assert!(
        description.contains("several `route` edges"),
        "the description states that one call site can carry several route edges, one per committed composition: {description}",
    );
}

/// `xservice_build_deps` is federation-only, takes the `repo` scope, and its
/// description states the relation is a BUILD dependency and NOT a runtime
/// coupling ([BR-58]) — the sentence an agent must not miss, because the rows
/// look exactly like the runtime bindings the sibling tools return. The
/// `workspace_status` description names its `build_dependency` section the
/// same way.
///
/// Near misses pinned out: "build" alone would match "rebuild" or "build
/// failure", and "runtime coupling" alone would match a sentence asserting the
/// opposite, so each check is the whole phrase with its negation.
///
/// [BR-58]: ../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn xservice_build_deps_says_it_is_a_build_dependency_not_a_runtime_coupling() {
    assert!(
        !single_tools().iter().any(|t| t.name == "xservice_build_deps"),
        "xservice_build_deps must not exist without a workspace manifest",
    );
    let federated = federated_tools();
    let tool = federated
        .iter()
        .find(|t| t.name == "xservice_build_deps")
        .expect("the federated backing registers xservice_build_deps");
    let description = tool.description.as_deref().unwrap_or_default();
    assert!(
        description.contains("A BUILD DEPENDENCY, NOT A RUNTIME COUPLING"),
        "the description states the relation is a build dependency, not a runtime coupling: {description}",
    );
    for field in ["`builds_against`", "`built_against_by`", "`kind`", "`scope`", "`artifact`", "`cross_context`"] {
        assert!(description.contains(field), "the description names {field}: {description}");
    }
    let schema = serde_json::to_string(&tool.input_schema).expect("schema serialises");
    assert!(schema.contains("\"repo\""), "the tool takes the repo scope: {schema}");

    let status = federated
        .iter()
        .find(|t| t.name == "workspace_status")
        .expect("the federated backing registers workspace_status");
    let status_description = status.description.as_deref().unwrap_or_default();
    assert!(
        status_description.contains("`build_dependency`")
            && status_description.contains("a BUILD dependency, NOT a runtime coupling"),
        "workspace_status names its build_dependency section as a build dependency: {status_description}",
    );
    // An agent reading an upgraded store must be told it is unread, with a
    // reason, and never a member with no manifests (FR-WS-33, S-462 task 2).
    for clause in ["not yet extracted", "`members.unread_reasons`", "never counted as a member with no manifests"] {
        assert!(
            status_description.contains(clause),
            "workspace_status says an unextracted member is unread with its reason ({clause}): {status_description}",
        );
    }
}

/// S-461 ([BR-57]): the two tools that carry the declared relations name them
/// and say they are DECLARED, NOT OBSERVED — the sentence an agent must not
/// miss, because a declared contract row names a holder and a target exactly as
/// a binding names a consumer and a provider.
///
/// Near misses pinned out: "declared" alone matches the `declared_apart`
/// sentence the status description already carried, and "not observed" alone
/// would match a sentence asserting the opposite, so each check is the whole
/// phrase or the backtick-quoted key.
///
/// [BR-57]: ../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn route_providers_and_status_name_the_declared_relations_as_declared_not_observed() {
    let federated = federated_tools();
    let description = |name: &str| {
        federated
            .iter()
            .find(|t| t.name == name)
            .and_then(|t| t.description.as_deref())
            .unwrap_or_else(|| panic!("the federated backing registers {name}"))
            .to_string()
    };

    let route = description("xservice_route_providers");
    for clause in [
        "`declared_contracts`",
        "DECLARED-CONTRACT relation",
        "`bound_external`",
        "BOTH ARE DECLARED, NOT OBSERVED",
        "`declared_scope_note`",
        "`base`",
    ] {
        assert!(route.contains(clause), "xservice_route_providers names {clause}: {route}");
    }
    assert!(
        route.contains("never inside it") && route.contains("no declared contract or bound external is a binding"),
        "the relations are stated beside the bindings, never as one: {route}",
    );

    let status = description("workspace_status");
    for clause in [
        "`coverage.declared_contracts`",
        "`headline.declared_contract_pairs`",
        "`headline.documents`",
        "`coverage.bound_external`",
        "`headline.no_provider_rows`",
        "BOTH ARE DECLARED, NOT OBSERVED",
    ] {
        assert!(status.contains(clause), "workspace_status names {clause}: {status}");
    }
    assert!(
        status.contains("a bound-external row stays `no-provider-in-workspace`"),
        "the status description says the bound row does not move: {status}",
    );
}

/// `xservice_type_refs` is federation-only, takes the `repo` scope, and its
/// description says the reference is **advisory** and **not a coupling**
/// ([BR-60]) — the two words an agent must not miss, because an importer looks
/// exactly like the cross-service callers its sibling tools return. The
/// `callers`/`impact` descriptions name their `via_type_reference` section as
/// apart from `cross_service` and tagged `via type reference`.
///
/// Near misses pinned out: "a coupling" alone would match a sentence asserting
/// one, so the check is the negated phrase; and "advisory" is checked in the
/// type-refs description itself, not in a sibling's.
///
/// [BR-60]: ../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn xservice_type_refs_says_it_is_advisory_and_not_a_coupling() {
    assert!(
        !single_tools().iter().any(|t| t.name == "xservice_type_refs"),
        "xservice_type_refs must not exist without a workspace manifest",
    );
    let federated = federated_tools();
    let description = |name: &str| {
        federated
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("the federated backing registers {name}"))
            .description
            .as_deref()
            .unwrap_or_default()
            .to_string()
    };
    let text = description("xservice_type_refs");
    for phrase in ["advisory", "not a coupling", "NOT A COUPLING (BR-60)"] {
        assert!(text.contains(phrase), "the description says {phrase:?}: {text}");
    }
    for field in ["`providers`", "`importers`", "`file`", "`line`", "`headline`", "`scope_note`"] {
        assert!(text.contains(field), "the description names {field}: {text}");
    }
    let tool = federated.iter().find(|t| t.name == "xservice_type_refs").unwrap();
    let schema = serde_json::to_string(&tool.input_schema).expect("schema serialises");
    assert!(schema.contains("\"repo\""), "the tool takes the repo scope: {schema}");

    for name in ["xservice_callers", "xservice_impact"] {
        let text = description(name);
        for clause in ["`via_type_reference`", "APART FROM `cross_service`", "via type reference", "NOT a coupling"] {
            assert!(text.contains(clause), "{name} names its type-reference section with {clause:?}: {text}");
        }
    }
}
