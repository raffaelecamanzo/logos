//! `mcp` — thin MCP adapter over [`logos_core::Engine`] (S-017, ADR-01,
//! ADR-13, NFR-MA-02).
//!
//! Hosts the rmcp [`ServerHandler`](rmcp::ServerHandler) exposing the
//! namespaced `logos:*` tools (FR-MC-01..06): 8 navigation tools, 9 quality
//! tools, 1 temporal tool (`hotspots`, CR-006), and 3 coverage tools
//! (`coverage_ingest`/`coverage_status`/`coverage_refresh`, CR-007/CR-036), each wired to one
//! [`Engine`](logos_core::Engine) method (the quality set delegates to the
//! governance engine, S-020).
//!
//! # Invariants
//! - **stdout-safety** (NFR-RA-01, ADR-13): stdout carries ONLY JSON-RPC
//!   framing. All logs render to stderr — enforced here by installing the
//!   fmt subscriber with `stderr` as its sole writer, and verified at trace
//!   level by `tests/stdout_safety.rs` (UAT-MC-02).
//! - **Thin surface** (NFR-MA-02, FR-MC-02): no business logic; every tool
//!   delegates to one Engine method (guarded by `tests/line_budget.rs`).
//! - **tokio never enters the core** (ADR-03): a current-thread runtime owns
//!   MCP I/O only; Engine calls run via `spawn_blocking`.
//!
//! # Surface contract (consumed by the `cli` crate, S-016)
//! `logos serve --mcp [--project <root>]` calls [`serve_stdio`] with the
//! resolved project root and exits with its result.

mod server;

pub use server::LogosMcp;

use std::path::Path;

use anyhow::{Context, Result};
use rmcp::ServiceExt;

/// Serve MCP over stdio, rooted at `root`, until the host disconnects
/// (FR-MC-01, FR-WT-04).
///
/// Blocks the calling thread for the whole session. Returning — on stdin EOF
/// or cancellation — tears down the engine (writer actor, pools, caches), so
/// a host disconnect leaves no orphaned serve process (FR-MC-06, NFR-RA-12).
///
/// # Errors
/// Returns an error if the engine cannot start (store open/migrate failure),
/// the I/O runtime cannot build, or the serve loop fails irrecoverably.
pub fn serve_stdio(root: impl AsRef<Path>) -> Result<()> {
    init_stderr_tracing();
    let engine = logos_core::Engine::start(root.as_ref())
        .map(std::sync::Arc::new)
        .context("starting the Logos engine for the MCP server")?;
    // S-022/FR-SY-04: host the debounced watcher beside the serve loop. All
    // policy lives in core's `watch` module; a spawn failure degrades to
    // watcherless serving (FR-SY-06: reconcile backstops freshness), and the
    // handle's drop on return orphans nothing (NFR-RA-12).
    let _watcher = engine
        .watch()
        .inspect_err(|e| tracing::warn!(target: "logos::mcp", "serving without a watcher: {e:#}"))
        .ok();
    // Current-thread runtime: MCP I/O only (ADR-03). Engine work runs on the
    // blocking pool via the LogosMcp submit-and-await bridge. No I/O reactor
    // needed: tokio's stdin/stdout are blocking-pool backed.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .context("building the MCP I/O runtime")?;
    runtime.block_on(serve_stdio_on(engine))
}

/// Run the MCP stdio serve loop on the **caller's** runtime over a shared
/// `Engine`, until the host disconnects (FR-MC-01, FR-WT-04).
///
/// Split out of [`serve_stdio`] so the combined `serve --mcp --ui` process
/// (CR-012, FR-UI-01) can race this loop against the web surface on **one**
/// runtime sharing **one** `Engine` and **one** watcher (the caller owns engine,
/// watcher, and runtime). stdout stays exclusively MCP-owned (NFR-RA-01).
///
/// # Errors
/// Returns an error if the initialize handshake fails for a reason other than a
/// pre-handshake disconnect (which winds down cleanly, exit 0), or the serve
/// loop fails irrecoverably.
pub async fn serve_stdio_on(engine: std::sync::Arc<logos_core::Engine>) -> Result<()> {
    let service = match LogosMcp::new(engine).serve(rmcp::transport::stdio()).await {
        Ok(service) => service,
        // A host that closes stdin before (or during) the initialize handshake
        // is a disconnect, not a fault: wind down cleanly with exit 0, same as
        // a post-handshake EOF (FR-MC-06, NFR-RA-12).
        Err(rmcp::service::ServerInitializeError::ConnectionClosed(reason)) => {
            tracing::info!(target: "logos::mcp", %reason, "host disconnected before initialize; winding down");
            return Ok(());
        }
        Err(e) => return Err(e).context("MCP initialize handshake failed"),
    };
    service.waiting().await.context("MCP serve loop failed")?;
    Ok(())
}

/// Install the stderr-only `tracing` fmt subscriber (ADR-13, NFR-RA-01),
/// filtered by `RUST_LOG`.
///
/// `try_init` keeps this composable: when the full observability stack
/// (S-019: stderr fmt layer + telemetry.db layer) is installed by the
/// hosting binary first, this call is a no-op and that stack — which obeys
/// the same stderr-only invariant — wins.
fn init_stderr_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
}
