//! The **workspace root as a config root** ([FR-WS-30], [ADR-67]) — the
//! engine-free seam the web surface's `/api/v1/workspace/config*` routes call.
//!
//! The workspace root holds the second tier [`resolve_chat`](super::resolve_chat)
//! inherits from, and it holds no graph: constructing an [`Engine`](crate::Engine)
//! there would open a store at the root, which is the fault the ADR-67 exception
//! exists to avoid. So these functions reach the **same** read and writers the
//! engine façade's `config_read` / `config_write` / `config_write_secret` reach
//! ([`read_documents`], [`write_config`], [`write_secret`]) — validate-before-write,
//! the atomic replace and the credential's 0o600 mode come unchanged — and emit
//! the **same** telemetry event each of those façade methods emits, through the
//! one emission point ([`traced`](crate::observability::traced)). That is the
//! "equivalent surface-side scope" ADR-67 promises in place of the façade's
//! tracing, so observability stays whole: a workspace-tier save is booked
//! exactly as a member's save is, under whatever surface the caller entered.
//!
//! [FR-WS-30]: ../../../docs/specs/requirements/FR-WS-30.md
//! [ADR-67]: ../../../docs/specs/architecture/decisions/ADR-67.md

use std::path::Path;

use anyhow::Result;

use super::{read_documents, write_config, write_secret};
use super::{ConfigReadModel, ConfigWriteOutcome, SecretWriteOutcome};
use crate::observability::{traced, Tool};

/// The workspace root's config read-model: [`read_documents`] at `root` with
/// **no** tier above it, because the workspace root has none. The effective-chat
/// slice's origins are therefore relative to this root — `member` means
/// *declared here*, `unset` that it is not, and `workspace` never appears.
///
/// # Errors
/// A present-but-invalid file at `root` fails loud, as [`read_documents`] does.
pub fn read_workspace_documents(root: &Path) -> Result<ConfigReadModel> {
    traced(Tool::ConfigRead, || Ok(read_documents(root, None)?))
}

/// [`write_config`] at the workspace root: validate the candidate, never the
/// file it replaces, and only then swap it in atomically — so a save over a
/// broken workspace `config.toml` succeeds and repairs it.
///
/// # Errors
/// The [`ConfigError`](super::ConfigError) [`write_config`] returns, carried in
/// the `anyhow` chain so a caller can still map validation vs I/O faults.
pub fn write_workspace_config(root: &Path, candidate: &str) -> Result<ConfigWriteOutcome> {
    traced(Tool::ConfigWrite, || Ok(write_config(root, candidate)?))
}

/// [`write_secret`] at the workspace root: write (or, blank, clear) the
/// credential every member that declares none inherits.
///
/// # Errors
/// The [`ConfigError`](super::ConfigError) [`write_secret`] returns (an
/// unparsable existing store is refused, never overwritten).
pub fn write_workspace_secret(root: &Path, api_key: &str) -> Result<SecretWriteOutcome> {
    traced(Tool::ConfigWriteSecret, || Ok(write_secret(root, api_key)?))
}
