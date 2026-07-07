//! Observability — the single `tracing` emission point and its two sinks
//! ([observability], S-019, [ADR-13]).
//!
//! # The one emission discipline ([NFR-OO-01], [FR-OB-01])
//!
//! Nothing in Logos logs directly. The [`Engine`](crate::Engine) chokepoint
//! methods and the three pipeline passes route through [`traced`], which opens
//! a span (human-visible context for the stderr layer) and emits **one**
//! telemetry-tagged completion event (`tool`, `duration_ms`, `ok`). Two layers
//! consume the stream, installed by [`init`]:
//!
//! - a `tracing-subscriber` fmt layer rendering human logs to **stderr only**
//!   ([FR-OB-02], [NFR-RA-01] — the hard stdout-safety invariant: stdout
//!   belongs to read-model output (CLI) or JSON-RPC framing (MCP), never logs),
//! - the custom [`layer::TelemetryLayer`] persisting telemetry events to
//!   `.logos/telemetry.db`, async/batched/best-effort ([FR-OB-03],
//!   [NFR-OO-02]).
//!
//! The `stats` read-models ([FR-OB-04], [NFR-OO-03]) are served from the same
//! store by [`stats::stats`] via [`Engine::stats`](crate::Engine::stats).
//!
//! [observability]: ../../../docs/specs/architecture/components/observability.md
//! [ADR-13]: ../../../docs/specs/architecture/decisions/ADR-13.md
//! [FR-OB-01]: ../../../docs/specs/requirements/FR-OB-01.md
//! [FR-OB-02]: ../../../docs/specs/requirements/FR-OB-02.md
//! [FR-OB-03]: ../../../docs/specs/requirements/FR-OB-03.md
//! [FR-OB-04]: ../../../docs/specs/requirements/FR-OB-04.md
//! [NFR-OO-01]: ../../../docs/specs/requirements/NFR-OO-01.md
//! [NFR-OO-02]: ../../../docs/specs/requirements/NFR-OO-02.md
//! [NFR-OO-03]: ../../../docs/specs/requirements/NFR-OO-03.md
//! [NFR-RA-01]: ../../../docs/specs/requirements/NFR-RA-01.md

mod db;
mod layer;
mod stats;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use tracing_subscriber::filter::{filter_fn, EnvFilter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

pub use layer::TelemetryGuard;

/// The reserved target tagging events for the telemetry layer ([FR-OB-03]).
/// Everything else on the stream is human-log material for the stderr layer.
pub(crate) const TELEMETRY_TARGET: &str = "logos::telemetry";

/// The telemetry store's filename within its resolved `.logos/` directory.
///
/// A single source of truth shared by the write path ([`init`]) and the read
/// path ([`stats::stats`]): with [`telemetry_logos_dir`] fixing the *directory*
/// both target, this fixes the *file*, so the two paths cannot silently diverge
/// on either half of the store path ([ADR-50]).
///
/// [ADR-50]: ../../../docs/specs/architecture/decisions/ADR-50.md
pub(crate) const TELEMETRY_DB_FILENAME: &str = "telemetry.db";

/// The `.logos/` directory that holds the shared telemetry store for `root`.
///
/// Telemetry is a *repo-global* concern ([ADR-50]): a linked worktree writes
/// to and reads from the **primary** checkout's `.logos/` so the usage signal
/// survives `git worktree remove` ([FR-OB-07], [NFR-OO-07]) — unlike
/// `logos.db`, which is branch-local. Both the write path ([`init`]) and the
/// read path ([`stats::stats`]) resolve through here so they can never target
/// different stores.
///
/// Resolution: the primary's `.logos/` when [`crate::workspace::primary_root`]
/// finds a distinct primary **and** that `.logos/` already exists; otherwise
/// the local `<root>/.logos`. The "already exists" clause is deliberate — a
/// worktree must never *create* state inside another checkout ([ADR-50]); when
/// the primary has no `.logos/` yet, telemetry stays local rather than seeding
/// a directory there.
///
/// [ADR-50]: ../../../docs/specs/architecture/decisions/ADR-50.md
/// [FR-OB-07]: ../../../docs/specs/requirements/FR-OB-07.md
/// [NFR-OO-07]: ../../../docs/specs/requirements/NFR-OO-07.md
pub(crate) fn telemetry_logos_dir(root: &Path) -> PathBuf {
    telemetry_logos_dir_for(crate::workspace::primary_root(root).as_deref(), root)
}

/// [`telemetry_logos_dir`] over an already-resolved `primary` — the seam
/// [`init`] uses to resolve the store directory and the `origin` stamp from a
/// **single** [`crate::workspace::primary_root`] call (the read path calls the
/// wrapper, which resolves it for them).
fn telemetry_logos_dir_for(primary: Option<&Path>, root: &Path) -> PathBuf {
    if let Some(primary) = primary {
        let primary_logos = primary.join(".logos");
        if primary_logos.is_dir() {
            return primary_logos;
        }
    }
    root.join(".logos")
}

/// Which adapter surface this process serves — stamped onto every telemetry
/// record so `stats` can break usage down by tool *and* surface ([FR-OB-04]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// The `logos` CLI binary.
    Cli,
    /// The `serve --mcp` stdio server.
    Mcp,
    /// The `serve --ui` localhost web dashboard (CR-012, feature-gated).
    Web,
}

impl Surface {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Surface::Cli => "cli",
            Surface::Mcp => "mcp",
            Surface::Web => "web",
        }
    }
}

/// One telemetry record — the row shape of `telemetry.db`'s `events` table.
#[derive(Debug, Clone)]
pub(crate) struct EventRecord {
    /// Unix seconds at emission.
    pub(crate) at: i64,
    /// `"cli"` or `"mcp"`.
    pub(crate) surface: &'static str,
    /// Engine method or pipeline pass name.
    pub(crate) tool: String,
    pub(crate) duration_ms: u64,
    pub(crate) ok: bool,
    /// The development increment this event belongs to ([FR-OB-08]): the
    /// worktree's branch name, or `"main"` from the primary checkout. A
    /// per-process constant computed once at [`init`] — never on the hot path
    /// — and orthogonal to [`surface`](Self::surface).
    ///
    /// [FR-OB-08]: ../../../docs/specs/requirements/FR-OB-08.md
    pub(crate) origin: String,
}

/// The development-increment `origin` stamped onto every event this process
/// emits ([FR-OB-08]): the checkout's branch name when `root` is a **linked
/// worktree**, else `"main"`.
///
/// Computed **once** by [`init`] at startup — a bounded, one-time git
/// resolution, never on the hot path ([NFR-OO-02]). It reuses the same
/// primary-vs-worktree distinction as [`telemetry_logos_dir`]
/// ([`crate::workspace::primary_root`], [ADR-15]): a `Some` primary means we
/// are in a linked worktree, so the branch names the increment; the primary
/// checkout (and the degrade-gracefully cases — not a git repo, no `git`,
/// detached HEAD) is `"main"`.
///
/// [ADR-15]: ../../../docs/specs/architecture/decisions/ADR-15.md
/// [ADR-50]: ../../../docs/specs/architecture/decisions/ADR-50.md
/// [FR-OB-08]: ../../../docs/specs/requirements/FR-OB-08.md
/// [NFR-OO-02]: ../../../docs/specs/requirements/NFR-OO-02.md
///
/// Production resolves the primary once and calls [`telemetry_origin_for`]
/// directly ([`init`]); this convenience wrapper serves the unit tests.
#[cfg(test)]
pub(crate) fn telemetry_origin(root: &Path) -> String {
    telemetry_origin_for(crate::workspace::primary_root(root).as_deref(), root)
}

/// [`telemetry_origin`] over an already-resolved `primary` — the seam [`init`]
/// uses so the directory and the stamp share one `primary_root` call. A
/// linked worktree (`Some` primary) attributes events to its branch, falling
/// back to `"main"` when the branch is unnameable (detached HEAD, no git); the
/// primary checkout (or no distinct primary — `None`) is always `"main"`.
fn telemetry_origin_for(primary: Option<&Path>, root: &Path) -> String {
    primary
        .and_then(|_| crate::workspace::current_branch(root))
        .unwrap_or_else(|| "main".to_string())
}

/// Install the global subscriber for a surface process: fmt layer → **stderr
/// only** ([NFR-RA-01]), telemetry layer → `telemetry.db` ([ADR-13]).
///
/// Call once at process start, *after* resolving the project root. Returns the
/// [`TelemetryGuard`] that flushes the last telemetry batch on drop — hold it
/// for the life of `main`.
///
/// - Verbosity follows `RUST_LOG` ([FR-OB-02]), defaulting to `warn` so a
///   clean run stays quiet. The filter applies **per-layer** to the fmt layer
///   only — telemetry events persist regardless of the human-log level.
/// - Telemetry activates only when the resolved `.logos/` already exists: an
///   arbitrary read command must not create state as a side effect. Logging
///   to stderr works either way.
/// - The store resolves through [`telemetry_logos_dir`], so a linked worktree
///   writes through to the **primary** repo's `.logos/telemetry.db` ([ADR-50],
///   [FR-OB-07]) — the read path ([`stats::stats`]) resolves the same way.
/// - Every event is stamped with an `origin` ([FR-OB-08]) — the worktree's
///   branch, or `"main"` — computed once here by [`telemetry_origin`] so the
///   shared store can be split dev-vs-main. Off the hot path ([NFR-OO-02]).
/// - A second call (or a test that already installed a subscriber) is a
///   no-op for logging; the returned guard is still safe to drop.
///
/// [ADR-50]: ../../../docs/specs/architecture/decisions/ADR-50.md
/// [FR-OB-07]: ../../../docs/specs/requirements/FR-OB-07.md
pub fn init(surface: Surface, root: &Path) -> TelemetryGuard {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    // Human logs: stderr, never stdout (NFR-RA-01) — a stray stdout byte
    // corrupts the MCP JSON-RPC stream (RK-02).
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(env_filter);

    // Resolve the primary checkout ONCE and share it between the store
    // directory and the origin stamp, so init does a single `primary_root`
    // git call (ADR-15) rather than one per concern.
    let primary = crate::workspace::primary_root(root);
    let logos_dir = telemetry_logos_dir_for(primary.as_deref(), root);
    let (telemetry_layer, guard) = if logos_dir.is_dir() {
        // The per-process origin stamp (FR-OB-08): computed once here, only on
        // the active path, so a telemetry-less run never pays the git cost.
        let origin = telemetry_origin_for(primary.as_deref(), root);
        let (sink, guard) = layer::spawn_writer(logos_dir.join(TELEMETRY_DB_FILENAME));
        let telemetry = layer::TelemetryLayer::new(surface, origin, sink)
            .with_filter(filter_fn(|meta| meta.target() == TELEMETRY_TARGET));
        (Some(telemetry), guard)
    } else {
        (None, TelemetryGuard::disabled())
    };

    let subscriber = tracing_subscriber::registry()
        .with(fmt_layer)
        .with(telemetry_layer);
    // Best-effort: if a subscriber is already installed (tests, double init)
    // we keep it — telemetry simply stays on whatever was installed first.
    let _ = tracing::subscriber::set_global_default(subscriber);
    guard
}

/// The one span+event body ([NFR-OO-01]): run `f` inside a span named for the
/// call, measure its wall-clock **once**, emit the single telemetry-tagged
/// completion event (`tool` / `duration_ms` / `ok`), and return the result
/// paired with that same measured `duration_ms`.
///
/// This is the sole place a pipeline phase is timed. [`traced`] discards the
/// duration; [`traced_timed`] hands it back so a caller can assemble the
/// per-phase index breakdown ([FR-OB-06]) from the *same* measurement that
/// reached telemetry — never a second, parallel timing path ([FR-OB-01],
/// [NFR-OO-01]).
///
/// [FR-OB-06]: ../../../docs/specs/requirements/FR-OB-06.md
/// [FR-OB-01]: ../../../docs/specs/requirements/FR-OB-01.md
fn traced_inner<T>(tool: &'static str, f: impl FnOnce() -> Result<T>) -> (Result<T>, u64) {
    let span = tracing::info_span!("logos", tool);
    let _enter = span.enter();
    let start = Instant::now();
    let result = f();
    let duration_ms = start.elapsed().as_millis() as u64;
    let ok = result.is_ok();
    tracing::info!(
        target: TELEMETRY_TARGET,
        tool,
        duration_ms,
        ok,
        "call completed"
    );
    (result, duration_ms)
}

/// The **single emission point** ([NFR-OO-01]): run `f` inside a span named
/// for the call and emit one telemetry-tagged completion event carrying
/// `tool` / `duration_ms` / `ok`.
///
/// Every Engine chokepoint method and pipeline pass funnels through here —
/// sinks differ, call sites don't ([ADR-13]).
pub(crate) fn traced<T>(tool: &'static str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    traced_inner(tool, f).0
}

/// [`traced`] that additionally returns the wall-clock it measured, in ms.
///
/// Same single seam ([`traced_inner`]) — one span, one telemetry event, one
/// `Instant` — with the measured `duration_ms` surfaced to the caller so the
/// pipeline can build the [FR-OB-06] per-phase breakdown without a parallel
/// timing path ([FR-OB-01], [NFR-OO-01]). The duration is reported whether the
/// call succeeded or failed.
///
/// [FR-OB-06]: ../../../docs/specs/requirements/FR-OB-06.md
pub(crate) fn traced_timed<T>(
    tool: &'static str,
    f: impl FnOnce() -> Result<T>,
) -> (Result<T>, u64) {
    traced_inner(tool, f)
}

/// [`traced`] for chokepoint calls that cannot fail (their result type has
/// no error half — e.g. `languages`, which degrades internally). Records
/// `ok = true` always.
pub(crate) fn traced_infallible<T>(tool: &'static str, f: impl FnOnce() -> T) -> T {
    match traced(tool, || Ok(f())) {
        Ok(value) => value,
        // The closure above always returns Ok.
        Err(_) => unreachable!("traced_infallible closure cannot fail"),
    }
}

/// [`traced_infallible`] that additionally returns the wall-clock it measured,
/// in ms — the same single-seam measurement emitted to telemetry, handed back
/// for the [FR-OB-06] per-phase breakdown.
///
/// [FR-OB-06]: ../../../docs/specs/requirements/FR-OB-06.md
pub(crate) fn traced_infallible_timed<T>(tool: &'static str, f: impl FnOnce() -> T) -> (T, u64) {
    let (result, duration_ms) = traced_inner(tool, || Ok(f()));
    match result {
        Ok(value) => (value, duration_ms),
        // The closure above always returns Ok.
        Err(_) => unreachable!("traced_infallible_timed closure cannot fail"),
    }
}

/// Aggregated usage/perf stats from `telemetry.db` — see [`crate::Engine::stats`].
pub(crate) use stats::stats;
