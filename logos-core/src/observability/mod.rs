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
mod tool;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use tracing_subscriber::filter::{filter_fn, EnvFilter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

pub use layer::TelemetryGuard;
pub(crate) use tool::{EventClass, Tool};

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

/// Which adapter surface an event is attributed to — stamped onto every
/// telemetry record so `stats` can break usage down by tool *and* surface
/// ([FR-OB-04]).
///
/// The first three are **process** surfaces: one is chosen at [`init`] and
/// stamped on every event the process emits ([FR-OB-03]). The remainder are
/// **override-only** surfaces, for adapters that run *inside* another surface's
/// process and would otherwise be indistinguishable from it — they are never
/// passed to [`init`], only to [`in_surface`] (or, for the watcher, named on the
/// event itself). Keeping them in the same enum means this type lists every
/// value that can appear in the store's `surface` column, so the read-model and
/// the writers cannot disagree about the vocabulary.
///
/// [FR-OB-03]: ../../../docs/specs/requirements/FR-OB-03.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// The `logos` CLI binary. A process surface.
    Cli,
    /// The `serve --mcp` stdio server. A process surface.
    Mcp,
    /// The `serve --ui` localhost web dashboard (CR-012, feature-gated). A
    /// process surface.
    Web,
    /// The debounced filesystem watcher (S-022). Override-only: it runs inside
    /// the `serve --mcp` process, whose process surface is [`Surface::Mcp`].
    Watcher,
    /// The application's own **shell chrome** ([FR-UI-34], [CR-097]).
    /// Override-only: it runs inside the `serve --ui` process, whose process
    /// surface is [`Surface::Web`], and without this a header render is
    /// indistinguishable from a graph query the user issued through the SPA.
    ///
    /// This is the one surface whose every event is self-referential *by
    /// construction* (see [`Surface::event_class`]): the app header re-reads
    /// the graph-state readout on **every** client-side navigation, so what it
    /// emits is a request the user's own navigation caused incidentally and
    /// never a question the user asked ([BR-42]). The test the classification
    /// applies is *whose question a request answers*, not which view issued it
    /// — which is why the answer lives at the adapter that knows, and not in
    /// the engine chokepoint that cannot ([FR-OB-09]).
    ///
    /// [FR-UI-34]: ../../../docs/specs/requirements/FR-UI-34.md
    /// [BR-42]: ../../../docs/specs/software-spec.md#316-observability--telemetry
    Shell,
    /// Logos's own **wiki generation pass** ([FR-OB-13], [CR-139]).
    /// Override-only: it runs inside the `serve --ui` process, whose process
    /// surface is [`Surface::Web`], and without this every engine call the pass
    /// makes is indistinguishable from a developer browsing the dashboard.
    ///
    /// The same argument `Surface::Chat` makes, pointed at a different
    /// caller — and the reason it is a *distinct* variant rather than a reuse
    /// of `Chat`: *"the chat agent answered a question"* and *"the wiki
    /// generator materialized pages"* are different claims, and collapsing
    /// them would recreate one conflation while removing another ([CR-139]
    /// §3.1, decision 1).
    ///
    /// # Counted, not excluded
    ///
    /// [`Surface::event_class`] answers `None` here, as it does for
    /// [`Surface::Watcher`]: the pass's subject is the indexed code, so its
    /// calls are real engine work and the *tool* decides their class. What this
    /// variant buys is **separability** — the work is attributable to Logos's
    /// own generator instead of being summed into the `web` bucket ([FR-OB-13]
    /// AC 1, [NFR-CC-04]). Only [`Surface::Shell`] forces the self-referential
    /// class, and it stays the only one.
    ///
    /// # Not feature-gated, unlike `Surface::Chat`
    ///
    /// `Chat` is gated with the `agents` substrate because it *is* that
    /// substrate — a build without egress has no chat agent to attribute
    /// ([NFR-SE-01]). The pass this names is the opposite: the materialize half
    /// is a pure local-filesystem read plus a `wiki.db` write, deterministic and
    /// offline ([FR-WK-20], [NFR-SE-01]), so there is no egress substrate to
    /// gate it with. It therefore joins [`Surface::Watcher`] and
    /// [`Surface::Shell`] as an ungated override-only variant, present in
    /// [`Surface::ALL`] — and so in the read-model's vocabulary — in every
    /// feature configuration.
    ///
    /// [CR-139]: ../../../docs/requests/CR-139-the-wiki-generation-pass-names-its-own-surface.md
    /// [FR-OB-13]: ../../../docs/specs/requirements/FR-OB-13.md
    /// [FR-WK-20]: ../../../docs/specs/requirements/FR-WK-20.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    WikiGen,
    /// The in-process chat agent ([FR-OB-10]). Override-only: it reaches the
    /// engine through the web adapter, so without this it is indistinguishable
    /// from a human browsing the dashboard — and *"Logos's own agent navigated
    /// the graph"* is a different claim from *"a developer did"*.
    ///
    /// Gated with the agent substrate it describes, so the surface is **absent**
    /// — not merely never emitted — in a build without `agents` ([FR-OB-10],
    /// [NFR-SE-01]).
    ///
    /// [FR-OB-10]: ../../../docs/specs/requirements/FR-OB-10.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    #[cfg(feature = "agents")]
    Chat,
}

/// The surface a **process** serves — the argument [`init`] takes.
///
/// [`Surface`] is the full vocabulary of values that can appear in the store's
/// `surface` column, and since [FR-OB-09] that vocabulary includes values no
/// process ever *is*: the watcher and the chat agent both run inside another
/// surface's process and are reached only through a per-event override. Letting
/// `init` take a bare `Surface` would make `init(Surface::Chat, root)` a
/// compiling, silently wrong call that stamps every event in the process
/// `chat`.
///
/// So the two roles get two types. This one is closed over the three real
/// process surfaces and converts into [`Surface`] one-way, which makes the
/// invariant the enum's doc used to merely assert into one the compiler keeps —
/// the same discipline [`Tool::event_class`](tool::Tool::event_class) applies to
/// classification.
///
/// [FR-OB-09]: ../../../docs/specs/requirements/FR-OB-09.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSurface {
    /// The `logos` CLI binary.
    Cli,
    /// The `serve --mcp` stdio server.
    Mcp,
    /// The `serve --ui` localhost web dashboard (CR-012, feature-gated).
    Web,
}

impl From<ProcessSurface> for Surface {
    fn from(surface: ProcessSurface) -> Self {
        match surface {
            ProcessSurface::Cli => Surface::Cli,
            ProcessSurface::Mcp => Surface::Mcp,
            ProcessSurface::Web => Surface::Web,
        }
    }
}

impl Surface {
    /// The wire value stored in the `surface` column.
    ///
    /// `pub` rather than crate-private because an adapter that *names* its own
    /// surface ([`in_surface`]) also has to say so in its own logs, and the
    /// value it prints must be the value the store holds or the two readouts
    /// disagree about the same event (the web adapter's `bridge`, [CR-097]).
    pub const fn as_str(self) -> &'static str {
        match self {
            Surface::Cli => "cli",
            Surface::Mcp => "mcp",
            Surface::Web => "web",
            Surface::Watcher => "watcher",
            Surface::Shell => "shell",
            Surface::WikiGen => "wikigen",
            #[cfg(feature = "agents")]
            Surface::Chat => "chat",
        }
    }

    /// The [`EventClass`] this surface *fixes* for every event it carries, or
    /// `None` when the surface does not decide and the tool does ([FR-OB-09],
    /// widened by [CR-097]).
    ///
    /// # Why most surfaces return `None`
    ///
    /// `web` carries both a graph query the user issued through the SPA and a
    /// Statistics-tab render — that they are indistinguishable by surface is
    /// precisely the defect [CR-091] was filed about, and answering `Some` here
    /// for `web` would reinstate the blanket `surface <> 'web'` filter under a
    /// new name. A surface answers `Some` only when *every* event it can carry
    /// is of one class by construction, which is true of exactly one of them:
    /// [`Surface::Shell`] exists solely so shell chrome can say so.
    ///
    /// # This match must stay exhaustive
    ///
    /// **Never add a `_ =>` arm** — the same rule, and the same reason, as
    /// [`Tool::event_class`](tool::Tool::event_class). Registering a surface is
    /// how an adapter declares whose question its reads answer ([BR-42]), so a
    /// surface that reached a wildcard would default silently *into* the usage
    /// figures. `unclassified_surface_fails_the_build` (in [`super::tests`])
    /// scans this function's source and fails if a fallback arm appears.
    ///
    /// [CR-091]: ../../../docs/requests/CR-091-telemetry-surface-classification-and-usage-attribution.md
    /// [CR-097]: ../../../docs/requests/CR-097-header-graph-state-readout.md
    /// [FR-OB-09]: ../../../docs/specs/requirements/FR-OB-09.md
    pub(crate) const fn event_class(self) -> Option<EventClass> {
        match self {
            // Shell chrome: the subject is Logos's own state, and nobody asked.
            Surface::Shell => Some(EventClass::ReadModelRequest),

            // Everything else carries both kinds; the tool decides. The wiki
            // generator is here and not above: its subject is the indexed code,
            // so its calls are real engine work — this variant separates them
            // from a developer's, it does not exclude them ([FR-OB-13]).
            Surface::Cli | Surface::Mcp | Surface::Web | Surface::Watcher | Surface::WikiGen => {
                None
            }
            #[cfg(feature = "agents")]
            Surface::Chat => None,
        }
    }

    /// Every surface value that can reach the store — the closed vocabulary the
    /// per-event override is sanctioned against.
    const ALL: &'static [Surface] = &[
        Surface::Cli,
        Surface::Mcp,
        Surface::Web,
        Surface::Watcher,
        Surface::Shell,
        Surface::WikiGen,
        #[cfg(feature = "agents")]
        Surface::Chat,
    ];

    /// The surface named by an on-event `surface = "…"` field, or `None` for an
    /// unknown value.
    ///
    /// This is what bounds the per-event override ([FR-OB-03]): an arbitrary
    /// string cannot invent a surface, only name one this enum already declares.
    fn from_wire(value: &str) -> Option<Surface> {
        Surface::ALL.iter().copied().find(|s| s.as_str() == value)
    }
}

/// The wire names of every surface whose events are self-referential, in
/// declaration order.
///
/// Derived from [`Surface::event_class`] over [`Surface::ALL`], so it *is* the
/// classification and cannot drift from it — the same construction
/// [`tool::self_referential_tools`] uses over the tool registry, and the other
/// half of the predicate [`tool::engine_query_predicate`] builds.
///
/// Deriving it rather than storing a per-row class is what makes the exclusion
/// apply uniformly to raw events, rolled-up days, and rows written before the
/// classification existed: `surface` is a column on both `events` and
/// `daily_rollup`, and the rollup aggregates away everything else.
pub(crate) fn self_referential_surfaces() -> Vec<&'static str> {
    Surface::ALL
        .iter()
        .filter(|s| matches!(s.event_class(), Some(EventClass::ReadModelRequest)))
        .map(|s| s.as_str())
        .collect()
}

// ── The generalised per-event surface override ([FR-OB-03], [FR-OB-09]) ──────
//
// `surface` is stamped once per process, which is why the read-model's old
// blanket `surface <> 'web'` filter could not tell a dashboard render from a
// graph query issued by the same `serve --ui` process (CR-091). The watcher
// already had the escape hatch — it names `surface = "watcher"` on the event
// itself — but an adapter that does not *emit* the event cannot use that: the
// chat agent's telemetry is emitted deep inside the engine chokepoint it calls,
// with no field of its own to set.
//
// `in_surface` generalises the override into an ambient, thread-scoped one an
// adapter enters **once at its route/call boundary**. Resolution therefore
// happens per request, never per engine call, and costs one thread-local `Cell`
// read on the emission path (NFR-OO-02 is unchanged).

thread_local! {
    /// The surface override in force on this thread, if any.
    static SURFACE_OVERRIDE: std::cell::Cell<Option<Surface>> =
        const { std::cell::Cell::new(None) };
}

/// Restores the previous override on drop, so a panic inside the scoped call
/// cannot leave a thread mis-attributing every later event.
struct SurfaceScope(Option<Surface>);

impl Drop for SurfaceScope {
    fn drop(&mut self) {
        SURFACE_OVERRIDE.with(|s| s.set(self.0));
    }
}

/// Run `f` with every telemetry event emitted **on this thread** attributed to
/// `surface` instead of the process surface ([FR-OB-03], [FR-OB-09]).
///
/// The adapter/route-boundary seam: an adapter that runs inside another
/// surface's process enters this once per request — the chat agent wraps the
/// blocking engine call its tool layer submits, so every event that call
/// produces lands under [`Surface::Chat`] without a single engine chokepoint
/// knowing the agent exists ([ADR-01]).
///
/// Scoping is **per thread**, matching where the events are emitted: the
/// adapters bridge to the synchronous core with `spawn_blocking` ([ADR-03]), so
/// the scope must be entered *inside* that closure, not around the `await`.
/// Nesting restores the outer scope on exit, including on unwind.
///
/// [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
/// [ADR-03]: ../../../docs/specs/architecture/decisions/ADR-03.md
/// [FR-OB-09]: ../../../docs/specs/requirements/FR-OB-09.md
pub fn in_surface<T>(surface: Surface, f: impl FnOnce() -> T) -> T {
    let previous = SURFACE_OVERRIDE.with(|s| s.replace(Some(surface)));
    let _restore = SurfaceScope(previous);
    f()
}

/// The ambient override in force on the emitting thread, if any.
fn ambient_surface() -> Option<Surface> {
    SURFACE_OVERRIDE.with(|s| s.get())
}

/// The surface override in force on the calling thread, if any — the
/// introspection half of [`in_surface`].
///
/// An adapter that installs a boundary scope uses this to prove it: the effect
/// of [`in_surface`] is otherwise visible only inside the telemetry layer, which
/// is private, so without this the *"my tool calls are attributed to my
/// surface"* contract could only be asserted indirectly. Returns `None` on the
/// process surface.
pub fn current_surface_override() -> Option<Surface> {
    ambient_surface()
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
    /// An opaque per-process session stamp ([FR-OB-12]): every event this
    /// process emits carries the same value, so the store can count sessions
    /// and not just calls — `origin` is a branch name and collapses every
    /// process in a worktree into one bucket, which is what made "what
    /// fraction of dev sessions made a navigation call?" unanswerable. A
    /// per-process constant computed once at [`init`] — never on the hot path
    /// ([NFR-OO-02]) — and orthogonal to both [`surface`](Self::surface) and
    /// [`origin`](Self::origin). Carries no user, machine or account identity
    /// ([NFR-CC-03]): it is [`generate_session_id`]'s process-random bits,
    /// nothing derived from the environment.
    ///
    /// [FR-OB-12]: ../../../docs/specs/requirements/FR-OB-12.md
    /// [NFR-OO-02]: ../../../docs/specs/requirements/NFR-OO-02.md
    /// [NFR-CC-03]: ../../../docs/specs/requirements/NFR-CC-03.md
    pub(crate) session_id: String,
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

/// A fresh opaque per-process `session_id` ([FR-OB-12]): 128 random bits, hex
/// encoded. Computed **once** by [`init`] at startup — a cheap, one-time call,
/// never on the hot path ([NFR-OO-02]) — and copied onto every event the
/// process emits.
///
/// Dependency-free, following the [`agent_core::retry`] jitter precedent:
/// [`std::collections::hash_map::RandomState`] is reseeded from the OS's CSPRNG
/// on every construction (it exists to resist HashDoS), so two independently
/// constructed instances give two independent 64-bit draws with no `rand`
/// crate and no socket surface ([NFR-SE-01]). This is exactly what
/// [NFR-CC-03] asks for: the value carries no user, machine or account
/// identity — it is pure per-process randomness, not a hash of the
/// environment.
///
/// [FR-OB-12]: ../../../docs/specs/requirements/FR-OB-12.md
/// [NFR-OO-02]: ../../../docs/specs/requirements/NFR-OO-02.md
/// [NFR-CC-03]: ../../../docs/specs/requirements/NFR-CC-03.md
/// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
fn generate_session_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let hi = RandomState::new().build_hasher().finish();
    let lo = RandomState::new().build_hasher().finish();
    format!("{hi:016x}{lo:016x}")
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
/// - Every event is also stamped with an opaque per-process `session_id`
///   ([FR-OB-12]), computed once here by [`generate_session_id`] — orthogonal
///   to `origin` and never on the hot path.
/// - A second call (or a test that already installed a subscriber) is a
///   no-op for logging; the returned guard is still safe to drop.
///
/// [ADR-50]: ../../../docs/specs/architecture/decisions/ADR-50.md
/// [FR-OB-07]: ../../../docs/specs/requirements/FR-OB-07.md
pub fn init(surface: ProcessSurface, root: &Path) -> TelemetryGuard {
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
        // The per-process session stamp (FR-OB-12): computed once here too,
        // independently of origin — no shared resolution, since it needs no
        // git call at all.
        let session_id = generate_session_id();
        let (sink, guard) = layer::spawn_writer(logos_dir.join(TELEMETRY_DB_FILENAME));
        let telemetry = layer::TelemetryLayer::new(surface.into(), origin, session_id, sink)
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
fn traced_inner<T>(tool: Tool, f: impl FnOnce() -> Result<T>) -> (Result<T>, u64) {
    let tool = tool.as_str();
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
pub(crate) fn traced<T>(tool: Tool, f: impl FnOnce() -> Result<T>) -> Result<T> {
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
    tool: Tool,
    f: impl FnOnce() -> Result<T>,
) -> (Result<T>, u64) {
    traced_inner(tool, f)
}

/// [`traced`] for chokepoint calls that cannot fail (their result type has
/// no error half — e.g. `languages`, which degrades internally). Records
/// `ok = true` always.
pub(crate) fn traced_infallible<T>(tool: Tool, f: impl FnOnce() -> T) -> T {
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
pub(crate) fn traced_infallible_timed<T>(tool: Tool, f: impl FnOnce() -> T) -> (T, u64) {
    let (result, duration_ms) = traced_inner(tool, || Ok(f()));
    match result {
        Ok(value) => (value, duration_ms),
        // The closure above always returns Ok.
        Err(_) => unreachable!("traced_infallible_timed closure cannot fail"),
    }
}

/// Create, migrate and seed a **file-backed** `telemetry.db` under `logos_dir`
/// — the crate-wide test seam for the engine-free per-member read
/// ([`read_stats`], [FR-UI-37]).
///
/// Lives here rather than in the fan-out's own test module because `db` is
/// private to this module: a fixture built anywhere else would have to
/// hand-write `CREATE TABLE`, which is a second copy of [`db::MIGRATIONS`] that
/// can drift from the first. Seeding through the real migration and the real
/// insert keeps one schema.
///
/// [FR-UI-37]: ../../../docs/specs/requirements/FR-UI-37.md
#[cfg(test)]
pub(crate) fn seed_store_for_tests(
    logos_dir: &Path,
    events: &[(&'static str, &str, u64, bool, i64, &str)],
) -> Result<()> {
    std::fs::create_dir_all(logos_dir)?;
    let mut conn = db::open(&logos_dir.join(TELEMETRY_DB_FILENAME))?;
    let batch: Vec<EventRecord> = events
        .iter()
        .map(|&(surface, tool, duration_ms, ok, at, origin)| EventRecord {
            at,
            surface,
            tool: tool.to_string(),
            duration_ms,
            ok,
            origin: origin.to_string(),
            session_id: "test-session".to_string(),
        })
        .collect();
    db::write_batch(&mut conn, &batch)
}

/// Aggregated usage/perf stats from `telemetry.db` — see [`crate::Engine::stats`]
/// for the engine-bound read, and `stats::read_stats` for the engine-free one a
/// workspace fan-out uses ([FR-UI-37]).
///
/// [FR-UI-37]: ../../../docs/specs/requirements/FR-UI-37.md
pub(crate) use stats::{
    attribution_coverage, read_stats, stats, DEFAULT_WINDOW_DAYS as DEFAULT_STATS_WINDOW_DAYS,
    NO_TELEMETRY_YET,
};
