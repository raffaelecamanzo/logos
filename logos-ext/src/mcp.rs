//! The MCP contribution point: an extension's tools, served beside the core
//! roster.
//!
//! An `#[tool]` method cannot be added to the MCP server from another crate, so
//! an extension contributes each tool as a definition plus a synchronous handler
//! over the server's engine, and the MCP adapter serves it as a dynamic route
//! (rmcp `ToolRoute::new_dyn`) composed onto its router with `+` — the shape
//! `LogosMcp::federated` already uses for the cross-service tools ([ADR-74]).
//!
//! The adapter runs the handler exactly as it runs a core tool: on the blocking
//! pool, its value serialised as the tool's JSON content, a failure mapped to a
//! structured error carrying the core's severity tag. So an extension tool
//! cannot be told apart from a core one on the wire.
//!
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md

use std::fmt;
use std::sync::Arc;

use logos_core::Engine;
use rmcp::model::{JsonObject, Tool};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// A tool's handler: the server's engine and the call's arguments in, the
/// read-model out.
pub type McpHandler = dyn Fn(&Engine, JsonObject) -> anyhow::Result<Value> + Send + Sync;

/// The argument a [read-only](McpTool::read_only) tool takes to select the
/// workspace member it answers for ([FR-WS-42]) — the name the core read-only
/// tools use.
///
/// [FR-WS-42]: ../../docs/specs/requirements/FR-WS-42.md
pub const REPO_PARAM: &str = "repo";

/// The schema description of [`REPO_PARAM`], worded as the core tools word it.
pub const REPO_PARAM_DESCRIPTION: &str =
    "Workspace member to answer for (its workspace-relative name); omit for the default member. An error under a single-root server.";

/// The sentence a default-member tool's description ends with, worded as the
/// core write and gate tools word it ([FR-WS-42]).
///
/// [FR-WS-42]: ../../docs/specs/requirements/FR-WS-42.md
pub const DEFAULT_MEMBER_DESCRIPTION: &str =
    "Under a workspace it acts on the default member only: it takes no `repo`.";

/// One MCP tool an extension serves.
#[derive(Clone)]
pub struct McpTool {
    tool: Tool,
    handler: Arc<McpHandler>,
    read_only: bool,
}

impl McpTool {
    /// A tool with this `tools/list` definition, answered by `handler`.
    ///
    /// The handler runs off the async reactor, so it may block. It reports the
    /// caller's mistake with [`invalid_params`] (or [`parse_arguments`]), which
    /// the adapter maps to an `invalid_params` error; any other error is a tool
    /// failure.
    ///
    /// Under a workspace the tool acts on the **default member** and cannot be
    /// pointed at another: the safe reading for a tool that may write. Its
    /// description gains [`DEFAULT_MEMBER_DESCRIPTION`] to say so, as a core
    /// write tool's does. A tool that persists nothing is declared with
    /// [`read_only`](Self::read_only).
    pub fn new(
        mut tool: Tool,
        handler: impl Fn(&Engine, JsonObject) -> anyhow::Result<Value> + Send + Sync + 'static,
    ) -> Self {
        tool.description = Some(match tool.description.take() {
            Some(description) => format!("{description} {DEFAULT_MEMBER_DESCRIPTION}").into(),
            None => DEFAULT_MEMBER_DESCRIPTION.into(),
        });
        Self::with_access(tool, handler, false)
    }

    fn with_access(
        tool: Tool,
        handler: impl Fn(&Engine, JsonObject) -> anyhow::Result<Value> + Send + Sync + 'static,
        read_only: bool,
    ) -> Self {
        Self {
            tool,
            handler: Arc::new(handler),
            read_only,
        }
    }

    /// A tool that **persists nothing**, so under a workspace it may answer for
    /// any member ([FR-WS-42]): its definition gains an optional [`REPO_PARAM`],
    /// which the adapter resolves to the member's engine and strips from the
    /// arguments before `handler` sees them ([`take_repo`](Self::take_repo)).
    /// `repo` is therefore reserved: a `repo` property of the tool's own schema
    /// is replaced by this one.
    ///
    /// [FR-WS-42]: ../../docs/specs/requirements/FR-WS-42.md
    pub fn read_only(
        mut tool: Tool,
        handler: impl Fn(&Engine, JsonObject) -> anyhow::Result<Value> + Send + Sync + 'static,
    ) -> Self {
        let mut schema = (*tool.input_schema).clone();
        let properties = schema
            .entry("properties")
            .or_insert_with(|| Value::Object(JsonObject::new()));
        if let Value::Object(properties) = properties {
            properties.insert(
                REPO_PARAM.to_string(),
                serde_json::json!({ "description": REPO_PARAM_DESCRIPTION, "type": ["string", "null"] }),
            );
        }
        tool.input_schema = Arc::new(schema);
        Self::with_access(tool, handler, true)
    }

    /// Whether the tool was declared [read-only](Self::read_only).
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Take a read-only tool's [`REPO_PARAM`] out of `arguments` — the member the
    /// call selects — leaving the arguments the handler was written against.
    ///
    /// Always `None` for a tool declared with [`new`](Self::new), which acts on
    /// the default member: a `repo` it does not declare in its own schema is
    /// refused rather than ignored, so a caller who named another member is never
    /// answered for the default one instead; a `null` one is dropped as absent.
    /// A `repo` its own schema declares is the tool's, and is left in place.
    ///
    /// # Errors
    /// [`InvalidParams`] when `repo` is present and neither a string nor null, or
    /// is given to a default-member tool that does not declare it.
    pub fn take_repo(&self, arguments: &mut JsonObject) -> anyhow::Result<Option<String>> {
        if !self.read_only {
            let declared = self
                .tool
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .is_some_and(|properties| properties.contains_key(REPO_PARAM));
            if !declared && arguments.get(REPO_PARAM).is_some_and(|repo| !repo.is_null()) {
                return Err(invalid_params(format!(
                    "`{}` acts on the default member only: it takes no `{REPO_PARAM}`",
                    self.tool.name
                )));
            }
            if !declared {
                arguments.remove(REPO_PARAM);
            }
            return Ok(None);
        }
        match arguments.remove(REPO_PARAM) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(repo)) => Ok(Some(repo)),
            Some(other) => Err(invalid_params(format!(
                "`{REPO_PARAM}` must be a workspace member's name, got {other}"
            ))),
        }
    }

    /// The tool's name.
    pub fn name(&self) -> &str {
        &self.tool.name
    }

    /// The tool's `tools/list` definition.
    pub fn tool(&self) -> &Tool {
        &self.tool
    }

    /// Answer one call.
    ///
    /// # Errors
    /// Whatever the handler reports.
    pub fn call(&self, engine: &Engine, arguments: JsonObject) -> anyhow::Result<Value> {
        (self.handler)(engine, arguments)
    }
}

impl fmt::Debug for McpTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpTool")
            .field("name", &self.tool.name)
            .field("read_only", &self.read_only)
            .finish_non_exhaustive()
    }
}

/// The caller's mistake: the arguments do not fit the tool. The MCP adapter maps
/// it to an `invalid_params` error rather than a tool failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidParams(pub String);

impl fmt::Display for InvalidParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidParams {}

/// An [`InvalidParams`] error with `message`.
pub fn invalid_params(message: impl Into<String>) -> anyhow::Error {
    InvalidParams(message.into()).into()
}

/// Deserialise a call's arguments into `T`, reporting a mismatch as
/// [`InvalidParams`] in the words rmcp uses for a core tool's arguments.
///
/// # Errors
/// [`InvalidParams`] when the arguments do not deserialise.
pub fn parse_arguments<T: DeserializeOwned>(arguments: JsonObject) -> anyhow::Result<T> {
    serde_json::from_value(Value::Object(arguments))
        .map_err(|e| invalid_params(format!("failed to deserialize parameters: {e}")))
}
