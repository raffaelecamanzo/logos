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

/// One MCP tool an extension serves.
#[derive(Clone)]
pub struct McpTool {
    tool: Tool,
    handler: Arc<McpHandler>,
}

impl McpTool {
    /// A tool with this `tools/list` definition, answered by `handler`.
    ///
    /// The handler runs off the async reactor, so it may block. It reports the
    /// caller's mistake with [`invalid_params`] (or [`parse_arguments`]), which
    /// the adapter maps to an `invalid_params` error; any other error is a tool
    /// failure.
    pub fn new(
        tool: Tool,
        handler: impl Fn(&Engine, JsonObject) -> anyhow::Result<Value> + Send + Sync + 'static,
    ) -> Self {
        Self {
            tool,
            handler: Arc::new(handler),
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
        f.debug_struct("McpTool").field("name", &self.tool.name).finish_non_exhaustive()
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
