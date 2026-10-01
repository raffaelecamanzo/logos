//! **S-480: the agent tools address a named member, and the workspace
//! read-models become tools** ([FR-WS-34], [NFR-SE-04], [NFR-PE-10]).
//!
//! Over a real two-member federation (`api`, `web`, each indexed into its own
//! store), this suite pins the four things the workspace roster will rely on:
//!
//! 1. the member toolsets are byte for byte what they were — against a golden
//!    taken from the code before S-480 touched it — and every repo-addressed
//!    definition is its member's definition plus a required `repo`, nothing else;
//! 2. building the addressed and workspace toolsets starts no engine, one call
//!    starts only the member it names, and an unknown `repo` is an error listing
//!    the members that starts nothing;
//! 3. a source call is confined to the addressed member's own sandbox — its root,
//!    its `ignored_dirs`, its `[chat] read_roots` — so a sibling member or a file
//!    of the workspace root is a containment refusal;
//! 4. each workspace read-model tool answers its twin's payload, compared whole
//!    against the MCP tool called over the real protocol (and, for the roster,
//!    which has no MCP tool, against the read-model its HTTP route serves).
//!
//! [FR-WS-34]: ../../docs/specs/requirements/FR-WS-34.md
//! [NFR-SE-04]: ../../docs/specs/requirements/NFR-SE-04.md
//! [NFR-PE-10]: ../../docs/specs/requirements/NFR-PE-10.md

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::rig::completion::ToolDefinition;
use agent_core::rig::tool::ToolSet;
use agent_core::{
    addressed_toolset, governance_toolset, graph_toolset, source_toolset, workspace_reading,
    workspace_toolset, xservice_toolset, BoundedDispatcher, DispatchError, Sandbox, ToolBudget,
    ToolDomain, XserviceBacking, WORKSPACE_TOOL_NAMES,
};
use logos_core::federation::{
    query, Backing, ContractBridge, EngineRegistry, Federation, Governance, Member, RegistryMode,
    ServiceBoundary, ServiceLayer,
};
use logos_core::Engine;
use serde_json::{json, Map, Value};
use tempfile::TempDir;

#[path = "../../mcp/tests/support/federated.rs"]
mod federated;

/// The member toolsets' definitions as they stood before S-480 — written from
/// the unmodified `graph_toolset` / `governance_toolset` / `source_toolset`.
const GOLDEN: &str = include_str!("fixtures/member_tool_definitions.json");

const API_LIB: &str = "pub fn shared() {}\npub fn api_only() { shared(); }\n";
const WEB_LIB: &str = "pub fn shared() {}\npub fn render() { shared(); }\n";

/// A literal client call in `web` and the axum route in `api` it binds — one
/// resolved cross-service edge, so reachability and the boundary rule have a
/// binding to read (the shape `chat-agent/tests/xservice_roster.rs` uses).
const WEB_BOUND_CLIENT: &str = r#"
use reqwest::Client;

pub async fn fetch_user(client: Client) {
    let _ = client.get("/users/{id}").await;
}
"#;
const API_ROUTE: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

fn app() -> Router {
    Router::new().route("/users/{user_id}", get(get_user))
}
"#;

/// `web` builds against `api`'s artifact — one build-dependency pair, so the
/// `xservice_build_deps` payload compared below is not an empty one.
const API_POM: &str = "<project><groupId>com.shop</groupId><artifactId>api</artifactId>\
    <version>1</version></project>";
const WEB_POM: &str = "<project><groupId>com.shop</groupId><artifactId>web</artifactId>\
    <version>1</version><dependencies><dependency><groupId>com.shop</groupId>\
    <artifactId>api</artifactId></dependency></dependencies></project>";

/// `web`'s own config: an ignored directory and a read root `api` does not have,
/// so each sandbox is provably the member's own.
const WEB_CONFIG: &str = "[semantics]\nignored_dirs = [\"generated\"]\n\n\
    [chat]\nread_roots = [\"../shared-docs\"]\n";

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write fixture");
}

fn index(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("symlink");
}

/// The two-member workspace on disk: real indexed stores, one bound edge, a
/// build dependency, `web`'s own config, a loose file of the workspace root and
/// a docs directory beside the members.
struct Workspace {
    tmp: TempDir,
}

impl Workspace {
    fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        let (api, web) = (root.join("api"), root.join("web"));
        write(&api, "src/lib.rs", API_LIB);
        write(&api, "src/main.rs", API_ROUTE);
        write(&api, "pom.xml", API_POM);
        write(&api, "generated/stub.rs", "// api's generated code\n");
        write(&web, "src/lib.rs", WEB_LIB);
        write(&web, "src/client.rs", WEB_BOUND_CLIENT);
        write(&web, "pom.xml", WEB_POM);
        write(&web, "generated/stub.rs", "// web's generated code\n");
        write(&web, ".logos/config.toml", WEB_CONFIG);
        write(root, "NOTES.md", "the workspace root's own file\n");
        write(
            root,
            "shared-docs/guide.md",
            "a guide both members link to\n",
        );
        #[cfg(unix)]
        {
            // `web` reaches a sibling member, the workspace root's own file and
            // the shared docs through in-tree links; `api` links the docs too.
            symlink("../api", &web.join("linked-api"));
            symlink("../NOTES.md", &web.join("root-notes.md"));
            symlink("../shared-docs", &web.join("docs"));
            symlink("../shared-docs", &api.join("docs"));
        }
        index(&api);
        index(&web);
        Self { tmp }
    }

    fn root(&self) -> &Path {
        self.tmp.path()
    }

    /// The federation over the two members; `governance` declares `web` (the
    /// frontend) may not call `api` (the backend), which the bound edge breaks.
    fn federation(&self, governance: bool) -> Federation {
        let root = self.root();
        let governance = if governance {
            Governance {
                service_layers: vec![
                    ServiceLayer {
                        name: "frontend".into(),
                        members: vec!["web".into()],
                    },
                    ServiceLayer {
                        name: "backend".into(),
                        members: vec!["api".into()],
                    },
                ],
                boundaries: vec![ServiceBoundary {
                    from: "frontend".into(),
                    to: "backend".into(),
                    reason: Some("the frontend goes through the gateway".into()),
                }],
                no_cross_service_callers: Vec::new(),
            }
        } else {
            Governance::default()
        };
        Federation {
            name: "shop".to_string(),
            root: root.to_path_buf(),
            members: vec![
                Member {
                    name: "api".to_string(),
                    root: root.join("api"),
                },
                Member {
                    name: "web".to_string(),
                    root: root.join("web"),
                },
            ],
            default: None,
            links: Vec::new(),
            governance,
            warm_concurrency: None,
            member_kinds: Default::default(),
        }
    }

    fn registry(&self, governance: bool) -> EngineRegistry<Engine> {
        EngineRegistry::<Engine>::new(self.federation(governance), RegistryMode::Lazy)
    }

    fn backing(&self, governance: bool) -> XserviceBacking {
        let backing = Arc::new(Backing::Federated(Box::new(self.registry(governance))));
        XserviceBacking::federated(backing, Arc::new(ContractBridge::new()))
            .expect("a federated backing mints a workspace backing")
    }
}

async fn definitions(set: &ToolSet) -> Vec<ToolDefinition> {
    set.get_tool_definitions().await.expect("definitions")
}

fn args(value: Value) -> String {
    value.to_string()
}

/// The typed error a failed call carries, as text.
fn failure(err: agent_core::rig::tool::ToolSetError) -> String {
    err.to_string()
}

// ── 1. Definitions ──────────────────────────────────────────────────────────

/// The member toolsets the member roster registers, byte for byte against the
/// golden written from the code before S-480 moved each definition behind a
/// resource-free `tool_definition()`.
#[tokio::test]
async fn the_member_toolsets_register_todays_definitions_byte_for_byte() {
    let dir = TempDir::new().expect("tempdir");
    let engine = Arc::new(Engine::open(dir.path()));
    let sandbox = Arc::new(Sandbox::new(dir.path(), std::iter::empty()).expect("sandbox"));
    let golden: Value = serde_json::from_str(GOLDEN).expect("the golden parses");
    let built = [
        ("graph", definitions(&graph_toolset(engine.clone())).await),
        ("governance", definitions(&governance_toolset(engine)).await),
        ("source", definitions(&source_toolset(sandbox)).await),
    ];
    for (domain, defs) in built {
        assert_eq!(
            serde_json::to_string(&defs).unwrap(),
            serde_json::to_string(&golden[domain]).unwrap(),
            "{domain}: the member definitions changed"
        );
    }
}

/// Every addressed definition, with `repo` taken back out, is its member
/// definition byte for byte; `repo` is a required string.
#[tokio::test]
async fn each_addressed_definition_is_its_member_definition_plus_a_required_repo() {
    let ws = Workspace::new();
    let xs = ws.backing(false);
    let engine = Arc::new(Engine::open(ws.root()));
    let sandbox = Arc::new(Sandbox::new(ws.root(), std::iter::empty()).expect("sandbox"));
    let mut checked = 0;
    for domain in ToolDomain::ALL {
        let member = match domain {
            ToolDomain::Graph => graph_toolset(engine.clone()),
            ToolDomain::Governance => governance_toolset(engine.clone()),
            ToolDomain::Source => source_toolset(sandbox.clone()),
        };
        let member = definitions(&member).await;
        let addressed = definitions(&addressed_toolset(domain, xs.clone())).await;
        assert_eq!(addressed.len(), member.len(), "{domain:?}");
        for (addressed, member) in addressed.iter().zip(&member) {
            assert_eq!(addressed.name, member.name);
            assert_eq!(addressed.description, member.description);

            let mut schema = addressed.parameters.clone();
            let repo = schema["properties"]
                .as_object_mut()
                .expect("properties")
                .remove("repo")
                .unwrap_or_else(|| panic!("{} has no `repo` property", addressed.name));
            assert_eq!(repo["type"], "string", "{}", addressed.name);
            let required = schema["required"].as_array_mut().expect("required");
            assert_eq!(
                required.last(),
                Some(&json!("repo")),
                "{}: repo is required",
                addressed.name
            );
            required.pop();
            if required.is_empty() && member.parameters.get("required").is_none() {
                schema.as_object_mut().unwrap().remove("required");
            }
            assert_eq!(
                serde_json::to_string(&schema).unwrap(),
                serde_json::to_string(&member.parameters).unwrap(),
                "{}: everything but `repo` is the member tool's schema",
                addressed.name
            );
            checked += 1;
        }
    }
    // Guard the guard: every member tool of every domain was compared.
    let total: usize = ToolDomain::ALL.iter().map(|d| d.tool_names().len()).sum();
    assert_eq!(checked, total);
    assert_eq!(total, 19);
}

/// What the model is offered: each workspace tool's schema names exactly the
/// arguments its call reads, and the source domain's `repo` says what its paths
/// are relative to — the calls above pass arguments directly, so nothing else
/// would notice a schema that stopped offering one.
#[tokio::test]
async fn the_schemas_offer_the_arguments_the_calls_read() {
    let ws = Workspace::new();
    let xs = ws.backing(false);
    let properties = |definition: &ToolDefinition| -> Vec<String> {
        definition.parameters["properties"]
            .as_object()
            .expect("properties")
            .keys()
            .cloned()
            .collect()
    };
    let offered: Vec<(String, Vec<String>)> = definitions(&workspace_toolset(xs.clone()))
        .await
        .iter()
        .map(|definition| (definition.name.clone(), properties(definition)))
        .collect();
    let expected: Vec<(String, Vec<String>)> = [
        ("workspace_status", &[][..]),
        ("workspace_reachability", &["repo", "all"][..]),
        ("workspace_check", &[][..]),
        ("xservice_build_deps", &["repo"][..]),
        ("workspace_roster", &[][..]),
    ]
    .iter()
    .map(|(name, keys)| {
        (
            name.to_string(),
            keys.iter().map(|k| k.to_string()).collect(),
        )
    })
    .collect();
    assert_eq!(offered, expected);

    for domain in ToolDomain::ALL {
        for definition in definitions(&addressed_toolset(domain, xs.clone())).await {
            let repo = definition.parameters["properties"]["repo"]["description"]
                .as_str()
                .expect("a described repo")
                .to_string();
            assert!(repo.starts_with("REQUIRED. The workspace member"), "{repo}");
            let paths = repo.contains(
                "Paths are relative to that member's root; nothing outside it is readable",
            );
            assert_eq!(
                paths,
                domain == ToolDomain::Source,
                "{}: only a source tool's repo speaks of paths: {repo}",
                definition.name
            );
        }
    }
}

// ── 2. Laziness and resolution ──────────────────────────────────────────────

#[tokio::test]
async fn building_the_toolsets_warms_nothing() {
    let ws = Workspace::new();
    let xs = ws.backing(false);
    for domain in ToolDomain::ALL {
        let set = addressed_toolset(domain, xs.clone());
        let _ = definitions(&set).await;
    }
    let set = workspace_toolset(xs.clone());
    let names: Vec<String> = definitions(&set)
        .await
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(names, WORKSPACE_TOOL_NAMES);
    assert_eq!(xs.registry().resident_count(), 0);
    assert_eq!(xs.registry().engine_starts(), 0);
}

#[tokio::test]
async fn one_addressed_call_constructs_only_that_members_engine() {
    let ws = Workspace::new();
    let xs = ws.backing(false);
    let graph = addressed_toolset(ToolDomain::Graph, xs.clone());
    assert_eq!(xs.registry().resident_count(), 0);

    let out = graph
        .call(
            "search",
            args(json!({ "repo": "api", "query": "api_only" })),
        )
        .await
        .expect("the addressed search runs");
    assert!(out.contains("api_only"), "{out}");
    assert_eq!(xs.registry().resident_members(), ["api"]);

    // A second domain addressing the same member opens nothing more.
    let governance = addressed_toolset(ToolDomain::Governance, xs.clone());
    governance
        .call("health", args(json!({ "repo": "api" })))
        .await
        .expect("the addressed health runs");
    assert_eq!(xs.registry().resident_members(), ["api"]);
    assert_eq!(xs.registry().engine_starts(), 1);
}

/// The mutation guard for the `repo` resolution: `web` is not the default
/// member (none is declared, so the default falls back to `api`, the first), and
/// `render` exists only in `web`. A call that resolved the default instead would
/// open `api` and find nothing.
#[tokio::test]
async fn an_addressed_call_reads_the_member_it_names_not_the_default() {
    let ws = Workspace::new();
    let xs = ws.backing(false);
    let out = addressed_toolset(ToolDomain::Graph, xs.clone())
        .call("search", args(json!({ "repo": "web", "query": "render" })))
        .await
        .expect("the addressed search runs");
    let hits: Value = serde_json::from_str(&out).expect("json");
    let names: Vec<&str> = hits["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .filter_map(|hit| hit["name"].as_str())
        .collect();
    assert!(names.contains(&"render"), "web's render is found: {out}");
    assert_eq!(xs.registry().resident_members(), ["web"]);

    let out = addressed_toolset(ToolDomain::Source, xs.clone())
        .call("read", args(json!({ "repo": "web", "path": "src/lib.rs" })))
        .await
        .expect("the addressed read runs");
    assert!(out.contains("render"), "web's own file: {out}");

    // The governance arm resolves its own engine: on a fresh backing, a
    // governance call addressed to `web` opens `web` and nothing else.
    let xs = ws.backing(false);
    addressed_toolset(ToolDomain::Governance, xs.clone())
        .call("health", args(json!({ "repo": "web" })))
        .await
        .expect("the addressed health runs");
    assert_eq!(xs.registry().resident_members(), ["web"]);
}

#[tokio::test]
async fn an_unknown_or_missing_repo_is_an_error_listing_the_members_and_warms_nothing() {
    let ws = Workspace::new();
    let xs = ws.backing(false);
    for domain in ToolDomain::ALL {
        let set = addressed_toolset(domain, xs.clone());
        let tool = domain.tool_names()[0];
        let unknown = set
            .call(tool, args(json!({ "repo": "ghost", "query": "x", "task": "x", "symbol": "x", "path": "x" })))
            .await
            .map(|_| ())
            .map_err(failure)
            .expect_err("an unknown member is refused");
        assert!(
            unknown.contains("no workspace member is named \"ghost\""),
            "{unknown}"
        );
        assert!(
            unknown.contains("the workspace's members are: api, web"),
            "{unknown}"
        );

        let missing = set
            .call(tool, args(json!({ "query": "x", "path": "x" })))
            .await
            .map(|_| ())
            .map_err(failure)
            .expect_err("a missing repo is refused");
        assert!(missing.contains("`repo` is required"), "{missing}");
        assert!(missing.contains("api, web"), "{missing}");

        // `null` — what a model sends for an all-optional schema — is a missing
        // `repo`, with the same guidance, not a malformed-arguments error.
        let null = set
            .call(tool, "null".to_string())
            .await
            .map(|_| ())
            .map_err(failure)
            .expect_err("null arguments are refused");
        assert!(null.contains("`repo` is required"), "{null}");
        assert!(null.contains("api, web"), "{null}");

        let not_a_name = set
            .call(tool, args(json!({ "repo": 1, "query": "x", "path": "x" })))
            .await
            .map(|_| ())
            .map_err(failure)
            .expect_err("a non-string repo is refused");
        assert!(
            not_a_name.contains("`repo` must be a string"),
            "{not_a_name}"
        );
        assert!(not_a_name.contains("api, web"), "{not_a_name}");
    }
    assert_eq!(xs.registry().resident_count(), 0);
    assert_eq!(xs.registry().engine_starts(), 0);
}

// ── 3. The addressed member's sandbox ───────────────────────────────────────

/// Dispatch one source call through the real dispatch seam, so a refusal is
/// classified exactly as the roster classifies it.
async fn source_call(
    xs: &XserviceBacking,
    tool: &str,
    call: Value,
) -> Result<String, DispatchError> {
    let budget = ToolBudget::new(10);
    BoundedDispatcher::new(addressed_toolset(ToolDomain::Source, xs.clone()), &budget)
        .dispatch(tool, args(call))
        .await
}

fn assert_contained(outcome: Result<String, DispatchError>, what: &str) {
    match outcome {
        Err(DispatchError::Containment(refusal)) => {
            assert!(!refusal.is_empty(), "{what}");
        }
        other => panic!("{what}: expected a containment refusal, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_source_call_is_confined_to_the_addressed_members_sandbox() {
    let ws = Workspace::new();
    let xs = ws.backing(false);
    let root_notes = ws.root().join("NOTES.md").display().to_string();

    // A sibling member, lexically and through an in-tree link.
    assert_contained(
        source_call(
            &xs,
            "read",
            json!({ "repo": "web", "path": "../api/src/lib.rs" }),
        )
        .await,
        "a sibling member by `..`",
    );
    assert_contained(
        source_call(
            &xs,
            "read",
            json!({ "repo": "web", "path": "linked-api/src/lib.rs" }),
        )
        .await,
        "a sibling member through a symlink — inside the workspace root, outside the member",
    );
    // The workspace root's own file: lexically, absolutely, and through a link.
    assert_contained(
        source_call(&xs, "read", json!({ "repo": "web", "path": "../NOTES.md" })).await,
        "the workspace root's file by `..`",
    );
    assert_contained(
        source_call(&xs, "read", json!({ "repo": "web", "path": root_notes })).await,
        "the workspace root's file by absolute path",
    );
    assert_contained(
        source_call(
            &xs,
            "read",
            json!({ "repo": "web", "path": "root-notes.md" }),
        )
        .await,
        "the workspace root's file through a symlink",
    );
    // The walks never surface a sibling's file either.
    let grep = source_call(&xs, "grep", json!({ "repo": "web", "pattern": "api_only" }))
        .await
        .expect("grep runs");
    assert!(
        !grep.contains("api_only()"),
        "web's grep reaches no api file: {grep}"
    );
    // And no engine was needed for any of it.
    assert_eq!(xs.registry().resident_count(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn the_sandbox_carries_the_addressed_members_own_ignored_dirs_and_read_roots() {
    let ws = Workspace::new();
    let xs = ws.backing(false);

    // `web` ignores `generated`; `api` declares no such thing.
    assert_contained(
        source_call(
            &xs,
            "read",
            json!({ "repo": "web", "path": "generated/stub.rs" }),
        )
        .await,
        "web's own ignored directory",
    );
    let api = source_call(
        &xs,
        "read",
        json!({ "repo": "api", "path": "generated/stub.rs" }),
    )
    .await
    .expect("api ignores no `generated`");
    assert!(api.contains("api's generated code"), "{api}");

    // `web` declares `../shared-docs` a read root; `api` links it but does not.
    let web = source_call(
        &xs,
        "read",
        json!({ "repo": "web", "path": "docs/guide.md" }),
    )
    .await
    .expect("web's declared read root is readable");
    assert!(web.contains("a guide both members link to"), "{web}");
    assert_contained(
        source_call(
            &xs,
            "read",
            json!({ "repo": "api", "path": "docs/guide.md" }),
        )
        .await,
        "api declares no read root",
    );
}

/// A member whose `config.toml` will not load is an error naming it — never a
/// sandbox built on the defaults, which would silently drop the member's own
/// `ignored_dirs` and make what it excludes readable.
#[tokio::test]
async fn a_members_unloadable_config_is_an_error_not_a_default_sandbox() {
    let ws = Workspace::new();
    let xs = ws.backing(false);
    // `web` ignores `generated`; a config that will not parse must not fall back
    // to defaults that do not.
    write(
        &ws.root().join("web"),
        ".logos/config.toml",
        "[semantics]\nignored_dirs = [\"generated\"]\nnot_a_key = 1\n",
    );
    match source_call(
        &xs,
        "read",
        json!({ "repo": "web", "path": "generated/stub.rs" }),
    )
    .await
    {
        Err(DispatchError::Tool(err)) => {
            let message = err.to_string();
            assert!(
                message.contains("config.toml"),
                "names the config: {message}"
            );
        }
        other => panic!("expected a recoverable error naming the config, got {other:?}"),
    }
    // The sibling's sandbox is unaffected.
    let api = source_call(&xs, "read", json!({ "repo": "api", "path": "src/lib.rs" }))
        .await
        .expect("api's config is fine");
    assert!(api.contains("api_only"), "{api}");
}

// ── 4. The workspace read-models answer their twins ─────────────────────────

/// Call a workspace tool through its toolset: its reading, lifted, and the
/// payload with the reading taken out — `shift_remove`, so the read-model's own
/// key order survives for the byte comparison below.
async fn workspace_call(set: &ToolSet, tool: &str, call: Value) -> (String, Value) {
    let out = set
        .call(tool, args(call))
        .await
        .unwrap_or_else(|e| panic!("{tool}: {e}"));
    let reading =
        workspace_reading(tool, &out).unwrap_or_else(|| panic!("{tool} carries a reading"));
    let mut payload: Value = serde_json::from_str(&out).expect("json");
    let object = payload.as_object_mut().expect("an object");
    assert_eq!(
        object.keys().next().map(String::as_str),
        Some("reading"),
        "{tool}: reading first"
    );
    object.shift_remove("reading");
    (reading, payload)
}

/// The two payloads serialize to the same bytes — not only the same data, which
/// `Value`'s order-insensitive equality would accept, but the same key order.
fn assert_same_payload(ours: &Value, theirs: &Value, what: &str) {
    assert_eq!(
        serde_json::to_string(ours).unwrap(),
        serde_json::to_string(theirs).unwrap(),
        "{what}: the tool's payload is its twin's, byte for byte"
    );
}

fn params(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// **S-484: each bridge-backed `xservice_*` tool names the members it read, as
/// its MCP twin does** — `member_reads` rides the tool's payload beside the
/// read-model, and the `reading` line the roster lifts is untouched by it.
#[tokio::test]
async fn each_bridge_backed_xservice_tool_names_the_members_it_read_as_its_twin_does() {
    let ws = Workspace::new();
    let set = xservice_toolset(ws.backing(false));
    let (client, server) = federated::boot(ws.registry(false)).await;

    for (tool, call) in [
        ("xservice_route_providers", json!({})),
        ("xservice_callers", json!({ "symbol": "shared" })),
        ("xservice_impact", json!({ "symbol": "shared", "repo": "api" })),
    ] {
        let out = set.call(tool, args(call.clone())).await.unwrap_or_else(|e| panic!("{tool}: {e}"));
        let ours: Value = serde_json::from_str(&out).expect("json");
        let theirs = federated::call(&client, tool, call.as_object().cloned().unwrap_or_default()).await;
        assert_eq!(ours["member_reads"], json!({ "read": ["api", "web"] }), "{tool}: {ours}");
        assert_eq!(ours["member_reads"], theirs["member_reads"], "{tool}: the twin names the same members");
        let reading = agent_core::xservice_reading(tool, &out).expect("a reading");
        assert!(!reading.contains("member_reads") && !reading.contains("read:"), "{tool}: {reading}");
    }

    client.cancel().await.ok();
    server.abort();
}

#[tokio::test]
async fn each_workspace_tool_answers_its_twins_payload() {
    let ws = Workspace::new();
    let xs = ws.backing(true);
    let set = workspace_toolset(xs.clone());
    let (client, server) = federated::boot(ws.registry(true)).await;

    // workspace_status
    let (reading, ours) = workspace_call(&set, "workspace_status", json!({})).await;
    let theirs = federated::call(&client, "workspace_status", Map::new()).await;
    assert_eq!(ours["members"].as_array().map(Vec::len), Some(2), "{ours}");
    assert_same_payload(&ours, &theirs, "workspace_status");
    // Every reading below is pinned whole: this layer's words literally, and the
    // core's own composed summaries interpolated from the payload beside them.
    let coverage = ours["coverage"]["resolved_edges_summary"].as_str().unwrap();
    let build = ours["build_dependency"]["summary"].as_str().unwrap();
    assert_eq!(
        reading,
        format!(
            "workspace_status \"shop\" — 2 member(s): 2 warm, 0 deferred, 0 degraded | store \
             open: 2 opened, 0 not attempted, 0 degraded | coverage: {coverage} | build \
             dependency (not a runtime coupling): {build}"
        )
    );

    // workspace_reachability, unscoped and scoped with the full dead set
    let (reading, ours) = workspace_call(&set, "workspace_reachability", json!({})).await;
    let theirs = federated::call(&client, "workspace_reachability", Map::new()).await;
    assert_same_payload(&ours, &theirs, "workspace_reachability");
    assert_eq!(
        reading,
        "workspace_reachability — ADVISORY: no callable live via cross-service; dead set \
         withheld (promotions only — pass all: true for it) | read 2 of 2 member(s) | seeded by \
         1 bridge invocation edge(s) beside a headline of 1 resolved"
    );
    let scoped = json!({ "repo": "api", "all": true });
    let (reading, ours) = workspace_call(&set, "workspace_reachability", scoped.clone()).await;
    let theirs = federated::call(
        &client,
        "workspace_reachability",
        params(&[("repo", json!("api")), ("all", json!(true))]),
    )
    .await;
    assert!(
        ours["dead"].is_array(),
        "the full dead set is present: {ours}"
    );
    assert_same_payload(&ours, &theirs, "workspace_reachability scoped");
    assert_eq!(ours["dead"].as_array().map(Vec::len), Some(1), "{ours}");
    assert_eq!(
        reading,
        "workspace_reachability for api — ADVISORY: no callable live via cross-service; 1 dead \
         app-wide | read 2 of 2 member(s) | seeded by 1 bridge invocation edge(s) beside a \
         headline of 1 resolved"
    );

    // A scope naming no member: the twin's payload, and a reading that says so
    // rather than reading the empty view as an empty answer for it.
    let ghost = json!({ "repo": "ghost", "all": true });
    let (reading, ours) = workspace_call(&set, "workspace_reachability", ghost).await;
    let theirs = federated::call(
        &client,
        "workspace_reachability",
        params(&[("repo", json!("ghost")), ("all", json!(true))]),
    )
    .await;
    assert_same_payload(&ours, &theirs, "workspace_reachability for a non-member");
    assert_eq!(
        reading,
        "workspace_reachability for ghost — NOT A MEMBER: \"ghost\" is not a workspace member, \
         so nothing was read for it and the empty view is not an absence; the members are: \
         api; web"
    );

    // workspace_check — the boundary rule is broken by web → api
    let (reading, ours) = workspace_call(&set, "workspace_check", json!({})).await;
    let theirs = federated::call(&client, "workspace_check", Map::new()).await;
    assert_eq!(
        ours["violations"].as_array().map(Vec::len),
        Some(1),
        "{ours}"
    );
    assert_same_payload(&ours, &theirs, "workspace_check");
    // The direction is pinned: the consumer `web` → the provider `api`.
    let violation = &ours["violations"][0];
    assert_eq!(
        reading,
        format!(
            "workspace_check \"shop\" — ADVISORY: 1 rule(s) over 1 cross-service binding(s), 1 \
             violation(s): {}: web:{} → api:{} [route]",
            violation["rule"].as_str().unwrap(),
            violation["from"]["symbol"].as_str().unwrap(),
            violation["to"]["symbol"].as_str().unwrap()
        )
    );

    // xservice_build_deps, unscoped and scoped
    let (reading, ours) = workspace_call(&set, "xservice_build_deps", json!({})).await;
    let theirs = federated::call(&client, "xservice_build_deps", Map::new()).await;
    assert_eq!(
        ours["headline"]["build_dependency_pairs"]["pairs"], 1,
        "{ours}"
    );
    assert_same_payload(&ours, &theirs, "xservice_build_deps");
    let build = ours["headline"]["summary"].as_str().unwrap().to_string();
    assert_eq!(
        reading,
        format!(
            "xservice_build_deps — BUILD DEPENDENCY, NOT A RUNTIME COUPLING: {build} | 2 member \
             row(s)"
        )
    );
    let (reading, ours) =
        workspace_call(&set, "xservice_build_deps", json!({ "repo": "web" })).await;
    let theirs = federated::call(
        &client,
        "xservice_build_deps",
        params(&[("repo", json!("web"))]),
    )
    .await;
    assert_eq!(ours["scope"], "web");
    assert_same_payload(&ours, &theirs, "xservice_build_deps scoped");
    assert_eq!(
        reading,
        format!(
            "xservice_build_deps for web — BUILD DEPENDENCY, NOT A RUNTIME COUPLING: {build} | 1 \
             member row(s)"
        )
    );
    // A scope naming no member reaches the core's `scope_note`, which the
    // reading carries rather than reading the empty rows as an answer.
    let (reading, ours) =
        workspace_call(&set, "xservice_build_deps", json!({ "repo": "ghost" })).await;
    let theirs = federated::call(
        &client,
        "xservice_build_deps",
        params(&[("repo", json!("ghost"))]),
    )
    .await;
    assert_same_payload(&ours, &theirs, "xservice_build_deps for a non-member");
    let note = ours["scope_note"]
        .as_str()
        .expect("a scope note for a non-member");
    assert_eq!(
        reading,
        format!(
            "xservice_build_deps for ghost — BUILD DEPENDENCY, NOT A RUNTIME COUPLING: {build} | 0 \
             member row(s) | {note}"
        )
    );

    // workspace_roster has no MCP tool: its twin is `GET /api/v1/workspace/roster`,
    // whose body is `query::workspace_roster` serialized.
    let (reading, ours) = workspace_call(&set, "workspace_roster", json!({})).await;
    let theirs = serde_json::to_value(query::workspace_roster(&ws.registry(true))).unwrap();
    assert_same_payload(&ours, &theirs, "workspace_roster");
    assert_eq!(reading, "workspace_roster \"shop\" — 2 member(s): api; web");

    client.cancel().await.ok();
    server.abort();
}

/// With no `[governance]` declared both surfaces answer "nothing checked": the
/// MCP twin with `null`, the tool with its reading alone and no report fields.
#[tokio::test]
async fn an_undeclared_rule_family_answers_nothing_on_both_surfaces() {
    let ws = Workspace::new();
    let set = workspace_toolset(ws.backing(false));
    let (client, server) = federated::boot(ws.registry(false)).await;
    let (reading, ours) = workspace_call(&set, "workspace_check", json!({})).await;
    let theirs = federated::call(&client, "workspace_check", Map::new()).await;
    assert_eq!(theirs, Value::Null);
    assert_eq!(ours, json!({}), "only the reading, no report");
    assert!(
        reading.contains("nothing was checked, which is not a pass"),
        "{reading}"
    );
    client.cancel().await.ok();
    server.abort();
}

/// Every workspace path outside the member stores, with its length and mtime.
fn tree(root: &Path) -> BTreeMap<PathBuf, (u64, std::time::SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir") {
            let entry = entry.expect("entry");
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).expect("metadata");
            if entry.file_name() == ".logos" && dir != root {
                continue; // a member's own store: opening it is reading it
            }
            if meta.is_dir() {
                stack.push(path.clone());
            }
            out.insert(path, (meta.len(), meta.modified().expect("mtime")));
        }
    }
    out
}

#[tokio::test]
async fn the_workspace_tools_write_nothing() {
    let ws = Workspace::new();
    let set = workspace_toolset(ws.backing(true));
    let before = tree(ws.root());
    for tool in WORKSPACE_TOOL_NAMES {
        set.call(tool, "{}".to_string())
            .await
            .unwrap_or_else(|e| panic!("{tool}: {e}"));
    }
    assert_eq!(
        tree(ws.root()),
        before,
        "no file outside the member stores changed"
    );
    assert!(
        !ws.root().join(".logos").exists(),
        "nothing was written at the workspace root"
    );
}
