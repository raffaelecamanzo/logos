//! [`Engine`] — the sole instrumented chokepoint (ADR-01, NFR-CC-01).
//!
//! # Design
//! - `Engine` holds the lifecycle — open, start, the watcher, the builder
//!   settings — and the accessors to the per-process state; every operation is
//!   a method of the feature-area façade it belongs to, reached from the engine
//!   (`engine.security().scan(…)`, S-829, [CR-259]): [`Graph`], [`Navigation`],
//!   [`Pipeline`], [`Quality`] (with [`Readouts`] and [`Review`]), [`History`],
//!   [`Coverage`], [`Security`] (with [`Findings`] and [`Tools`]) and
//!   [`Configuration`]. [`Setup`] bootstraps a project before any engine.
//! - Each façade borrows the engine and nothing else, so a façade method is
//!   still one call into `logos-core`, and its tracing instrumentation attaches
//!   where that call enters (NFR-CC-01, ADR-13).
//! - All methods return [`serde::Serialize`] read-models from [`crate::models`];
//!   adapters (`cli`, `mcp`) translate to their wire formats without any
//!   awareness of core internals (ADR-01, NFR-MA-02).
//! - The `Engine` is long-lived, rooted at one worktree root per process
//!   (ADR-04, ADR-15).
//! - Navigation/pipeline methods are infallible at the surface (degrade with
//!   warnings); the quality/governance methods (S-020) return `Result` —
//!   structural failures fail loud (ADR-14 Correctness), per-file failures
//!   degrade inside the read-model (`INCOMPLETE` freshness, NFR-RA-11).
//!
//! [CR-259]: ../../../docs/requests/CR-259-engine-is-split-by-feature-area.md

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::hydrate::{HydrationCache, HydrationConfig, SyncStamp};
use crate::runtime::{Runtime, RuntimeConfig, SharedWorkerPool};

mod config;
mod coverage;
mod findings;
mod graph;
mod history;
mod navigation;
mod pipeline;
mod quality;
mod readouts;
mod review;
mod security;
mod setup;
mod tools;

pub use config::Configuration;
pub use coverage::Coverage;
pub use findings::Findings;
pub use graph::Graph;
pub use history::History;
pub use navigation::Navigation;
pub use pipeline::Pipeline;
pub use quality::Quality;
pub use readouts::Readouts;
pub use review::Review;
pub use security::Security;
pub use setup::Setup;
pub use tools::Tools;

/// The target every `Engine` and façade log line is written under — the one
/// the engine's module path named before its façades moved into modules of
/// their own (S-829), so a `RUST_LOG` directive and the stderr lines read as
/// before.
const LOG_TARGET: &str = "logos_core::engine";

/// Thick-core engine — the single entry point to every Logos operation: its
/// lifecycle and the per-process state, and one accessor per feature-area
/// façade whose methods are the operations (`engine.security().scan(…)`,
/// S-829).
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
    /// build languages, resolve queries and compile any override; embedded
    /// queries compile on each language's first use, CR-197). `None` for a
    /// transient engine (built on demand per call) or if the substrate failed
    /// to load.
    registry: Option<crate::plugin::LanguageRegistry>,
    /// The bounded petgraph hydration cache, keyed by `(scope, last_sync_at)`
    /// ([ADR-04], [ADR-05], [NFR-PE-07]). Held for the Engine's lifetime so
    /// repeated aggregate runs reuse hydrated views; bounded so it stays within
    /// the RSS envelope ([NFR-PE-06], [AQ-02]). Present (but only *exercised* via
    /// [`Graph::hydrate`], which needs the runtime) on both engine flavours.
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
    /// The governance engine's in-process state ([S-020]): the compiled
    /// `rules.toml` cache (globs compiled once per content change, [FR-GV-01])
    /// and the last `scan` parameters `rescan` replays.
    ///
    /// [S-020]: ../../../docs/planning/journal.md#s-020-governance-engine-and-quality-gate
    /// [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
    governance: crate::governance::GovernanceState,
    /// The in-memory state extensions keep per engine ([FR-DS-04], [ADR-74]),
    /// one value per type, created on first use ([`Engine::extension_state`]).
    /// An extension works over `&Engine` and cannot add a field here, so a
    /// per-engine memo it owns — the wiki's native-tier render, memoized by the
    /// graph revision — lives in this slot instead. `Mutex` because the web
    /// surface shares the engine behind `Arc` across request tasks
    /// ([web-surface]).
    ///
    /// [FR-DS-04]: ../../../docs/specs/requirements/FR-DS-04.md
    /// [ADR-74]: ../../../docs/specs/architecture/decisions/ADR-74.md
    /// [web-surface]: ../../../docs/specs/architecture/components/web-surface.md
    extension_state: std::sync::Mutex<ExtensionState>,
    /// `status`'s per-language call residue ([FR-RS-09], CR-201), memoized by
    /// [`ResidueKey`]: `Some((key, residues))` when the walk at `key` is cached. A
    /// `status` at the same key is one point read; anything that moves the key
    /// re-walks. In memory only — the residue is computed, never stored.
    ///
    /// [FR-RS-09]: ../../../docs/specs/requirements/FR-RS-09.md
    call_residue: std::sync::Mutex<Option<(ResidueKey, ResidueByLanguage)>>,
    /// How many residue walks this engine has run: one per memo miss. Read by
    /// the tests that prove a hit walks nothing (S-605).
    call_residue_walks: AtomicU64,
    /// Where the security surfaces find the user-level directory and the
    /// scanner binaries; `None` reads the process environment on each call
    /// ([`Engine::with_scan_environment`]).
    scan_env: Option<crate::security::ScanEnvironment>,
    /// The tool manifest the security surfaces resolve managed copies and
    /// minimum versions against, and install from; `None` is the
    /// [builtin](crate::tools::Manifest::builtin) one
    /// ([`Engine::with_tools_manifest`]).
    tools_manifest: Option<crate::tools::Manifest>,
}

/// The per-language call residue, keyed by language name.
type ResidueByLanguage = std::collections::BTreeMap<String, crate::models::CallResidue>;

/// What [`Engine::extension_state`] holds: one shared value per type.
type ExtensionState = std::collections::HashMap<
    std::any::TypeId,
    std::sync::Arc<dyn std::any::Any + Send + Sync>,
>;

/// What [`Graph::call_residue`]'s memo is keyed on (CR-201): every input of
/// the walk that can change while an engine lives.
///
/// The graph is the revision ([FR-SY-09]); the schema version is there because
/// a migration can rewrite ledger rows without advancing it. The whole
/// `[resolution]` section is compared, not the keys the walk happens to read
/// today, so a key the binder starts reading is covered without an edit here.
/// The plugin registry is fixed for the engine's lifetime and needs no slot.
///
/// [FR-SY-09]: ../../../docs/specs/requirements/FR-SY-09.md
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResidueKey {
    graph_revision: u64,
    schema_version: i64,
    resolution: crate::config::Resolution,
}
/// Per-phase cold-start attribution, produced by
/// [`ColdStartPhases::measure`] ([CR-116], [NFR-PE-05], [S-368]).
///
/// [`Engine::start`] carries none of this timing — this struct and the function
/// that produces it exist to answer CR-116 §3.2: whether the phases [NFR-PE-05]
/// enumerated alone exceeded its then-500 ms budget, or whether the excess sat
/// in phases it did not enumerate (store open, schema migration, pool
/// startup). **It was the latter** — the three enumerated phases came in at
/// max 457.5 ms across eight fresh-process samples, so [S-369] amended
/// [NFR-PE-05] to enumerate all six phases below and re-derived its budget
/// from the measured full total to ≤ 600 ms. The six phases are now exactly
/// the requirement's enumeration; [`other`](Self::other) is the incidental
/// cold-path work the requirement's *total* bound also covers.
///
/// [CR-116]: ../../../docs/requests/CR-116-cold-start-budget-and-its-guard-disagree.md
/// [NFR-PE-05]: ../../../docs/specs/requirements/NFR-PE-05.md
/// [S-368]: ../../../docs/planning/journal.md#s-368-attribute-the-cold-start-cost-across-its-phases
/// [S-369]: ../../../docs/planning/journal.md#s-369-reconcile-the-cold-start-budget-with-what-it-actually-bounds
#[derive(Debug, Default, Clone, Copy)]
pub struct ColdStartPhases {
    /// Parsing every grammar's embedded `plugin.toml`.
    pub plugin_toml_parse: Duration,
    /// Everything else `LanguageRegistry::load` does besides parsing and
    /// query compilation: ABI assertion, override-dir resolution, and
    /// plugin/extension/filename bookkeeping.
    pub registry_construction: Duration,
    /// Resolving every capability's query and compiling the languages that
    /// carry an on-disk override. Embedded queries compile on their language's
    /// first use instead (CR-197), outside the cold start. Compiled queries are
    /// shared process-wide (HF-3), so an override compiles only on the
    /// process's first registry load; a later one times cache hits.
    pub query_compilation: Duration,
    /// Opening the writer store's file and applying the pragma contract.
    pub store_open: Duration,
    /// Running the writer store's schema migrations.
    pub schema_migration: Duration,
    /// Spawning the writer actor thread, opening the read-only pool, and
    /// building (or attaching) the worker pool.
    pub pool_startup: Duration,
    /// Everything on the cold path that is not one of the six phases above:
    /// worktree-root resolution, `.logos/` directory creation, worktree-seed
    /// detection/copy, governance-contract seeding, linked-worktree git-hook
    /// seeding (CR-106), constructing the `Engine` value itself, the
    /// coordinate migration (S-704) and the post-construction seed-diff
    /// reconcile. Exists
    /// so [`sum`](Self::sum) reconciles with the
    /// externally measured wall time rather than merely approximating it
    /// (CR-116 AC1).
    ///
    /// Not one of [NFR-PE-05]'s six enumerated phases, but inside its bound
    /// all the same: the requirement bounds the **total** wall time to a ready
    /// engine, precisely so no guard can pass by excluding work a user still
    /// waits through.
    ///
    /// [NFR-PE-05]: ../../../docs/specs/requirements/NFR-PE-05.md
    pub other: Duration,
}

impl ColdStartPhases {
    /// Sum of every phase, including [`other`](Self::other) — reconciles with
    /// the externally measured [`ColdStartPhases::measure`] wall time
    /// to within the noise of the handful of `Instant::now()` calls
    /// themselves (CR-116 AC1).
    pub fn sum(&self) -> Duration {
        self.plugin_toml_parse
            + self.registry_construction
            + self.query_compilation
            + self.store_open
            + self.schema_migration
            + self.pool_startup
            + self.other
    }

    /// The plugin-substrate subtotal: embedded `plugin.toml` parse +
    /// `LanguageRegistry` construction + query compilation. Store open, schema
    /// migration, pool startup and [`other`](Self::other) are excluded.
    ///
    /// These are the three phases [NFR-PE-05] enumerated **before** the
    /// [S-369] amendment, and this is the number that decided [CR-116] §3.2 —
    /// max 457.5 ms against a 500 ms budget over eight fresh-process samples,
    /// ruling out branch (a) (a genuine breach) in favour of branch (b) (the
    /// guards bounded more than the requirement enumerated).
    ///
    /// It is deliberately **not** named for [NFR-PE-05] any more. Since the
    /// amendment the requirement enumerates all six phases and bounds only
    /// their total, so no subset of them — this one included — is budgeted in
    /// its name (the same rule that retired the 200 ms store-and-pool guard's
    /// citation). Kept because it is the continuity of the measurement that
    /// settled the CR, and because query compilation dominating it (~434 of
    /// ~440 ms) is the standing fact about where cold start goes.
    ///
    /// [CR-116]: ../../../docs/requests/CR-116-cold-start-budget-and-its-guard-disagree.md
    /// [NFR-PE-05]: ../../../docs/specs/requirements/NFR-PE-05.md
    /// [S-369]: ../../../docs/planning/journal.md#s-369-reconcile-the-cold-start-budget-with-what-it-actually-bounds
    pub fn registry_subtotal(&self) -> Duration {
        self.plugin_toml_parse + self.registry_construction + self.query_compilation
    }

    /// The total of the **six** phases [NFR-PE-05] enumerates since [S-369]:
    /// the [`registry_subtotal`](Self::registry_subtotal) three plus store
    /// open, schema migration and pool startup. Excludes
    /// [`other`](Self::other).
    ///
    /// The requirement bounds [`sum`](Self::sum) — the whole wait — not this,
    /// so this is a diagnostic split (named phases versus incidental
    /// cold-path work), not a budget. `sum() - enumerated_phases()` is exactly
    /// [`other`](Self::other).
    ///
    /// [NFR-PE-05]: ../../../docs/specs/requirements/NFR-PE-05.md
    /// [S-369]: ../../../docs/planning/journal.md#s-369-reconcile-the-cold-start-budget-with-what-it-actually-bounds
    pub fn enumerated_phases(&self) -> Duration {
        self.registry_subtotal() + self.store_open + self.schema_migration + self.pool_startup
    }

    /// [`Engine::start`], instrumented with per-phase cold-start attribution
    /// ([CR-116], [NFR-PE-05], [S-368]).
    ///
    /// Mirrors [`Engine::start_with`] step for step, timing each phase with a
    /// plain `Instant` delta rather than routing through it. `Engine::start`
    /// carries **no** instrumentation — every `Instant` lives here — so this
    /// diagnostic path's own overhead never lands on the measured production
    /// cold start, and the delta between this function's wall time and
    /// `Engine::start`'s over the same root is the instrumentation's own cost
    /// (CR-116 R2).
    ///
    /// This is a *mirror*, not a refactor: the production path is never routed
    /// through this function. The steps the two share without a phase of their
    /// own — the worktree seed, the hook seeding and the open-time finish — are
    /// one helper each ([`seed_worktree`], [`seed_hooks`], [`finish_start`]),
    /// so they cannot drift apart; an edit to the rest of
    /// [`Engine::start_with`]'s call sequence must be applied here too. S-338
    /// (hook seeding), S-369 (the shared `--git-common-dir` resolution) and
    /// S-704 (the coordinate migration) each have a side-effect guard
    /// (`worktree_hooks.rs::a_phase_reported_engine_start_seeds_the_worktree_hooks_too`,
    /// `worktree.rs::a_phase_reported_engine_start_seeds_the_store_and_the_contract_too`
    /// and
    /// `coordinate_migration.rs::a_phase_reported_engine_start_migrates_the_store_too`).
    ///
    /// [`ColdStartPhases::other`] absorbs everything that is not one of the
    /// six phases [NFR-PE-05] enumerates — root resolution, directory
    /// creation, worktree-seed detection/copy, governance-contract seeding
    /// (FR-WT-06), linked-worktree git-hook seeding (CR-106), engine
    /// construction, the coordinate migration (S-704) and the
    /// post-construction seed-diff reconcile — so
    /// [`ColdStartPhases::sum`] reconciles with the externally measured wall
    /// time to within the noise of the handful of `Instant::now()` calls
    /// themselves, not merely "within a stated margin" by approximation.
    ///
    /// # Errors
    /// Same as [`Engine::start`].
    ///
    /// [CR-116]: ../../../docs/requests/CR-116-cold-start-budget-and-its-guard-disagree.md
    /// [NFR-PE-05]: ../../../docs/specs/requirements/NFR-PE-05.md
    /// [S-368]: ../../../docs/planning/journal.md#s-368-attribute-the-cold-start-cost-across-its-phases
    pub fn measure(root: impl AsRef<Path>) -> Result<(Engine, Self)> {
        let mut phases = Self::default();

        let t_other = Instant::now();
        let root = crate::workspace::resolve_root(root.as_ref());
        let logos_dir = root.join(".logos");
        std::fs::create_dir_all(&logos_dir)
            .with_context(|| format!("creating the .logos directory at {}", logos_dir.display()))?;
        let db_path = logos_dir.join("logos.db");
        let seed = seed_worktree(&root, &db_path);
        // The hook-reachability step (CR-106, S-338) runs on every start and
        // belongs inside the measured cold path, attributed to `other`.
        // Omitting it here would make the attribution understate the
        // production total by exactly this step's cost on the roots where it
        // does work (a linked worktree with hooks installed: one
        // `--git-common-dir` subprocess plus a readlink/symlink per managed
        // hook), which is the wrong direction for a measurement CR-116 §3.2 is
        // decided on. `a_phase_reported_engine_start_seeds_the_worktree_hooks_too`
        // (logos-core/tests/worktree_hooks.rs) is the guard that keeps the two
        // step lists together.
        seed_hooks(&root);
        phases.other += t_other.elapsed();

        let (runtime, runtime_timings) =
            Runtime::open_with_config_timed(&db_path, RuntimeConfig::default()).with_context(
                || format!("starting the execution runtime for root {}", root.display()),
            )?;
        phases.store_open = runtime_timings.store_connect;
        phases.schema_migration = runtime_timings.schema_migration;
        phases.pool_startup = runtime_timings.pool_startup;

        let registry = match crate::plugin::LanguageRegistry::load_with_timings(&root) {
            Ok((registry, reg_timings)) => {
                phases.plugin_toml_parse = reg_timings.manifest_parse;
                phases.query_compilation = reg_timings.query_compile;
                phases.registry_construction = reg_timings.construction;
                Some(registry)
            }
            Err(err) => {
                tracing::warn!("could not load plugin registry at startup: {err}");
                None
            }
        };

        let t_seed = Instant::now();
        let engine = Engine {
            root,
            runtime: Some(runtime),
            registry,
            hydration: HydrationCache::new(HydrationConfig::default()),
            sync_stamp: AtomicU64::new(SyncStamp::INITIAL.0),
            nav_prologue_done: AtomicBool::new(false),
            governance: crate::governance::GovernanceState::default(),
            extension_state: std::sync::Mutex::default(),
            call_residue: std::sync::Mutex::new(None),
            call_residue_walks: AtomicU64::new(0),
            scan_env: None,
            tools_manifest: None,
        };
        finish_start(&engine, seed);
        phases.other += t_seed.elapsed();

        Ok((engine, phases))
    }
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
            governance: crate::governance::GovernanceState::default(),
            extension_state: std::sync::Mutex::default(),
            call_residue: std::sync::Mutex::new(None),
            call_residue_walks: AtomicU64::new(0),
            scan_env: None,
            tools_manifest: None,
        }
    }

    /// Start a **long-lived** engine rooted at `root`, ready to serve reads and
    /// writes ([ADR-04]).
    ///
    /// Ensures `<root>/.logos/` exists, then brings up the [`Runtime`] over
    /// `<root>/.logos/logos.db`: the writer opens and migrates the store, the
    /// read-only pool attaches, and the shared worker pool spins up. The whole
    /// sequence — every phase of it, plus the incidental root/`.logos`/seed
    /// work around them — is the cold-start path [NFR-PE-05] bounds at
    /// ≤ 600 ms in total ([CR-116] §9, [S-369]); afterwards
    /// the engine is held for the process lifetime so the pools (and the derived
    /// caches layered on them) are reused across calls.
    ///
    /// [`Engine::start_with`] under [`StartOptions::default`].
    ///
    /// # Errors
    /// Returns an error if `.logos/` cannot be created or the runtime cannot be
    /// opened (store open/migrate, reader pool, or worker pool failure).
    ///
    /// [CR-116]: ../../../docs/requests/CR-116-cold-start-budget-and-its-guard-disagree.md
    /// [NFR-PE-05]: ../../../docs/specs/requirements/NFR-PE-05.md
    /// [S-369]: ../../../docs/planning/journal.md#s-369-reconcile-the-cold-start-budget-with-what-it-actually-bounds
    pub fn start(root: impl AsRef<Path>) -> Result<Self> {
        Self::start_with(root, StartOptions::default())
    }

    /// Start a long-lived engine like [`Engine::start`], under `options`: an
    /// explicit hydration-cache bound ([`StartOptions::hydration`]) or a
    /// budgeted share of a workspace's pools ([`StartOptions::pools`]).
    ///
    /// The shared start path: resolve the root, seed a DB-less worktree, bring
    /// up the runtime under the options' [`RuntimeConfig`], and load the plugin
    /// substrate.
    ///
    /// # Errors
    /// Same as [`Engine::start`]; additionally, a `read_connections` of zero is
    /// rejected by the pool (no reader could serve navigation).
    pub fn start_with(root: impl AsRef<Path>, options: StartOptions) -> Result<Self> {
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

        let seed = seed_worktree(&root, &db_path);
        seed_hooks(&root);

        let runtime = Runtime::open_with_config(&db_path, options.runtime).with_context(|| {
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
            hydration: HydrationCache::new(options.hydration),
            sync_stamp: AtomicU64::new(SyncStamp::INITIAL.0),
            nav_prologue_done: AtomicBool::new(false),
            governance: crate::governance::GovernanceState::default(),
            extension_state: std::sync::Mutex::default(),
            call_residue: std::sync::Mutex::new(None),
            call_residue_walks: AtomicU64::new(0),
            scan_env: None,
            tools_manifest: None,
        };
        finish_start(&engine, seed);
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

    /// This engine's instance of the extension state `T` ([FR-DS-04],
    /// [ADR-74]), created with `T::default()` on first use and shared by every
    /// later call on the same engine.
    ///
    /// An extension works over `&Engine` and owns no field on it, so a memo it
    /// keeps per engine — the wiki's native-tier render, keyed by the graph
    /// revision — lives here, and lives exactly as long as the engine does. A
    /// type is a slot: two extensions that want separate state declare
    /// separate types.
    ///
    /// [FR-DS-04]: ../../../docs/specs/requirements/FR-DS-04.md
    /// [ADR-74]: ../../../docs/specs/architecture/decisions/ADR-74.md
    pub fn extension_state<T: Default + Send + Sync + 'static>(&self) -> std::sync::Arc<T> {
        let mut slots = self.extension_state.lock().expect("extension-state lock");
        let slot = slots
            .entry(std::any::TypeId::of::<T>())
            .or_insert_with(|| std::sync::Arc::new(T::default()));
        std::sync::Arc::clone(slot)
            .downcast::<T>()
            .unwrap_or_else(|_| unreachable!("an extension-state slot is keyed by its own type"))
    }

    /// Spawn the debounced filesystem watcher over this engine's root
    /// ([S-022], [FR-SY-04]): each debounce window's changes coalesce into one
    /// [`Pipeline::sync`] submitted through the single-writer actor ([ADR-02]).
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

    /// The resolved worktree root (ADR-15).
    ///
    /// Public so a chat surface a build composes ([web-surface], S-170) can
    /// resolve the project-root paths it needs beside the engine — the gitignored
    /// `.logos/chat.db` memory/conversation store and the source-tool sandbox root.
    /// It exposes a path the engine already resolved; it is **not** a graph or
    /// governance query (the agent adds none, [ADR-41]) and writes nothing.
    ///
    /// [web-surface]: ../../../docs/specs/architecture/components/web-surface.md
    /// [ADR-41]: ../../../docs/specs/architecture/decisions/ADR-41.md
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// This engine with the security surfaces looking in `env` instead of the
    /// process environment: the user-level directory (overrides and consent)
    /// and the scanner search path. The coverage execution consent (S-736) is
    /// read from and recorded in the same user-level directory. For an
    /// embedder, and for tests that must not change the process environment.
    #[must_use]
    pub fn with_scan_environment(mut self, env: crate::security::ScanEnvironment) -> Self {
        self.scan_env = Some(env);
        self
    }

    /// This engine with the security surfaces resolving managed copies and
    /// minimum versions against `manifest`, and installing from it, instead
    /// of the builtin manifest. For tests: fixture artifacts, or a manifest
    /// pinning a newer version. No shipped surface calls it.
    #[must_use]
    pub fn with_tools_manifest(mut self, manifest: crate::tools::Manifest) -> Self {
        self.tools_manifest = Some(manifest);
        self
    }

    // ── The feature-area façades (S-829) ─────────────────────────────────

    /// The code graph's state: hydrated views, the sync clock, the index's
    /// status and inventory.
    pub fn graph(&self) -> Graph<'_> {
        Graph { engine: self }
    }

    /// The navigation tools (FR-NV-01..13) and the runtime they read through.
    pub fn navigation(&self) -> Navigation<'_> {
        Navigation { engine: self }
    }

    /// The indexing pipeline: `index`, `sync`, `ensure_indexed`.
    pub fn pipeline(&self) -> Pipeline<'_> {
        Pipeline { engine: self }
    }

    /// The quality and governance evaluations, with the readouts and the
    /// branch review beneath them.
    pub fn quality(&self) -> Quality<'_> {
        Quality { engine: self }
    }

    /// The git-history surfaces: mining, the temporal report, the hotspots.
    pub fn history(&self) -> History<'_> {
        History { engine: self }
    }

    /// The coverage evidence surfaces: ingest, run, status, install.
    pub fn coverage(&self) -> Coverage<'_> {
        Coverage { engine: self }
    }

    /// The security surfaces, with the findings and the scanner tools beneath
    /// them.
    pub fn security(&self) -> Security<'_> {
        Security { engine: self }
    }

    /// The checked-in policy files: read, write, apply.
    pub fn config(&self) -> Configuration<'_> {
        Configuration { engine: self }
    }
}

/// How [`Engine::start_with`] starts an engine: the hydration-cache bound and
/// the runtime's pools. The default is [`Engine::start`]'s — the default cache
/// bound and core-sized pools.
#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    /// The bounded petgraph hydration cache's configuration ([ADR-04],
    /// [ADR-05]).
    pub hydration: HydrationConfig,
    /// The runtime's pools: the read-only connections and the worker pool.
    pub runtime: RuntimeConfig,
}

impl StartOptions {
    /// An explicit hydration-cache bound.
    ///
    /// This is the seam through which the deferred [AQ-02]/[AA-04] RSS-budget
    /// decision is wired without touching call sites: when the cache ceiling is
    /// resolved during dogfood profiling it becomes a [`HydrationConfig`] passed
    /// here (or a default change), not a code rewrite. [`Engine::start`] uses
    /// [`HydrationConfig::default`].
    ///
    /// [AQ-02]: ../../../docs/specs/architecture.md#14-open-questions
    /// [AA-04]: ../../../docs/specs/architecture.md#24-assumptions
    pub fn hydration(hydration: HydrationConfig) -> Self {
        Self { hydration, ..Self::default() }
    }

    /// A **budgeted share** of a workspace's resources rather than the ones an
    /// engine would size for itself ([NFR-PE-11], [ADR-63]): exactly
    /// `read_connections` read-only connections instead of one per core, and —
    /// when `worker_pool` is `Some` — the workspace's one shared `rayon` pool
    /// instead of a private core-sized one.
    ///
    /// The seam a **workspace** uses: the [federation registry] hands every
    /// resident member engine the share of the workspace-wide
    /// [`WorkspaceBudget`](crate::federation::WorkspaceBudget) it is entitled
    /// to, so N members cost a host-derived ceiling rather than `N × cores` live
    /// connections and `N × cores` worker threads. Nothing else calls this — the
    /// single-root path stays on [`Engine::start`] and its core-sized pools,
    /// byte-for-byte as today ([FR-WS-03], [ADR-52]).
    ///
    /// A `worker_pool` of `None` builds a private pool exactly as
    /// [`Engine::start`] does, which is what makes the sharing *injected rather
    /// than discovered*: an engine given no pool has no way to find one.
    ///
    /// [federation registry]: crate::federation::EngineRegistry
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    /// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
    /// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
    /// [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md
    pub fn pools(read_connections: usize, worker_pool: Option<SharedWorkerPool>) -> Self {
        Self {
            runtime: RuntimeConfig { reader_pool_size: read_connections, worker_pool, ..RuntimeConfig::default() },
            ..Self::default()
        }
    }
}

/// Seed-from-main bootstrap ([ADR-15], [FR-WT-03]): first use in a DB-less
/// linked worktree copies the primary checkout's DB; the main↔branch diff is
/// reconciled by [`finish_start`] once the runtime and registry are up. A failed
/// copy — or no seed at all — falls back to a fresh store, which the auto-index
/// prologue ([FR-IX-07]) then full-indexes. Returns the seed copied, if any.
///
/// Both start paths — [`Engine::start_with`] and the phase-reported
/// [`ColdStartPhases::measure`] — run this one step, so the attribution times
/// exactly the production cold path's seeding.
///
/// [FR-WT-03]: ../../../docs/specs/requirements/FR-WT-03.md
/// [FR-IX-07]: ../../../docs/specs/requirements/FR-IX-07.md
fn seed_worktree(root: &Path, db_path: &Path) -> Option<crate::workspace::SeedSource> {
    if db_path.exists() {
        return None;
    }
    // ONE `git rev-parse --git-common-dir` for both seeds (CR-116
    // §9 item 5, S-369). The graph-store seed and the governance-contract
    // seed below both need the primary checkout, and each used to
    // resolve it with its own identical subprocess — pure waste on a
    // cold path budgeted by NFR-PE-05, and the more schedule-sensitive
    // half of it, since a process spawn varies far more than in-process
    // work. `None` here means "no distinct primary": neither seed runs,
    // exactly as before.
    let primary = crate::workspace::primary_root(root);

    let seed = primary
        .clone()
        .and_then(crate::workspace::seed_source_from_primary)
        .and_then(|seed| match crate::graph_store::seed_copy(&seed.db_path, db_path) {
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
                // runtime opens a fresh store.
                tracing::warn!(
                    "seed-from-main copy failed; falling back to a fresh store \
                     and a full index: {err:#}"
                );
                None
            }
        });

    // Reuses the `primary` resolved above rather than re-running the
    // identical `--git-common-dir` query (CR-116 §9 item 5, S-369).
    if let Some(primary) = primary {
        seed_contract(&primary, root);
    }
    seed
}

/// Governance-contract seeding ([FR-WT-06], CR-112): independent of whether
/// the graph-store copy succeeded — a `rules.toml` carried into a DB-less
/// worktree is worth having even if the graph seed failed, and vice versa.
/// `.logos/` is conventionally gitignored, so without this the seeded graph
/// would have no architecture contract and every governance evaluation inside
/// the worktree would be vacuous. Each file is copied verbatim (never parsed
/// or rewritten), so no copied value can become an absolute path into the
/// primary checkout (FR-IN-06). A file the worktree already has of its own —
/// travelled through git (FR-WT-02) or left by a prior partial seed — is left
/// alone.
///
/// [FR-WT-06]: ../../../docs/specs/requirements/FR-WT-06.md
fn seed_contract(primary: &Path, root: &Path) {
    let contract = crate::workspace::seed_contract(primary, root);
    for (name, outcome) in [("rules.toml", contract.rules), ("config.toml", contract.config)] {
        match outcome {
            crate::workspace::ContractFileOutcome::Copied => {
                tracing::info!(
                    file = name,
                    primary = %primary.display(),
                    "seeded the governance contract file from the primary checkout (FR-WT-06)"
                );
            }
            crate::workspace::ContractFileOutcome::Absent | crate::workspace::ContractFileOutcome::AlreadyPresent => {}
            crate::workspace::ContractFileOutcome::Failed(error) => {
                // Fail-soft, matching the graph-store seed (ADR-11): the
                // worktree stays usable, just without this policy file.
                tracing::warn!(
                    file = name,
                    primary = %primary.display(),
                    error = %error,
                    "seeding {name} from the primary checkout failed; the \
                     worktree's governance contract may be incomplete"
                );
            }
        }
    }
}

/// Hook reachability (CR-106, [FR-IN-03], [FR-IN-06]): deliberately OUTSIDE
/// [`seed_worktree`]'s DB-less gate. `core.hooksPath` is the relative
/// `.logos/hooks`, which git resolves against *this* working tree, so a linked
/// worktree needs its own hooks directory or no logos git hook fires in it at
/// all — including the blocking `pre-push` gate. Gating this on "first use"
/// would cover only worktrees created after the installation; running it every
/// start covers a worktree that predates it too, on its next logos command. It
/// costs one `stat` outside a linked worktree (the `.git`-is-a-file
/// discriminator) and seeds nothing when hooks are not installed — opt-in stays
/// opt-in.
///
/// [FR-IN-03]: ../../../docs/specs/requirements/FR-IN-03.md
/// [FR-IN-06]: ../../../docs/specs/requirements/FR-IN-06.md
fn seed_hooks(root: &Path) {
    if let Some(seeded) = crate::hooks::seed_from_primary(root) {
        for warning in &seeded.warnings {
            tracing::warn!("seeding this worktree's git hooks: {warning}");
        }
        if !seeded.is_empty() {
            tracing::info!(
                linked = seeded.linked.len(),
                copied = seeded.copied.len(),
                "made the installed git hooks reachable from this worktree (CR-106)"
            );
        }
    }
}

/// The open-time steps every start runs once `engine` is assembled. Move a
/// store whose symbols carry another coordinate than the project's — one built
/// before ADR-85, or a changed `[project] id` or remote — before anything syncs
/// into it (FR-EX-35, S-704); a just-seeded worktree store migrates here too,
/// so the diff reconcile re-extracts under the coordinate the seed now carries.
/// Then read `.h` as the store's graph was built, and finish the worktree
/// bootstrap: reconcile only the main↔branch diff over the just-seeded store —
/// O(diff-from-main), not O(repo) ([FR-WT-03]). Fail-soft by design.
///
/// [FR-WT-03]: ../../../docs/specs/requirements/FR-WT-03.md
fn finish_start(engine: &Engine, seed: Option<crate::workspace::SeedSource>) {
    let pipeline: Pipeline<'_> = engine.pipeline();
    pipeline.migrate_symbol_coordinate();
    pipeline.route_shared_extensions();
    if let Some(seed) = seed {
        pipeline.reconcile_seed_diff(&seed);
    }
}

/// The indexed-file set an evidence report's paths bind against, supplied
/// at the api layer from a graph read so `history.db` is never `ATTACH`-ed to
/// the canonical graph ([BR-28], [ADR-79]). A transient or empty graph yields
/// an empty set, so every report path is honestly reported unmatched. Shared
/// by the coverage and security ingests — one binder, one indexed set.
///
/// [ADR-79]: ../../../docs/specs/architecture/decisions/ADR-79.md
/// [BR-28]: ../../../docs/specs/software-spec.md#323-coverage-test-evidence
fn evidence_indexed_paths(engine: &Engine) -> Result<Vec<String>> {
    Ok(match engine.runtime() {
        Some(rt) => rt
            .submit_read(|store| store.indexed_files())?
            .into_iter()
            .map(|f| f.path)
            .collect(),
        None => Vec::new(),
    })
}

/// Today's date in UTC, from the system clock — the "today" suppression
/// expiry is judged against. Read here, at the surface layer, and passed down:
/// the evidence store never reads the clock ([BR-27]).
///
/// [BR-27]: ../../../docs/specs/software-spec.md
pub(crate) fn today_utc() -> chrono::NaiveDate {
    utc_date_of(std::time::SystemTime::now())
}

/// The UTC calendar date `instant` falls on; an instant before the epoch reads
/// as the epoch's date.
fn utc_date_of(instant: std::time::SystemTime) -> chrono::NaiveDate {
    let days = instant
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400);
    chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("the epoch is a date")
        + chrono::Days::new(days)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    use crate::models::navigation::{
        CalleesResult, CallersResult, ContextBundle, ExploreResult, ImpactResult, ImplementorsResult, NodeInfo,
        ReferencingDocsResult, SearchResult, StatusInfo,
    };
    use crate::models::pipeline::{IndexResult, InitResult, SyncResult};
    use crate::models::quality::{
        DocGapsReport, DsmReport, EvolutionReport, GateResult, HealthInfo, LanguagesInfo, RulesReport, ScanResult,
        SessionInfo, StatsInfo,
    };
    use crate::observability::Tool;

    /// The two security reads are classed as quality verdicts and the scan and
    /// ingests as engine-internal (S-667) — what `stats` attributes the calls
    /// to. That each method traces under its own tool is
    /// `tests/security_telemetry.rs`, a binary of its own: the shared emission
    /// callsite's interest is cached across concurrently running tests.
    #[test]
    fn the_security_tools_are_classed_as_reads_and_writes() {
        use crate::observability::ToolClass;
        assert_eq!(Tool::SecurityStatus.tool_class(), ToolClass::QualityGate);
        assert_eq!(Tool::SecurityFindings.tool_class(), ToolClass::QualityGate);
        for internal in [Tool::SecurityScan, Tool::SecurityIngest, Tool::SecurityIngestAuto, Tool::WatchSecurityIngest] {
            assert_eq!(internal.tool_class(), ToolClass::EngineInternal, "{internal:?}");
        }
    }

    /// `today` for suppression expiry is the UTC date: a day starts at
    /// midnight UTC and lasts 86 400 s.
    #[test]
    fn the_utc_date_of_an_instant_turns_at_midnight() {
        use std::time::{Duration, UNIX_EPOCH};
        let date = |secs: u64| utc_date_of(UNIX_EPOCH + Duration::from_secs(secs)).to_string();
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(86_399), "1970-01-01");
        assert_eq!(date(86_400), "1970-01-02");
        // 2026-10-16T00:00:00Z, and one second before it.
        assert_eq!(date(1_792_108_800), "2026-10-16");
        assert_eq!(date(1_792_108_799), "2026-10-15");
        assert_eq!(utc_date_of(UNIX_EPOCH - Duration::from_secs(5)).to_string(), "1970-01-01");
    }

    /// A **hand-maintained roster** of engine return types that satisfy the
    /// `serde::Serialize` adapter contract (ADR-01, NFR-MA-02). It is
    /// documentation, and it says so because it was read as a gate and is not
    /// one.
    ///
    /// # What it does not do
    ///
    /// There is no completeness check, and nothing ties this list to `impl
    /// Engine`. Sprint 72's review proved the consequence rather than asserting
    /// it: adding a public `Engine` method returning a newly serialized quality
    /// shape left **2261 tests green**. `LatestHealth`, `QualityReadout`,
    /// `MetricSnapshot`, `VerifyReport` and roughly twenty more return types
    /// are absent below and have shipped that way for many sprints.
    ///
    /// # Why it stays documentation
    ///
    /// The bound is already enforced where it is actually needed: every adapter
    /// that serializes one of these types fails to compile without it, so a
    /// return type that is genuinely not `Serialize` is caught by the surface
    /// that serializes it. Deriving the roster from `impl Engine`'s return
    /// types would make the list complete and would catch nothing the adapters
    /// do not — work that manufactures a gate rather than closing a hole. The
    /// group counts below are descriptive labels on that reading, kept correct
    /// because a wrong count is what made this list look like a census.
    ///
    /// Sprint 72 sprint review, deferred item 5.10; decided 2026-09-20. The
    /// generic bound is used rather than a runtime `todo!()` so the test passes
    /// without panicking.
    #[test]
    fn rostered_engine_return_types_are_serialize() {
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

        // Quality / governance (8)
        assert_serialize::<ScanResult>();
        assert_serialize::<GateResult>();
        assert_serialize::<RulesReport>();
        assert_serialize::<EvolutionReport>();
        assert_serialize::<DsmReport>();
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
        assert_serialize::<crate::coverage::CoverageRunSummary>();
        // Config write-back (S-096): the read-model + write outcome.
        assert_serialize::<crate::config::ConfigReadModel>();
        assert_serialize::<crate::config::ConfigWriteOutcome>();
        // Config apply (S-097): the explicit-apply summary.
        assert_serialize::<crate::config::ConfigApplyOutcome>();
    }

    /// [`ColdStartPhases`]'s three totals ([CR-116], [S-368], [S-369]) as pure
    /// functions over known field values — checked exactly rather than only
    /// ever against noisy wall-clock reconciliation in the integration test.
    ///
    /// Powers of two are used as the field values so each total can only come
    /// out right by summing exactly the intended subset: any wrong subset
    /// produces a different number.
    ///
    /// [CR-116]: ../../../docs/requests/CR-116-cold-start-budget-and-its-guard-disagree.md
    /// [S-368]: ../../../docs/planning/journal.md#s-368-attribute-the-cold-start-cost-across-its-phases
    /// [S-369]: ../../../docs/planning/journal.md#s-369-reconcile-the-cold-start-budget-with-what-it-actually-bounds
    #[test]
    fn cold_start_phase_totals_are_exact() {
        let phases = ColdStartPhases {
            plugin_toml_parse: Duration::from_millis(1),
            registry_construction: Duration::from_millis(2),
            query_compilation: Duration::from_millis(4),
            store_open: Duration::from_millis(8),
            schema_migration: Duration::from_millis(16),
            pool_startup: Duration::from_millis(32),
            other: Duration::from_millis(64),
        };
        // The whole measured wall time — what NFR-PE-05 bounds at ≤600 ms
        // since S-369.
        assert_eq!(
            phases.sum(),
            Duration::from_millis(1 + 2 + 4 + 8 + 16 + 32 + 64)
        );
        // The six phases NFR-PE-05 enumerates: everything but `other`.
        assert_eq!(
            phases.enumerated_phases(),
            Duration::from_millis(1 + 2 + 4 + 8 + 16 + 32)
        );
        // The plugin-substrate subtotal alone — the pre-amendment three-phase
        // figure that decided CR-116 §3.2. Store open, schema migration, pool
        // startup and `other` must NOT be folded in.
        assert_eq!(phases.registry_subtotal(), Duration::from_millis(1 + 2 + 4));
        // The split is exhaustive by construction: the phases NFR-PE-05 names
        // plus the incidental work it does not name is the whole bound.
        assert_eq!(phases.enumerated_phases() + phases.other, phases.sum());
    }

    /// The config write-back seam round-trips through the façade (S-096,
    /// [FR-UI-12]): `config().read` returns content + parsed model, a valid
    /// `config().write` is reflected on the next read, and an invalid candidate
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
            .config()
            .write(PolicyFile::Config, "max_file_size = 4096\n")
            .expect("a valid config write succeeds");
        let docs = engine.config().read(None).expect("config_read");
        assert_eq!(docs.config.parsed.max_file_size, 4096);
        assert!(docs.config.content.contains("max_file_size = 4096"));

        // A rules write carries the provenance stamp and still parses on read.
        let written = engine
            .config()
            .write(PolicyFile::Rules, "[constraints]\nmax_cc = 9\n")
            .expect("a valid rules write succeeds");
        assert!(written.provenance_stamped);
        let docs = engine.config().read(None).expect("config_read after rules write");
        assert_eq!(
            docs.rules.parsed.constraints.max_cc,
            Some(crate::config::Budget::Absolute(9))
        );
        assert!(docs.rules.content.contains("CR-025"));

        // An invalid candidate is rejected with a typed error; the file is intact.
        let before = std::fs::read_to_string(tmp.path().join(".logos/config.toml")).unwrap();
        let err = engine
            .config()
            .write(PolicyFile::Config, "unknown_key = true\n")
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

    /// Every navigation tool degrades on a transient engine to its own
    /// read-model, still naming what it was asked, with one warning
    /// `<tool> failed: <reason>` (ADR-14) — the text each tool's own fallback
    /// wrote before S-829 routed them through one degrade path.
    #[test]
    fn each_navigation_tool_degrades_with_its_own_warning_and_query() {
        let engine = Engine::open("/tmp");
        let nav = engine.navigation();
        let reason = "failed: navigation requires a long-lived engine";
        let expect = |tool: &str, warnings: &[String]| {
            assert_eq!(warnings.len(), 1, "{tool}: {warnings:?}");
            assert!(warnings[0].starts_with(&format!("{tool} {reason}")), "{tool}: {warnings:?}");
        };
        let r = nav.search("q", None, None);
        assert_eq!(r.query, "q");
        expect("search", &r.warnings);
        let r = nav.context("q", None, false);
        assert_eq!(r.task, "q");
        expect("context", &r.warnings);
        let r = nav.explore("q", None);
        assert_eq!(r.query, "q");
        expect("explore", &r.warnings);
        let r = nav.node("q", false);
        assert_eq!(r.query, "q");
        expect("node", &r.warnings);
        let r = nav.callers("q", None);
        assert_eq!(r.query, "q");
        expect("callers", &r.warnings);
        let r = nav.callees("q", None);
        assert_eq!(r.query, "q");
        expect("callees", &r.warnings);
        let r = nav.impact("q", None);
        assert_eq!(r.query, "q");
        expect("impact", &r.warnings);
        let r = nav.implements("q");
        assert_eq!(r.query, "q");
        expect("implements", &r.warnings);
        let r = nav.referencing_docs("q");
        assert_eq!(r.query, "q");
        expect("referencing_docs", &r.warnings);
        let r = nav.affected(&["a.rs".to_string()], true);
        assert_eq!((r.changed.as_slice(), r.tests_only), (["a.rs".to_string()].as_slice(), true));
        expect("affected", &r.warnings);
    }

    /// The config reads and the secret write keep their typed
    /// [`ConfigError`](crate::config::ConfigError) through the façade's one
    /// conversion: a surface maps it to a usage error (exit 2, HTTP 422) only
    /// by downcasting.
    #[test]
    fn config_read_and_secret_write_keep_the_typed_config_error() {
        let tmp = tempfile::TempDir::new().expect("temp root");
        std::fs::create_dir_all(tmp.path().join(".logos")).expect(".logos");
        std::fs::write(tmp.path().join(".logos/config.toml"), "unknown_key = true\n").expect("config.toml");
        std::fs::write(tmp.path().join(".logos/secrets.toml"), "not toml [\n").expect("secrets.toml");
        let engine = Engine::open(tmp.path());
        let read = engine.config().read(None).expect_err("an invalid config.toml fails the read");
        assert!(read.downcast_ref::<crate::config::ConfigError>().is_some(), "read: {read:#}");
        let secret = engine.config().write_secret("k").expect_err("an unparseable secrets.toml fails the write");
        assert!(secret.downcast_ref::<crate::config::ConfigError>().is_some(), "write_secret: {secret:#}");
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
        let result = engine.pipeline().index();
        assert_eq!(result.files_indexed, 0, "no runtime → nothing indexed");
        assert!(
            result.warnings.iter().any(|w| w.contains("failed")),
            "the failure reason is surfaced in warnings, got {:?}",
            result.warnings
        );
        // A failed pipeline is a correctness failure, never an empty success
        // (FR-EH-05): the CLI exits 1 on `failed`.
        assert!(result.failed, "a failed pipeline is marked failed");
        // `sync` and `ensure_indexed` share the same degradation path.
        let synced = engine.pipeline().sync(&[PathBuf::from("a.rs")]);
        assert!(synced.warnings.iter().any(|w| w.contains("failed")));
        assert!(synced.failed, "a failed sync pipeline is marked failed");
        let ensured = engine.pipeline().ensure_indexed();
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
        engine.pipeline().reconcile_seed_diff(&seed); // must not panic or error out

        let status = engine.graph().status();
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

    /// `Graph::languages()` lists the Rust grammar with its full descriptor
    /// through the public facade (FR-PL-06). Exercises the read-model mapping
    /// (including the `skipped` list) that the CLI/MCP surfaces emit.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn languages_lists_rust_with_descriptor() {
        let tmp = std::env::temp_dir(); // no overrides under here
        let info = Engine::open(&tmp).graph().languages();

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
        // …and declares its cross-file reach (S-570, FR-PL-09), surfaced as the
        // descriptor spells it.
        let reach = rust.reach.as_ref().expect("a code grammar declares a reach");
        assert_eq!(reach.level, "resolved");
        assert_eq!(reach.cross_file, ["calls", "imports", "type_relations"]);
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

        let info = graph::languages_from(&reg);
        let docker = info
            .languages
            .iter()
            .find(|l| l.name == "dockerfile")
            .expect("the artifact plugin is listed");
        assert!(docker.artifact, "listed as the artifact class");
        assert_eq!(docker.extensions, ["dockerfile"], "extension claim listed");
        assert_eq!(docker.filenames, ["Dockerfile"], "filename claim listed");
    }

    /// One extension's per-engine state, for the [`Engine::extension_state`]
    /// tests: a counter that shows which instance a caller was handed.
    #[derive(Default)]
    struct Tally(AtomicU64);

    /// A second, unrelated extension's state.
    #[derive(Default)]
    struct OtherTally(AtomicU64);

    /// [`Engine::extension_state`] hands every caller on one engine the same
    /// value for a type — the memo an extension keeps there survives between
    /// its calls — keeps a second type's value apart, and gives another engine
    /// its own (S-644: the wiki's native-tier memo moved into this slot from an
    /// `Engine` field, so it must stay per engine, not per process).
    #[test]
    fn extension_state_is_one_value_per_type_per_engine() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let engine = Engine::open(tmp.path());

        engine.extension_state::<Tally>().0.fetch_add(3, Ordering::Relaxed);
        let again = engine.extension_state::<Tally>();
        assert_eq!(again.0.load(Ordering::Relaxed), 3, "a later call sees the first call's value");
        assert!(
            std::sync::Arc::ptr_eq(&again, &engine.extension_state::<Tally>()),
            "one shared value per type"
        );

        assert_eq!(
            engine.extension_state::<OtherTally>().0.load(Ordering::Relaxed),
            0,
            "a second type is a second slot"
        );

        let other = Engine::open(tmp.path());
        assert_eq!(
            other.extension_state::<Tally>().0.load(Ordering::Relaxed),
            0,
            "another engine starts from its own default"
        );
    }
}
