//! [`Engine`] — the sole instrumented chokepoint (ADR-01, NFR-CC-01).
//!
//! # Design
//! - One `impl Engine` block contains **every** public method — one per
//!   planned tool — so tracing instrumentation attaches exactly here
//!   (NFR-CC-01, ADR-13).
//! - All methods return [`serde::Serialize`] read-models from [`crate::models`];
//!   adapters (`cli`, `mcp`) translate to their wire formats without any
//!   awareness of core internals (ADR-01, NFR-MA-02).
//! - The `Engine` is long-lived, rooted at one worktree root per process
//!   (ADR-04, ADR-15).
//! - Navigation/pipeline methods are infallible at the surface (degrade with
//!   warnings); the quality/governance methods (S-020) return `Result` —
//!   structural failures fail loud (ADR-14 Correctness), per-file failures
//!   degrade inside the read-model (`INCOMPLETE` freshness, NFR-RA-11).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};

use crate::hydrate::{
    Granularity, GraphView, HydrationCache, HydrationConfig, HydrationStats, Scope, SyncStamp,
};
use crate::init::InitOptions;
use crate::model::{EdgeKind, NodeKind};
use crate::models::{
    navigation::{
        AffectedResult, CalleesResult, CallersResult, ContextBundle, ExploreResult, GraphElements,
        GraphGranularity, GraphLayer, ImpactResult, ImplementorsResult, LanguageComposition, NodeInfo,
        ReferencingDocsResult, SearchResult, StatusInfo,
    },
    pipeline::{IndexResult, InitResult, SyncResult},
    quality::{
        DocGapsReport, DoctorReport, DsmReport, EvolutionReport, GateResult, HealthInfo,
        LanguageDescriptor, LanguagesInfo, MetricSnapshot, RulesReport, ScanResult, SessionInfo,
        SkippedLanguage, StatsInfo, TestGapsReport, VerifyReport,
    },
};
use crate::runtime::Runtime;

/// Thick-core engine — the single public façade over all Logos operations.
///
/// Construct a *transient* engine with [`Engine::open`] (CLI one-shot commands:
/// no live runtime), or a *long-lived* engine with [`Engine::start`] (`serve
/// --mcp`: owns the [`Runtime`] — writer actor, RO pool, worker pool — and the
/// derived caches built on top of it, [ADR-04]). One instance per process /
/// worktree root.
pub struct Engine {
    /// Resolved worktree root (ADR-15).
    root: PathBuf,
    /// The live concurrency layer (writer actor + RO pool + worker pool), held
    /// across calls so cold-start cost ([NFR-PE-05]) is paid once and the pools
    /// and any derived caches survive between operations ([ADR-04],
    /// [NFR-PE-07]). `None` for a transient [`Engine::open`] engine that does no
    /// graph I/O (e.g. the `languages` listing path).
    runtime: Option<Runtime>,
    /// The plugin substrate built **once** at [`Engine::start`] and held for the
    /// process lifetime — the canonical derived cache [ADR-04] calls for, and the
    /// heaviest item in the [NFR-PE-05] cold-start budget (parse `plugin.toml`s,
    /// build languages, compile queries). `None` for a transient engine (built
    /// on demand per call) or if the substrate failed to load.
    registry: Option<crate::plugin::LanguageRegistry>,
    /// The bounded petgraph hydration cache, keyed by `(scope, last_sync_at)`
    /// ([ADR-04], [ADR-05], [NFR-PE-07]). Held for the Engine's lifetime so
    /// repeated aggregate runs reuse hydrated views; bounded so it stays within
    /// the RSS envelope ([NFR-PE-06], [AQ-02]). Present (but only *exercised* via
    /// [`Engine::hydrate`], which needs the runtime) on both engine flavours.
    hydration: HydrationCache,
    /// The in-memory `last_sync_at` clock ([ADR-04]). Advances when the graph
    /// changes (the indexing pipeline [S-010] bumps it on commit); the hydration
    /// cache keys on it, so an advance invalidates every cached view.
    sync_stamp: AtomicU64,
    /// Whether the one-time navigation prologue ([FR-IX-07] auto-index on
    /// first evaluation) has run. Memoised so steady-state navigation calls do
    /// **zero** extra reads — the per-call freshness contract stays
    /// "never reconcile" ([FR-RC-05], [ADR-11]).
    nav_prologue_done: AtomicBool,
    /// Unix-seconds timestamp of the last **full index** completed by this
    /// engine (`0` = none this process) — the `last_full_index_at` half of
    /// the [FR-NV-07] status read-model. In-process only until the persisted
    /// `project_metadata` column lands with a later story.
    last_full_index_at: AtomicU64,
    /// The governance engine's in-process state ([S-020]): the compiled
    /// `rules.toml` cache (globs compiled once per content change, [FR-GV-01])
    /// and the last `scan` parameters `rescan` replays.
    ///
    /// [S-020]: ../../../docs/planning/journal.md#s-020-governance-engine-and-quality-gate
    /// [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
    governance: crate::governance::GovernanceState,
    /// The native (extracted) wiki tier's render cache ([FR-WK-10], [ADR-32]),
    /// memoized by the persisted graph revision ([FR-SY-09]): `Some((revision,
    /// native))` when a render at `revision` is cached. A call at the same
    /// revision is an O(1) hit; a revision advance is a miss that re-renders. The
    /// native tier has no store — this in-memory cache is the entire memoization,
    /// nothing is persisted to `wiki.db`. `Mutex` because the web surface shares
    /// the engine behind `Arc` across request tasks ([web-surface]).
    ///
    /// [web-surface]: ../../../docs/specs/architecture/components/web-surface.md
    native_wiki: std::sync::Mutex<Option<(u64, crate::wiki::NativeWiki)>>,
}

impl Engine {
    // ── Constructors ─────────────────────────────────────────────────────

    /// Resolve `root` as the worktree project root for a **transient** engine.
    ///
    /// `root` is a *hint*: it resolves to the containing working-tree root via
    /// `git rev-parse --show-toplevel` ([ADR-15], [FR-WT-01]) — in a linked
    /// worktree, that worktree's own root — and falls back to the hint
    /// verbatim outside git.
    ///
    /// No canonical store is opened and no concurrency runtime is started, so
    /// this is the cheap path for one-shot, store-free operations (e.g. listing
    /// languages, which only needs the plugin substrate under
    /// `<root>/.logos/plugins/`, FR-PL-04). Use [`Engine::start`] for any
    /// operation that reads or writes the graph.
    ///
    /// [FR-WT-01]: ../../../docs/specs/requirements/FR-WT-01.md
    pub fn open(root: impl AsRef<Path>) -> Self {
        Self {
            root: crate::workspace::resolve_root(root.as_ref()),
            runtime: None,
            registry: None,
            hydration: HydrationCache::default(),
            sync_stamp: AtomicU64::new(SyncStamp::INITIAL.0),
            nav_prologue_done: AtomicBool::new(false),
            last_full_index_at: AtomicU64::new(0),
            governance: crate::governance::GovernanceState::default(),
            native_wiki: std::sync::Mutex::new(None),
        }
    }

    /// Start a **long-lived** engine rooted at `root`, ready to serve reads and
    /// writes ([ADR-04]).
    ///
    /// Ensures `<root>/.logos/` exists, then brings up the [`Runtime`] over
    /// `<root>/.logos/logos.db`: the writer opens and migrates the store, the
    /// read-only pool attaches, and the shared worker pool spins up. The whole
    /// sequence is the cold-start path measured against [NFR-PE-05]; afterwards
    /// the engine is held for the process lifetime so the pools (and the derived
    /// caches layered on them) are reused across calls.
    ///
    /// # Errors
    /// Returns an error if `.logos/` cannot be created or the runtime cannot be
    /// opened (store open/migrate, reader pool, or worker pool failure).
    ///
    /// [NFR-PE-05]: ../../../docs/specs/requirements/NFR-PE-05.md
    pub fn start(root: impl AsRef<Path>) -> Result<Self> {
        Self::start_with_hydration_config(root, HydrationConfig::default())
    }

    /// Start a long-lived engine like [`Engine::start`], but with an explicit
    /// hydration-cache bound.
    ///
    /// This is the seam through which the deferred [AQ-02]/[AA-04] RSS-budget
    /// decision is wired without touching call sites: when the cache ceiling is
    /// resolved during dogfood profiling it becomes a [`HydrationConfig`] passed
    /// here (or a default change), not a code rewrite. [`Engine::start`] uses
    /// [`HydrationConfig::default`].
    ///
    /// # Errors
    /// Same as [`Engine::start`].
    ///
    /// [AQ-02]: ../../../docs/specs/architecture.md#14-open-questions
    /// [AA-04]: ../../../docs/specs/architecture.md#24-assumptions
    pub fn start_with_hydration_config(
        root: impl AsRef<Path>,
        hydration: HydrationConfig,
    ) -> Result<Self> {
        // The hint resolves to the containing working-tree root ([ADR-15],
        // [FR-WT-01]): in a linked worktree, THAT worktree's root — so each
        // worktree owns its own `.logos/logos.db` and a server launched with
        // `--project <worktree>` never serves main's graph ([NFR-CC-02],
        // [FR-WT-04]). Outside git the hint is used verbatim.
        //
        // [FR-WT-01]: ../../../docs/specs/requirements/FR-WT-01.md
        // [FR-WT-04]: ../../../docs/specs/requirements/FR-WT-04.md
        // [NFR-CC-02]: ../../../docs/specs/requirements/NFR-CC-02.md
        let root = crate::workspace::resolve_root(root.as_ref());
        let logos_dir = root.join(".logos");
        std::fs::create_dir_all(&logos_dir)
            .with_context(|| format!("creating the .logos directory at {}", logos_dir.display()))?;
        let db_path = logos_dir.join("logos.db");

        // Seed-from-main bootstrap ([ADR-15], [FR-WT-03]): first use in a
        // DB-less linked worktree copies the primary checkout's DB; the
        // main↔branch diff is reconciled below once the runtime and registry
        // are up. A failed copy — or no seed at all — falls back to a fresh
        // store, which the auto-index prologue ([FR-IX-07]) then full-indexes.
        //
        // [FR-WT-03]: ../../../docs/specs/requirements/FR-WT-03.md
        // [FR-IX-07]: ../../../docs/specs/requirements/FR-IX-07.md
        let seed = if db_path.exists() {
            None
        } else {
            crate::workspace::seed_source(&root).and_then(
                |seed| match crate::graph_store::seed_copy(&seed.db_path, &db_path) {
                    Ok(()) => {
                        tracing::info!(
                            primary = %seed.primary_root.display(),
                            head = %seed.head,
                            "seeded the worktree store from the primary checkout (ADR-15)"
                        );
                        Some(seed)
                    }
                    Err(err) => {
                        // seed_copy cleaned up its partial db+wal pair; the
                        // runtime below opens a fresh store.
                        tracing::warn!(
                            "seed-from-main copy failed; falling back to a fresh store \
                             and a full index: {err:#}"
                        );
                        None
                    }
                },
            )
        };

        let runtime = Runtime::open(&db_path).with_context(|| {
            format!("starting the execution runtime for root {}", root.display())
        })?;

        // Build the plugin substrate once (ADR-04). A load failure (typically a
        // malformed user-supplied override) is non-fatal and matches the
        // `languages()` policy: warn to stderr and serve with no registry rather
        // than refuse to start — graph reads/writes do not need it (ADR-14 defers
        // typed errors).
        let registry = match crate::plugin::LanguageRegistry::load(&root) {
            Ok(registry) => Some(registry),
            Err(err) => {
                tracing::warn!("could not load plugin registry at startup: {err}");
                None
            }
        };

        let engine = Self {
            root,
            runtime: Some(runtime),
            registry,
            hydration: HydrationCache::new(hydration),
            sync_stamp: AtomicU64::new(SyncStamp::INITIAL.0),
            nav_prologue_done: AtomicBool::new(false),
            last_full_index_at: AtomicU64::new(0),
            governance: crate::governance::GovernanceState::default(),
            native_wiki: std::sync::Mutex::new(None),
        };

        // Finish the worktree bootstrap: reconcile only the main↔branch diff
        // over the just-seeded store — O(diff-from-main), not O(repo)
        // ([FR-WT-03]). Fail-soft by design.
        if let Some(seed) = seed {
            engine.reconcile_seed_diff(&seed);
        }

        Ok(engine)
    }

    /// The live execution runtime, if this engine was created with
    /// [`Engine::start`].
    ///
    /// The pipeline ([S-010]) submits write batches and the navigation/hydration
    /// paths ([S-009], [S-013]) submit reads through this handle. `None` for a
    /// transient [`Engine::open`] engine.
    ///
    /// [S-009]: ../../../docs/planning/journal.md#s-009-graph-hydration-and-petgraph-views
    /// [S-010]: ../../../docs/planning/journal.md#s-010-indexing-and-incremental-sync-pipeline
    /// [S-013]: ../../../docs/planning/journal.md#s-013-navigation-service-and-the-eight-tools
    pub fn runtime(&self) -> Option<&Runtime> {
        self.runtime.as_ref()
    }

    /// Spawn the debounced filesystem watcher over this engine's root
    /// ([S-022], [FR-SY-04]): each debounce window's changes coalesce into one
    /// [`Engine::sync`] submitted through the single-writer actor ([ADR-02]).
    ///
    /// Hold the returned [`WatchHandle`](crate::watch::WatchHandle) for the
    /// serve loop's lifetime; dropping it stops the watcher and joins its
    /// worker. The watcher is non-load-bearing for correctness ([FR-SY-06]):
    /// treat a spawn failure as a degraded start, not a fatal one.
    ///
    /// # Errors
    /// Returns an error if `config.toml` is invalid or the OS watcher cannot
    /// attach to the root.
    ///
    /// [S-022]: ../../../docs/planning/journal.md#s-022-incremental-sync-hardening-with-watcher-and-git-hooks
    /// [FR-SY-04]: ../../../docs/specs/requirements/FR-SY-04.md
    /// [FR-SY-06]: ../../../docs/specs/requirements/FR-SY-06.md
    pub fn watch(self: &Arc<Self>) -> Result<crate::watch::WatchHandle> {
        crate::watch::spawn(Arc::clone(self))
    }

    /// The plugin substrate cached at [`Engine::start`], if it loaded.
    ///
    /// Extraction ([S-007]) and the `languages` listing read this once-built
    /// registry instead of re-parsing descriptors and recompiling queries on
    /// every call ([ADR-04], [NFR-PE-05]). `None` for a transient
    /// [`Engine::open`] engine or if the substrate failed to load.
    ///
    /// [S-007]: ../../../docs/planning/journal.md#s-007-rust-extraction-engine
    pub fn registry(&self) -> Option<&crate::plugin::LanguageRegistry> {
        self.registry.as_ref()
    }

    // ── Graph hydration (S-009, ADR-05) ───────────────────────────────────

    /// Hydrate (or reuse the cached) petgraph view of the whole project at
    /// `granularity` ([FR-DB-05], [FR-DB-06], [ADR-05]).
    ///
    /// The first call at a given `last_sync_at` reads the graph from the RO pool
    /// and builds the petgraph; subsequent calls at the same stamp return the
    /// cached `Arc<GraphView>` without touching SQLite ([NFR-PE-07]). Advancing
    /// the sync stamp ([`Engine::advance_sync_stamp`]) invalidates the cache, so
    /// the next call re-hydrates.
    ///
    /// # Errors
    /// Returns an error if this is a transient [`Engine::open`] engine (hydration
    /// needs the runtime's RO pool), or if the read fails.
    ///
    /// [FR-DB-05]: ../../../docs/specs/requirements/FR-DB-05.md
    /// [FR-DB-06]: ../../../docs/specs/requirements/FR-DB-06.md
    /// [NFR-PE-07]: ../../../docs/specs/requirements/NFR-PE-07.md
    pub fn hydrate(&self, granularity: Granularity) -> Result<Arc<GraphView>> {
        let runtime = self.runtime.as_ref().ok_or_else(|| {
            anyhow!(
                "graph hydration requires a long-lived engine (Engine::start); \
                 a transient Engine::open engine has no read-only pool to hydrate from"
            )
        })?;
        self.hydration
            .get_or_build(runtime, Scope::Project, granularity, self.sync_stamp())
    }

    /// The current in-memory `last_sync_at` ([ADR-04]). Starts at
    /// [`SyncStamp::INITIAL`].
    pub fn sync_stamp(&self) -> SyncStamp {
        SyncStamp(self.sync_stamp.load(Ordering::Acquire))
    }

    /// Advance `last_sync_at`, invalidating the hydration cache on the next
    /// [`Engine::hydrate`] ([ADR-04]).
    ///
    /// The indexing pipeline ([S-010]) calls this after committing a write batch
    /// so a re-hydration reflects the new graph state. Returns the new stamp.
    ///
    /// [S-010]: ../../../docs/planning/journal.md#s-010-indexing-and-incremental-sync-pipeline
    pub fn advance_sync_stamp(&self) -> SyncStamp {
        // fetch_add returns the previous value; the new stamp is +1.
        SyncStamp(self.sync_stamp.fetch_add(1, Ordering::AcqRel) + 1)
    }

    /// A snapshot of hydration-cache effectiveness for `stats` ([NFR-PE-07]).
    pub fn hydration_stats(&self) -> HydrationStats {
        self.hydration.stats()
    }

    // ── Bootstrap ────────────────────────────────────────────────────────

    /// Initialise a `.logos/` directory in `root` and create the canonical DB,
    /// starter policy templates, and the generated `.gitignore` (FR-IN-01,
    /// FR-IN-04).
    ///
    /// Idempotent and non-clobbering (DL-07): on an already-initialised
    /// project this re-opens the store, applies any pending forward
    /// migrations, refreshes only the managed `.gitignore` block, and never
    /// overwrites an existing policy file.
    ///
    /// Equivalent to [`Engine::init_with`] with [`InitOptions::default`] —
    /// the plain (non-`-i`) CLI contract.
    ///
    /// # Errors
    /// Returns an error if `.logos/` cannot be created, the store cannot be
    /// opened/migrated, or a Logos-owned artifact cannot be written.
    pub fn init(root: impl AsRef<Path>) -> Result<InitResult> {
        Self::init_with(root, &InitOptions::default())
    }

    /// [`Engine::init`] plus the optional host-integration steps selected by
    /// `options` (S-023): `.mcp.json` MCP-server-block injection and the
    /// managed `CLAUDE.md` steer (FR-IN-02), and the `core.hooksPath`
    /// git-hook installer (FR-IN-03).
    ///
    /// Extends — does not replace — the Sprint 4 minimal bootstrap: the
    /// store is still created/migrated through [`Engine::start`] (the
    /// canonical create-dir + open/migrate path), then [`crate::init`] runs
    /// the file-generation steps. Each step's outcome is reported in
    /// [`InitResult::steps`]; targets that cannot be touched *safely*
    /// (malformed host config, foreign `core.hooksPath`, not a git repo) are
    /// `Skipped` with a reason, never clobbered (DL-06, DL-07).
    ///
    /// # Errors
    /// Returns an error if `.logos/` cannot be created, the store cannot be
    /// opened/migrated, or a Logos-owned artifact cannot be written.
    pub fn init_with(root: impl AsRef<Path>, options: &InitOptions) -> Result<InitResult> {
        crate::observability::traced("init", || {
            // Resolve the hint exactly as `start` will ([ADR-15], [FR-WT-01]) so
            // the reported paths match where the store actually lands.
            let root = &crate::workspace::resolve_root(root.as_ref());
            let logos_dir = root.join(".logos");
            let db_path = logos_dir.join("logos.db");
            let existed = db_path.exists();
            drop(Self::start(root)?);
            let steps = crate::init::run(root, options)?;
            Ok(InitResult {
                logos_dir: logos_dir.display().to_string(),
                db_path: db_path.display().to_string(),
                message: if existed {
                    "already initialised — store opened, pending migrations applied".to_string()
                } else {
                    "initialised — run `logos index` to build the code graph".to_string()
                },
                steps,
            })
        })
    }

    /// Install the optional sync git hooks for the repository at `root`
    /// ([FR-SY-05], [FR-IN-03]): managed post-commit/post-checkout/post-merge
    /// scripts under `.logos/hooks/` + `core.hooksPath`, non-clobbering over
    /// foreign hook setups. The `init -i` flow (S-023) calls this for its
    /// optional hook-install step; see [`crate::hooks`] for the contract.
    ///
    /// # Errors
    /// Returns an error if `root` is not a git work tree, `git` is absent, or
    /// the scripts cannot be written.
    ///
    /// [FR-SY-05]: ../../../docs/specs/requirements/FR-SY-05.md
    /// [FR-IN-03]: ../../../docs/specs/requirements/FR-IN-03.md
    pub fn install_hooks(root: impl AsRef<Path>) -> Result<crate::hooks::HooksResult> {
        crate::hooks::install(root.as_ref())
    }

    // ── Navigation (8 tools, FR-NV-01..09, S-013) ─────────────────────────
    //
    // All eight are best-effort-fresh point queries (ADR-11, NFR-DM-02): they
    // read exclusively from the runtime's read-only pool and NEVER reconcile
    // the working tree per call (FR-RC-05). The surface is infallible
    // (ADR-14): a failed read degrades to an empty read-model carrying the
    // reason in `warnings`; an unknown symbol yields an empty result plus
    // "did you mean" suggestions (FR-NV-09) — never an error.

    /// FTS5 full-text search over the code graph, optionally filtered by node
    /// `kind`, at most `limit` (default 20) hits (FR-NV-01).
    ///
    /// Sub-100 ms p95 budget (NFR-PE-01); best-effort-fresh (NFR-DM-02).
    pub fn search(
        &self,
        query: &str,
        kind: Option<NodeKind>,
        limit: Option<usize>,
    ) -> SearchResult {
        crate::observability::traced("search", || {
            crate::navigate::search(self, query, kind, limit)
        })
        .unwrap_or_else(|err| {
            tracing::warn!("search failed: {err:#}");
            SearchResult {
                query: query.to_string(),
                warnings: vec![format!("search failed: {err}")],
                ..SearchResult::default()
            }
        })
    }

    /// Deterministic context bundle for a `task` description (FR-NV-02).
    ///
    /// One call replaces several ad-hoc reads — the token-saving thesis
    /// (AS-02): FTS5-seed → 1-hop expand (OQ-05) → centrality rank → cap at
    /// `max_nodes` (default 25). Code only when `include_code` is `true`.
    pub fn context(
        &self,
        task: &str,
        max_nodes: Option<usize>,
        include_code: bool,
    ) -> ContextBundle {
        crate::observability::traced("context", || {
            crate::navigate::context(self, task, max_nodes, include_code)
        })
        .unwrap_or_else(|err| {
            tracing::warn!("context failed: {err:#}");
            ContextBundle {
                task: task.to_string(),
                warnings: vec![format!("context failed: {err}")],
                ..ContextBundle::default()
            }
        })
    }

    /// Neighbourhood exploration around `query`: source grouped by file, at
    /// most `max_files` (default 10) groups (FR-NV-03).
    pub fn explore(&self, query: &str, max_files: Option<usize>) -> ExploreResult {
        crate::observability::traced("explore", || {
            crate::navigate::explore(self, query, max_files)
        })
        .unwrap_or_else(|err| {
            tracing::warn!("explore failed: {err:#}");
            ExploreResult {
                query: query.to_string(),
                warnings: vec![format!("explore failed: {err}")],
                ..ExploreResult::default()
            }
        })
    }

    /// Full node info for a single `symbol`: metadata, immediate edges, and
    /// code opt-in (FR-NV-04).
    pub fn node(&self, symbol: &str, include_code: bool) -> NodeInfo {
        crate::observability::traced("node", || crate::navigate::node(self, symbol, include_code))
            .unwrap_or_else(|err| {
                tracing::warn!("node failed: {err:#}");
                NodeInfo {
                    query: symbol.to_string(),
                    warnings: vec![format!("node failed: {err}")],
                    ..NodeInfo::default()
                }
            })
    }

    /// Direct callers of `symbol`, at most `limit` (default 50) (FR-NV-05).
    pub fn callers(&self, symbol: &str, limit: Option<usize>) -> CallersResult {
        crate::observability::traced("callers", || crate::navigate::callers(self, symbol, limit))
            .unwrap_or_else(|err| {
                tracing::warn!("callers failed: {err:#}");
                CallersResult {
                    query: symbol.to_string(),
                    warnings: vec![format!("callers failed: {err}")],
                    ..CallersResult::default()
                }
            })
    }

    /// Direct callees of `symbol`, at most `limit` (default 50) (FR-NV-05).
    pub fn callees(&self, symbol: &str, limit: Option<usize>) -> CalleesResult {
        crate::observability::traced("callees", || crate::navigate::callees(self, symbol, limit))
            .unwrap_or_else(|err| {
                tracing::warn!("callees failed: {err:#}");
                CalleesResult {
                    query: symbol.to_string(),
                    warnings: vec![format!("callees failed: {err}")],
                    ..CalleesResult::default()
                }
            })
    }

    /// Transitive impact of changing `symbol`, both directions labeled
    /// (FR-NV-06, DL-03): upstream "breaks if changed", downstream
    /// "depends on", bounded by `depth` (default 3).
    pub fn impact(&self, symbol: &str, depth: Option<usize>) -> ImpactResult {
        crate::observability::traced("impact", || crate::navigate::impact(self, symbol, depth))
            .unwrap_or_else(|err| {
                tracing::warn!("impact failed: {err:#}");
                ImpactResult {
                    query: symbol.to_string(),
                    warnings: vec![format!("impact failed: {err}")],
                    ..ImpactResult::default()
                }
            })
    }

    /// Which code implements a documentation/requirement node (FR-NV-10,
    /// S-037): the code symbols a doc node points at over doc→code edges.
    /// Empty (never an error) when no such edge exists; "did you mean"
    /// suggestions when the doc node is unknown (FR-NV-09).
    pub fn implements(&self, doc: &str) -> ImplementorsResult {
        crate::observability::traced("implements", || crate::navigate::implements(self, doc))
            .unwrap_or_else(|err| {
                tracing::warn!("implements failed: {err:#}");
                ImplementorsResult {
                    query: doc.to_string(),
                    warnings: vec![format!("implements failed: {err}")],
                    ..ImplementorsResult::default()
                }
            })
    }

    /// Which documents reference a code symbol (FR-NV-10, S-037): the doc
    /// sections pointing at the symbol over doc→code edges. Empty (never an
    /// error) when no doc references it; suggestions when the symbol is
    /// unknown (FR-NV-09).
    pub fn referencing_docs(&self, symbol: &str) -> ReferencingDocsResult {
        crate::observability::traced("referencing_docs", || {
            crate::navigate::referencing_docs(self, symbol)
        })
        .unwrap_or_else(|err| {
            tracing::warn!("referencing_docs failed: {err:#}");
            ReferencingDocsResult {
                query: symbol.to_string(),
                warnings: vec![format!("referencing_docs failed: {err}")],
                ..ReferencingDocsResult::default()
            }
        })
    }

    /// Whole reverse-transitive closure of files affected by a changed set
    /// (FR-CL-04, DL-08): every file depending — directly or transitively, over
    /// calls/imports/references — on any of `files`. `tests_only` narrows the
    /// closure to test-marked files.
    pub fn affected(&self, files: &[String], tests_only: bool) -> AffectedResult {
        crate::observability::traced("affected", || {
            crate::navigate::affected(self, files, tests_only)
        })
        .unwrap_or_else(|err| {
            tracing::warn!("affected failed: {err:#}");
            AffectedResult {
                changed: files.to_vec(),
                tests_only,
                warnings: vec![format!("affected failed: {err}")],
                ..AffectedResult::default()
            }
        })
    }

    /// Current index and sync health of the code graph (FR-NV-07): counts,
    /// store size, resolution coverage, and the freshness statement.
    pub fn status(&self) -> StatusInfo {
        crate::observability::traced("status", || crate::navigate::status(self)).unwrap_or_else(
            |err| {
                tracing::warn!("status failed: {err:#}");
                StatusInfo {
                    warnings: vec![format!("status failed: {err}")],
                    ..StatusInfo::default()
                }
            },
        )
    }

    /// Read-only graph-elements accessor feeding the web surface's interactive
    /// graph canvas ([FR-UI-08], [ADR-29]): a presentation-shaped nodes+edges
    /// snapshot of the hydrated graph, whole-graph (`seed = None`) or seed-scoped
    /// (the seed's connected neighbourhood), bounded by a visible-element `cap`
    /// (default 250) with the elided remainder reported, never silently dropped
    /// ([NFR-CC-04]).
    ///
    /// `layers` / `edge_types` are the optional **server-side re-budgeting
    /// filters** (S-122, [FR-UI-15]): when present they narrow the candidate set
    /// *before* the degree-rank+truncate, so deselecting a layer or edge type
    /// backfills the freed visible budget with previously-elided nodes of the
    /// remaining scope (the graph stays full, not merely smaller). `None` ⇒ no
    /// filter (all layers / all edge types); an empty slice ⇒ filter everything
    /// out (the honest empty graph). See [`navigate::graph_elements`] for the exact
    /// re-budgeting semantics.
    ///
    /// `granularity` is the optional **semantic cluster-zoom tier** (S-124,
    /// [FR-UI-15], [ADR-36]): `module`/`file`/`symbol` select the existing
    /// module-rollup / file-rollup / visualization hydration view ([ADR-34]). `None`
    /// is the symbol tier — the pre-S-124 behaviour. The rollup tiers are the code
    /// subgraph (docs/artifacts excluded, [FR-DG-06]) and feed no metric path, so
    /// every tier is metric-neutral ([FR-QM-08]).
    ///
    /// `intent_overlay` is the **bounded documentation-intent overlay** (S-128,
    /// [FR-UI-16], [ADR-37]): `false` (the default) is byte-identical to the
    /// pre-S-128 snapshot; `true` admits the governing-doc nodes adjacent (via the
    /// existing `DocReference`/`TracesTo` edges) to the kept code nodes up to a
    /// separate reserved budget computed outside the structural ranking, so the
    /// overlay can never starve the code anchors (the [CR-014] doc-flooding guard).
    /// Presentation-only over the visualization view — metric-neutral and read-only.
    ///
    /// A **pure reader** over the cached hydrated view — no compute-and-persist;
    /// calling it once or repeatedly mutates no store, the read-only discipline
    /// [ADR-28] established for `latest_*` and [ADR-29] extends to the graph
    /// ([FR-UI-03]). Consumed only by the [web-surface]; CLI/MCP are unaffected.
    ///
    /// [FR-UI-08]: ../../../docs/specs/requirements/FR-UI-08.md
    /// [FR-UI-15]: ../../../docs/specs/requirements/FR-UI-15.md
    /// [FR-UI-16]: ../../../docs/specs/requirements/FR-UI-16.md
    /// [FR-UI-03]: ../../../docs/specs/requirements/FR-UI-03.md
    /// [FR-DG-06]: ../../../docs/specs/requirements/FR-DG-06.md
    /// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    /// [ADR-29]: ../../../docs/specs/architecture/decisions/ADR-29.md
    /// [ADR-34]: ../../../docs/specs/architecture/decisions/ADR-34.md
    /// [ADR-36]: ../../../docs/specs/architecture/decisions/ADR-36.md
    /// [ADR-37]: ../../../docs/specs/architecture/decisions/ADR-37.md
    /// [CR-014]: ../../../docs/requests/CR-014-context-seed-doc-flooding.md
    /// [web-surface]: ../../../docs/specs/architecture/components/web-surface.md
    /// [`navigate::graph_elements`]: crate::navigate::graph_elements
    pub fn graph_elements(
        &self,
        seed: Option<&str>,
        cap: Option<usize>,
        layers: Option<&[GraphLayer]>,
        edge_types: Option<&[EdgeKind]>,
        granularity: Option<GraphGranularity>,
        intent_overlay: bool,
    ) -> GraphElements {
        crate::observability::traced("graph_elements", || {
            crate::navigate::graph_elements(
                self,
                seed,
                cap,
                layers,
                edge_types,
                granularity,
                intent_overlay,
            )
        })
        .unwrap_or_else(|err| {
            tracing::warn!("graph_elements failed: {err:#}");
            GraphElements {
                seed: seed.map(str::to_string),
                granularity: granularity.unwrap_or_default(),
                warnings: vec![format!("graph_elements failed: {err}")],
                ..GraphElements::default()
            }
        })
    }

    // ── Pipeline (2 triggers) ─────────────────────────────────────────────

    /// Full re-index of the project (FR-IX-01..06).
    ///
    /// discover → extract → resolve → annotate → persist (ADR-10). Requires a
    /// long-lived [`Engine::start`] engine (it submits write batches to the
    /// writer actor). Infallible at the surface (ADR-14 defers typed errors): a
    /// failure is logged to stderr and returned as an [`IndexResult`] carrying
    /// the reason in `warnings` rather than panicking.
    pub fn index(&self) -> IndexResult {
        match crate::observability::traced("index", || self.run_index()) {
            Ok(result) => result,
            Err(err) => degraded_index(&err),
        }
    }

    /// Incremental sync over the given changed `paths` (FR-SY-01..06).
    ///
    /// Capture-before-delete ensures cross-file links rebind (ADR-10). Same
    /// infallible-surface posture as [`index`](Self::index): a failure is logged
    /// and surfaced in the [`SyncResult`] `warnings`.
    pub fn sync(&self, paths: &[PathBuf]) -> SyncResult {
        match crate::observability::traced("sync", || self.run_sync(paths)) {
            Ok(result) => result,
            Err(err) => degraded_sync(&err),
        }
    }

    /// Auto-index on first evaluation (FR-IX-07): if the graph has never been
    /// indexed, run a full [`index`](Self::index) before serving; otherwise a
    /// no-op.
    ///
    /// This is the prologue a navigation/query call runs so the very first
    /// evaluation against an un-indexed project transparently builds the graph.
    /// Navigation methods (S-013) call this before reading; it is exposed now so
    /// the auto-index contract is in place and testable. Returns the
    /// [`IndexResult`] of the index that ran, or a zero-valued result when the
    /// graph was already populated.
    pub fn ensure_indexed(&self) -> IndexResult {
        match crate::observability::traced("ensure_indexed", || self.run_ensure_indexed()) {
            Ok(Some(result)) => result,
            Ok(None) => IndexResult::default(),
            Err(err) => degraded_index(&err),
        }
    }

    // ── Quality / Governance (10 tools, S-020) ───────────────────────────
    //
    // All ten are guaranteed-fresh aggregate evaluations (ADR-11,
    // NFR-DM-02): each reconciles the working tree first (O(changed),
    // FR-RC-01/02) unless told not to (FR-RC-04), and every result carries
    // the FR-RC-03 freshness line. Unlike the infallible navigation surface,
    // these return `Result` — a structural failure (store fault, invalid
    // rules.toml, broken config) fails loud (ADR-14 Correctness; the typed
    // `CoreError` enum itself lands with S-026), while per-file degradations
    // ride inside the read-model as an `INCOMPLETE` stamp + warnings
    // (NFR-RA-11, Degraded).

    /// Full architecture-quality scan: reconcile-then-score (ADR-11).
    ///
    /// Returns the 0–10000 geometric-mean signal (ADR-12), the rule
    /// violations, and persists a `metric_snapshots` row (FR-GV-09).
    /// `reconcile = false` is the `--no-reconcile` escape hatch (FR-RC-04).
    ///
    /// # Errors
    /// Returns an error on a structural failure: transient engine, store
    /// fault, invalid `rules.toml` (a [`ConfigError`](crate::config::ConfigError),
    /// usage exit 2), or a failed write batch.
    pub fn scan(&self, reconcile: bool) -> Result<ScanResult> {
        self.governance.record_scan(reconcile);
        crate::observability::traced("scan", || crate::governance::scan(self, reconcile))
    }

    /// Re-scan with the same parameters as the last [`scan`](Self::scan)
    /// (ADR-11); behaves like a default scan when none ran yet.
    ///
    /// # Errors
    /// Same as [`scan`](Self::scan).
    pub fn rescan(&self) -> Result<ScanResult> {
        let reconcile = self.governance.last_scan_reconcile();
        crate::observability::traced("rescan", || crate::governance::scan(self, reconcile))
    }

    /// Gate check (FR-GV-04/05, BR-10): compute a fresh snapshot (always
    /// persisted, FR-GV-09), then either upsert it as the baseline (`save`)
    /// or fail iff `current < baseline − ε` (ε ≈ 1.0 absorbs float noise on
    /// the integer signal). `threshold` adds an explicit floor. No baseline
    /// → informational pass.
    ///
    /// # Errors
    /// Returns an error on a structural failure (transient engine, store
    /// fault, failed write batch).
    pub fn gate(&self, threshold: Option<u32>, save: bool, reconcile: bool) -> Result<GateResult> {
        crate::observability::traced("gate", || {
            crate::governance::gate(self, threshold, save, reconcile)
        })
    }

    /// Architecture-rules compliance report against `rules.toml` (FR-GV-02):
    /// constraints, layer ordering (unassigned files exempt), and
    /// boundaries; re-materialises the derived policy graph each run
    /// (BR-12, idempotent). `rules_path` overrides the default
    /// `<root>/.logos/rules.toml` (`check --rules FILE`).
    ///
    /// # Errors
    /// Returns an error on a structural failure; an invalid contract is a
    /// [`ConfigError`](crate::config::ConfigError) (usage exit 2).
    pub fn check_rules(&self, rules_path: Option<&Path>, reconcile: bool) -> Result<RulesReport> {
        crate::observability::traced("check_rules", || {
            crate::governance::check_rules(self, rules_path, reconcile)
        })
    }

    /// Signal evolution over stored snapshots (FR-GV-06): the most recent
    /// `limit` (default 30) `metric_snapshots` rows with per-metric deltas.
    /// Reports history — does not reconcile (BR-03).
    ///
    /// # Errors
    /// Returns an error for a transient engine or on a read failure.
    pub fn evolution(&self, limit: Option<u32>) -> Result<EvolutionReport> {
        crate::observability::traced("evolution", || crate::governance::evolution(self, limit))
    }

    /// Dependency structure matrix (FR-GV-07): cell `(i, j)` counts dep
    /// edges `i → j`; rows ordered by layer order then name; module
    /// granularity by default.
    ///
    /// # Errors
    /// Returns an error on a structural failure.
    pub fn dsm(
        &self,
        granularity: Option<crate::governance::DsmGranularity>,
        reconcile: bool,
    ) -> Result<DsmReport> {
        crate::observability::traced("dsm", || {
            crate::governance::dsm(self, granularity, reconcile)
        })
    }

    /// Test-gap analysis (FR-GV-08, BR-16): non-test, non-entry-point
    /// functions unreachable from any test node over `calls` BFS, capped at
    /// `limit` (default 50), always carrying the static-coverage caveat.
    ///
    /// Ordered by **blast radius** ([FR-GV-17]): caller fan-in × the containing
    /// file's hotspot score, most-urgent first. The hotspot ranking is computed
    /// **here at the façade** (read-only, from the already-mined facts — see
    /// [`blast_radius_weights`](Self::blast_radius_weights)) and supplied to the
    /// governance computation, so the governance engine never links history
    /// ([CR-038], [ADR-28]). The single shared accessor behind the web Gaps
    /// view, the MCP `test_gaps` tool, and the CLI — so all three return the
    /// identical ordering. With no history/hotspot store the ranking degrades to
    /// the FR-GV-08 file/name order, the caveat still emitted, never a fabricated
    /// ranking ([NFR-CC-04]); the ranking never feeds `gate`/`scan` ([BR-28]).
    ///
    /// # Errors
    /// Returns an error on a structural failure.
    ///
    /// [FR-GV-17]: ../../../docs/specs/requirements/FR-GV-17.md
    /// [CR-038]: ../../../docs/requests/CR-038-web-ui-information-architecture-restructure.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub fn test_gaps(&self, limit: Option<u32>, reconcile: bool) -> Result<TestGapsReport> {
        crate::observability::traced("test_gaps", || {
            let ranks = self.blast_radius_weights();
            crate::governance::test_gaps(self, limit, reconcile, ranks.as_ref())
        })
    }

    /// The read-only file → hotspot-score map that weights the blast-radius
    /// ranking of [`test_gaps`](Self::test_gaps) ([FR-GV-17]). Reads the
    /// **already-mined** temporal facts via [`latest_hotspots`](Self::latest_hotspots)
    /// — it never mines and never appends a snapshot, so computing the ranking on
    /// a read leaves every store unchanged ([ADR-28]).
    ///
    /// `None` whenever no usable hotspot signal exists — a degraded/empty
    /// temporal tier, or any history-read failure — so the caller degrades to the
    /// FR-GV-08 file/name order and never fabricates a ranking ([NFR-CC-04]). A
    /// history error is swallowed to `None` on purpose: a missing temporal tier
    /// must not turn the always-available `test_gaps` into a failure.
    fn blast_radius_weights(&self) -> Option<std::collections::HashMap<String, i64>> {
        // `limit = None` → the full ranked board, so every gap's containing file
        // can be weighted (a truncated board would silently zero-weight the tail).
        let report = self.latest_hotspots(None, false, false).ok()?;
        if report.degraded.is_some() || report.files.is_empty() {
            return None;
        }
        Some(
            report
                .files
                .iter()
                .map(|h| (h.path.clone(), h.score))
                .collect(),
        )
    }

    /// Documentation-gap analysis (FR-GV-14): exported functions/methods
    /// referenced by no `DocSection` over `DocReference` edges, capped at
    /// `limit` (default 50) — the read-only analog of
    /// [`test_gaps`](Self::test_gaps), always carrying the reference-presence
    /// caveat.
    ///
    /// # Errors
    /// Returns an error on a structural failure.
    pub fn doc_gaps(&self, limit: Option<u32>, reconcile: bool) -> Result<DocGapsReport> {
        crate::observability::traced("doc_gaps", || {
            crate::governance::doc_gaps(self, limit, reconcile)
        })
    }

    /// ARCHITECTURE health check — DB presence/size, schema version, FTS
    /// coherence, structural integrity, the [FR-GV-20] admission tripwire, and
    /// graph counts. For INDEX freshness see [`status`](Self::status).
    ///
    /// [FR-GV-20]: ../../../docs/specs/requirements/FR-GV-20.md
    ///
    /// # Errors
    /// Returns an error on a structural failure (an FTS desync is *reported*
    /// in the read-model, not an error — `health` exists to diagnose it).
    pub fn health(&self, reconcile: bool) -> Result<HealthInfo> {
        crate::observability::traced("health", || crate::governance::health(self, reconcile))
    }

    /// The fast **structural-integrity + admission-tripwire** check (CR-052,
    /// [FR-GV-18]; S-215, [FR-GV-20]; [NFR-RA-13], [ADR-46]/[ADR-48]): asserts
    /// one node per `symbol_id` and zero orphan rows, and flags every indexed
    /// file the *current* admission rules would reject — in O(a handful of
    /// indexed queries) plus O(files) matcher work, no reindex — returning the
    /// `doctor` verdict. A pure read of the persisted graph — it does not
    /// reconcile. The CLI exits 1 on `!report.ok`, and the same verdict
    /// hard-fails `session_end` / `check_rules`.
    ///
    /// # Errors
    /// Returns an error on a structural failure (a missing graph runtime).
    ///
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    /// [FR-GV-20]: ../../../docs/specs/requirements/FR-GV-20.md
    /// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
    /// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
    /// [ADR-48]: ../../../docs/specs/architecture/decisions/ADR-48.md
    pub fn doctor(&self) -> Result<DoctorReport> {
        crate::observability::traced("doctor", || crate::governance::doctor(self))
    }

    /// The on-demand **deep** consistency check (CR-052, [FR-GV-19], [NFR-RA-06],
    /// [ADR-46]): reindex the project into a throwaway shadow store via the
    /// always-purge [`index`](Self::index) path and diff node/edge/file counts +
    /// symbol sets against the live graph, reporting drift with a capped sample of
    /// leaked/orphaned symbols and embedding the fast structural + admission
    /// check ([FR-GV-18], [FR-GV-20]). Catches the Channel-B orphans (files the
    /// live store retains but a fresh index drops) the fast `doctor` cannot see.
    ///
    /// The live store is opened read-only for the census and never mutated; the
    /// shadow store is torn down (db + `-wal`/`-shm`) on completion. On-demand
    /// only — a full reindex is seconds-to-minutes, so `verify` is never on a hot
    /// path. The CLI exits 1 on `!report.ok`.
    ///
    /// # Errors
    /// Returns an error on a transient engine, an unloadable plugin registry, or a
    /// failed shadow reindex / read.
    ///
    /// [FR-GV-19]: ../../../docs/specs/requirements/FR-GV-19.md
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
    pub fn verify(&self) -> Result<VerifyReport> {
        crate::observability::traced("verify", || crate::governance::verify(self))
    }

    /// Begin a quality session (FR-GV-04): the MCP spelling of
    /// `gate --save` — compute a fresh snapshot and upsert it as the
    /// baseline `session_end` compares against.
    ///
    /// # Errors
    /// Returns an error on a structural failure.
    pub fn session_start(&self) -> Result<SessionInfo> {
        crate::observability::traced("session_start", || crate::governance::session_start(self))
    }

    /// End the quality session (FR-GV-05): re-score and compare to the
    /// baseline — the MCP spelling of a bare `gate`.
    ///
    /// # Errors
    /// Returns an error on a structural failure.
    pub fn session_end(&self) -> Result<GateResult> {
        crate::observability::traced("session_end", || {
            crate::governance::gate(self, None, false, true)
        })
    }

    // ── Observability ─────────────────────────────────────────────────────

    /// Aggregated usage/perf stats from `telemetry.db` over a trailing window
    /// of `window_days` (default 7, [FR-OB-04]): per-tool/surface call counts,
    /// latency p50/p95/p99, and the estimated reads + tokens saved by
    /// navigation — the dogfood metric ([NFR-OO-03]), honestly an estimate.
    ///
    /// Works on both engine flavours (it reads `telemetry.db` directly, not
    /// the graph runtime). A project with no telemetry yet degrades to a
    /// zeroed read-model carrying the reason in `warnings` (ADR-14 posture).
    ///
    /// [FR-OB-04]: ../../../docs/specs/requirements/FR-OB-04.md
    /// [NFR-OO-03]: ../../../docs/specs/requirements/NFR-OO-03.md
    pub fn stats(&self, window_days: Option<u32>) -> StatsInfo {
        let mut info = crate::observability::traced("stats", || {
            crate::observability::stats(&self.root, window_days)
        })
        .unwrap_or_else(|err| {
            tracing::warn!("stats failed: {err:#}");
            StatsInfo {
                warnings: vec![format!("stats failed: {err}")],
                ..StatsInfo::default()
            }
        });
        // Per-relation-class cross-artifact binding counts (CR-011, FR-OB-04,
        // FR-CG-11) are a live-graph property, not telemetry: read the ledger
        // best-effort from `logos.db` and merge. A missing or unreadable graph
        // leaves the map empty — the stats surface stays infallible (ADR-14).
        match self.artifact_binding_counts() {
            Ok(by_relation) => info.artifact_bindings = by_relation,
            Err(err) => tracing::debug!("artifact binding counts unavailable: {err:#}"),
        }
        info
    }

    /// Per-relation-class cross-artifact binding counts read from the live graph
    /// ledger (CR-011, [FR-CG-11]) — the `stats` surface's artifact half.
    ///
    /// Works on both engine flavours by opening `logos.db` read-only (like
    /// [`stats`](Engine::stats) reads `telemetry.db`), independent of the graph
    /// runtime. An un-indexed project (no `logos.db`) yields an empty map, not an
    /// error.
    ///
    /// # Errors
    /// Returns an error only if an existing store is unreadable/corrupt; the
    /// caller degrades that to an empty map.
    ///
    /// [FR-CG-11]: ../../../docs/specs/requirements/FR-CG-11.md
    fn artifact_binding_counts(
        &self,
    ) -> Result<std::collections::BTreeMap<String, crate::models::pipeline::RelationCoverage>> {
        let db_path = self.root.join(".logos").join("logos.db");
        if !db_path.is_file() {
            return Ok(std::collections::BTreeMap::new());
        }
        let store = crate::graph_store::SqliteGraphStore::open_readonly(&db_path)?;
        Ok(crate::resolve::coverage(&store)?.by_relation)
    }

    /// Registered languages from the plugin substrate (FR-PL-06).
    ///
    /// A long-lived [`Engine::start`] engine maps from the
    /// [`LanguageRegistry`](crate::plugin::LanguageRegistry) cached at startup
    /// (ADR-04, NFR-PE-05) — no per-call descriptor parse / query recompile. A
    /// transient [`Engine::open`] engine has no cached registry, so it loads one
    /// on demand (resolving any on-disk query overrides under
    /// `<root>/.logos/plugins/`).
    ///
    /// A load failure (a malformed descriptor or a query that fails to compile,
    /// typically in a user-supplied override) is reported to stderr naming the
    /// file (FR-PL-02) and yields an empty listing rather than a panic, since the
    /// Engine surface is infallible until error types land (ADR-14).
    pub fn languages(&self) -> LanguagesInfo {
        use crate::plugin::LanguageRegistry;

        crate::observability::traced_infallible("languages", || {
            // Prefer the cached registry (the started-engine path); fall back
            // to a fresh load for a transient engine that holds none.
            if let Some(registry) = self.registry.as_ref() {
                return Self::languages_from(registry);
            }

            match LanguageRegistry::load(&self.root) {
                Ok(registry) => Self::languages_from(&registry),
                Err(err) => {
                    tracing::warn!("could not load plugin registry: {err}");
                    LanguagesInfo::default()
                }
            }
        })
    }

    // ── Internal helpers (private) ────────────────────────────────────────

    /// Map a loaded [`LanguageRegistry`](crate::plugin::LanguageRegistry) into the
    /// `languages` read-model — the loaded grammars plus any ABI-skipped ones.
    fn languages_from(registry: &crate::plugin::LanguageRegistry) -> LanguagesInfo {
        let languages = registry
            .iter()
            .map(|plugin| {
                let semantics = plugin.semantics();
                LanguageDescriptor {
                    name: plugin.name().to_string(),
                    extensions: plugin.extensions().to_vec(),
                    // Artifact plugins (CR-010) claim basenames and are flagged as
                    // the third class, so `logos languages` lists their filename
                    // claims alongside extensions ([FR-PL-06], [FR-CG-04]).
                    filenames: plugin.filenames().to_vec(),
                    artifact: plugin.is_artifact(),
                    module_separator: semantics.module_separator.clone(),
                    capabilities: plugin.capabilities().to_vec(),
                    abi_version: semantics.abi_version as u32,
                    overridden_capabilities: plugin.overridden_capabilities().to_vec(),
                }
            })
            .collect();

        let skipped = registry
            .skipped()
            .iter()
            .map(|s| SkippedLanguage {
                name: s.name.clone(),
                reason: s.reason.to_string(),
            })
            .collect();

        LanguagesInfo { languages, skipped }
    }

    /// The resolved worktree root (ADR-15).
    ///
    /// Public so the `ui`-gated chat surface ([web-surface]/[chat-agent], S-170)
    /// can resolve the project-root paths it needs beside the engine — the
    /// gitignored `.logos/chat.db` memory/conversation store and the source-tool
    /// [`Sandbox`](../../../docs/specs/architecture/components/chat-agent.md) root.
    /// It exposes a path the engine already resolved; it is **not** a graph or
    /// governance query (the agent adds none, [ADR-41]) and writes nothing.
    ///
    /// [web-surface]: ../../../docs/specs/architecture/components/web-surface.md
    /// [chat-agent]: ../../../docs/specs/architecture/components/chat-agent.md
    /// [ADR-41]: ../../../docs/specs/architecture/decisions/ADR-41.md
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The **lazy** git-history mining trigger ([FR-GH-02], [ADR-22]): resolve
    /// the effective `[history]` config, then mine `history.db` incrementally.
    ///
    /// This is the seam the temporal surfaces ([S-048]) call on a read — and the
    /// *only* path that mines. It is deliberately **absent** from
    /// `gate`/`session_start`/`session_end` ([Self::gate]), `sync` ([Self::sync]),
    /// and every navigation method, so those paths run no history mining and no
    /// git subprocess beyond the existing HEAD tag ([BR-26], [FR-GH-02]
    /// acceptance).
    ///
    /// It is `pub` — the api-facade-facing library seam, returning the
    /// [`MineOutcome`](crate::history::MineOutcome) `Serialize` read-model per the
    /// [ADR-01] Engine convention. It is **not** an MCP tool: tools are
    /// registered explicitly in the `mcp` crate, so the `logos:*` tool-set is
    /// unchanged until [S-048] wires the surface.
    ///
    /// # Errors
    /// Returns an error only on an unexpected git/store failure or invalid
    /// `rules.toml`; the expected degraded states ride inside the
    /// [`MineOutcome`](crate::history::MineOutcome) ([NFR-RA-05]).
    ///
    /// [FR-GH-02]: ../../../docs/specs/requirements/FR-GH-02.md
    /// [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
    /// [ADR-22]: ../../../docs/specs/architecture/decisions/ADR-22.md
    /// [BR-26]: ../../../docs/specs/software-spec.md#322-git-history-analytics
    /// [S-048]: ../../../docs/planning/journal.md#s-048-hotspot-ranking-and-temporal-reporting-surfaces
    pub fn ensure_history_mined(&self) -> Result<crate::history::MineOutcome> {
        let rules = crate::config::load_rules_from_root(&self.root)?;
        crate::history::mine(&self.root, &rules.history.effective())
    }

    /// The temporal-tier read seam ([FR-GH-03]..[FR-GH-05], [FR-GH-09]): lazily
    /// mine (via the same [`ensure_history_mined`](Self::ensure_history_mined)
    /// trigger), then compute the per-file temporal metrics and append a snapshot
    /// row, returning the [`TemporalReport`](crate::history::TemporalReport)
    /// `Serialize` read-model ([ADR-01]).
    ///
    /// Like `ensure_history_mined` this is `pub` (the api-facade-facing library
    /// seam the hotspot surface [S-048] ranks from) and deliberately **off** the
    /// gate/sync/navigation paths ([BR-26]). Output is a pure function of the tree
    /// at HEAD + the effective `[history]` config ([BR-27], [NFR-RA-06]); a
    /// degraded repository rides inside the report as `n/a`, never an error
    /// ([NFR-RA-05]). The `logos:*` MCP tool-set is unchanged until [S-048] wires
    /// the surface.
    ///
    /// # Errors
    /// Returns an error only on an unexpected git/store failure, an invalid
    /// `rules.toml`, or a non-compiling `[history] defect_patterns`.
    ///
    /// [FR-GH-03]: ../../../docs/specs/requirements/FR-GH-03.md
    /// [FR-GH-05]: ../../../docs/specs/requirements/FR-GH-05.md
    /// [FR-GH-09]: ../../../docs/specs/requirements/FR-GH-09.md
    /// [BR-27]: ../../../docs/specs/software-spec.md#322-git-history-analytics
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [S-048]: ../../../docs/planning/journal.md#s-048-hotspot-ranking-and-temporal-reporting-surfaces
    pub fn temporal_report(&self) -> Result<crate::history::TemporalReport> {
        let rules = crate::config::load_rules_from_root(&self.root)?;
        crate::history::temporal_report(&self.root, &rules.history.effective())
    }

    /// The hotspot surface ([FR-GH-06]): rank indexed files by
    /// `churn rank × structural-complexity rank` — a **Rust-side join** of the
    /// temporal tier's churn (`history.db`, [`temporal_report`](Self::temporal_report))
    /// against per-file aggregated cyclomatic complexity (`logos.db`), never an
    /// SQL `ATTACH` across the two stores ([ADR-22], [BR-26]). `limit` caps the
    /// returned board (`logos hotspots --limit N`).
    ///
    /// This is the single `api` entrypoint behind both the CLI `hotspots`
    /// subcommand and the MCP `hotspots` tool ([NFR-CC-01]) — they share its
    /// `Serialize` read-model byte-for-byte ([FR-GH-06] CLI/MCP parity). Like
    /// [`temporal_report`](Self::temporal_report) it is lazy and **off** the
    /// gate/sync/navigation paths ([BR-26]); a degraded repository rides inside
    /// the report as `n/a` + notice, never an error ([FR-GH-08], [NFR-RA-05]).
    ///
    /// `production_scope = true` applies the optional production-scope filter
    /// ([CR-076]): whole test files (`is_test`-only, [FR-AN-05]) are dropped
    /// from the candidate set before ranking. `false` (the default) is
    /// byte-identical to the pre-[CR-076] whole-repo board — the filter is
    /// opt-in and never moves the gate ([BR-26]).
    ///
    /// # Errors
    /// Returns an error only on an unexpected git/store failure, an invalid
    /// `rules.toml`, or a non-compiling `[history] defect_patterns`.
    ///
    /// [FR-GH-06]: ../../../docs/specs/requirements/FR-GH-06.md
    /// [FR-GH-08]: ../../../docs/specs/requirements/FR-GH-08.md
    /// [ADR-22]: ../../../docs/specs/architecture/decisions/ADR-22.md
    /// [BR-26]: ../../../docs/specs/software-spec.md#322-git-history-analytics
    /// [NFR-CC-01]: ../../../docs/specs/requirements/NFR-CC-01.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [CR-076]: ../../../docs/requests/CR-076-hotspots-production-scope-filter.md
    pub fn hotspots(
        &self,
        limit: Option<usize>,
        untested: bool,
        production_scope: bool,
    ) -> Result<crate::history::HotspotReport> {
        crate::observability::traced("hotspots", || {
            // Churn axis: the temporal tier (mines lazily, computes per file).
            let temporal = self.temporal_report()?;
            self.rank_hotspots(temporal, limit, untested, production_scope)
        })
    }

    /// Join a temporal report with the complexity and coverage axes into the
    /// ranked [`HotspotReport`](crate::history::HotspotReport). The complexity
    /// (`logos.db`) and coverage (`history.db`) reads are pure point queries —
    /// this is the shared, **read-only** core of both [`hotspots`](Self::hotspots)
    /// (which mines first) and [`latest_hotspots`](Self::latest_hotspots) (which
    /// reads the last-mined facts). Never an `ATTACH` across the two stores
    /// ([ADR-22], [BR-26], [BR-28]).
    ///
    /// `production_scope` gates the optional whole-test-file exclusion
    /// ([CR-076](../../../docs/requests/CR-076-hotspots-production-scope-filter.md)):
    /// the `is_test` verdict ([FR-AN-05]) is read from the same `nodes`/
    /// `functions` rows already fetched for the complexity axis, so enabling
    /// the filter costs one extra point query (`test_node_ids`), never a new
    /// store read shape.
    fn rank_hotspots(
        &self,
        temporal: crate::history::TemporalReport,
        limit: Option<usize>,
        untested: bool,
        production_scope: bool,
    ) -> Result<crate::history::HotspotReport> {
        // Complexity axis: per-file Σ cyclomatic_complexity from logos.db,
        // read in Rust — joined here, never via a cross-DB ATTACH (BR-26).
        // A transient engine with no graph runtime contributes no
        // complexity, so every file is honestly excluded (NFR-RA-05). The
        // same rows feed the production-scope candidate-set filter (CR-076).
        let (complexity, test_only_files) = match self.runtime() {
            Some(rt) => {
                let (nodes, functions, test_node_ids) = rt.submit_read(|store| {
                    Ok((
                        store.all_nodes()?,
                        store.function_metrics()?,
                        store.test_node_ids()?,
                    ))
                })?;
                let complexity = crate::history::aggregate_complexity(&nodes, &functions);
                let test_ids: std::collections::HashSet<_> = test_node_ids.into_iter().collect();
                let test_only_files = crate::history::test_only_files(&nodes, &functions, &test_ids);
                (complexity, test_only_files)
            }
            None => (
                std::collections::BTreeMap::new(),
                std::collections::BTreeSet::new(),
            ),
        };
        // Coverage axis ([FR-CV-07]): the latest snapshot, freshness-resolved
        // against the current tree. `None` = no coverage ingested → the
        // labeled static-reachability fallback ([FR-GV-08]). Read from
        // `history.db` only — never an `ATTACH` to `logos.db` ([BR-28]).
        let coverage = crate::history::coverage::read_latest(&self.root)?.map(|view| {
            view.files
                .iter()
                .map(|f| {
                    (
                        f.path.clone(),
                        crate::history::FileCoverage {
                            fresh: f.fresh,
                            coverage_bp: f.coverage_bp().unwrap_or(0),
                        },
                    )
                })
                .collect()
        });
        Ok(crate::history::rank(
            temporal,
            &complexity,
            coverage.as_ref(),
            limit,
            untested,
            production_scope,
            &test_only_files,
        ))
    }

    // ── Read-only read-model accessors (S-082, CR-018, ADR-28) ──────────────
    //
    // The non-persisting twins of the evaluate-and-persist `scan`/`gate`/
    // `hotspots` paths, consumed **only** by the [web-surface] Health, Overview,
    // Metrics, Hotspots, and Commits views so a dashboard GET reflects the last
    // persisted snapshot and never triggers an evaluate-and-persist write
    // ([FR-UI-03], [ADR-28]). The CLI/MCP `scan`/`gate`/`hotspots` methods above
    // are unchanged — they keep persisting on purpose.
    //
    // [web-surface]: ../../../docs/specs/architecture/components/web-surface.md
    // [FR-UI-03]: ../../../docs/specs/requirements/FR-UI-03.md
    // [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md

    /// The most-recent **persisted** metric snapshot, or `None` on a
    /// never-`scan`-ned store — a pure read that computes and persists nothing
    /// ([ADR-28]). Backs the dashboard's Metrics/Health metric breakdown; on
    /// `None` the view renders the honest "run `logos scan`" empty state
    /// ([NFR-CC-04]).
    ///
    /// # Errors
    /// Returns an error only on a transient engine (no runtime) or a store-read
    /// failure — never from compute (there is none).
    pub fn latest_metrics(&self) -> Result<Option<MetricSnapshot>> {
        crate::observability::traced("latest_metrics", || {
            crate::governance::latest_metrics(self)
        })
    }

    /// The read-only twin of [`scan`](Self::scan): the last persisted snapshot's
    /// metric breakdown plus the read-only temporal tier, or `None` when no
    /// snapshot exists ([ADR-28]). Reads the last row — no reconcile, no score,
    /// no persist; carries no worst-offenders/violations (review-phase detail the
    /// metric-bearing `scan` computes fresh, off the read-only dashboard path).
    ///
    /// # Errors
    /// Returns an error only on a transient engine or a store-read failure.
    pub fn latest_scan(&self) -> Result<ScanResult> {
        crate::observability::traced("latest_scan", || crate::governance::latest_scan(self))
    }

    /// The read-only gate **verdict** ([ADR-28]): compare the last persisted
    /// snapshot to the saved baseline without computing or persisting one. Backs
    /// the dashboard's Health verdict band; mirrors [`gate`](Self::gate)'s
    /// comparison (BR-10) but never re-baselines or writes.
    ///
    /// # Errors
    /// Returns an error only on a transient engine or a store-read failure.
    pub fn latest_gate(&self) -> Result<GateResult> {
        crate::observability::traced("latest_gate", || crate::governance::latest_gate(self))
    }

    /// The read-only twin of [`temporal_report`](Self::temporal_report): recompute
    /// the per-file temporal metrics from the **already-mined** facts at the
    /// stored cursor, **without mining and without appending a snapshot**
    /// ([ADR-28], [CR-018]). Backs the dashboard's Commits view (and the Health
    /// temporal tier) so a GET reflects the last `logos hotspots`/`scan` mine.
    ///
    /// # Errors
    /// Returns an error only on an unexpected store-read failure or a
    /// non-compiling `[history] defect_patterns`.
    ///
    /// [CR-018]: ../../../docs/requests/CR-018-web-dashboard-write-on-read.md
    pub fn latest_temporal_report(&self) -> Result<crate::history::TemporalReport> {
        crate::observability::traced("latest_temporal_report", || {
            let rules = crate::config::load_rules_from_root(&self.root)?;
            crate::history::latest_temporal_report(&self.root, &rules.history.effective())
        })
    }

    /// The read-only twin of [`hotspots`](Self::hotspots): rank the
    /// last-mined temporal facts (via [`latest_temporal_report`](Self::latest_temporal_report))
    /// against the complexity and coverage axes, **without** mining or appending
    /// a snapshot ([ADR-28], [CR-018]). Backs the dashboard's Hotspots view (and
    /// the Coverage view's untested-hotspots join) so a GET never writes. Shares
    /// the optional `production_scope` filter ([CR-076]) with [`hotspots`](Self::hotspots).
    ///
    /// # Errors
    /// Returns an error only on an unexpected git/store failure or a
    /// non-compiling `[history] defect_patterns`.
    ///
    /// [CR-076]: ../../../docs/requests/CR-076-hotspots-production-scope-filter.md
    pub fn latest_hotspots(
        &self,
        limit: Option<usize>,
        untested: bool,
        production_scope: bool,
    ) -> Result<crate::history::HotspotReport> {
        crate::observability::traced("latest_hotspots", || {
            let temporal = self.latest_temporal_report()?;
            self.rank_hotspots(temporal, limit, untested, production_scope)
        })
    }

    /// The per-project **language composition** ([FR-UI-10], [CR-021]): the
    /// languages actually present in the indexed graph, each with its graph
    /// node/symbol count and contributing-file count, in deterministic order
    /// (node count descending, then language ascending — [NFR-RA-06]). Distinct
    /// from [`languages`](Self::languages), which lists every *registered*
    /// grammar regardless of project use — here a registered-but-unused grammar
    /// is absent. Backs the Dashboard's project-only Languages card ([FR-UI-09]).
    ///
    /// A pure reader that computes-and-persists nothing ([ADR-28], [FR-UI-03]):
    /// it opens `logos.db` read-only — so, like [`latest_*`](Self::latest_metrics)
    /// it works on both engine flavours and never touches the writer — and an
    /// un-indexed root (no `logos.db`) returns an empty composition the view
    /// renders as its honest empty state ([NFR-CC-04]).
    ///
    /// # Errors
    /// Returns an error only when an existing `logos.db` is unreadable or at the
    /// wrong schema version; an absent store is the empty (un-indexed) case, not
    /// an error.
    ///
    /// [FR-UI-10]: ../../../docs/specs/requirements/FR-UI-10.md
    /// [FR-UI-09]: ../../../docs/specs/requirements/FR-UI-09.md
    /// [FR-UI-03]: ../../../docs/specs/requirements/FR-UI-03.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    /// [CR-021]: ../../../docs/requests/CR-021-dashboard-redesign-quality-coverage-rollups.md
    pub fn language_composition(&self) -> Result<LanguageComposition> {
        crate::observability::traced("language_composition", || {
            let db_path = self.root.join(".logos").join("logos.db");
            if !db_path.is_file() {
                // Un-indexed root: an empty composition, not an error ([FR-UI-10]).
                return Ok(LanguageComposition::default());
            }
            let store = crate::graph_store::SqliteGraphStore::open_readonly(&db_path)?;
            use crate::graph_store::GraphStore;
            Ok(LanguageComposition {
                languages: store.language_composition()?,
            })
        })
    }

    /// Ingest an external coverage report into the evidence store ([FR-CV-01]..
    /// [FR-CV-04]): the `logos coverage ingest <report>` subcommand and the MCP
    /// `coverage_ingest` twin behind this one `api` entrypoint ([NFR-CC-01]).
    ///
    /// `format` is the `--format` flag (`"lcov"`/`"cobertura"`); `None`
    /// auto-detects from content. Supplies the indexed-file set the report-path
    /// mapper binds against from a `logos.db` graph read **here at the `api`
    /// layer** — the evidence store is never `ATTACH`-ed to the canonical graph
    /// ([BR-28]). Coverage is advisory: this never moves the gate ([BR-28]).
    ///
    /// # Errors
    /// Returns an error (mapping to a non-zero exit) when `--format` is an
    /// unknown token, the report is unreadable / unrecognized / malformed (atomic
    /// rejection, no partial write), `root` has no resolvable HEAD, or
    /// `rules.toml` is invalid. Per-file outcomes (unmatched, stale-rejected,
    /// idempotent no-op) ride inside the returned summary, never errors.
    ///
    /// [FR-CV-01]: ../../../docs/specs/requirements/FR-CV-01.md
    /// [BR-28]: ../../../docs/specs/software-spec.md#323-coverage-test-evidence
    /// [NFR-CC-01]: ../../../docs/specs/requirements/NFR-CC-01.md
    pub fn coverage_ingest(
        &self,
        report_path: &Path,
        format: Option<&str>,
    ) -> Result<crate::history::IngestSummary> {
        crate::observability::traced("coverage_ingest", || {
            let format_override = Self::parse_coverage_format(format)?;
            // One `config.toml` read resolves the `[coverage_ingest]` table that
            // both the format default and the provenance hash derive from — never
            // two reads that could disagree if the file changes mid-call.
            let ingest_cfg = crate::config::load_config_from_root(&self.root)?
                .coverage_ingest
                .effective();
            self.coverage_ingest_with(report_path, format_override, &ingest_cfg)
        })
    }

    /// Resolve a `--format`-style token (`"lcov"`/`"cobertura"`) into a
    /// [`CoverageFormat`](crate::history::CoverageFormat); `None` means content
    /// auto-detection. Shared by the manual, automatic, and refresh ingest paths so
    /// the unknown-token error is identical across all three ([NFR-CC-01]).
    ///
    /// # Errors
    /// Errors on an unrecognized token.
    fn parse_coverage_format(
        token: Option<&str>,
    ) -> Result<Option<crate::history::CoverageFormat>> {
        match token {
            Some(token) => Ok(Some(crate::history::CoverageFormat::from_flag(token).ok_or_else(
                || anyhow!("unknown coverage --format {token:?} (expected lcov or cobertura)"),
            )?)),
            None => Ok(None),
        }
    }

    /// The shared ingest body behind both [`Engine::coverage_ingest`] and
    /// [`Engine::coverage_ingest_auto`]: it takes an already-resolved
    /// [`EffectiveCoverageIngest`](crate::config::EffectiveCoverageIngest) so a
    /// single `config.toml` read drives both the format selection and the snapshot
    /// provenance hash (no time-of-check/time-of-use skew, [FR-CV-09]). Not
    /// `traced` itself — each public caller owns the span, so an auto-ingest emits
    /// exactly one telemetry record, not a nested pair.
    ///
    /// The `[coverage]` table ([FR-CV-09], `rules.toml`) and `[coverage_ingest]`
    /// table (`config.toml`) are folded together into the snapshot's config hash
    /// ([ADR-38]).
    fn coverage_ingest_with(
        &self,
        report_path: &Path,
        format_override: Option<crate::history::CoverageFormat>,
        ingest_cfg: &crate::config::EffectiveCoverageIngest,
    ) -> Result<crate::history::IngestSummary> {
        let rules = crate::config::load_rules_from_root(&self.root)?;
        let cfg = rules.coverage.effective();
        // The indexed-file set the mapper binds against, supplied at the api
        // layer from a graph read ([BR-28]); a transient/empty graph yields an
        // empty set, so every report path is honestly reported unmatched.
        let indexed_paths: Vec<String> = match self.runtime() {
            Some(rt) => rt
                .submit_read(|store| store.indexed_files())?
                .into_iter()
                .map(|f| f.path)
                .collect(),
            None => Vec::new(),
        };
        crate::history::coverage::ingest(
            &self.root,
            report_path,
            format_override,
            &cfg,
            ingest_cfg,
            &indexed_paths,
        )
    }

    /// Watcher-driven **automatic** coverage ingest ([FR-CV-10], [ADR-38]): ingest
    /// the coverage `artifact` the watcher observed change, through the same path
    /// as [`Engine::coverage_ingest`] — a local file read + parse + store, **never**
    /// a test run ([NFR-SE-01]). The `[coverage_ingest].format` override is honored
    /// (default `auto` content-detection).
    ///
    /// This is **non-load-bearing** ([ADR-11], [FR-SY-06]): the watcher treats any
    /// error (a half-written artifact mid-CI-run, a malformed report, a missing
    /// HEAD) as a warning and never blocks the sync. Reusing the manual ingest path
    /// keeps a single atomic-rejection contract — a bad artifact leaves the store
    /// byte-identical ([NFR-RA-05]).
    ///
    /// # Errors
    /// The same loud errors as [`Engine::coverage_ingest`] (unreadable/unrecognized/
    /// malformed report, no resolvable HEAD, invalid config). The watcher degrades
    /// them to a warning at the call site.
    ///
    /// [FR-CV-10]: ../../../docs/specs/requirements/FR-CV-10.md
    /// [ADR-38]: ../../../docs/specs/architecture/decisions/ADR-38.md
    /// [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
    /// [FR-SY-06]: ../../../docs/specs/requirements/FR-SY-06.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    pub fn coverage_ingest_auto(&self, artifact: &Path) -> Result<crate::history::IngestSummary> {
        crate::observability::traced("coverage_ingest_auto", || {
            // One config read drives both the configured format and the snapshot
            // hash — the same `ingest_cfg` instance flows into both, so the parse
            // format and the recorded provenance can never disagree.
            let ingest_cfg = crate::config::load_config_from_root(&self.root)?
                .coverage_ingest
                .effective();
            let format_override = Self::parse_coverage_format(ingest_cfg.format_override())?;
            self.coverage_ingest_with(artifact, format_override, &ingest_cfg)
        })
    }

    /// The opt-in `logos coverage refresh` / `coverage_refresh` ([FR-CV-10],
    /// [ADR-38]): run the configured `[coverage_ingest].refresh_cmd` as an
    /// explicit, user-invoked subprocess (via `sh -c`, cwd = project root), then
    /// ingest the artifact it produced. This is the **single** place Logos ever
    /// spawns a coverage subprocess, and only on explicit invocation — never on the
    /// serve/watcher path ([ADR-38]'s load-bearing boundary, [NFR-SE-01]).
    ///
    /// After the command succeeds, the produced artifact is discovered among the
    /// built-in conventions and any literal configured `artifact_glob` (newest
    /// existing file) and ingested through [`Engine::coverage_ingest`].
    ///
    /// # Errors
    /// Returns an error (non-zero exit) when no `refresh_cmd` is configured, the
    /// command cannot be spawned or exits non-zero, no recognizable artifact exists
    /// after it ran, or the ensuing ingest fails.
    ///
    /// [FR-CV-10]: ../../../docs/specs/requirements/FR-CV-10.md
    /// [ADR-38]: ../../../docs/specs/architecture/decisions/ADR-38.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    pub fn coverage_refresh(&self) -> Result<crate::history::CoverageRefreshSummary> {
        crate::observability::traced("coverage_refresh", || {
            let config = crate::config::load_config_from_root(&self.root)?;
            let ingest_cfg = config.coverage_ingest.effective();
            let command = ingest_cfg.refresh_cmd.clone().ok_or_else(|| {
                anyhow!(
                    "no [coverage_ingest].refresh_cmd configured in .logos/config.toml — \
                     set it (e.g. refresh_cmd = \"cargo llvm-cov --cobertura --output-path \
                     target/llvm-cov/cobertura.xml\") or run `logos coverage ingest <report>`"
                )
            })?;

            // The lone explicit coverage subprocess (off the serve/watcher path,
            // [ADR-38]). `sh -c` runs the free-form command rooted at the project.
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(&command)
                .current_dir(&self.root)
                .output()
                .with_context(|| format!("running coverage refresh command: {command}"))?;
            if !output.status.success() {
                // Surface BOTH streams: coverage tools (cargo-llvm-cov, pytest-cov,
                // nyc) often write their primary error to stdout, not stderr, so a
                // stderr-only diagnostic would drop the actual cause.
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                let diagnostic = match (stderr.trim(), stdout.trim()) {
                    ("", out) => out.to_string(),
                    (err, "") => err.to_string(),
                    (err, out) => format!("stderr: {err}\nstdout: {out}"),
                };
                return Err(anyhow!(
                    "coverage refresh command failed (exit {}): {}\n{}",
                    output
                        .status
                        .code()
                        .map_or_else(|| "signal".to_string(), |c| c.to_string()),
                    command,
                    diagnostic
                ));
            }

            let artifact = crate::history::coverage::discover_artifact(&self.root, &ingest_cfg)
                .ok_or_else(|| {
                    anyhow!(
                        "`{command}` ran successfully but produced no recognizable coverage \
                         artifact in a conventional or configured location — check the command's \
                         output path or set [coverage_ingest].artifact_glob"
                    )
                })?;
            // Reuse the already-loaded `ingest_cfg` (one config read for the whole
            // refresh) rather than re-reading config through the public entrypoint.
            let format_override = Self::parse_coverage_format(ingest_cfg.format_override())?;
            let ingest = self.coverage_ingest_with(&artifact, format_override, &ingest_cfg)?;
            let artifact_rel = artifact
                .strip_prefix(&self.root)
                .unwrap_or(&artifact)
                .to_string_lossy()
                .into_owned();
            Ok(crate::history::CoverageRefreshSummary {
                command,
                artifact: artifact_rel,
                ingest,
            })
        })
    }

    /// The coverage status surface ([FR-CV-05], [FR-CV-06]): per-file freshness
    /// (fresh value / stale label / `n/a`, hash-based against the snapshot anchor)
    /// plus the overall freshness fraction and snapshot provenance. The
    /// `logos coverage status` subcommand and the MCP `coverage_status` twin share
    /// this one `api` entrypoint byte-for-byte ([NFR-CC-01]).
    ///
    /// With no coverage ingested, returns an `n/a` status carrying a one-line
    /// notice — never an error, so the surface exits 0 ([FR-CV-06]). Reads only
    /// `history.db` ([BR-28]); advisory, never moves the gate.
    ///
    /// # Errors
    /// Returns an error only on an unexpected store failure.
    ///
    /// [FR-CV-05]: ../../../docs/specs/requirements/FR-CV-05.md
    /// [FR-CV-06]: ../../../docs/specs/requirements/FR-CV-06.md
    pub fn coverage_status(&self) -> Result<crate::history::CoverageStatus> {
        crate::observability::traced("coverage_status", || {
            crate::history::coverage::status(&self.root)
        })
    }

    /// The symbol-level **reachability x runtime-coverage cross** read-model
    /// ([FR-UI-17], [CR-036]): per non-test function/method, the pair
    /// `(reachable_from_test, runtime_exec_fraction)` and a Q1-Q4 classification --
    /// the engine-side backing of the `/quadrant` view and the Dashboard
    /// trust-score card so the web stays presentation-only ([FR-UI-03], [ADR-28]).
    ///
    /// The two axes are joined **here at the `api` layer**, mirroring
    /// [`coverage_ingest`](Self::coverage_ingest)'s `indexed_paths`: the symbol
    /// spans come from a `logos.db` graph read and reachability from the
    /// governance `test_gaps` BFS ([FR-GV-08]), both handed to the history-engine
    /// as plain data so it never links the governance-engine -- the
    /// history -> governance boundary stays one-directional ([UAT-GH-02]). The
    /// reachability set is the SAME `test_reachable_set` core `test_gaps` uses, so
    /// the two surfaces can never disagree about what a test reaches.
    ///
    /// Read-side only: it reads `history.db` (+ graph spans) and persists nothing
    /// ([ADR-28]); the gated metric path never calls it, so `gate`/`scan` are
    /// byte-identical with or without it ([BR-28]). A symbol with an unresolvable
    /// span or no fresh coverage is `n/a` on the runtime axis -- never a guessed
    /// fraction ([NFR-RA-05]). Deterministic at a fixed HEAD + store state
    /// ([NFR-RA-06]). A transient engine (no graph runtime) yields an empty symbol
    /// set, honestly.
    ///
    /// # Errors
    /// Returns an error only on an unexpected graph- or `history.db`-read failure.
    ///
    /// [FR-UI-17]: ../../../docs/specs/requirements/FR-UI-17.md
    /// [FR-GV-08]: ../../../docs/specs/requirements/FR-GV-08.md
    /// [FR-UI-03]: ../../../docs/specs/requirements/FR-UI-03.md
    /// [CR-036]: ../../../docs/requests/CR-036-automatic-coverage-ingest-and-coverage-cross-quadrant.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    /// [BR-28]: ../../../docs/specs/software-spec.md#323-coverage-test-evidence
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [UAT-GH-02]: ../../../docs/specs/requirements/UAT-GH-02.md
    pub fn coverage_cross(&self) -> Result<crate::history::CoverageCrossReport> {
        crate::observability::traced("coverage_cross", || {
            // Graph read at the api layer (the `indexed_paths` precedent): nodes
            // + spans, `Calls` edges, and the persisted `is_test` set. A transient
            // engine has no runtime -> an empty graph, so the cross is honestly
            // empty rather than fabricated ([NFR-RA-05]).
            let (nodes, edges, test_node_ids) = match self.runtime() {
                Some(rt) => rt.submit_read(|store| {
                    Ok((
                        store.all_nodes()?,
                        store.all_edges()?,
                        store.test_node_ids()?,
                    ))
                })?,
                None => (Vec::new(), Vec::new(), Vec::new()),
            };
            let test_ids: std::collections::HashSet<_> = test_node_ids.into_iter().collect();
            // Reachability over `Calls` from every test node -- the SAME core
            // `test_gaps` uses ([FR-GV-08]). Computed here and supplied to the
            // history-engine as a per-symbol boolean ([UAT-GH-02]).
            let reachable = crate::governance::test_reachable_set(&edges, &test_ids);

            // Every non-test function/method (entry points included -- an executed
            // entry point unreached by a test is a meaningful Q3 finding). The
            // runtime axis + classification is the history-engine's job.
            //
            // `test_reachable_set` seeds the BFS from the test nodes, so `reachable`
            // contains the test-node ids themselves; the `!test_ids.contains` filter
            // below drops every test node from the symbol set first, so a test node
            // can never reach the `reachable.contains(&n.id)` check and be emitted as
            // a (spurious) Q1/Q3 symbol. The two filters are load-bearing together.
            let symbols: Vec<crate::history::CrossSymbolInput> = nodes
                .iter()
                .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
                .filter(|n| !test_ids.contains(&n.id))
                .map(|n| crate::history::CrossSymbolInput {
                    symbol: n.symbol.to_scip_string(),
                    name: n.name.clone(),
                    file: n.file_path.clone().unwrap_or_default(),
                    start_line: n.start_line,
                    end_line: n.end_line,
                    reachable_from_test: reachable.contains(&n.id),
                })
                .collect();

            crate::history::coverage_cross(&self.root, symbols)
        })
    }

    // ── Source wiki (S-052, CR-008, ADR-24) ──────────────────────────────────

    /// Upsert a wiki page by slug ([FR-WK-02]): byte-verbatim body (1 MiB cap),
    /// write-time anchor resolution to content hashes, write-time HEAD tag, and a
    /// mandatory non-empty generator label, with loud rejection of an unknown
    /// anchor / empty generator / over-cap body (store left byte-identical).
    ///
    /// Anchors are the `"<kind>:<key>"` wire form — `file:<repo-relative-path>`
    /// or `symbol:<canonical symbol>`. They are resolved against whatever graph
    /// currently exists **without** forcing an index ([nav_runtime_no_prologue]):
    /// file anchors resolve by hashing the file on disk, symbol anchors by
    /// resolving the node in the graph. `wiki.db` is never `ATTACH`-ed to
    /// `logos.db` and never gated ([BR-29]).
    ///
    /// # Errors
    /// Returns an error (the surface maps to a non-zero exit, store unchanged) on
    /// an invalid slug, an empty generator, an over-cap body, or an unknown
    /// anchor; or for a transient [`Engine::open`] engine (no read-only pool).
    ///
    /// [FR-WK-02]: ../../../docs/specs/requirements/FR-WK-02.md
    /// [BR-29]: ../../../docs/specs/software-spec.md#324-source-wiki
    pub fn wiki_write(
        &self,
        slug: &str,
        title: &str,
        body: &str,
        anchors: &[String],
        generator: &str,
    ) -> Result<crate::wiki::WriteSummary> {
        crate::observability::traced("wiki_write", || {
            let runtime = self.nav_runtime_no_prologue()?;
            let head = crate::wiki::head_sha(&self.root).unwrap_or_default();
            let mut conn = crate::wiki::open(&self.root)?;
            runtime.submit_read(|store| {
                // Capture the persisted graph revision the page is built at, so
                // the two-tier view can later derive its freshness with no write
                // on the page view ([FR-WK-12], [FR-SY-09], [ADR-32]).
                let built_at_revision = store.graph_revision()?;
                let draft = crate::wiki::PageDraft {
                    slug,
                    title,
                    body,
                    anchors,
                    generator,
                    written_head: &head,
                    built_at_revision,
                };
                let resolver = crate::wiki::GraphAnchorResolver::new(store, &self.root);
                crate::wiki::write(&mut conn, &resolver, &draft)
            })
        })
    }

    /// Read a wiki page by slug with mandatory provenance and read-time per-anchor
    /// freshness ([FR-WK-03], [FR-WK-04]).
    ///
    /// Freshness is computed against the **current working tree** (no `sync`
    /// required, the [ADR-23] precedent): an edited anchor reads stale, a gone
    /// anchor reads missing. The orphan lifecycle runs here ([FR-WK-07]): a page
    /// whose every anchor is gone is auto-deleted and logged, and this read
    /// returns `Ok(None)`. A page with at least one surviving anchor is returned
    /// with the missing ones flagged.
    ///
    /// # Errors
    /// Returns an error only on an unexpected store failure, or for a transient
    /// [`Engine::open`] engine.
    ///
    /// [FR-WK-03]: ../../../docs/specs/requirements/FR-WK-03.md
    /// [FR-WK-04]: ../../../docs/specs/requirements/FR-WK-04.md
    /// [FR-WK-07]: ../../../docs/specs/requirements/FR-WK-07.md
    /// [ADR-23]: ../../../docs/specs/architecture/decisions/ADR-23.md
    pub fn wiki_read(&self, slug: &str) -> Result<Option<crate::wiki::WikiPage>> {
        crate::observability::traced("wiki_read", || {
            let runtime = self.nav_runtime_no_prologue()?;
            let mut conn = crate::wiki::open(&self.root)?;
            runtime.submit_read(|store| {
                let resolver = crate::wiki::GraphAnchorResolver::new(store, &self.root);
                crate::wiki::read(&mut conn, &resolver, slug)
            })
        })
    }

    /// Explicitly delete a wiki page by slug ([FR-WK-07], CLI-only surface).
    ///
    /// # Errors
    /// Returns an error when no page with that slug exists — a non-zero exit so a
    /// typo'd delete is loud, never a silent no-op.
    pub fn wiki_delete(&self, slug: &str) -> Result<()> {
        crate::observability::traced("wiki_delete", || {
            let conn = crate::wiki::open(&self.root)?;
            crate::wiki::delete(&conn, slug)
        })
    }

    /// The recorded auto-deletions ([FR-WK-07]), newest first — the durable trace
    /// the `wiki status` work-list (S-053) surfaces.
    ///
    /// # Errors
    /// Returns an error only on an unexpected store failure.
    pub fn wiki_pruned_log(&self) -> Result<Vec<crate::wiki::PrunedPage>> {
        crate::observability::traced("wiki_pruned_log", || {
            let conn = crate::wiki::open(&self.root)?;
            crate::wiki::pruned_log(&conn)
        })
    }

    /// FTS5 bm25 search over wiki page titles + bodies, or enumerate every page
    /// in `list` mode ([FR-WK-05]). Each hit carries its staleness flag and
    /// provenance summary, computed against the current tree (no `sync`, the
    /// [ADR-23] precedent). The `logos wiki search` subcommand and the
    /// `wiki_search` MCP twin share this one `api` entrypoint byte-for-byte
    /// ([FR-WK-09], [NFR-CC-01]); a pure read that never prunes.
    ///
    /// The FTS5 index lives inside `wiki.db`, so search survives a full `index`
    /// like the pages it indexes; `wiki.db` is never `ATTACH`-ed to `logos.db`
    /// and never gated ([BR-29]).
    ///
    /// # Errors
    /// Returns an error only on an unexpected store failure, or for a transient
    /// [`Engine::open`] engine.
    ///
    /// [FR-WK-05]: ../../../docs/specs/requirements/FR-WK-05.md
    /// [FR-WK-09]: ../../../docs/specs/requirements/FR-WK-09.md
    /// [ADR-23]: ../../../docs/specs/architecture/decisions/ADR-23.md
    /// [BR-29]: ../../../docs/specs/software-spec.md#324-source-wiki
    pub fn wiki_search(&self, query: &str, list: bool) -> Result<Vec<crate::wiki::WikiHit>> {
        crate::observability::traced("wiki_search", || {
            let runtime = self.nav_runtime_no_prologue()?;
            let conn = crate::wiki::open(&self.root)?;
            runtime.submit_read(|store| {
                // The current persisted graph revision ([FR-SY-09]) each hit's
                // built-at revision is compared against, so the search "State"
                // carries the same "stale — regeneration pending" verdict the
                // page view shows ([FR-WK-12], [CR-039]); `0` (no `index` yet)
                // yields no pending verdict. The exact seam `wiki_status` uses.
                let revision = store.graph_revision()?;
                let resolver = crate::wiki::GraphAnchorResolver::new(store, &self.root);
                crate::wiki::search(&conn, &resolver, query, list, revision)
            })
        })
    }

    /// The wiki store summary + regeneration work-list ([FR-WK-06]): page/stale
    /// counts and the freshness fraction, the pruned-orphan log ([FR-WK-07]), and
    /// the work-list — stale pages, missing-anchor pages, and page-worthy
    /// entities that no page anchors yet (currently none: modules, top-level
    /// files, and the typed Requirement/Adr/Story nodes are all excluded,
    /// [CR-034]/[CR-056]), plus the agent-tier structured sections that are
    /// absent or revision-stale ([FR-WK-06] as modified by [CR-027], [FR-WK-12],
    /// [CR-056] — the six Overview prose children and the present consolidated
    /// documentation categories; per-file objectives are no longer seeded and
    /// the native tier is never listed). The `logos wiki status` subcommand and
    /// the `wiki_status` MCP twin share this one `api` entrypoint byte-for-byte
    /// ([FR-WK-09], [NFR-CC-01]); a deterministic function of `wiki.db` + the
    /// current graph/tree ([NFR-RA-06]), and a pure read that never prunes.
    /// Structured-section re-queue cadence is dampened by the configurable
    /// `[wiki].revision_stale_threshold` ([FR-WK-17]); `revision_stale_count`
    /// stays truthful regardless — dampening never masks reporting.
    ///
    /// # Errors
    /// Returns an error only on an unexpected store failure, or for a transient
    /// [`Engine::open`] engine.
    ///
    /// [FR-WK-06]: ../../../docs/specs/requirements/FR-WK-06.md
    /// [FR-WK-07]: ../../../docs/specs/requirements/FR-WK-07.md
    /// [FR-DG-07]: ../../../docs/specs/requirements/FR-DG-07.md
    /// [FR-WK-09]: ../../../docs/specs/requirements/FR-WK-09.md
    /// [FR-WK-17]: ../../../docs/specs/requirements/FR-WK-17.md
    pub fn wiki_status(&self) -> Result<crate::wiki::WikiStatus> {
        crate::observability::traced("wiki_status", || {
            let runtime = self.nav_runtime_no_prologue()?;
            let conn = crate::wiki::open(&self.root)?;
            // The revision-stale re-queue dampening threshold ([FR-WK-17],
            // [`WikiConfig::effective_revision_stale_threshold`]) — a plain
            // local `config.toml` read, no LLM/network call ([NFR-SE-01]).
            let threshold = crate::config::load_config_from_root(&self.root)?
                .wiki
                .effective_revision_stale_threshold();
            runtime.submit_read(|store| {
                // The current persisted graph revision ([FR-SY-09]) seeds the
                // agent-tier structured sections: a page built at an older
                // revision is "stale — regeneration pending"; `0` (no `index`
                // yet) yields no structured work, so `init` is never blocked
                // ([FR-WK-12], [ADR-32]).
                let revision = store.graph_revision()?;
                let resolver = crate::wiki::GraphAnchorResolver::new(store, &self.root);
                let status = crate::wiki::status(&conn, &resolver, &resolver, revision, threshold)?;
                Ok(status)
            })
        })
    }

    /// Format the `wiki status` work-list ([FR-WK-06]) into the ordered, offline
    /// **generation queue** ([FR-WK-13]) — the ready-to-run plan that drives the
    /// connected agent's off-request generation loop ([ADR-33]): the six Overview
    /// prose children, then the present consolidated documentation categories,
    /// then unanchored page-worthy entities (currently always empty, [CR-056]),
    /// then stale/missing existing-page refreshes, each carrying its target slug
    /// and a runnable `wiki write` skeleton; the deterministic native tier
    /// ([FR-WK-10]) is never queued. The `logos wiki generate` subcommand renders
    /// this as a human prompt block by default and the serialized queue under
    /// `--json`.
    ///
    /// A **pure read of `wiki.db` + the current graph revision**, sharing the
    /// exact [`wiki_status`](Self::wiki_status) read path: it performs no
    /// `wiki.db` write, no LLM call, and no network call ([NFR-SE-01]) — Logos
    /// drives the loop, the agent synthesizes the prose off the request path. A
    /// deterministic function of `wiki.db` + the revision ([NFR-RA-06]), so the
    /// `--json` queue is byte-identical for a fixed store + revision; an empty
    /// work-list yields an empty queue (the honest "nothing to generate",
    /// [NFR-CC-04]). One instrumented façade span — it dispatches the *inner*
    /// `wiki::status` read, never the already-traced public `wiki_status`, so it
    /// emits exactly one telemetry record and never nests a façade span
    /// ([NFR-CC-01]).
    ///
    /// # Errors
    /// Returns an error only on an unexpected store failure, or for a transient
    /// [`Engine::open`] engine.
    ///
    /// [FR-WK-06]: ../../../docs/specs/requirements/FR-WK-06.md
    /// [FR-WK-13]: ../../../docs/specs/requirements/FR-WK-13.md
    /// [FR-WK-10]: ../../../docs/specs/requirements/FR-WK-10.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [NFR-CC-01]: ../../../docs/specs/requirements/NFR-CC-01.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [ADR-33]: ../../../docs/specs/architecture/decisions/ADR-33.md
    pub fn wiki_generate(&self) -> Result<crate::wiki::WikiGenerationQueue> {
        crate::observability::traced("wiki_generate", || {
            let runtime = self.nav_runtime_no_prologue()?;
            let conn = crate::wiki::open(&self.root)?;
            // Same dampening threshold `wiki_status` resolves ([FR-WK-17]) — the
            // queue is a pure reformatting of the same work-list ([FR-WK-13]).
            let threshold = crate::config::load_config_from_root(&self.root)?
                .wiki
                .effective_revision_stale_threshold();
            let status = runtime.submit_read(|store| {
                let revision = store.graph_revision()?;
                let resolver = crate::wiki::GraphAnchorResolver::new(store, &self.root);
                crate::wiki::status(&conn, &resolver, &resolver, revision, threshold)
            })?;
            Ok(crate::wiki::generation_queue(&status))
        })
    }

    /// The **native (extracted) wiki tier** ([FR-WK-10], [ADR-32]): the three
    /// deterministic sections live-rendered from the graph — Codebase structure,
    /// the Files view, and the dependency Mermaid diagram —
    /// carrying the "extracted — live from graph @revision N" provenance label.
    ///
    /// **Memoized by the persisted graph revision** ([FR-SY-09]): the first call
    /// at a revision renders from the graph and caches the result; subsequent
    /// calls at the same revision return the cached value (a cache hit is O(1),
    /// [NFR-PE-01]); a revision advance (after `index`/`sync`) is a cache miss
    /// that re-renders. **Nothing is ever written to `wiki.db`** — the native
    /// tier has no store ([ADR-32]); the render performs no second file walk and
    /// no LLM/network call ([NFR-SE-01]), so it is safe on the write-free-on-read
    /// GET path ([ADR-28], [BR-35]) and is never read by the gated metric path
    /// ([BR-29]).
    ///
    /// # Errors
    /// Returns an error only on an unexpected store failure, or for a transient
    /// [`Engine::open`] engine (no read-only pool).
    ///
    /// [FR-WK-10]: ../../../docs/specs/requirements/FR-WK-10.md
    /// [FR-SY-09]: ../../../docs/specs/requirements/FR-SY-09.md
    /// [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    /// [ADR-32]: ../../../docs/specs/architecture/decisions/ADR-32.md
    /// [BR-29]: ../../../docs/specs/software-spec.md#324-source-wiki
    pub fn wiki_native(&self) -> Result<crate::wiki::NativeWiki> {
        crate::observability::traced("wiki_native", || {
            let runtime = self.nav_runtime_no_prologue()?;
            let revision = runtime.submit_read(|store| store.graph_revision())?;

            // Cache hit: the render at this revision is already memoized.
            {
                let cache = self.native_wiki.lock().expect("native-wiki cache lock");
                if let Some((cached_revision, native)) = cache.as_ref() {
                    if *cached_revision == revision {
                        return Ok(native.clone());
                    }
                }
            }

            // Cache miss: re-render from the graph at the current revision and
            // memoize. Nothing is written to `wiki.db` ([ADR-32]).
            let native =
                runtime.submit_read(|store| crate::wiki::render_native(store, revision))?;
            let mut cache = self.native_wiki.lock().expect("native-wiki cache lock");
            *cache = Some((revision, native.clone()));
            Ok(native)
        })
    }

    /// Whether the `docs/` source file(s) for a consolidated documentation
    /// category ([`DocCategory`](crate::wiki::DocCategory), [CR-034]) exist under
    /// the project root — the presence signal the wiki menu's **Specs** tier uses
    /// to decide whether to show the optional **Frontend Design** entry
    /// ([FR-WK-11] as modified, S-133). Rendered against the **same fixed slug
    /// contract** the generation work-list ([FR-WK-06]) seeds, so the menu and the
    /// producer can never disagree on which category pages exist.
    ///
    /// A pure local-filesystem read (one `is_file`/`read_dir`) — no graph/store
    /// read, no `wiki.db` write, no LLM, and no network ([NFR-SE-01]) — so it is
    /// safe on the write-free-on-read GET path ([ADR-28]) and never moves the
    /// quality signal (the wiki store is gate-immune). Infallible by design: a
    /// missing directory reads as "absent", never an error.
    ///
    /// [FR-WK-11]: ../../../docs/specs/requirements/FR-WK-11.md
    /// [FR-WK-06]: ../../../docs/specs/requirements/FR-WK-06.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    pub fn wiki_doc_category_present(&self, category: crate::wiki::DocCategory) -> bool {
        // Through the single observability chokepoint ([ADR-13], [NFR-CC-01]) like
        // every other Engine accessor — `traced_infallible` because the presence
        // check cannot fail (a missing directory reads as `false`), matching the
        // `languages()` infallible-accessor precedent.
        crate::observability::traced_infallible("wiki_doc_category_present", || {
            category.present_under(&self.root)
        })
    }

    /// The **SRS-mode gate** ([FR-WK-21], [ADR-57]): `true` (Case 1 — SRS present)
    /// when the project carries both load-bearing swe-skills artifacts —
    /// `docs/specs/architecture.md` **and** at least one `FR-*`/`NFR-*`/`UAT-*`
    /// file under `docs/specs/requirements/` — else `false` (Case 2). This is the
    /// façade half of the predicate the wiki work-list ([FR-WK-06]) / generation
    /// queue ([FR-WK-13]) branch on: in Case 1 the Design/Specs pages are produced
    /// by the deterministic presented tier ([FR-WK-20]) and the agent queue is
    /// restricted to the Summary tier; in Case 2 the agent infers the full set
    /// from the code graph. It is the surface accessor the CLI/MCP/hook/UI
    /// (wired later) consult to choose presentation over inference.
    ///
    /// A pure local-filesystem read (a handful of `is_file`/`read_dir`) — no
    /// graph/store read, no `wiki.db` write, no LLM, and no network ([NFR-SE-01]) —
    /// so it is safe on the write-free-on-read GET path ([ADR-28]) and never moves
    /// the quality signal. Infallible by design: a missing file/directory reads as
    /// Case 2, never an error; a deterministic function of the on-disk layout
    /// ([NFR-RA-06]).
    ///
    /// [FR-WK-21]: ../../../docs/specs/requirements/FR-WK-21.md
    /// [FR-WK-06]: ../../../docs/specs/requirements/FR-WK-06.md
    /// [FR-WK-13]: ../../../docs/specs/requirements/FR-WK-13.md
    /// [FR-WK-20]: ../../../docs/specs/requirements/FR-WK-20.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    /// [ADR-57]: ../../../docs/specs/architecture/decisions/ADR-57.md
    pub fn wiki_srs_mode(&self) -> bool {
        // `traced_infallible` like `wiki_doc_category_present` — the gate is a pure
        // local-FS read that cannot fail (a missing artifact reads as Case 2).
        crate::observability::traced_infallible("wiki_srs_mode", || {
            crate::wiki::wiki_srs_mode(&self.root)
        })
    }

    /// The **User Guide** tier's page set ([FR-WK-23], [FR-WK-11]) — one
    /// `(slug, title)` pair per `docs/howto/*.md` file, in the same order
    /// [`Engine::wiki_materialize`] writes them (`README.md` first, at
    /// `guide/overview`). The web menu ([FR-UI-06]) uses this to render (and
    /// gate the presence of) the User Guide tier without a fixed enum like
    /// [`DocCategory`](crate::wiki::DocCategory) — the guide file set is
    /// per-project. **Gated on [`Engine::wiki_srs_mode`]** — empty in Case 2
    /// even when `docs/howto/` has files, since `wiki_materialize` never writes
    /// a Case-2 guide page and a Case-2 menu listing would be a permanent,
    /// un-fulfillable "not yet generated" placeholder. Also empty when
    /// `docs/howto/` has no `*.md` file, so the tier is simply absent rather
    /// than an empty group ([FR-WK-11]).
    ///
    /// A pure local-filesystem read (a `read_dir` + per-file read) — no
    /// graph/store read, no `wiki.db` write, no LLM, and no network
    /// ([NFR-SE-01]) — so it is safe on the write-free-on-read GET path
    /// ([ADR-28]) and never moves the quality signal. Infallible by design: a
    /// missing directory reads as an empty tier, never an error.
    ///
    /// [FR-WK-23]: ../../../docs/specs/requirements/FR-WK-23.md
    /// [FR-WK-11]: ../../../docs/specs/requirements/FR-WK-11.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    pub fn wiki_guide_pages(&self) -> Vec<(String, String)> {
        crate::observability::traced_infallible("wiki_guide_pages", || {
            crate::wiki::wiki_guide_pages(&self.root)
        })
    }

    /// The reconciliation sweep ([FR-WK-22]): bulk-purge any stored page whose
    /// slug falls outside the active-mode valid set — the five Overview/Summary
    /// slugs ∪ the present consolidated-category slugs — logging each removal to
    /// the pruned-log ([`Engine::wiki_pruned_log`] surfaces the trace). Retires
    /// orphaned pages left by a prior or superseded generation run that are
    /// unreachable from the menu ([FR-WK-11]) and un-regenerable by the work-list
    /// ([FR-WK-06]) — the lazy all-anchors-gone orphan lifecycle ([FR-WK-07])
    /// never reaches them, since it fires only on read.
    ///
    /// Idempotent: once every stored slug is valid, a re-run purges nothing. A
    /// pure store + local-FS operation — no LLM, no network call ([NFR-SE-01]).
    /// Runs at the end of the [FR-WK-20] `wiki materialize` path (S-262); exposed
    /// as its own façade operation so it is independently callable and testable
    /// ahead of that wiring.
    ///
    /// Returns the purged slugs, in slug order.
    ///
    /// # Errors
    /// Returns an error only on an unexpected store failure.
    ///
    /// [FR-WK-22]: ../../../docs/specs/requirements/FR-WK-22.md
    /// [FR-WK-11]: ../../../docs/specs/requirements/FR-WK-11.md
    /// [FR-WK-06]: ../../../docs/specs/requirements/FR-WK-06.md
    /// [FR-WK-07]: ../../../docs/specs/requirements/FR-WK-07.md
    /// [FR-WK-20]: ../../../docs/specs/requirements/FR-WK-20.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    pub fn wiki_reconcile(&self) -> Result<Vec<String>> {
        crate::observability::traced("wiki_reconcile", || {
            let mut conn = crate::wiki::open(&self.root)?;
            crate::wiki::reconcile(&mut conn, &self.root)
        })
    }

    /// The deterministic **presented tier** ([FR-WK-20], [ADR-57], [CR-062]): in
    /// SRS mode (Case 1, [`Engine::wiki_srs_mode`]) assemble each present
    /// Design/Specs category — glob → sorted section-per-source-document
    /// consolidated Markdown — the single-file Architecture page
    /// (`docs/specs/architecture.md` → the `overview/architecture` slug), **and**
    /// the **User Guide** tier's per-file pages ([FR-WK-23]) — one page per
    /// `docs/howto/*.md` file, `README.md` → `guide/overview`, rendered
    /// verbatim — directly from the project's authored `docs/specs/**`/
    /// `docs/howto/**` sources, upsert each into `wiki.db` with `generator =
    /// "logos:doc-present"`, one source-file anchor per document, and the
    /// current built-at revision, then run the [FR-WK-22] reconciliation sweep
    /// so orphaned prior-run pages (including a `guide/*` page whose source file
    /// was deleted) are purged in the same operation. A category whose source
    /// glob matches no file, or an absent `docs/howto/`, yields no page.
    ///
    /// In **Case 2** (no authored SRS) it is a no-op returning the empty summary:
    /// presentation is reserved for projects carrying the load-bearing
    /// `architecture.md` + a requirement; the connected agent infers the full set
    /// from the code graph as before ([FR-WK-21]).
    ///
    /// Deterministic and offline — pure local-FS reads + `wiki.db` writes, no LLM
    /// and no network ([NFR-SE-01]); re-running with unchanged sources is
    /// byte-identical ([FR-WK-20]). This relaxes the [ADR-24] "binary never writes
    /// the store" property for faithful presentation while preserving the
    /// prose-authoring ban ([ADR-57]): the binary copies authored docs verbatim, it
    /// never *authors* prose.
    ///
    /// # Errors
    /// Returns an error on a write-path rejection or an unexpected store failure,
    /// or for a transient [`Engine::open`] engine (no read-only pool).
    ///
    /// [FR-WK-20]: ../../../docs/specs/requirements/FR-WK-20.md
    /// [FR-WK-21]: ../../../docs/specs/requirements/FR-WK-21.md
    /// [FR-WK-22]: ../../../docs/specs/requirements/FR-WK-22.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    /// [ADR-24]: ../../../docs/specs/architecture/decisions/ADR-24.md
    /// [ADR-57]: ../../../docs/specs/architecture/decisions/ADR-57.md
    /// [CR-062]: ../../../docs/requests/CR-062-wiki-present-authored-docs.md
    pub fn wiki_materialize(&self) -> Result<crate::wiki::MaterializeSummary> {
        crate::observability::traced("wiki_materialize", || {
            // Case 2: presentation is a Case-1 operation — write nothing, sweep
            // nothing, so the agent-inference path is untouched ([FR-WK-21]).
            if !crate::wiki::wiki_srs_mode(&self.root) {
                return Ok(crate::wiki::MaterializeSummary {
                    srs_mode: false,
                    materialized: Vec::new(),
                    pruned: Vec::new(),
                });
            }
            let runtime = self.nav_runtime_no_prologue()?;
            let head = crate::wiki::head_sha(&self.root).unwrap_or_default();
            let mut conn = crate::wiki::open(&self.root)?;
            // Capture the built-at revision from the same read the resolver uses,
            // so a presented page's freshness derives from one revision ([FR-SY-09]).
            let materialized = runtime.submit_read(|store| {
                let built_at_revision = store.graph_revision()?;
                let resolver = crate::wiki::GraphAnchorResolver::new(store, &self.root);
                crate::wiki::materialize(&mut conn, &resolver, &self.root, &head, built_at_revision)
            })?;
            // Sweep orphaned prior-run pages once the present set is written
            // ([FR-WK-22]); the valid set includes the just-written slugs.
            let pruned = crate::wiki::reconcile(&mut conn, &self.root)?;
            Ok(crate::wiki::MaterializeSummary { srs_mode: true, materialized, pruned })
        })
    }

    /// Materialize the embedded wiki-generation skill ([FR-WK-08], CLI-only
    /// surface) into `dir` (or the project root when `None`), restoring the
    /// embedded content when `force` is set and skipping an existing install
    /// otherwise. Pure local filesystem I/O — no network ([NFR-SE-01]).
    ///
    /// This is the post-install / post-upgrade refresh path behind
    /// `logos wiki skill --emit [dir] [--force]`; the `init -i` materialization
    /// runs the same engine through [`crate::init`].
    ///
    /// [FR-WK-08]: ../../../docs/specs/requirements/FR-WK-08.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    pub fn wiki_skill_emit(
        &self,
        dir: Option<&Path>,
        force: bool,
    ) -> Result<crate::wiki::EmitSummary> {
        crate::observability::traced("wiki_skill_emit", || {
            crate::wiki::materialize_skill(dir.unwrap_or(&self.root), force)
        })
    }

    /// Materialize the Claude Code **SessionEnd** quality-report hook
    /// ([FR-IN-07], [FR-GV-05], [FR-GV-09], [ADR-49], CLI-only surface): a
    /// marker-tagged hook script plus a non-clobbering merge into the project's
    /// **shared** `.claude/settings.json`. On session end it prints the current
    /// quality signal, the blessed baseline, and any rule violations as a
    /// non-blocking readout — it never propagates `check`/`gate`'s non-zero exit
    /// ([FR-GV-05]); `force` re-emits it.
    ///
    /// Pure local filesystem I/O behind `logos wiki hook --emit [--force]` (the
    /// `init -i` step runs the same engine through [`crate::init`]). Installing
    /// the hook performs **no** LLM call and opens **no** network connection
    /// ([NFR-SE-01]) — it only shells out to the pure-read `scan`/`gate`/`check`
    /// commands.
    ///
    /// [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
    /// [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
    /// [FR-GV-09]: ../../../docs/specs/requirements/FR-GV-09.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    /// [ADR-49]: ../../../docs/specs/architecture/decisions/ADR-49.md
    pub fn wiki_quality_report_hook_emit(&self, force: bool) -> Result<crate::wiki::HookEmitSummary> {
        crate::observability::traced("wiki_quality_report_hook_emit", || {
            crate::wiki::materialize_quality_report_hook(&self.root, force)
        })
    }

    // ── Config write-back (S-096, CR-025, ADR-31) ────────────────────────────

    /// Read both checked-in policy files for this engine's root — raw content +
    /// parsed model ([FR-UI-12], [CR-025]): the read half of the validated
    /// config-write seam the web config editor builds on.
    ///
    /// A pure read of `.logos/config.toml` and `.logos/rules.toml`; it touches no
    /// graph store, so it works on both engine flavours (a transient
    /// [`Engine::open`] engine is enough). An absent file is reported
    /// `exists = false` with the effective default model, not an error
    /// ([NFR-DM-04]).
    ///
    /// # Errors
    /// A present-but-invalid policy file fails loud through the load path — an
    /// unknown key, a non-compiling glob, or an out-of-range value is a
    /// [`ConfigError`](crate::config::ConfigError) (usage exit 2).
    ///
    /// [FR-UI-12]: ../../../docs/specs/requirements/FR-UI-12.md
    /// [CR-025]: ../../../docs/requests/CR-025-interactive-config-editing.md
    /// [NFR-DM-04]: ../../../docs/specs/requirements/NFR-DM-04.md
    pub fn config_read(&self) -> Result<crate::config::ConfigReadModel> {
        crate::observability::traced("config_read", || {
            Ok(crate::config::read_documents(&self.root)?)
        })
    }

    /// Validate an edited policy-file `candidate` against the **existing** load
    /// path and, only if valid, replace the file **atomically**
    /// (write-temp-then-rename) — the write half of the config-editor seam
    /// ([FR-UI-12], [ADR-31]).
    ///
    /// `candidate` is the full candidate TOML document. It is run through the
    /// same `#[serde(deny_unknown_fields)]` parse + glob/range validation the CLI
    /// loader runs ([FR-CF-01]/[FR-CF-03]) — no new validation is invented — so an
    /// invalid edit is rejected with **no partial write** and the file is left
    /// byte-identical ([NFR-RA-07]). A [`PolicyFile::Rules`](crate::config::PolicyFile)
    /// write additionally stamps a provenance comment and is validated *with* the
    /// stamp, so the written contract still parses via the standard load path
    /// ([BR-35]).
    ///
    /// Like [`config_read`](Self::config_read) this is pure filesystem I/O over
    /// the resolved root and needs no graph runtime. Save alone runs no pipeline —
    /// reindex/re-eval is a separate explicit action ([FR-UI-13]).
    ///
    /// # Errors
    /// A [`ConfigError`](crate::config::ConfigError) (usage exit 2) for an invalid
    /// candidate, or [`ConfigError::Write`](crate::config::ConfigError::Write) if
    /// the atomic write itself fails (the original file is unchanged).
    ///
    /// [FR-UI-12]: ../../../docs/specs/requirements/FR-UI-12.md
    /// [FR-UI-13]: ../../../docs/specs/requirements/FR-UI-13.md
    /// [FR-CF-01]: ../../../docs/specs/requirements/FR-CF-01.md
    /// [FR-CF-03]: ../../../docs/specs/requirements/FR-CF-03.md
    /// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
    /// [ADR-31]: ../../../docs/specs/architecture/decisions/ADR-31.md
    /// [BR-35]: ../../../docs/specs/software-spec.md#326-web-ui
    pub fn config_write(
        &self,
        file: crate::config::PolicyFile,
        candidate: &str,
    ) -> Result<crate::config::ConfigWriteOutcome> {
        crate::observability::traced("config_write", || {
            let outcome = match file {
                crate::config::PolicyFile::Config => {
                    crate::config::write_config(&self.root, candidate)?
                }
                crate::config::PolicyFile::Rules => {
                    crate::config::write_rules(&self.root, candidate)?
                }
            };
            Ok(outcome)
        })
    }

    /// Write (or clear) the chat API key in the gitignored `.logos/secrets.toml`
    /// via the same validated atomic write the policy files use (S-169,
    /// [FR-CF-06], [NFR-SE-07]).
    ///
    /// `api_key` is the raw key the Config editor posted; a blank value clears it.
    /// The key writes to `secrets.toml` (never the checked-in `config.toml`), and
    /// the returned [`SecretWriteOutcome`](crate::config::SecretWriteOutcome)
    /// carries only the **masked** new state (presence + last-4) — the raw key is
    /// never echoed back ([NFR-SE-07]). Like the policy write-back this is pure
    /// filesystem I/O and runs no pipeline.
    ///
    /// # Errors
    /// A [`ConfigError`](crate::config::ConfigError) (usage exit 2) if an existing
    /// `secrets.toml` does not parse, or
    /// [`ConfigError::Write`](crate::config::ConfigError::Write) if the atomic
    /// write itself fails (the original file is unchanged).
    ///
    /// [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
    /// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
    pub fn config_write_secret(
        &self,
        api_key: &str,
    ) -> Result<crate::config::SecretWriteOutcome> {
        crate::observability::traced("config_write_secret", || {
            Ok(crate::config::write_secret(&self.root, api_key)?)
        })
    }

    /// Apply a validated config edit ([FR-UI-13], [ADR-31]): act on the new
    /// policy the editor just saved through [`config_write`](Self::config_write).
    /// This is the **explicit Apply** action — Save alone runs no pipeline, so
    /// the engine's derived state changes only when this is invoked.
    ///
    /// - [`PolicyFile::Config`](crate::config::PolicyFile::Config) runs
    ///   reconcile/index through the [pipeline-orchestrator], reusing the
    ///   existing admission-fingerprint reconciliation ([FR-SY-07]) so the graph
    ///   reflects the new admission policy — purging now-unadmitted files,
    ///   O(changed), not a full reindex.
    /// - [`PolicyFile::Rules`](crate::config::PolicyFile::Rules) re-runs the
    ///   [governance-engine] scan against the *unchanged* graph (`reconcile =
    ///   false`, no reindex) so the gate / quality views reflect the new
    ///   contract — a rules change alters governance evaluation, not graph
    ///   admission. Like every scan it persists a `metric_snapshots` row and
    ///   refreshes the violation set ([FR-GV-09]), so the dashboard's
    ///   latest-scan view reflects the new contract after Apply.
    ///
    /// The whole apply is a **single** instrumented façade span: it dispatches
    /// to the *inner* reconcile/scan seams (`run_reconcile`,
    /// [`crate::governance::scan`]) rather than the already-traced public
    /// [`index`](Self::index)/[`scan`](Self::scan), so it emits exactly one
    /// `config_apply` telemetry record and never nests a second façade span
    /// ([NFR-CC-01]). It runs on the engine pool, not the surface thread — every
    /// mutation routes through the writer actor, which serializes it against a
    /// running `serve` watcher ([ADR-02], [ADR-31]).
    ///
    /// # Errors
    /// Returns an error for a transient [`Engine::open`] engine (no runtime), or
    /// on a structural reconcile/scan failure ([ADR-14] fail-loud); per-file
    /// degradations ride inside the summary's `warnings`/`files_failed`.
    ///
    /// [pipeline-orchestrator]: ../../../docs/specs/architecture/components/pipeline-orchestrator.md
    /// [governance-engine]: ../../../docs/specs/architecture/components/governance-engine.md
    /// [FR-UI-13]: ../../../docs/specs/requirements/FR-UI-13.md
    /// [FR-SY-07]: ../../../docs/specs/requirements/FR-SY-07.md
    /// [FR-GV-09]: ../../../docs/specs/requirements/FR-GV-09.md
    /// [NFR-CC-01]: ../../../docs/specs/requirements/NFR-CC-01.md
    /// [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
    /// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
    /// [ADR-31]: ../../../docs/specs/architecture/decisions/ADR-31.md
    pub fn config_apply(
        &self,
        file: crate::config::PolicyFile,
    ) -> Result<crate::config::ConfigApplyOutcome> {
        use crate::config::{ConfigApplyOutcome, PolicyFile};
        crate::observability::traced("config_apply", || match file {
            PolicyFile::Config => {
                let outcome = self.run_reconcile()?;
                Ok(ConfigApplyOutcome::Reconciled {
                    reconciled_files: outcome.reconciled_files,
                    full_index: outcome.full_index,
                    unresolved_refs: outcome.resolution.refs_unresolved,
                    files_failed: outcome.files_failed,
                    warnings: outcome.warnings,
                })
            }
            PolicyFile::Rules => {
                // No reindex: a rules change alters governance evaluation, not
                // graph admission ([FR-UI-13]). Re-evaluate against the current
                // graph (`reconcile = false`) through the inner scan body — not
                // the traced public `scan` — so the apply stays one façade span.
                // Going through the inner body also deliberately skips
                // `record_scan`: an apply is not an operator-triggered scan and
                // must not become the parameters a later `rescan` replays.
                let scan = crate::governance::scan(self, false)?;
                Ok(ConfigApplyOutcome::Reevaluated {
                    signal: scan.signal,
                    violations: scan.violations.len(),
                    freshness: scan.freshness,
                    warnings: scan.warnings,
                })
            }
        })
    }

    /// The runtime a navigation tool reads through, after the **one-time**
    /// auto-index prologue ([FR-IX-07]): the very first navigation call on a
    /// never-indexed graph transparently builds it; every later call skips the
    /// check entirely (memoised), so the steady-state freshness contract is
    /// exactly [ADR-11]'s "never reconcile per call" ([FR-RC-05]).
    ///
    /// A failed prologue (e.g. no plugin registry) is logged by
    /// [`ensure_indexed`](Self::ensure_indexed)'s degradation path and **not**
    /// retried — navigation still serves whatever graph exists, best-effort.
    ///
    /// # Errors
    /// Returns an error for a transient [`Engine::open`] engine (no RO pool).
    pub(crate) fn nav_runtime(&self) -> Result<&Runtime> {
        let runtime = self.nav_runtime_no_prologue()?;
        if !self.nav_prologue_done.swap(true, Ordering::AcqRel) {
            // Degrades internally (warnings + stderr), never panics; an
            // actual index advances the sync stamp itself. A racing second
            // caller sees `true` and proceeds WITHOUT waiting for the index
            // to finish — deliberate: best-effort-fresh ([ADR-11]) allows
            // serving whatever graph exists at read time.
            let _ = self.ensure_indexed();
            // [FR-SY-08]/[ADR-20] (S-041): the one-time prologue also runs the
            // fingerprint-gated config-change purge before serving, so a read
            // after a config narrowing never surfaces now-unadmitted symbols.
            // A scoped exception to [FR-RC-05] — it purges on a *detected*
            // config change at most once per change, not a per-call working-
            // tree reconcile. When `ensure_indexed` above ran a full index it
            // already recorded the fingerprint, so this gates to a no-op (one
            // comparison, no discovery walk). Degrades like the auto-index
            // prologue: logged, never fatal — navigation still serves.
            self.run_prologue_purge();
        }
        Ok(runtime)
    }

    /// The navigation runtime *without* the auto-index prologue — for `status`
    /// (which must report an unindexed graph, not silently build one) and for
    /// follow-up reads inside a tool that already ran the prologue.
    ///
    /// # Errors
    /// Returns an error for a transient [`Engine::open`] engine (no RO pool).
    pub(crate) fn nav_runtime_no_prologue(&self) -> Result<&Runtime> {
        self.runtime.as_ref().ok_or_else(|| {
            anyhow!(
                "navigation requires a long-lived engine (Engine::start); a transient \
                 Engine::open engine has no read-only pool to serve reads (ADR-02)"
            )
        })
    }

    /// The governance engine's in-process state ([S-020]).
    ///
    /// [S-020]: ../../../docs/planning/journal.md#s-020-governance-engine-and-quality-gate
    pub(crate) fn governance(&self) -> &crate::governance::GovernanceState {
        &self.governance
    }

    /// Run the [ADR-11] pre-evaluation reconcile (S-020): discover → blake3
    /// dirty-detect → sync the dirty set, O(changed) ([FR-RC-01], [FR-RC-02]),
    /// then advance the sync stamp so hydrated views reflect the reconciled
    /// graph.
    ///
    /// # Errors
    /// Returns an error for a transient engine or on a structural reconcile
    /// failure ([ADR-14] fail-loud); per-file failures degrade inside the
    /// outcome ([NFR-RA-11]).
    ///
    /// [FR-RC-01]: ../../../docs/specs/requirements/FR-RC-01.md
    /// [FR-RC-02]: ../../../docs/specs/requirements/FR-RC-02.md
    /// [NFR-RA-11]: ../../../docs/specs/requirements/NFR-RA-11.md
    pub(crate) fn run_reconcile(&self) -> Result<crate::pipeline::ReconcileOutcome> {
        let (runtime, registry, config) = self.pipeline_ctx()?;
        let outcome = crate::pipeline::reconcile(runtime, registry, &self.root, &config)?;
        // The reconcile may have written; invalidate hydrated views so the
        // score sees the fresh graph (ADR-04). A full-index degrade also
        // stamps the FR-NV-07 clock.
        self.advance_sync_stamp();
        if outcome.full_index {
            self.record_full_index();
        }
        Ok(outcome)
    }

    /// Resolve the live runtime, the cached plugin registry, and the on-disk
    /// config — the three inputs every pipeline operation needs.
    ///
    /// # Errors
    /// Returns an error if this is a transient [`Engine::open`] engine (no
    /// runtime), if the plugin registry failed to load, or if `config.toml` is
    /// present but invalid.
    pub(crate) fn pipeline_ctx(
        &self,
    ) -> Result<(
        &Runtime,
        &crate::plugin::LanguageRegistry,
        crate::config::Config,
    )> {
        let runtime = self.runtime.as_ref().context(
            "graph operations require a started engine (Engine::start), not a transient one",
        )?;
        let registry = self
            .registry
            .as_ref()
            .context("the plugin registry failed to load; cannot extract any files")?;
        let config = crate::config::load_config_from_root(&self.root)?;
        Ok((runtime, registry, config))
    }

    /// Fallible body of [`index`](Self::index).
    fn run_index(&self) -> Result<IndexResult> {
        let (runtime, registry, config) = self.pipeline_ctx()?;
        let result = crate::pipeline::index(runtime, registry, &self.root, &config)?;
        // Invalidate the hydration cache so the next hydrate() reflects the new
        // graph state ([ADR-04], [ADR-05], [S-009]).
        self.advance_sync_stamp();
        self.record_full_index();
        Ok(result)
    }

    /// Fallible body of [`sync`](Self::sync).
    fn run_sync(&self, paths: &[PathBuf]) -> Result<SyncResult> {
        let (runtime, registry, config) = self.pipeline_ctx()?;
        // A public `Engine::sync` carries a *partial* changed-file set (the
        // debounced watcher batch, a git hook, or an explicit CLI `sync`), so it
        // reconciles only those paths and never sweeps files outside the set
        // (CR-052 / FR-SY-10). The full-stored-set disk-deletion sweep is reserved
        // for the full-walk `reconcile` path (`run_reconcile`).
        let result = crate::pipeline::sync(
            runtime,
            registry,
            &self.root,
            &config,
            paths,
            crate::pipeline::SyncScope::Partial,
        )?;
        self.advance_sync_stamp();
        Ok(result)
    }

    /// Apply the main↔branch diff over a just-seeded store ([FR-WT-03],
    /// [ADR-15]): compute the paths differing from the primary checkout's HEAD
    /// (committed branch work + dirty working tree + untracked files) and run
    /// one incremental sync over exactly that set.
    ///
    /// Fail-soft: a failed diff or sync is logged and surfaced to telemetry
    /// (the `worktree_seed` tool record, [FR-WT-03]'s observability
    /// criterion), never fatal — the seeded graph is at worst stale by the
    /// diff, and the next evaluation's reconcile backstop ([ADR-11]) catches
    /// up. A registry-less engine degrades the same way.
    ///
    /// [FR-WT-03]: ../../../docs/specs/requirements/FR-WT-03.md
    fn reconcile_seed_diff(&self, seed: &crate::workspace::SeedSource) {
        let outcome = crate::observability::traced("worktree_seed", || {
            let paths = crate::workspace::diff_from_primary(&self.root, &seed.head)?;
            if paths.is_empty() {
                return Ok(SyncResult::default());
            }
            self.run_sync(&paths)
        });
        match outcome {
            Ok(result) => tracing::info!(
                files_added = result.files_added,
                files_modified = result.files_modified,
                files_removed = result.files_removed,
                "diff-reconciled the seeded store against the worktree (FR-WT-03)"
            ),
            Err(err) => tracing::warn!(
                "diff-reconcile after seeding failed; the next evaluation's \
                 reconcile backstop (ADR-11) will catch up: {err:#}"
            ),
        }
    }

    /// Fallible body of [`ensure_indexed`](Self::ensure_indexed).
    fn run_ensure_indexed(&self) -> Result<Option<IndexResult>> {
        let (runtime, registry, config) = self.pipeline_ctx()?;
        let result = crate::pipeline::ensure_indexed(runtime, registry, &self.root, &config)?;
        if result.is_some() {
            self.advance_sync_stamp();
            self.record_full_index();
        }
        Ok(result)
    }

    /// Run the navigation-prologue config-change purge ([FR-SY-08], [ADR-20],
    /// S-041) and degrade fail-soft, mirroring [`ensure_indexed`].
    ///
    /// Gated on the durable admission fingerprint: an unchanged config is one
    /// read and no discovery walk; a narrowed config purges the now-unadmitted
    /// stored nodes through the single-writer actor and records the new
    /// fingerprint so the purge runs at most once per change. When something was
    /// purged, the hydration cache is invalidated so the read it precedes
    /// reflects the smaller graph ([ADR-04]). A failure is logged and swallowed
    /// — navigation is best-effort ([ADR-11]); the next governance reconcile
    /// catches up.
    ///
    /// [FR-SY-08]: ../../../docs/specs/requirements/FR-SY-08.md
    fn run_prologue_purge(&self) {
        let outcome = crate::observability::traced("nav_prologue_purge", || {
            let (runtime, registry, config) = self.pipeline_ctx()?;
            crate::pipeline::purge_on_config_change(runtime, registry, &self.root, &config)
        });
        match outcome {
            Ok(0) => {}
            Ok(purged) => {
                // Nodes were removed; invalidate hydrated views so the read this
                // prologue precedes reflects the purge ([ADR-04]).
                self.advance_sync_stamp();
                tracing::info!(
                    purged,
                    "navigation prologue purged now-unadmitted nodes after a config change \
                     (FR-SY-08, ADR-20)"
                );
            }
            Err(err) => tracing::warn!(
                "navigation-prologue config-change purge failed; the next governance \
                 reconcile (ADR-11) will catch up: {err:#}"
            ),
        }
    }

    /// Stamp the in-process `last_full_index_at` clock ([FR-NV-07]) after a
    /// completed full index.
    fn record_full_index(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.last_full_index_at.store(now, Ordering::Release);
    }

    /// Unix-seconds timestamp of the last full index this engine completed,
    /// or `None` if none ran this process (the persisted column is a later
    /// story — see the field docs).
    pub(crate) fn last_full_index_at(&self) -> Option<u64> {
        match self.last_full_index_at.load(Ordering::Acquire) {
            0 => None,
            secs => Some(secs),
        }
    }
}

/// Log a pipeline failure and degrade to an [`IndexResult`] carrying the reason,
/// honouring the infallible-surface posture (ADR-14 defers typed errors).
fn degraded_index(err: &anyhow::Error) -> IndexResult {
    tracing::warn!("index failed: {err:#}");
    IndexResult {
        warnings: vec![format!("index failed: {err}")],
        ..IndexResult::default()
    }
}

/// Log a sync failure and degrade to a [`SyncResult`] carrying the reason.
fn degraded_sync(err: &anyhow::Error) -> SyncResult {
    tracing::warn!("sync failed: {err:#}");
    SyncResult {
        warnings: vec![format!("sync failed: {err}")],
        ..SyncResult::default()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that all engine method signatures compile and that the return
    /// types implement `serde::Serialize` — the core contract for adapter
    /// surfaces (ADR-01, NFR-MA-02).
    ///
    /// This test uses generic bounds rather than calling `todo!()` at runtime
    /// so it passes without panicking.
    #[test]
    fn engine_return_types_are_serialize() {
        fn assert_serialize<T: serde::Serialize>() {}

        // Navigation (8)
        assert_serialize::<SearchResult>();
        assert_serialize::<ContextBundle>();
        assert_serialize::<ExploreResult>();
        assert_serialize::<NodeInfo>();
        assert_serialize::<CallersResult>();
        assert_serialize::<CalleesResult>();
        assert_serialize::<ImpactResult>();
        assert_serialize::<StatusInfo>();
        // Traceability (S-037, FR-NV-10)
        assert_serialize::<ImplementorsResult>();
        assert_serialize::<ReferencingDocsResult>();

        // Pipeline (2 + 1 bootstrap)
        assert_serialize::<IndexResult>();
        assert_serialize::<SyncResult>();
        assert_serialize::<InitResult>();

        // Quality / governance (11)
        assert_serialize::<ScanResult>();
        assert_serialize::<GateResult>();
        assert_serialize::<RulesReport>();
        assert_serialize::<EvolutionReport>();
        assert_serialize::<DsmReport>();
        assert_serialize::<TestGapsReport>();
        assert_serialize::<DocGapsReport>();
        assert_serialize::<HealthInfo>();
        assert_serialize::<SessionInfo>();

        // Observability (2)
        assert_serialize::<StatsInfo>();
        assert_serialize::<LanguagesInfo>();

        // History (S-046): the lazy-mine seam's read-model.
        assert_serialize::<crate::history::MineOutcome>();
        // History (S-048): the hotspot ranking read-model.
        assert_serialize::<crate::history::HotspotReport>();
        // Coverage (S-051): the ingest summary and status read-models.
        assert_serialize::<crate::history::IngestSummary>();
        assert_serialize::<crate::history::CoverageStatus>();
        assert_serialize::<crate::history::CoverageRefreshSummary>();
        // Config write-back (S-096): the read-model + write outcome.
        assert_serialize::<crate::config::ConfigReadModel>();
        assert_serialize::<crate::config::ConfigWriteOutcome>();
        // Config apply (S-097): the explicit-apply summary.
        assert_serialize::<crate::config::ConfigApplyOutcome>();
    }

    /// The config write-back seam round-trips through the façade (S-096,
    /// [FR-UI-12]): `config_read` returns content + parsed model, a valid
    /// `config_write` is reflected on the next read, and an invalid candidate
    /// returns a typed [`ConfigError`](crate::config::ConfigError) leaving the
    /// file byte-identical. Exercised over a transient [`Engine::open`] engine —
    /// the seam is pure filesystem I/O and needs no graph runtime.
    #[test]
    fn config_read_write_round_trips_through_the_facade() {
        use crate::config::PolicyFile;

        let tmp = tempfile::TempDir::new().expect("temp root");
        let engine = Engine::open(tmp.path());

        // A valid write is reflected on the next read.
        engine
            .config_write(PolicyFile::Config, "max_file_size = 4096\n")
            .expect("a valid config write succeeds");
        let docs = engine.config_read().expect("config_read");
        assert_eq!(docs.config.parsed.max_file_size, 4096);
        assert!(docs.config.content.contains("max_file_size = 4096"));

        // A rules write carries the provenance stamp and still parses on read.
        let written = engine
            .config_write(PolicyFile::Rules, "[constraints]\nmax_cc = 9\n")
            .expect("a valid rules write succeeds");
        assert!(written.provenance_stamped);
        let docs = engine.config_read().expect("config_read after rules write");
        assert_eq!(docs.rules.parsed.constraints.max_cc, Some(9));
        assert!(docs.rules.content.contains("CR-025"));

        // An invalid candidate is rejected with a typed error; the file is intact.
        let before = std::fs::read_to_string(tmp.path().join(".logos/config.toml")).unwrap();
        let err = engine
            .config_write(PolicyFile::Config, "unknown_key = true\n")
            .expect_err("an invalid candidate is rejected");
        assert!(
            err.downcast_ref::<crate::config::ConfigError>().is_some(),
            "the façade preserves the typed ConfigError: {err:#}"
        );
        let after = std::fs::read_to_string(tmp.path().join(".logos/config.toml")).unwrap();
        assert_eq!(
            before, after,
            "the rejected write leaves the file byte-identical"
        );
    }

    /// A transient [`Engine::open`] engine holds no live runtime — the
    /// store-free constructor does not pay the cold-start cost.
    #[test]
    fn open_engine_has_no_runtime() {
        let engine = Engine::open("/tmp");
        assert!(
            engine.runtime().is_none(),
            "Engine::open must not start a runtime (transient, store-free path)"
        );
        assert_eq!(engine.root(), Path::new("/tmp"));
    }

    /// A pipeline trigger on a transient (no-runtime) engine degrades to a
    /// warning-carrying result instead of panicking — the infallible-surface
    /// posture (ADR-14). The reverse contract of `Engine::start` for pipeline ops.
    #[test]
    fn index_on_a_transient_engine_degrades_gracefully() {
        let engine = Engine::open("/tmp");
        let result = engine.index();
        assert_eq!(result.files_indexed, 0, "no runtime → nothing indexed");
        assert!(
            result.warnings.iter().any(|w| w.contains("failed")),
            "the failure reason is surfaced in warnings, got {:?}",
            result.warnings
        );
        // `sync` and `ensure_indexed` share the same degradation path.
        let synced = engine.sync(&[PathBuf::from("a.rs")]);
        assert!(synced.warnings.iter().any(|w| w.contains("failed")));
        let ensured = engine.ensure_indexed();
        assert!(ensured.warnings.iter().any(|w| w.contains("failed")));
    }

    /// The seed bootstrap's diff-reconcile is fail-soft ([FR-WT-03]/[ADR-11]):
    /// an unresolvable diff base (history rewritten, seed pruned) degrades
    /// with a warning — it must never abort startup or poison the engine,
    /// which keeps serving whatever the seed contained.
    #[test]
    fn a_failed_seed_diff_reconcile_degrades_without_breaking_the_engine() {
        let tmp = tempfile::TempDir::new().expect("temp root");
        let engine = Engine::start(tmp.path()).expect("engine starts");

        // A seed whose base commit cannot exist here (the temp root is not
        // even a git repo): diff_from_primary fails, the engine shrugs.
        let seed = crate::workspace::SeedSource {
            primary_root: tmp.path().to_path_buf(),
            db_path: tmp.path().join(".logos/logos.db"),
            head: "0123456789abcdef0123456789abcdef01234567".to_string(),
        };
        engine.reconcile_seed_diff(&seed); // must not panic or error out

        let status = engine.status();
        assert!(
            status.warnings.is_empty(),
            "the degradation is a log line, not a read-model fault: {status:?}"
        );
    }

    /// The long-lived engine is `Send + Sync` so the MCP surface can share it
    /// behind an `Arc` across blocking tasks (the runtime keeps no bare,
    /// non-`Sync` `Connection` in the struct — ADR-02/ADR-03).
    #[test]
    fn engine_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Engine>();
    }

    /// `Engine::languages()` lists the Rust grammar with its full descriptor
    /// through the public facade (FR-PL-06). Exercises the read-model mapping
    /// (including the `skipped` list) that the CLI/MCP surfaces emit.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn languages_lists_rust_with_descriptor() {
        let tmp = std::env::temp_dir(); // no overrides under here
        let info = Engine::open(&tmp).languages();

        let rust = info
            .languages
            .iter()
            .find(|l| l.name == "rust")
            .expect("rust grammar is listed via the Engine facade");
        assert_eq!(rust.extensions, ["rs"]);
        assert_eq!(rust.module_separator, "::");
        assert!(rust.capabilities.iter().any(|c| c == "symbols"));
        assert!(rust.abi_version > 0, "ABI version is surfaced");
        assert!(rust.overridden_capabilities.is_empty());
        assert!(
            info.skipped.is_empty(),
            "no grammar skipped in a clean load"
        );
        // A code grammar carries no filename claims and is not the artifact class.
        assert!(rust.filenames.is_empty());
        assert!(!rust.artifact);
    }

    /// `logos languages` lists an artifact plugin with its **filename** claims and
    /// the artifact-class flag, alongside extension claims (CR-010, [FR-PL-06] /
    /// [FR-CG-04] as modified). Built over a synthetic artifact grammar — the
    /// substrate ships none of its own — to prove the read-model surfaces the
    /// claims the format stories' plugins will carry.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn languages_lists_an_artifact_plugin_with_its_claims() {
        use crate::plugin::{grammars, AbiRange, LanguageRegistry};
        const M: &str = r#"
            name = "dockerfile"
            extensions = ["dockerfile"]
            module_separator = "/"
            abi_version = 15
            capabilities = []
            artifact = true
            filenames = ["Dockerfile"]
        "#;
        let mut entries = grammars::compiled();
        entries.push(grammars::GrammarEntry {
            manifest_label: "dockerfile/plugin.toml",
            manifest_toml: M,
            language: tree_sitter_rust::LANGUAGE,
            embedded_queries: &[],
        });
        let reg = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
            .expect("the synthetic artifact grammar loads");

        let info = Engine::languages_from(&reg);
        let docker = info
            .languages
            .iter()
            .find(|l| l.name == "dockerfile")
            .expect("the artifact plugin is listed");
        assert!(docker.artifact, "listed as the artifact class");
        assert_eq!(docker.extensions, ["dockerfile"], "extension claim listed");
        assert_eq!(docker.filenames, ["Dockerfile"], "filename claim listed");
    }
}
