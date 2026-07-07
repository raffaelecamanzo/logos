//! Graph hydration — derived petgraph views over the canonical store
//! ([graph-hydration component], [ADR-05], [ADR-04]).
//!
//! SQLite is the system of record; petgraph is a **derived, in-memory,
//! ephemeral** view hydrated on demand for whole-graph algorithms (`tarjan_scc`,
//! `condensation`, `toposort`, longest-path) that are awkward and slow as
//! recursive SQL ([ADR-05]). The rule: anything answerable by an index is a SQL
//! point query ([graph-store]); anything needing whole-graph traversal runs on a
//! hydrated [`GraphView`].
//!
//! This module owns three things:
//!
//! 1. [`GraphView`] + [`build_view`] — the four per-granularity views
//!    ([`Granularity`]) built deterministically from a node/edge snapshot
//!    ([FR-DB-05], [FR-DB-06], [NFR-RA-06]).
//! 2. [`HydrationCache`] — the Engine-owned, `(scope, last_sync_at)`-keyed,
//!    bounded LRU cache that amortises hydration across aggregate runs
//!    ([NFR-PE-07]) and invalidates on `last_sync_at` advance ([ADR-04]).
//! 3. The small value types that make up the cache key: [`Scope`] and
//!    [`SyncStamp`].
//!
//! Hydration is reached through the long-lived [`Engine`](crate::Engine):
//! [`Engine::hydrate`](crate::Engine::hydrate) reads through the RO pool
//! ([ADR-02]) so it is never blocked by an in-flight write, and the cache lives
//! for the Engine's lifetime ([ADR-04]).
//!
//! # `last_sync_at`
//!
//! The cache's temporal key component is a monotonic [`SyncStamp`] the Engine
//! holds in memory and advances when the graph changes. Persisting a real
//! `last_sync_at` timestamp is the indexing pipeline's job ([S-010]); hydration
//! only needs a value that *changes* when the graph does, which is exactly what
//! the stamp provides — and it keeps invalidation deterministic for tests.
//!
//! [graph-hydration component]: ../../../docs/specs/architecture/components/graph-hydration.md
//! [graph-store]: ../../../docs/specs/architecture/components/graph-store.md
//! [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
//! [ADR-04]: ../../../docs/specs/architecture/decisions/ADR-04.md
//! [ADR-05]: ../../../docs/specs/architecture/decisions/ADR-05.md
//! [FR-DB-05]: ../../../docs/specs/requirements/FR-DB-05.md
//! [FR-DB-06]: ../../../docs/specs/requirements/FR-DB-06.md
//! [NFR-PE-07]: ../../../docs/specs/requirements/NFR-PE-07.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
//! [S-010]: ../../../docs/planning/journal.md#s-010-indexing-and-incremental-sync-pipeline

mod cache;
mod view;

#[cfg(test)]
mod tests;

pub use cache::{HydrationCache, HydrationConfig, HydrationStats};
pub use view::{build_view, EdgeData, GraphView, Vertex};

/// The subset of the graph a view covers.
///
/// One process serves exactly one worktree root ([ADR-04]), so the only scope
/// TK01 hydrates is the whole project. The enum exists so the cache key matches
/// the architecture's `(scope, last_sync_at)` shape and so sub-scoping (a module
/// subtree, a package) can be added later without reshaping the key.
///
/// [ADR-04]: ../../../docs/specs/architecture/decisions/ADR-04.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    /// The whole indexed project (the single worktree root).
    Project,
}

/// Which per-granularity petgraph view to hydrate ([FR-DB-06]).
///
/// See the [`view`] module docs for the precise edge semantics of each.
///
/// [FR-DB-06]: ../../../docs/specs/requirements/FR-DB-06.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Granularity {
    /// Symbol-level vertices; every dependency edge **except** the lexical
    /// `Contains`. The canonical dependency graph for whole-graph metrics and
    /// SCC ([FR-DB-06]).
    ExcludeContains,
    /// Symbol-level vertices; **all** edge kinds including `Contains`. The
    /// complete lexical-plus-dependency graph (never used for dependency
    /// metrics).
    Symbol,
    /// File-rollup: vertices are files; dependency edges lifted to files and
    /// deduplicated with a weight.
    File,
    /// Module-rollup: vertices are modules (membership derived via `Contains`);
    /// dependency edges lifted to modules and deduplicated with a weight.
    Module,
    /// **Presentation-only** visualization view: symbol-level vertices that
    /// *keep* the non-code layers (documentation and config/artifact kinds) and
    /// the cross-layer `DocReference`/`TracesTo`/`ArtifactRef`/`ArtifactBinding`
    /// edges, so the web canvas's graph-elements accessor can render the full
    /// code/doc/artifact graph layer-tagged ([FR-UI-08], [ADR-34]).
    ///
    /// This is the **only** granularity that admits non-code vertices/edges. It
    /// is consumed solely by `graph_elements()` — never by a metric, cycle, DSM,
    /// or dead-code path — so the four code-subgraph views above and their
    /// exclusion predicate are left untouched and the aggregate signal stays
    /// byte-identical ([FR-DG-06], [FR-QM-08], [ADR-19]). A future contributor
    /// must never wire this view into a metric/algorithm consumer.
    ///
    /// [FR-UI-08]: ../../../docs/specs/requirements/FR-UI-08.md
    /// [FR-DG-06]: ../../../docs/specs/requirements/FR-DG-06.md
    /// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
    /// [ADR-19]: ../../../docs/specs/architecture/decisions/ADR-19.md
    /// [ADR-34]: ../../../docs/specs/architecture/decisions/ADR-34.md
    Visualization,
}

/// The temporal component of the hydration cache key — a stand-in for
/// `last_sync_at` ([ADR-04], [ADR-05]).
///
/// Monotonic and opaque: the Engine advances it whenever the graph changes, and
/// the cache treats any change as "invalidate everything". Comparing two stamps
/// answers exactly one question — "is this the same graph state?" — which is all
/// the cache needs.
///
/// [ADR-04]: ../../../docs/specs/architecture/decisions/ADR-04.md
/// [ADR-05]: ../../../docs/specs/architecture/decisions/ADR-05.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SyncStamp(pub u64);

impl SyncStamp {
    /// The initial stamp before any sync has been recorded.
    pub const INITIAL: SyncStamp = SyncStamp(0);
}
