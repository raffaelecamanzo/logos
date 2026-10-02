//! **S-480: the agent tools address a named member, and the workspace
//! read-models become tools** ([FR-WS-34], [NFR-SE-04], [NFR-PE-10]).
//!
//! Over a real two-member federation (`api`, `web`, each indexed into its own
//! store), this suite pins the things the workspace roster will rely on:
//!
//! 1. the member toolsets are byte for byte what they were — against a golden
//!    taken from the code before S-480 touched it — and every repo-addressed
//!    definition is its member's definition plus a required `repo`, nothing else;
//! 2. building the addressed and workspace toolsets starts no engine, one call
//!    starts only the member it names, and an unknown `repo` is an error listing
//!    the members that starts nothing;
//! 3. a source call is confined to the addressed member's own sandbox — its root,
//!    its `ignored_dirs`, its **effective** `[chat] read_roots` (the ones its own
//!    chat reads through, S-482) — so a sibling member or a file of the
//!    workspace root is a containment refusal;
//! 4. the workspace tools write nothing outside the member stores.
//!
//! The fifth — each workspace read-model tool answers its twin's payload,
//! compared whole against the MCP tool called over the real protocol — needs the
//! `mcp` surface crate, so it lives in `web/tests/mcp_parity_workspace_tools.rs`
//! (HF-2): this crate, below the surfaces, dev-depends on neither `mcp` nor
//! `rmcp`. Both suites read the workspace in `tests/support/workspace.rs`.
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
    addressed_toolset, governance_toolset, graph_toolset, source_toolset, workspace_toolset,
    BoundedDispatcher, DispatchError, Sandbox, ToolBudget, ToolDomain, XserviceBacking,
    WORKSPACE_TOOL_NAMES,
};
use logos_core::Engine;
use serde_json::{json, Value};
use tempfile::TempDir;

#[path = "support/workspace.rs"]
mod workspace;

use workspace::{args, write, Workspace};

/// The member toolsets' definitions as they stood before S-480 — written from
/// the unmodified `graph_toolset` / `governance_toolset` / `source_toolset`.
const GOLDEN: &str = include_str!("fixtures/member_tool_definitions.json");

async fn definitions(set: &ToolSet) -> Vec<ToolDefinition> {
    set.get_tool_definitions().await.expect("definitions")
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

/// The addressed sandbox's read roots are the member's **effective** ones — the
/// ones its own chat reads through ([NFR-SE-04], [ADR-67]). `web` declares
/// `read_roots` but no `model`, so once the workspace root declares a `model`,
/// `web` inherits the workspace's `[chat]` table **whole** and its own
/// `read_roots` stop applying: the addressed `read` refuses `docs/guide.md` as
/// a containment, exactly as `web`'s member chat does (its twin is
/// `web/src/chat/configured.rs`'s
/// `an_inherited_policy_drops_the_members_own_read_roots_from_the_member_chat`).
/// The other direction holds too: read roots the workspace table declares
/// resolve against the workspace root and reach every member inheriting it.
///
/// [NFR-SE-04]: ../../docs/specs/requirements/NFR-SE-04.md
/// [ADR-67]: ../../docs/specs/architecture/decisions/ADR-67.md
#[cfg(unix)]
#[tokio::test]
async fn the_addressed_read_roots_are_the_members_effective_ones_as_its_chat_resolves_them() {
    let ws = Workspace::new();
    let xs = ws.backing(false);
    write(ws.root(), ".logos/config.toml", "[chat]\nmodel = \"workspace/model\"\n");
    assert_contained(
        source_call(&xs, "read", json!({ "repo": "web", "path": "docs/guide.md" })).await,
        "web inherits the workspace table, which declares no read root",
    );

    write(
        ws.root(),
        ".logos/config.toml",
        "[chat]\nmodel = \"workspace/model\"\nread_roots = [\"shared-docs\"]\n",
    );
    for repo in ["web", "api"] {
        let guide = source_call(&xs, "read", json!({ "repo": repo, "path": "docs/guide.md" }))
            .await
            .unwrap_or_else(|err| panic!("{repo} reads the workspace's read root: {err:?}"));
        assert!(guide.contains("a guide both members link to"), "{repo}: {guide}");
    }
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
