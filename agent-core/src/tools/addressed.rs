//! The **repo-addressed** member tools (S-480, [FR-WS-34]): each graph,
//! governance and source tool, lifted onto a workspace by one required `repo`
//! argument naming the member the call addresses.
//!
//! # The member tool, plus `repo`, and nothing else
//! An addressed tool's definition is its member tool's definition — the same
//! `name`, `description` and schema — with one property added, `repo`, and that
//! property added to `required`. A call strips `repo` and hands the remaining
//! arguments, untouched, to the member tool itself, so the read-model it runs and
//! the output it returns are the member tool's own. The member toolsets
//! ([`graph_toolset`](super::graph_toolset) and its two siblings) are not altered
//! by any of this: an addressed tool builds one per call.
//!
//! # Resolved per call, through the registry ([NFR-PE-10], [ADR-63])
//! Building an addressed toolset touches no engine. A call resolves the
//! addressed member then, and only then: its engine through
//! [`EngineRegistry::engine_for`] — lazily, under the workspace budget and its
//! LRU eviction — or, for a source tool, a [`Sandbox`] built for that member
//! alone. A `repo` the workspace does not have is a tool error naming the
//! workspace's members, raised before anything is opened, and never a fall-back
//! to a default member ([NFR-CC-04]).
//!
//! # The addressed member's sandbox ([NFR-SE-04])
//! A source call's sandbox is the addressed member's root, with that member's
//! own `ignored_dirs` and `[chat] read_roots` (its `.logos/config.toml`, read
//! roots resolved against its root). Paths are member-relative, so a sibling
//! member or a file of the workspace root itself is reachable only through `..`,
//! an absolute path or an escaping symlink — each a containment refusal, carried
//! to the dispatch seam as the typed [`SandboxError`](super::SandboxError) the
//! member tool raised, so it stays turn-fatal exactly as it is in a member chat.
//!
//! [FR-WS-34]: ../../../docs/specs/requirements/FR-WS-34.md
//! [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
//! [NFR-SE-04]: ../../../docs/specs/requirements/NFR-SE-04.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md

use std::sync::Arc;

use logos_core::federation::EngineRegistry;
use logos_core::Engine;
use rig_core::completion::ToolDefinition;
use rig_core::tool::{ToolDyn, ToolError, ToolSet, ToolSetError};
use rig_core::wasm_compat::WasmBoxedFuture;
use serde_json::{json, Map, Value};

use super::{
    governance_toolset, graph_toolset, source_toolset, Sandbox, ToolCallError, ToolDomain,
    XserviceBacking,
};

/// The argument every addressed tool requires.
const REPO: &str = "repo";

/// One member tool, addressed to a workspace member by `repo`.
pub(super) struct AddressedTool {
    domain: ToolDomain,
    /// The member tool's definition plus `repo` ([`addressed_definition`]).
    definition: ToolDefinition,
    xs: XserviceBacking,
}

impl AddressedTool {
    /// Address the member tool `member` defines, of `domain`, over `xs`.
    pub(super) fn new(domain: ToolDomain, member: ToolDefinition, xs: XserviceBacking) -> Self {
        Self {
            definition: addressed_definition(domain, member),
            domain,
            xs,
        }
    }

    /// Resolve `repo`, build the addressed member's toolset, and run the member
    /// tool on the arguments that remain.
    async fn dispatch(&self, args: String) -> Result<String, ToolError> {
        let (repo, member_args) = split_repo(&args, self.xs.registry())?;
        let (domain, xs) = (self.domain, self.xs.clone());
        // Starting an engine opens and migrates a store, and a sandbox reads a
        // config file: both on the blocking pool under the chat surface, like
        // every other engine call this layer makes ([ADR-03], [FR-OB-10]).
        let member_tools = tokio::task::spawn_blocking(move || {
            super::in_chat_surface(|| member_toolset(domain, &xs, &repo))
        })
        .await
        .map_err(|err| tool_error(ToolCallError::Runtime(err.to_string())))??;
        match member_tools.call(&self.definition.name, member_args).await {
            Ok(output) => Ok(output),
            // The member tool's own error, unwrapped from the set's envelope so
            // the dispatch seam finds the same typed cause it would in a member
            // chat — a `SandboxError` containment refusal above all.
            Err(ToolSetError::ToolCallError(err)) => Err(err),
            Err(err) => Err(tool_error(ToolCallError::Runtime(err.to_string()))),
        }
    }
}

impl ToolDyn for AddressedTool {
    fn name(&self) -> String {
        self.definition.name.clone()
    }

    fn definition<'a>(&'a self, _prompt: String) -> WasmBoxedFuture<'a, ToolDefinition> {
        Box::pin(async move { self.definition.clone() })
    }

    fn call<'a>(&'a self, args: String) -> WasmBoxedFuture<'a, Result<String, ToolError>> {
        Box::pin(self.dispatch(args))
    }
}

/// The member tool's definition with a required `repo` added: the `repo`
/// property appended to `properties` and `"repo"` to `required` (created when
/// the member tool requires nothing). Name, description and every other byte of
/// the schema are the member tool's.
pub(super) fn addressed_definition(domain: ToolDomain, member: ToolDefinition) -> ToolDefinition {
    let mut parameters = member.parameters;
    let schema = parameters
        .as_object_mut()
        .expect("every member tool's parameters are an object schema");
    schema
        .entry("properties")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("a member tool's `properties` is an object")
        .insert(REPO.to_string(), repo_property(domain));
    match schema.get_mut("required") {
        Some(Value::Array(required)) => required.push(json!(REPO)),
        _ => {
            schema.insert("required".to_string(), json!([REPO]));
        }
    }
    ToolDefinition {
        name: member.name,
        description: member.description,
        parameters,
    }
}

/// The `repo` property: what it names, what it costs, and — for a source tool —
/// what its paths are relative to.
fn repo_property(domain: ToolDomain) -> Value {
    let paths = match domain {
        ToolDomain::Source => {
            " Paths are relative to that member's root; nothing outside it is readable — no \
             sibling member and no file of the workspace root."
        }
        ToolDomain::Graph | ToolDomain::Governance => "",
    };
    json!({
        "type": "string",
        "description": format!(
            "REQUIRED. The workspace member this call addresses: its workspace-relative name, \
             as workspace_roster lists it. The call reads that member alone and starts only its \
             engine; a name that is not a member is an error listing the members, never a \
             default.{paths}"
        )
    })
}

/// Split `repo` off the caller's arguments, refusing a missing or unknown member
/// **before** anything is opened. Returns the member and the remaining
/// arguments, re-serialized for the member tool.
fn split_repo(
    args: &str,
    registry: &EngineRegistry<Engine>,
) -> Result<(String, String), ToolError> {
    let mut object = match serde_json::from_str(args).map_err(ToolError::JsonError)? {
        Value::Object(object) => object,
        // A model sends `null` for an all-optional schema; `repo` never is.
        Value::Null => Map::new(),
        other => {
            return Err(invalid(format!(
                "arguments must be a JSON object, got {other}"
            )))
        }
    };
    let members = || {
        registry
            .members()
            .iter()
            .map(|member| member.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let repo = match object.remove(REPO) {
        Some(Value::String(repo)) => repo,
        Some(other) => {
            return Err(invalid(format!(
                "`repo` must be a string naming a workspace member, got {other}; the \
                 workspace's members are: {}",
                members()
            )))
        }
        None => {
            return Err(invalid(format!(
                "`repo` is required: name the workspace member this call addresses; the \
                 workspace's members are: {}",
                members()
            )))
        }
    };
    if !registry.members().iter().any(|member| member.name == repo) {
        return Err(invalid(format!(
            "no workspace member is named {repo:?}; the workspace's members are: {}",
            members()
        )));
    }
    Ok((repo, Value::Object(object).to_string()))
}

/// The addressed member's toolset for `domain`: an engine from the registry, or
/// the member's own sandbox.
fn member_toolset(
    domain: ToolDomain,
    xs: &XserviceBacking,
    repo: &str,
) -> Result<ToolSet, ToolError> {
    let registry = xs.registry();
    match domain {
        ToolDomain::Graph => Ok(graph_toolset(member_engine(registry, repo)?)),
        ToolDomain::Governance => Ok(governance_toolset(member_engine(registry, repo)?)),
        ToolDomain::Source => Ok(source_toolset(Arc::new(member_sandbox(registry, repo)?))),
    }
}

fn member_engine(registry: &EngineRegistry<Engine>, repo: &str) -> Result<Arc<Engine>, ToolError> {
    registry
        .engine_for(repo)
        .map_err(|err| tool_error(ToolCallError::Engine(err)))
}

/// The member's sandbox: its root, its `ignored_dirs`, and its `[chat]
/// read_roots` resolved against that root — what [`Sandbox::from_root`] builds,
/// plus the read roots, from one read of the member's config.
fn member_sandbox(registry: &EngineRegistry<Engine>, repo: &str) -> Result<Sandbox, ToolError> {
    let root = registry
        .members()
        .iter()
        .find(|member| member.name == repo)
        .map(|member| member.root.as_path())
        .ok_or_else(|| invalid(format!("no workspace member is named {repo:?}")))?;
    let config = logos_core::config::load_config_from_root(root)
        .map_err(|err| tool_error(ToolCallError::Engine(err.into())))?;
    Sandbox::new(root, config.semantics.ignored_dirs)
        .and_then(|sandbox| sandbox.with_read_roots(root, &config.chat.read_roots))
        .map_err(tool_error)
}

fn invalid(message: String) -> ToolError {
    tool_error(ToolCallError::InvalidArgument(message))
}

fn tool_error(err: impl std::error::Error + Send + Sync + 'static) -> ToolError {
    ToolError::ToolCallError(Box::new(err))
}
