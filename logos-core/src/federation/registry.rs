//! The `root → Engine` registry and its repo-qualified fan-out helper
//! ([FR-WS-03], [NFR-PE-10], [ADR-52]).
//!
//! A workspace federates N member repositories, each with its **own**
//! [`Engine`](crate::Engine) over its own `.logos/logos.db` ([ADR-52]). This
//! module multiplexes those engines behind one [`EngineRegistry`] so a
//! cross-service query can [`fan_out`](EngineRegistry::fan_out) over members and
//! tag each result with the member that produced it, or reach a single member
//! through [`engine_for`](EngineRegistry::engine_for).
//!
//! # Construction policy ([NFR-PE-10])
//! Member engines are **not** all built up front. A [`RegistryMode::Lazy`]
//! registry (CLI one-shots) builds a member's engine only when a command first
//! touches it, so a scoped answer constructs only the engines it needs — never
//! all N. Under [`RegistryMode::Serve`] the mode-level invariant is **watch-on-
//! touch** (every member built — whenever it is built — is also watched); the
//! *eager-warming scope* is chosen by the constructor: [`new`](EngineRegistry::new)
//! warms every member up front, while [`new_serve_default`](EngineRegistry::new_serve_default)
//! — the context-aware `serve --ui` policy — warms only the default and leaves the
//! rest lazy ([FR-WS-06]). A per-member start (or watch) failure **degrades** —
//! it is logged and skipped — rather than aborting the whole workspace.
//!
//! # Residency policy ([NFR-PE-11], [ADR-63])
//! Laziness bounds the members a *scoped* query touches, but the commands that
//! define a workspace — `workspace status`, `workspace check`, `xservice search`
//! — touch every member by design, so laziness bounds nothing there. Residency
//! is therefore capped by a workspace-wide [`ConnectionBudget`]: a member engine
//! opens only its budgeted share of read connections, and admitting a new member
//! first **evicts the least-recently-touched** ones until the cap has room. The
//! cap is derived from the host's descriptor limit, never from the member count,
//! so a host with a larger allowance keeps more members resident with no code
//! change. This applies on the **CLI fan-out path as well as `serve`**, extending
//! [NFR-PE-10]'s serve-only eviction.
//!
//! # One worker pool, not one per member ([NFR-PE-11], [ADR-63])
//! Connections are only half the multiple. Left to itself an
//! [`Engine`](crate::Engine) builds a `rayon` pool of one worker per core, so a
//! resident set costs `residents × cores` threads on top of its connections —
//! ~864 on the measured 12-core host. The registry therefore builds **one**
//! [`SharedWorkerPool`] sized by [`ConnectionBudget::worker_threads`] and injects
//! it into every member engine it starts, so a workspace's thread cost is what a
//! single engine would have spawned for itself.
//!
//! The registry holds the pool only [`Weak`](crate::WeakWorkerPool)ly: the
//! resident engines own it, so the last one to be evicted tears it down and no
//! worker thread outlives the residency it serves. While *any* resident holds it
//! the weak handle re-shares that same pool, which is what makes eviction and
//! sharing compose — evicting a member and touching it again rejoins the pool
//! rather than building a second one.
//!
//! Submission is unchanged: every job still enters through
//! `runtime.worker_pool().install(…)` from a caller that is not itself a pool
//! worker, so sharing adds no nested blocking submission and one member's long
//! job cannot deadlock another's — it can only queue behind it. That queueing is
//! real, though: an eviction's teardown joins the evicted member's watcher, whose
//! final sync runs on this same pool, and the admission lock is held across it.
//!
//! A **steady-state** navigation query is not exposed to that: `search`,
//! `callers`, `node` and the rest read on the calling thread through the read
//! pool ([ADR-11]) and never enter the worker pool, so the [NFR-PE-01] budget is
//! independent of another member's CPU work. The exception is a member's *first*
//! navigation call, which runs the [FR-IX-07] auto-index prologue — a full index,
//! on this pool. On a workspace whose members are not yet warm (the case
//! [CR-100] measured) that prologue is the one navigation path a shared pool
//! couples across members, which is exactly the trade [ADR-63] Consequences
//! records. Warming members up front ([FR-WS-14]) is what keeps it off the query
//! path.
//!
//! [CR-100]: ../../../docs/requests/CR-100-workspace-resource-budget.md
//! [FR-IX-07]: ../../../docs/specs/requirements/FR-IX-07.md
//! [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
//!
//! An evicted member is reconstructed on its next touch and is indistinguishable
//! from a never-evicted one: the store is canonical ([FR-DB-01]) and an engine
//! holds no authoritative state. Reconstruction is the policy's *cost*, so the
//! registry counts it ([`reconstructions`](EngineRegistry::reconstructions)) —
//! thrash is then a measurable signal rather than a silent latency tax.
//!
//! # Single-root invariant
//! The registry is never on the single-root path. [`Backing::resolve`] returns
//! [`Backing::Single`] — the one [`Engine`](crate::Engine) used exactly as today
//! — when discovery finds no workspace, and only allocates an [`EngineRegistry`]
//! when a workspace is present ([ADR-52]). Single-root behaviour is byte-for-byte
//! unchanged.
//!
//! [FR-DB-01]: ../../../docs/specs/requirements/FR-DB-01.md
//! [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
//! [FR-WS-06]: ../../../docs/specs/requirements/FR-WS-06.md
//! [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
//! [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
//! [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
//! [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
//! [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result};

use super::budget::ConnectionBudget;
use super::open_state::{MemberOpen, MemberOpenState, StoreFile};
use super::{Federation, Member};
use crate::{Engine, SharedWorkerPool, WeakWorkerPool};

/// A per-member unit the [`EngineRegistry`] multiplexes ([FR-WS-03]).
///
/// Implemented by [`Engine`](crate::Engine) for production; abstracted so the
/// registry's lazy/eager construction, watcher, and eviction policy can be
/// exercised without standing up real on-disk engines.
///
/// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
pub trait MemberEngine: Send + Sync + 'static {
    /// The watcher handle held for as long as the engine is resident under
    /// [`RegistryMode::Serve`]; dropping it stops that member's watcher.
    ///
    /// `Sync` as well as `Send` because residents live behind the registry's
    /// [`RwLock`] and the registry itself is shared across serve request tasks —
    /// stating the bound here reports a non-shareable watcher at the `impl`
    /// rather than at some distant `Arc<EngineRegistry<_>>` use site.
    type Watcher: Send + Sync;

    /// Build a long-lived engine rooted at a member's working-tree root on its
    /// budgeted share of the workspace's resources ([NFR-PE-11], [ADR-63]): at
    /// most `read_connections` read-only connections, and `worker_pool` — the
    /// **one** pool every resident member shares — for its CPU jobs.
    ///
    /// The budgeted share replaces the per-core connection pool *and* the private
    /// per-core worker pool an engine would size for itself; those defaults
    /// belong to the single-root path, which never reaches this trait
    /// ([`Engine::start`](crate::Engine::start)).
    ///
    /// # Errors
    /// Propagates a store-open / migrate / runtime failure so the registry can
    /// report the member as degraded.
    ///
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    /// [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md
    fn start(
        root: &Path,
        read_connections: usize,
        worker_pool: SharedWorkerPool,
    ) -> Result<Arc<Self>>;

    /// Spawn this member's filesystem watcher, returning the handle to hold.
    ///
    /// # Errors
    /// Propagates a watcher-attach failure; the registry treats it as a degraded
    /// (watcherless) member start rather than a fatal one ([FR-SY-06]).
    ///
    /// [FR-SY-06]: ../../../docs/specs/requirements/FR-SY-06.md
    fn watch(self: &Arc<Self>) -> Result<Self::Watcher>;
}

impl MemberEngine for Engine {
    type Watcher = crate::watch::WatchHandle;

    fn start(
        root: &Path,
        read_connections: usize,
        worker_pool: SharedWorkerPool,
    ) -> Result<Arc<Self>> {
        Ok(Arc::new(Engine::start_with_pools(
            root,
            read_connections,
            Some(worker_pool),
        )?))
    }

    fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
        Engine::watch(self)
    }
}

/// How a registry constructs its member engines ([FR-WS-03], [NFR-PE-10]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryMode {
    /// CLI one-shot: a member's engine is built on first touch and no watcher is
    /// spawned. A scoped answer constructs only the engines it needs.
    Lazy,
    /// `serve`: a member's engine is **watched whenever it is built**, on its
    /// first touch as much as during an eager warm.
    ///
    /// How many members are warmed up front is the *constructor's* choice, not
    /// the mode's — [`EngineRegistry::new`] warms the budget's worth,
    /// [`EngineRegistry::new_serve_default`] only the default member — and
    /// residency afterwards is capped by the budget either way.
    Serve,
}

/// A value tagged with the workspace member that produced it — how the fan-out
/// helper keeps cross-service answers **repo-qualified** ([FR-WS-03]).
///
/// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberScoped<T> {
    /// The owning member's [`name`](Member::name) (its workspace-relative path).
    pub member: String,
    /// The per-member value.
    pub value: T,
}

/// One resident member engine and (under serve) its watcher, with the logical
/// clock tick of its last touch for LRU eviction.
struct Resident<E: MemberEngine> {
    /// Held for the engine's residency; dropping it stops the watcher. `None`
    /// under [`RegistryMode::Lazy`] or when the watcher failed to spawn.
    ///
    /// Declared **before** `engine` so it drops first: a watcher's shutdown
    /// flushes any pending edits through the engine it watches, and Rust drops
    /// struct fields in declaration order, so the engine must still be alive
    /// when the handle is dropped. Reordering these two silently turns an
    /// eviction into a lost final sync.
    _watcher: Option<E::Watcher>,
    engine: Arc<E>,
    /// Value of the registry's [`tick`](EngineRegistry::tick) at last touch.
    ///
    /// Atomic so a **hit** needs only a read lock on the resident map: touching
    /// an already-resident member must not queue behind another member's
    /// admission (see [`EngineRegistry::admission`]).
    last_touch: AtomicU64,
}

/// The eviction accounting and the shared worker pool — the state an
/// **admission** owns, held behind the admission lock so a start can never
/// disagree with the bookkeeping that authorised it.
#[derive(Default)]
struct Admission {
    /// Every member this registry has built at least once — the denominator
    /// that turns a start into a *re*construction.
    started_before: HashSet<String>,
    /// The outcome of each member's most recent **cold open attempt**:
    /// `None` — it opened; `Some(diagnostic)` — it was attempted and failed
    /// ([FR-WS-16]).
    ///
    /// Keyed on the member name, so a member absent from this map was never
    /// attempted at all. That three-way split — opened / never attempted /
    /// attempted-and-failed — is what keeps a **lazily skipped** member and a
    /// **budget-evicted** one out of the degraded set ([BR-45]):
    ///
    /// - never attempted ⇒ no entry, because only [`EngineRegistry::engine_for`]
    ///   writes here and laziness means it was never called for that member.
    /// - evicted ⇒ `None`, because eviction reclaims a start that **succeeded**
    ///   and this map records open *attempts*, never residency. An evicted
    ///   member re-touched records `None` again.
    ///
    /// Last write wins, so a member that failed once and opened later is not
    /// degraded, and one that opened and later failed is — in both cases the
    /// latest attempt is the one the answer rests on.
    ///
    /// A [`BTreeMap`] so the readout is name-ordered and deterministic
    /// ([NFR-RA-06]); it holds one entry per *attempted* member, not per
    /// attempt.
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [BR-45]: ../../../docs/specs/software-spec.md#327-workspace-federation
    opens: BTreeMap<String, Option<String>>,
    /// The one `rayon` pool every resident member engine shares ([NFR-PE-11],
    /// [ADR-63]) — held **weakly**, because the residents own it.
    ///
    /// An admission upgrades it: a live pool is re-shared with the incoming
    /// member (so evict-then-reconstruct rejoins the pool rather than building a
    /// second one), and a dead one — the state after the last resident was
    /// evicted, when no worker thread remains — is replaced by a fresh pool. A
    /// strong reference here would keep `worker_threads` threads alive for a
    /// workspace with nothing resident, which is exactly the orphan [ADR-63]'s
    /// teardown clause forbids.
    ///
    /// [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md
    worker_pool: WeakWorkerPool,
}

impl Admission {
    /// The pool the next member engine joins: the live shared one if any
    /// resident still holds it, otherwise a freshly built pool of `threads`
    /// workers, remembered weakly for the members admitted after it.
    ///
    /// # Errors
    /// Propagates a `rayon` pool-build failure, which the caller reports as a
    /// degraded member start — the same class as a failed store open.
    fn join_or_build_pool(&mut self, threads: usize) -> Result<SharedWorkerPool> {
        if let Some(pool) = self.worker_pool.upgrade() {
            return Ok(pool);
        }
        let pool = SharedWorkerPool::with_threads(threads)
            .context("building the workspace's shared worker pool")?;
        self.worker_pool = pool.downgrade();
        Ok(pool)
    }

    /// Record that `member` was just built, reporting whether this was a
    /// *re*construction — a rebuild of a member seen before.
    fn record_start(&mut self, member: &str) -> bool {
        !self.started_before.insert(member.to_string())
    }

    /// Record the outcome of one cold open attempt on `member`
    /// ([FR-WS-16]).
    ///
    /// Called on **both** arms of the cold path — a start that succeeded and one
    /// that failed — because "never attempted" is a third outcome that only the
    /// absence of an entry can express.
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    fn record_open(&mut self, member: &str, failure: Option<String>) {
        self.opens.insert(member.to_string(), failure);
    }
}

/// What is at a member's canonical store path `<root>/.logos/logos.db` — the one
/// fact that disambiguates SQLite's `CANTOPEN`
/// ([`super::open_state::classify`]).
///
/// Three states, not two. `exists()` alone would conflate "nothing here" with "a
/// directory here", which license opposite conclusions: a *non-regular* file is
/// decisive (nothing can open it), while *nothing at all* is uninformative,
/// because [`Engine::start`](crate::Engine::start) creates the store on open —
/// so an absent file is equally consistent with a never-indexed member and with
/// descriptor exhaustion partway through creating it.
///
/// `symlink_metadata` is deliberately **not** used: a symlink to a real store is
/// a store, and `metadata` follows it. A dangling symlink resolves to `Err`,
/// which lands in `Obstructed` — correct, since nothing can open that either.
fn store_file(root: &Path) -> StoreFile {
    match root.join(".logos").join("logos.db").metadata() {
        Ok(meta) if meta.is_file() => StoreFile::Present,
        Ok(_) => StoreFile::Obstructed,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => StoreFile::Absent,
        // Anything else (a dangling symlink, a permission-denied stat) is a path
        // that cannot be opened as a store, whatever occupies it.
        Err(_) => StoreFile::Obstructed,
    }
}

/// Evict least-recently-touched members from `resident` until at most `cap`
/// remain, returning the evicted entries least-recently-touched **first**.
///
/// The residents are **returned, not dropped here**: dropping one joins its
/// writer thread and stops its watcher, and the caller — which holds the
/// admission lock — drops them after releasing the map's write lock and *before*
/// starting the incoming engine. That keeps the ordering the ceiling depends on
/// (every evicted connection is closed before a new one opens) while taking the
/// slow teardown off the lock a resident-member hit needs.
///
/// A member whose engine **another caller still holds** is skipped, and the
/// next-least-recently-touched evicted instead. Evicting it would free
/// nothing (its connections stay open for as long as that caller lives) and
/// would guarantee the next touch built a *second* engine over the same
/// store — two writer actors, two hydration caches, and one of them with no
/// watcher. The serve surface makes this concrete: it resolves the default
/// member's engine once and holds it for the process lifetime, so on a
/// workspace larger than the budget the default is the first member LRU
/// would discard. Skipping held members keeps "one live engine per member
/// store" true, which is what makes an evicted member's reconstruction
/// indistinguishable from a never-evicted one ([FR-DB-01], [NFR-PE-11]).
///
/// The strong-count check and the removal happen under the **same** write-lock
/// acquisition, so a concurrent hit — which clones its `Arc` under the read lock
/// — cannot slip between them and have its engine evicted out from under it.
///
/// The cost is that residency can exceed `cap` when callers hold many
/// engines at once. That is honest rather than harmful: those connections
/// were live either way, and [`live_read_connections`](EngineRegistry::live_read_connections)
/// reports the excess instead of hiding it.
///
/// [FR-DB-01]: ../../../docs/specs/requirements/FR-DB-01.md
/// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
fn evict_to<E: MemberEngine>(
    resident: &mut HashMap<String, Resident<E>>,
    cap: usize,
) -> Vec<(String, Resident<E>)> {
    if resident.len() <= cap {
        return Vec::new();
    }
    let mut by_recency: Vec<(String, u64)> = resident
        .iter()
        .filter(|(_, r)| Arc::strong_count(&r.engine) == 1)
        .map(|(name, r)| (name.clone(), r.last_touch.load(Ordering::Relaxed)))
        .collect();
    by_recency.sort_by_key(|(_, touch)| *touch); // least-recently-touched first
    let evict_count = resident.len() - cap;
    by_recency
        .into_iter()
        .take(evict_count)
        .filter_map(|(name, _)| resident.remove(&name).map(|r| (name, r)))
        .collect()
}

/// The `root → Engine` registry over a workspace's members ([FR-WS-03],
/// [NFR-PE-10], [NFR-PE-11], [ADR-52], [ADR-63]).
///
/// Shareable behind an [`Arc`] across request tasks (the interior state is
/// locked), so the web surface and concurrent fan-out see one registry. Each
/// member engine owns its **own store, writer and read pool**, so touching one
/// member never advances another's state; the `rayon` worker pool is the one
/// thing they share, and it carries no per-member state — only CPU
/// ([NFR-PE-11], [ADR-63]).
///
/// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
/// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
/// [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md
pub struct EngineRegistry<E: MemberEngine = Engine> {
    federation: Federation,
    mode: RegistryMode,
    /// The host-derived ceiling on live read connections and worker threads, and
    /// the residency they imply. Never a function of the member count
    /// ([NFR-PE-11]).
    budget: ConnectionBudget,
    /// The resident member engines. An [`RwLock`] rather than a `Mutex` because
    /// a **hit** is the common case and must not queue behind an admission: a
    /// fan-out over a large workspace spends its time in cold starts and engine
    /// teardowns, and under one lock every unrelated request for an
    /// already-resident member — the serve surface's default member above all —
    /// waited for the whole fan-out.
    resident: RwLock<HashMap<String, Resident<E>>>,
    /// Serialises **admission**: eviction, teardown, engine start and insertion
    /// happen under this lock and in that order, so the live count never even
    /// transiently exceeds the budget and two concurrent touches of the same
    /// member cannot each build an engine.
    ///
    /// Lock order is always `admission` → `resident`; nothing takes them the
    /// other way round.
    admission: Mutex<Admission>,
    /// Engine starts that rebuilt a previously-evicted member. The cost of the
    /// budget, counted so thrash is measurable ([NFR-PE-11]).
    ///
    /// Atomic rather than admission state so the readout never queues behind an
    /// admission: a cold start under that lock can run for seconds (open and
    /// migrate a store, spawn a watcher, join an evicted member's writer thread),
    /// and a caller asking how much thrash it has paid should not have to wait
    /// for one.
    ///
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    reconstructions: AtomicU64,
    /// **Every** successful engine start this registry has served — first
    /// builds and reconstructions alike ([NFR-PE-10]).
    ///
    /// Distinct from [`reconstructions`](Self::reconstructions) in exactly the
    /// case that matters for a laziness claim: a caller that opened a member's
    /// engine a *second* time within one fan-out iteration leaves the member
    /// resident and previously-started, so it moves neither the residency count
    /// nor the reconstruction count — and a test asserting only those two
    /// cannot see it. This counter can ([NFR-PE-10]). Atomic for the same
    /// reason as [`reconstructions`](Self::reconstructions).
    ///
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    starts: AtomicU64,
    /// Engine starts that **failed**, across every touch this registry has served.
    ///
    /// The per-member `Err` a fan-out returns is the only other record, and it
    /// survives just as far as its read-model: [`workspace_status`](super::workspace_status)
    /// walks every member four times, and the coverage and topic tiers have no
    /// per-member error channel, so a member that fails to open during those
    /// walks is silently dropped. Counting failures registry-side is what lets a
    /// caller assert "no member failed to open" over the whole command rather
    /// than over its first walk ([NFR-PE-11], [FR-WS-16]). Atomic for the same
    /// reason as [`reconstructions`](Self::reconstructions).
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    start_failures: AtomicU64,
    /// Monotonic logical clock — bumped on every touch to order residents by
    /// recency for LRU eviction. A tick, not wall-clock, so eviction is
    /// deterministic.
    tick: AtomicU64,
}

impl<E: MemberEngine> EngineRegistry<E> {
    /// Build a registry over `federation`'s members in `mode`.
    ///
    /// [`RegistryMode::Serve`] warms every member eagerly (one engine + one
    /// watcher each); [`RegistryMode::Lazy`] starts empty and builds on first
    /// touch. A member that fails to start under serve is logged and skipped
    /// (degraded), never fatal.
    ///
    /// The eager warm stops at the budget this host derives: a workspace larger
    /// than the budget warms the first `max_resident_members` members and leaves
    /// the rest deferred, to be built on first touch ([NFR-PE-11], [NFR-PE-10]).
    ///
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    pub fn new(federation: Federation, mode: RegistryMode) -> Self {
        Self::with_budget(federation, mode, ConnectionBudget::from_host())
    }

    /// Build a registry over `federation`'s members in `mode` under an explicit
    /// `budget`, rather than the one this host would derive ([NFR-PE-11]).
    ///
    /// The seam that makes the ceiling verifiable: a test can ask what a
    /// 256-descriptor host would budget without having to be one, and a caller
    /// that already knows its resource envelope can state it.
    ///
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    pub fn with_budget(
        federation: Federation,
        mode: RegistryMode,
        budget: ConnectionBudget,
    ) -> Self {
        let registry = Self::build(federation, mode, budget);
        if mode == RegistryMode::Serve {
            registry.warm_all();
        }
        registry
    }

    /// Build a **serve-path** registry that warms only the **default member**
    /// eagerly (and watches it), leaving every other member lazy — the
    /// context-aware `serve --ui` policy ([FR-WS-06], [NFR-PE-10]).
    ///
    /// This is the serve registry [`RegistryMode::Serve`] refines: members carry
    /// watch-on-touch semantics (a member built on its first cross-service query
    /// is also watched, exactly as under [`RegistryMode::Serve`]), but opening the
    /// workspace does **not** pay N× cold-start + N× watchers up front. Only the
    /// default member — the one the shared single-root `/api/v1/*` surface runs
    /// against — is warmed at startup; the rest are constructed on first use.
    ///
    /// A default member that fails to warm **degrades at this layer** (logged, not
    /// fatal): the registry is still returned, so a caller that only fans out over
    /// members (e.g. the cross-service query surface) still answers for the healthy
    /// ones, mirroring [`warm_all`](Self::warm_all)'s per-member degrade contract
    /// ([ADR-53]). A caller that *requires* the default engine for a shared
    /// single-root surface — the `serve` path via [`default_engine`](Backing::default_engine)
    /// — will still surface that failure when it resolves the default, exactly as a
    /// single-root serve fails loud on a corrupt engine.
    ///
    /// [FR-WS-06]: ../../../docs/specs/requirements/FR-WS-06.md
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    pub fn new_serve_default(federation: Federation) -> Self {
        Self::serve_default_with_budget(federation, ConnectionBudget::from_host())
    }

    /// [`new_serve_default`](Self::new_serve_default) under an explicit budget —
    /// the same seam [`with_budget`](Self::with_budget) opens for [`new`](Self::new).
    pub fn serve_default_with_budget(federation: Federation, budget: ConnectionBudget) -> Self {
        let registry = Self::build(federation, RegistryMode::Serve, budget);
        if let Err(err) = registry.default_engine() {
            tracing::warn!(
                "workspace default member engine failed to warm; serving degraded without an \
                 eager default: {err:#}"
            );
        }
        registry
    }

    /// Construct the registry struct **without** warming any member — the shared
    /// skeleton [`new`](Self::new) and [`new_serve_default`](Self::new_serve_default)
    /// layer their construction policy on top of.
    fn build(federation: Federation, mode: RegistryMode, budget: ConnectionBudget) -> Self {
        Self {
            federation,
            mode,
            budget,
            resident: RwLock::new(HashMap::new()),
            admission: Mutex::new(Admission::default()),
            reconstructions: AtomicU64::new(0),
            starts: AtomicU64::new(0),
            start_failures: AtomicU64::new(0),
            tick: AtomicU64::new(0),
        }
    }

    /// The workspace this registry federates.
    pub fn federation(&self) -> &Federation {
        &self.federation
    }

    /// The workspace's members, in discovery order.
    pub fn members(&self) -> &[Member] {
        &self.federation.members
    }

    /// The construction policy this registry runs under.
    pub fn mode(&self) -> RegistryMode {
        self.mode
    }

    /// Get (building on first touch) the engine for one member, by
    /// [`name`](Member::name) ([FR-WS-03]).
    ///
    /// The scoped path: a CLI one-shot that needs a single member calls this and
    /// constructs only that engine, not all N ([NFR-PE-10]). Under
    /// [`RegistryMode::Serve`] a freshly built engine is also watched.
    ///
    /// # Errors
    /// Returns an error if `member` is not a member of this workspace, or if the
    /// engine fails to start.
    ///
    /// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    pub fn engine_for(&self, member: &str) -> Result<Arc<E>> {
        let Some(target) = self.federation.members.iter().find(|m| m.name == member) else {
            anyhow::bail!("no such workspace member: {member:?}");
        };

        // The hit path: a read lock, a clone, done. It never waits on an
        // in-flight admission, so a fan-out's cold starts no longer serialise
        // unrelated requests for members that are already resident.
        if let Some(engine) = self.touch_resident(member) {
            return Ok(engine);
        }

        // The miss path is serialised end to end so two concurrent touches of
        // the same member cannot each construct an engine (the second would
        // waste a cold start, orphan a watcher, and put a second writer actor on
        // one store). Re-check under the lock: the member may have been admitted
        // while this caller queued.
        let mut admission = self.lock_admission();
        if let Some(engine) = self.touch_resident(member) {
            return Ok(engine);
        }

        // Make room BEFORE opening any connection, so the live count never even
        // transiently exceeds the budget ([NFR-PE-11]). `evict_to` removes the
        // entries under the map's write lock and hands them back; dropping them
        // *here* — still under the admission lock, still before the start below —
        // closes their connections first, unless a caller still holds one. The
        // teardown (joining a writer thread, stopping a watcher) is therefore off
        // the map lock, where it would have blocked every concurrent hit, but
        // still ahead of the start, which is what the ordering exists for.
        // Two statements deliberately: binding the evicted residents first ends
        // the temporary write guard, so the `drop` below runs off the map lock.
        // Folding them into one expression would put the teardown back under it.
        let evicted = evict_to(
            &mut self.write_resident(),
            self.budget.max_resident_members().saturating_sub(1),
        );
        drop(evicted);

        // Every resident member shares one pool ([NFR-PE-11], [ADR-63]). A live
        // pool is re-shared; only a workspace with nothing resident builds one.
        // A pool that cannot be built is a degraded member start of the same
        // class as a store that cannot be opened, so both land in one arm.
        let started = admission
            .join_or_build_pool(self.budget.worker_threads())
            .and_then(|worker_pool| {
                E::start(
                    &target.root,
                    self.budget.per_member_read_connections(),
                    worker_pool,
                )
            });
        let engine = match started {
            Ok(engine) => engine,
            Err(err) => {
                self.start_failures.fetch_add(1, Ordering::Relaxed);
                let err = err.context(format!(
                    "starting the engine for workspace member {member:?}"
                ));
                // Record the attempt-and-failure BEFORE returning, with the same
                // contextualised diagnostic the caller receives — the per-member
                // `Err` reaches only whichever read-model asked, and
                // `workspace status` walks every member four times through
                // tiers that have no error channel at all ([FR-WS-16]).
                admission.record_open(member, Some(format!("{err:#}")));
                return Err(err);
            }
        };
        admission.record_open(member, None);
        self.starts.fetch_add(1, Ordering::Relaxed);
        if admission.record_start(member) {
            self.reconstructions.fetch_add(1, Ordering::Relaxed);
        }
        let watcher = self.spawn_watcher(member, &engine);
        let displaced = self.write_resident().insert(
            member.to_string(),
            Resident {
                _watcher: watcher,
                engine: Arc::clone(&engine),
                last_touch: AtomicU64::new(self.tick.fetch_add(1, Ordering::Relaxed)),
            },
        );
        // Unreachable: the admission lock is held across the re-check above and
        // this insert, and nothing else inserts. Asserted rather than assumed
        // because a displaced `Resident` would drop *under the write guard* —
        // joining a watcher and a writer thread with every hit blocked, which is
        // precisely the teardown that `evict_to` was restructured to keep off
        // this lock.
        debug_assert!(
            displaced.is_none(),
            "a second engine was admitted for member {member:?} while one was resident"
        );
        Ok(engine)
    }

    /// Clone out an already-resident member's engine, marking it most-recently
    /// touched — the hit path, under a **read** lock only.
    fn touch_resident(&self, member: &str) -> Option<Arc<E>> {
        let resident = self.read_resident();
        let entry = resident.get(member)?;
        // `fetch_max`, not `store`: the tick allocation and the write are two
        // operations, so a thread preempted between them could otherwise write a
        // stale tick over a newer one and make the hottest member look like the
        // least-recently-touched — one spurious eviction and rebuild. Monotone by
        // construction instead.
        entry
            .last_touch
            .fetch_max(self.tick.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
        Some(Arc::clone(&entry.engine))
    }

    /// The **default member's** engine ([FR-WS-05]): `[workspace] default`,
    /// falling back to the first member in discovery order. The shared
    /// single-root tools run against this member under the federated backing, so
    /// the default-member *policy* lives here in the core, not in the surfaces
    /// (NFR-MA-02) — one definition every adapter (MCP, CLI, web) shares.
    ///
    /// # Errors
    /// The workspace has no members, or the resolved member's engine fails to
    /// start.
    ///
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    pub fn default_engine(&self) -> Result<Arc<E>> {
        let member = self
            .federation
            .default
            .clone()
            .or_else(|| self.federation.members.first().map(|m| m.name.clone()))
            .context("the workspace has no members to answer a single-root query")?;
        self.engine_for(&member)
    }

    /// Run `f` over **every** member, tagging each result with its member — the
    /// repo-qualified cross-service fan-out ([FR-WS-03]).
    ///
    /// Each member's result is a [`Result`]: a member whose engine fails to
    /// start is reported as an `Err` for that member rather than aborting the
    /// whole query, so a partly-degraded workspace still answers ([ADR-53]).
    /// `f` runs eagerly, once per member, in discovery order. Each member's
    /// engine owns its own store, writer and read pool, so a per-member call
    /// never advances another member's state; since S-325 they do share one
    /// `rayon` worker pool, so CPU jobs queue behind one another rather than
    /// running on private pools ([NFR-PE-11], [ADR-63] Consequences).
    ///
    /// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    pub fn fan_out<T>(&self, f: impl Fn(&Member, &Arc<E>) -> T) -> Vec<MemberScoped<Result<T>>> {
        self.federation
            .members
            .iter()
            .map(|member| MemberScoped {
                member: member.name.clone(),
                value: self.engine_for(&member.name).map(|engine| f(member, &engine)),
            })
            .collect()
    }

    /// The members with a resident (constructed) engine right now, sorted by
    /// name — introspection for eviction accounting and tests.
    pub fn resident_members(&self) -> Vec<String> {
        let mut names: Vec<String> = self.read_resident().keys().cloned().collect();
        names.sort();
        names
    }

    /// The number of resident member engines.
    pub fn resident_count(&self) -> usize {
        self.read_resident().len()
    }

    /// The workspace-wide budget this registry holds its residents inside
    /// ([NFR-PE-11]).
    ///
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    pub fn budget(&self) -> ConnectionBudget {
        self.budget
    }

    /// Live read connections held by resident member engines right now — the
    /// quantity [NFR-PE-11] bounds.
    ///
    /// Normally at or below [`ConnectionBudget::total_read_connections`]. It can
    /// exceed it when callers hold engines the registry would otherwise have
    /// evicted, because a held engine is never evicted (see [`evict_to`])
    /// — and this readout **rises** to say so rather than reporting the budget it
    /// wishes were true. Compare it against the budget; do not assume it.
    ///
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    pub fn live_read_connections(&self) -> usize {
        self.resident_count() * self.budget.per_member_read_connections()
    }

    /// How many engine starts rebuilt a member that had been evicted before.
    ///
    /// Reconstruction is what the budget trades residency for, so it is counted
    /// rather than absorbed: a walk that touches each member once should report
    /// **zero**, and a rising count is eviction thrash made visible ([ADR-63]
    /// Consequences).
    ///
    /// [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md
    pub fn reconstructions(&self) -> u64 {
        self.reconstructions.load(Ordering::Relaxed)
    }

    /// **Every** successful engine start this registry has served — first builds
    /// and reconstructions alike ([NFR-PE-10]).
    ///
    /// The instrument a laziness claim is asserted against. Residency and
    /// reconstruction counts cannot carry it: both are blind to a caller that
    /// opens a member's engine twice inside one fan-out iteration (the member
    /// stays resident and stays previously-started), and residency is pinned to
    /// the budget by eviction no matter how many engines were built. A read-model
    /// that must not construct engines of its own therefore asserts on this —
    /// `starts == walks × members` — not on `resident_count()`.
    ///
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    pub fn engine_starts(&self) -> u64 {
        self.starts.load(Ordering::Relaxed)
    }

    /// How many engine starts have **failed** across every touch this registry
    /// has served.
    ///
    /// The registry-wide record of what a per-member `Err` only reports as far as
    /// one read-model. A command that walks every member several times — and
    /// whose later walks carry no per-member error channel — asserts on this to
    /// mean "no member failed to open", rather than "no member failed to open
    /// during the walk that happens to report errors" ([NFR-PE-11], [FR-WS-16]).
    ///
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    pub fn start_failures(&self) -> u64 {
        self.start_failures.load(Ordering::Relaxed)
    }

    /// Each roster member's [open state](MemberOpenState) — the open-state axis
    /// [FR-WS-16]'s degraded reporting and non-zero exit rest on.
    ///
    /// One entry per member of the workspace roster, **in manifest order**
    /// ([NFR-RA-06]), so the answer is over the workspace and not over whatever
    /// subset happened to be touched. Reading it costs no engine start, no
    /// connection and no store read: the outcomes were recorded as the opens
    /// happened, and the only I/O is a `metadata` call on the store path of each
    /// member that **failed**, to tell SQLite's ambiguous `CANTOPEN` apart from
    /// a genuinely missing store ([NFR-PE-10]).
    ///
    /// # What is deliberately not consulted
    /// Residency. A member evicted to stay inside the budget reads `opened`,
    /// because its open succeeded and eviction only reclaimed it ([BR-45],
    /// [NFR-PE-11]) — reporting it degraded would turn the budget's normal
    /// operation into a failing command. A member laziness never reached reads
    /// `not-attempted` for the same reason: nothing was attempted, so nothing
    /// failed.
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [BR-45]: ../../../docs/specs/software-spec.md#327-workspace-federation
    pub fn open_states(&self) -> Vec<MemberOpen> {
        let opens = self.lock_admission().opens.clone();
        self.federation
            .members
            .iter()
            .map(|member| MemberOpen {
                member: member.name.clone(),
                state: match opens.get(&member.name) {
                    None => MemberOpenState::NotAttempted,
                    Some(None) => MemberOpenState::Opened,
                    Some(Some(diagnostic)) => {
                        MemberOpenState::degraded(diagnostic, store_file(&member.root))
                    }
                },
            })
            .collect()
    }

    /// Worker threads the workspace's **shared** `rayon` pool is running right
    /// now — `0` when no engine holds it ([NFR-PE-11], [ADR-63]).
    ///
    /// The whole thread cost of a federated workspace, whatever its member count:
    /// one pool of [`ConnectionBudget::worker_threads`] workers, or none at all.
    /// A zero here after the last resident engine has been evicted is the
    /// teardown claim [ADR-63] makes, stated as a readout rather than as a hope.
    ///
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    /// [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md
    pub fn shared_worker_threads(&self) -> usize {
        self.lock_admission()
            .worker_pool
            .upgrade()
            .map_or(0, |pool| pool.threads())
    }

    /// Evict least-recently-touched member engines until at most `cap` remain,
    /// bounding steady-state resource cost ([NFR-PE-10], [NFR-PE-11]).
    ///
    /// Dropping a resident drops its engine `Arc` and (under serve) stops its
    /// watcher. An evicted member is rebuilt — and re-watched, under serve — on
    /// its next touch. Returns the evicted member names, least-recently-touched
    /// first.
    ///
    /// Admission already evicts to the budget on every touch, so this is for a
    /// caller that wants to shrink *below* the budget (a serve loop trimming an
    /// idle workspace), not the mechanism that enforces the ceiling.
    ///
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    pub fn evict_to_capacity(&self, cap: usize) -> Vec<String> {
        // Under the admission lock so a trim cannot interleave with an
        // admission's evict-then-start, and dropped outside the map's write lock
        // so the teardown does not block concurrent hits.
        let _admission = self.lock_admission();
        let evicted = evict_to(&mut self.write_resident(), cap);
        evicted.into_iter().map(|(name, _)| name).collect()
    }

    /// Lock the admission state, **recovering** a poisoned lock rather than
    /// propagating the poison.
    ///
    /// The state is eviction bookkeeping and a pool handle; a poisoned view is
    /// still usable. The registry is shared behind an [`Arc`] across serve
    /// request tasks, and the module's contract is per-member degradation — so a
    /// single member's panic (e.g. inside a build held under this lock) must not
    /// brick every subsequent `engine_for` / `fan_out` for the healthy members.
    /// Recovering the guard keeps that all-or-nothing failure from happening
    /// ([ADR-53]).
    ///
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    fn lock_admission(&self) -> std::sync::MutexGuard<'_, Admission> {
        self.admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Read the resident map, recovering a poisoned lock for the same reason
    /// [`lock_admission`](Self::lock_admission) does.
    fn read_resident(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, Resident<E>>> {
        self.resident
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Write the resident map, recovering a poisoned lock for the same reason
    /// [`lock_admission`](Self::lock_admission) does.
    fn write_resident(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, Resident<E>>> {
        self.resident
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Eagerly build (and, under serve, watch) as many members as the budget can
    /// hold resident, in discovery order. Degraded members are logged and
    /// skipped.
    ///
    /// Stopping at the budget is the point: warming all N to keep `budget` of
    /// them would pay N cold starts and N watcher spawns to discard all but a
    /// handful, making **boot cost a function of the member count** — precisely
    /// what [NFR-PE-10]'s lazy construction exists to prevent and what this
    /// story removes everywhere else. The members past the budget are simply
    /// deferred, and build on first touch like any other ([FR-IX-07]).
    ///
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    /// [FR-IX-07]: ../../../docs/specs/requirements/FR-IX-07.md
    fn warm_all(&self) {
        for member in self
            .federation
            .members
            .iter()
            .take(self.budget.max_resident_members())
        {
            if let Err(err) = self.engine_for(&member.name) {
                tracing::warn!(
                    member = %member.name,
                    "workspace member engine failed to start; serving degraded without it: {err:#}"
                );
            }
        }
    }

    /// Under serve, spawn the member's watcher (degrading on failure); under lazy
    /// mode, no watcher is spawned.
    fn spawn_watcher(&self, member: &str, engine: &Arc<E>) -> Option<E::Watcher> {
        if self.mode != RegistryMode::Serve {
            return None;
        }
        match engine.watch() {
            Ok(handle) => Some(handle),
            Err(err) => {
                tracing::warn!(
                    member = %member,
                    "workspace member watcher failed to spawn; watching degraded: {err:#}"
                );
                None
            }
        }
    }
}

/// The serve/CLI backing choice ([ADR-52], [FR-WS-03]): a single-root engine, or
/// a federated [`EngineRegistry`].
///
/// This is the seam that keeps the single-root path unchanged. [`resolve`] picks
/// [`Backing::Single`] when discovery found **no** workspace — the one engine
/// used exactly as today, with no registry allocated and no fan-out — and
/// [`Backing::Federated`] only when a workspace is present.
///
/// [`resolve`]: Backing::resolve
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
/// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
pub enum Backing<E: MemberEngine = Engine> {
    /// No workspace: the single-root engine, byte-for-byte unchanged.
    Single(Arc<E>),
    /// A workspace: the member-engine registry.
    ///
    /// **Boxed** to keep the two variants the same order of size. The registry
    /// inlines a whole [`Federation`] — member set, links, and (since S-258) the
    /// `[governance]` rule family — running to hundreds of bytes, while
    /// [`Single`](Self::Single) is a lone `Arc`; without the box every `Backing`,
    /// including a single-root one carrying no workspace, would be sized for the
    /// federated variant (`clippy::large_enum_variant`).
    ///
    /// The saving is in *size*, not in copies: every construction site wraps this
    /// enum in an `Arc` immediately, so it is moved once at startup and never per
    /// dispatch.
    Federated(Box<EngineRegistry<E>>),
}

impl<E: MemberEngine> Backing<E> {
    /// Decide the backing from a discovery result ([ADR-52]).
    ///
    /// `federation` is `discover(hint)`'s output: `None` → [`Backing::Single`]
    /// built from `single` (the registry is bypassed entirely); `Some` →
    /// [`Backing::Federated`] over an [`EngineRegistry`] in `mode`. `single` is
    /// invoked **only** on the single-root path, so the federated path never
    /// pays for a single-root engine.
    ///
    /// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
    pub fn resolve(
        federation: Option<Federation>,
        mode: RegistryMode,
        single: impl FnOnce() -> Arc<E>,
    ) -> Self {
        match federation {
            None => Backing::Single(single()),
            // Only here is a budget derived at all: the single-root arm above
            // allocates no registry, so it keeps its core-sized pool exactly as
            // today ([FR-WS-03], [ADR-52]).
            Some(federation) => {
                Backing::Federated(Box::new(EngineRegistry::new(federation, mode)))
            }
        }
    }

    /// The single-root engine, if this is the single-root backing.
    pub fn as_single(&self) -> Option<&Arc<E>> {
        match self {
            Backing::Single(engine) => Some(engine),
            Backing::Federated(_) => None,
        }
    }

    /// The member-engine registry, if this is the federated backing.
    ///
    /// The [`Box`] is transparent to callers: they still get a plain
    /// `&EngineRegistry<E>`.
    pub fn as_federated(&self) -> Option<&EngineRegistry<E>> {
        match self {
            Backing::Federated(registry) => Some(registry.as_ref()),
            Backing::Single(_) => None,
        }
    }

    /// Whether this backing federates a workspace.
    pub fn is_federated(&self) -> bool {
        matches!(self, Backing::Federated(_))
    }

    /// The engine the shared single-root surfaces run against: the one engine
    /// under [`Backing::Single`], or the federated workspace's default member
    /// ([`EngineRegistry::default_engine`]). The default-member policy lives here
    /// in the core so every adapter (MCP, web) shares **one** definition
    /// (NFR-MA-02) rather than copy-pasting the `Single`/`Federated` unwrap.
    ///
    /// # Errors
    /// Under [`Backing::Federated`], the workspace has no members or the resolved
    /// default member's engine fails to start (see [`EngineRegistry::default_engine`]).
    pub fn default_engine(&self) -> Result<Arc<E>> {
        match self {
            Backing::Single(engine) => Ok(Arc::clone(engine)),
            Backing::Federated(registry) => registry.default_engine(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry is shared behind an [`Arc`] across serve request tasks, so
    /// it must be `Send + Sync` with a **real** [`Engine`] and its real watcher
    /// handle. Asserted at compile time because the interior locking is what
    /// provides it: swapping a lock for one with tighter bounds on its contents
    /// would otherwise break the web surface, not this module.
    const _: fn() = || {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<EngineRegistry<Engine>>();
        assert_send_sync::<Backing<Engine>>();
    };

    use std::cell::Cell;
    use std::path::PathBuf;

    // Per-test-thread construction spies. Each `#[test]` runs on its own thread,
    // so thread-local counters are isolated per test — no cross-test bleed — and
    // let us assert exactly how many engines/watchers a registry built.
    thread_local! {
        static STARTS: Cell<usize> = const { Cell::new(0) };
        static WATCHES: Cell<usize> = const { Cell::new(0) };
        /// Read connections open across every LIVE spy engine, incremented on
        /// construction and decremented on `Drop` — so this counts genuinely
        /// live connections, including an engine an evicting caller still holds,
        /// which residency arithmetic alone would miss.
        static LIVE_CONNECTIONS: Cell<usize> = const { Cell::new(0) };
        /// High-water mark of `LIVE_CONNECTIONS` — the instrument [NFR-PE-11]'s
        /// ceiling is asserted against.
        static PEAK_CONNECTIONS: Cell<usize> = const { Cell::new(0) };
        /// Watchers currently running: spawned minus dropped. `WATCHES` counts
        /// spawns only, so without this no test could tell "64 watchers were
        /// spawned and 64 stopped" from "64 watchers are still running" — the
        /// difference between a bounded and an unbounded steady state.
        static LIVE_WATCHERS: Cell<usize> = const { Cell::new(0) };
    }

    fn reset_spies() {
        STARTS.with(|c| c.set(0));
        WATCHES.with(|c| c.set(0));
        LIVE_CONNECTIONS.with(|c| c.set(0));
        PEAK_CONNECTIONS.with(|c| c.set(0));
        LIVE_WATCHERS.with(|c| c.set(0));
    }
    fn starts() -> usize {
        STARTS.with(Cell::get)
    }
    fn watches() -> usize {
        WATCHES.with(Cell::get)
    }
    fn peak_connections() -> usize {
        PEAK_CONNECTIONS.with(Cell::get)
    }
    fn live_watchers() -> usize {
        LIVE_WATCHERS.with(Cell::get)
    }

    /// A fake member engine that records its root and counts constructions,
    /// standing in for a real [`Engine`] so lazy/eager/eviction policy is
    /// testable without any on-disk store.
    ///
    /// It also *holds* its budgeted connection count for its whole lifetime, so
    /// the suite can observe the live-connection ceiling the same way the OS
    /// would — by what is open, not by what the registry believes.
    #[derive(Debug)]
    struct SpyEngine {
        root: PathBuf,
        /// The pool size the registry asked for — the member's budgeted share.
        read_connections: usize,
        /// The `rayon` pool the registry injected. Held (not merely recorded)
        /// because holding it is what a real member engine does, and it is the
        /// only reason the pool outlives one admission: a spy that dropped it
        /// would make every member build its own and hide the sharing this story
        /// is about.
        worker_pool: SharedWorkerPool,
    }
    struct SpyWatcher;

    impl MemberEngine for SpyEngine {
        type Watcher = SpyWatcher;

        fn start(
            root: &Path,
            read_connections: usize,
            worker_pool: SharedWorkerPool,
        ) -> Result<Arc<Self>> {
            STARTS.with(|c| c.set(c.get() + 1));
            LIVE_CONNECTIONS.with(|c| c.set(c.get() + read_connections));
            PEAK_CONNECTIONS.with(|peak| {
                peak.set(peak.get().max(LIVE_CONNECTIONS.with(Cell::get)));
            });
            Ok(Arc::new(SpyEngine {
                root: root.to_path_buf(),
                read_connections,
                worker_pool,
            }))
        }

        fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
            WATCHES.with(|c| c.set(c.get() + 1));
            LIVE_WATCHERS.with(|c| c.set(c.get() + 1));
            Ok(SpyWatcher)
        }
    }

    impl Drop for SpyWatcher {
        fn drop(&mut self) {
            let _ = LIVE_WATCHERS.try_with(|c| c.set(c.get() - 1));
        }
    }

    impl Drop for SpyEngine {
        fn drop(&mut self) {
            // `try_with`: a thread-local may already be destroyed if an engine
            // outlives the test thread. Nothing to account for then.
            let _ = LIVE_CONNECTIONS.try_with(|c| c.set(c.get() - self.read_connections));
        }
    }

    /// A federation of `names`, each a member rooted at `/ws/<name>` — no disk.
    fn fed(names: &[&str]) -> Federation {
        let root = PathBuf::from("/ws");
        Federation {
            name: "w".to_string(),
            members: names
                .iter()
                .map(|name| Member {
                    name: (*name).to_string(),
                    root: root.join(name),
                })
                .collect(),
            root,
            default: None,
            links: Vec::new(),
            governance: Default::default(),
        }
    }

    /// A budget with room to spare, so a test about lazy/eager/LRU policy is
    /// not also a test of this host's descriptor limit. Stated in explicit
    /// limits rather than a magic capacity, because the *derivation* is what
    /// keeps eviction budget-driven ([NFR-PE-11]).
    fn roomy_budget() -> ConnectionBudget {
        ConnectionBudget::from_limits(65_536, 4)
    }

    /// What the stock 256-descriptor macOS default budgets on a 12-core host —
    /// the exact envelope a 72-member workspace died in ([NFR-PE-11]).
    fn stock_macos_budget() -> ConnectionBudget {
        ConnectionBudget::from_limits(256, 12)
    }

    /// The read-pool size the **single-root** path uses: whatever the engine
    /// sizes for itself, one connection per core. Read from
    /// [`RuntimeConfig::default`] rather than restated, so the single-root
    /// assertions below pin the engine's own default and can never drift into
    /// pinning a workspace budget ([FR-WS-03], [ADR-52]).
    fn single_root_pool() -> usize {
        crate::RuntimeConfig::default().reader_pool_size
    }

    /// A workspace of `n` synthetic members — `m000..m{n-1}`, no disk. Big-N
    /// fixtures are minimal by construction: the ceiling is a property of the
    /// registry, so proving it needs many members, not real stores.
    fn big_fed(n: usize) -> Federation {
        let names: Vec<String> = (0..n).map(|i| format!("m{i:03}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        fed(&refs)
    }

    fn lazy(names: &[&str]) -> EngineRegistry<SpyEngine> {
        EngineRegistry::with_budget(fed(names), RegistryMode::Lazy, roomy_budget())
    }
    fn serve(names: &[&str]) -> EngineRegistry<SpyEngine> {
        EngineRegistry::with_budget(fed(names), RegistryMode::Serve, roomy_budget())
    }

    /// A lazy registry constructs nothing up front, then builds a member's
    /// engine only on first touch and caches it — no watcher on the CLI path.
    #[test]
    fn lazy_builds_a_member_engine_on_first_touch_only() {
        reset_spies();
        let registry = lazy(&["a", "b", "c"]);
        assert_eq!(starts(), 0, "lazy construction builds nothing up front");

        registry.engine_for("a").unwrap();
        assert_eq!(starts(), 1, "first touch builds exactly one engine");
        assert_eq!(watches(), 0, "the CLI/lazy path spawns no watcher");

        registry.engine_for("a").unwrap();
        assert_eq!(starts(), 1, "a second touch reuses the cached engine");
        assert_eq!(registry.resident_members(), ["a"]);
    }

    /// `engine_starts()` counts **every** construction, which is what makes it
    /// the instrument a laziness claim can be asserted against — unlike
    /// residency (pinned to the budget by eviction) and unlike
    /// `reconstructions()` (blind to a re-open of a still-resident member).
    #[test]
    fn engine_starts_counts_every_construction_including_a_still_resident_reopen() {
        reset_spies();
        let registry = lazy(&["a", "b"]);
        assert_eq!(registry.engine_starts(), 0, "nothing built up front");

        registry.engine_for("a").unwrap();
        registry.engine_for("b").unwrap();
        assert_eq!(registry.engine_starts(), 2);
        assert_eq!(registry.engine_starts(), starts() as u64, "agrees with the spy");

        // A repeated touch of a RESIDENT member reuses the cached engine, so it
        // is not a start — the counter counts constructions, not touches.
        registry.engine_for("a").unwrap();
        assert_eq!(registry.engine_starts(), 2);
        assert_eq!(registry.reconstructions(), 0);

        // An evicted member's rebuild is both a start and a reconstruction, so
        // the two counters diverge by exactly the first builds.
        registry.evict_to_capacity(0);
        registry.engine_for("a").unwrap();
        assert_eq!(registry.engine_starts(), 3);
        assert_eq!(registry.reconstructions(), 1);
        assert_eq!(
            registry.engine_starts() - registry.reconstructions(),
            2,
            "starts minus reconstructions is the number of distinct members built"
        );
    }

    /// A scoped one-shot that needs one member constructs only that engine, not
    /// all N ([NFR-PE-10] acceptance).
    #[test]
    fn scoped_touch_constructs_only_the_needed_engine() {
        reset_spies();
        let registry = lazy(&["a", "b", "c"]);
        registry.engine_for("b").unwrap();
        assert_eq!(starts(), 1, "only the touched member is built");
        assert_eq!(registry.resident_members(), ["b"]);
    }

    /// Under serve, every member is warmed eagerly with exactly one watcher each.
    #[test]
    fn serve_warms_and_watches_every_member() {
        reset_spies();
        let registry = serve(&["a", "b", "c"]);
        assert_eq!(starts(), 3, "serve is eager: all members built up front");
        assert_eq!(watches(), 3, "one watcher per member under serve");
        assert_eq!(registry.resident_members(), ["a", "b", "c"]);
    }

    /// The fan-out helper runs a query across members and tags each result with
    /// its owning member ([FR-WS-03] acceptance).
    #[test]
    fn fan_out_tags_each_result_with_its_member() {
        reset_spies();
        let registry = lazy(&["api", "web"]);
        let results = registry.fan_out(|member, engine| {
            // The value is derived from the member's own engine, proving the
            // per-member routing.
            (member.name.clone(), engine.root.clone())
        });

        assert_eq!(results.len(), 2);
        for scoped in &results {
            let (name, root) = scoped.value.as_ref().expect("engine started");
            assert_eq!(
                &scoped.member, name,
                "the tag matches the member the engine belongs to"
            );
            assert_eq!(root, &PathBuf::from("/ws").join(name));
        }
        let members: Vec<&str> = results.iter().map(|s| s.member.as_str()).collect();
        assert_eq!(members, ["api", "web"], "tagged in discovery order");
    }

    /// Each member gets its **own** distinct engine instance — the structural
    /// guarantee that a member's sync advances only its own state ([FR-WS-03]).
    #[test]
    fn each_member_gets_its_own_engine_instance() {
        let registry = lazy(&["a", "b"]);
        let a = registry.engine_for("a").unwrap();
        let b = registry.engine_for("b").unwrap();
        assert!(!Arc::ptr_eq(&a, &b), "distinct engine instances per member");
        assert_ne!(a.root, b.root, "each engine is rooted at its own member");
    }

    /// A fan-out over a workspace with a member that fails to start reports that
    /// member as an `Err` and still answers for the healthy members ([ADR-53]).
    #[test]
    fn fan_out_degrades_a_failing_member_without_aborting() {
        // A registry whose engine type always fails to start.
        struct FailingEngine;
        impl MemberEngine for FailingEngine {
            type Watcher = ();
            fn start(
                _root: &Path,
                _read_connections: usize,
                _worker_pool: SharedWorkerPool,
            ) -> Result<Arc<Self>> {
                anyhow::bail!("store is corrupt")
            }
            fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
                Ok(())
            }
        }
        let registry =
            EngineRegistry::<FailingEngine>::with_budget(fed(&["a", "b"]), RegistryMode::Lazy, roomy_budget());
        let results = registry.fan_out(|_, _| ());
        assert_eq!(results.len(), 2, "every member is still reported");
        assert!(
            results.iter().all(|s| s.value.is_err()),
            "a failing member surfaces as Err, not a panic or a dropped member"
        );
    }

    // ── FR-WS-16 / BR-45: the open-state axis ─────────────────────────────
    //
    // `degraded` means **attempted and failed**. The two states that must never
    // be confused with it are a member laziness never reached and a member the
    // budget evicted — the first is the design ([NFR-PE-10]), the second is the
    // budget working ([NFR-PE-11]). Both would previously have been reported as
    // failures by any derivation that read residency instead of attempts.

    /// The open state of `member` in `states`, by name.
    fn open_state(states: &[MemberOpen], member: &str) -> MemberOpenState {
        states
            .iter()
            .find(|open| open.member == member)
            .unwrap_or_else(|| panic!("{member} is in the roster: {states:?}"))
            .state
            .clone()
    }

    /// A registry whose every member fails to start — the fixture the degraded
    /// assertions need, and the one shape [CR-100] observed at scale.
    ///
    /// [CR-100]: ../../../docs/requests/CR-100-workspace-resource-budget.md
    struct UnopenableEngine;
    impl MemberEngine for UnopenableEngine {
        type Watcher = ();
        fn start(
            _root: &Path,
            _read_connections: usize,
            _worker_pool: SharedWorkerPool,
        ) -> Result<Arc<Self>> {
            // The verbatim SQLite wording CR-100 observed, so the classification
            // under test is exercised on the real string and not a stand-in.
            anyhow::bail!(
                "applying the read-only connection contract (FR-DB-02): \
                 unable to open database file: Error code 14"
            )
        }
        fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
            Ok(())
        }
    }

    /// **The story's central regression assertion.** A member evicted to stay
    /// inside the budget reads `opened`, **never** `degraded` ([BR-45]).
    ///
    /// This is the sharpest failure mode in [FR-WS-16]: eviction is the [S-324]
    /// budget's normal operation, it happens on every all-member fan-out over a
    /// workspace larger than the residency cap, and an open-state derived from
    /// *residency* rather than from *open attempts* would report a healthy
    /// 72-member workspace as 64 degraded members and exit non-zero on every
    /// run — turning the fix into a worse defect than the one it replaces.
    ///
    /// Asserted at three points: while resident, after an explicit eviction, and
    /// after the rebuild that follows the next touch.
    ///
    /// [BR-45]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [S-324]: ../../../docs/planning/journal.md#s-324-workspace-wide-read-connection-budget-with-lru-member-engine-eviction
    #[test]
    fn an_evicted_member_reads_opened_and_is_never_reported_degraded() {
        reset_spies();
        let registry = lazy(&["a", "b", "c"]);
        for member in ["a", "b", "c"] {
            registry.engine_for(member).unwrap();
        }
        assert_eq!(
            registry.open_states().len(),
            3,
            "one row per roster member"
        );
        for member in ["a", "b", "c"] {
            assert_eq!(open_state(&registry.open_states(), member), MemberOpenState::Opened);
        }

        // Evict everything: residency drops to zero, open state does not move.
        registry.evict_to_capacity(0);
        assert_eq!(registry.resident_count(), 0, "nothing resident any more");
        let states = registry.open_states();
        assert!(
            states.iter().all(|open| open.state == MemberOpenState::Opened),
            "eviction reclaims a SUCCESS — it is not a failure to report: {states:?}"
        );
        let rollup = super::super::open_state::rollup(&states);
        assert!(
            rollup.degraded_members.is_empty(),
            "an evicted workspace names nobody degraded: {rollup:?}"
        );
        assert!(
            rollup.covers_all_members,
            "and its figures still cover every member"
        );

        // And the rebuild on next touch is likewise not a failure.
        registry.engine_for("a").unwrap();
        assert_eq!(open_state(&registry.open_states(), "a"), MemberOpenState::Opened);
    }

    /// **The same regression under the budget's own eviction**, not an explicit
    /// `evict_to_capacity` call. A workspace larger than the residency cap
    /// evicts during the fan-out itself; every member still reads `opened`
    /// ([NFR-PE-11], [BR-45]).
    ///
    /// The explicit-eviction test above could pass while admission-driven
    /// eviction still mislabelled, because that path evicts *before* the
    /// incoming start rather than after a completed one.
    #[test]
    fn budget_driven_eviction_during_a_fan_out_leaves_every_member_opened() {
        reset_spies();
        let budget = stock_macos_budget();
        let members = budget.max_resident_members() * 3;
        let registry =
            EngineRegistry::<SpyEngine>::with_budget(big_fed(members), RegistryMode::Lazy, budget);

        let results = registry.fan_out(|_, _| ());
        assert!(
            results.iter().all(|scoped| scoped.value.is_ok()),
            "every member opened"
        );
        assert!(
            registry.resident_count() < members,
            "the budget really did evict during the walk (resident {} of {members})",
            registry.resident_count()
        );

        let states = registry.open_states();
        assert_eq!(states.len(), members, "one row per roster member");
        assert!(
            states.iter().all(|open| open.state == MemberOpenState::Opened),
            "a member reclaimed by the budget mid-walk is not degraded"
        );
        assert_eq!(registry.start_failures(), 0, "and nothing actually failed");
    }

    /// A member **laziness never reached** reads `not-attempted`, never
    /// `degraded` and never `opened` ([NFR-PE-10], [BR-45]).
    ///
    /// The distinction is structural: only [`EngineRegistry::engine_for`] writes
    /// the ledger, so an untouched member has no entry at all — there is no
    /// path by which a never-attempted member could acquire a failure.
    #[test]
    fn a_lazily_skipped_member_reads_not_attempted_and_is_never_degraded() {
        reset_spies();
        let registry = lazy(&["touched", "skipped"]);
        registry.engine_for("touched").unwrap();

        let states = registry.open_states();
        assert_eq!(open_state(&states, "touched"), MemberOpenState::Opened);
        assert_eq!(
            open_state(&states, "skipped"),
            MemberOpenState::NotAttempted,
            "nothing was attempted, so nothing failed"
        );

        let rollup = super::super::open_state::rollup(&states);
        assert!(
            rollup.degraded_members.is_empty(),
            "laziness names nobody degraded: {rollup:?}"
        );
        assert_eq!(rollup.not_attempted, 1);
        assert!(
            !rollup.covers_all_members,
            "the answer still covers 1 of 2 members, which is a coverage fact \
             rather than a failure"
        );
    }

    /// A member **attempted and failed** reads `degraded`, is named, and — for
    /// the [CR-100] diagnostic over a store that is present — carries the
    /// host-resource cause rather than SQLite's store-corruption wording
    /// ([FR-WS-16] AC2/AC3).
    ///
    /// The fixture's member roots do not exist on disk, so the store file is
    /// absent and the cause is `store-unavailable`; the host-resource half of
    /// the classification is pinned in `federation::degraded`'s own table, which
    /// takes the store presence as an explicit input rather than probing.
    #[test]
    fn an_attempted_and_failed_open_reads_degraded_and_is_named() {
        let registry = EngineRegistry::<UnopenableEngine>::with_budget(
            fed(&["a", "b"]),
            RegistryMode::Lazy,
            roomy_budget(),
        );
        assert!(registry.fan_out(|_, _| ()).iter().all(|s| s.value.is_err()));

        let states = registry.open_states();
        assert!(
            states.iter().all(|open| open.state.is_degraded()),
            "both members were attempted and failed: {states:?}"
        );

        let rollup = super::super::open_state::rollup(&states);
        assert_eq!(
            rollup.degraded_members,
            ["a", "b"],
            "named, in manifest order"
        );
        assert_eq!(rollup.opened, 0);
        assert!(!rollup.covers_all_members);
        let notice = rollup.notice(&states).expect("two members degraded");
        assert!(
            notice.contains("a:") && notice.contains("b:"),
            "the human notice names them, each with its own reason line: {notice}"
        );
        // The fixture's member roots do not exist, so the store path is ABSENT and
        // the classification correctly claims no cause — the verbatim CR-100
        // diagnostic is what reaches the operator, never a `logos index` that
        // could not help an fd-exhausted member.
        assert!(
            notice.contains("unable to open database file"),
            "the verbatim diagnostic survives to the human channel: {notice}"
        );
        assert!(
            !notice.contains("logos index"),
            "and no re-index remedy is invented from an absent store: {notice}"
        );
    }

    /// A member that failed once and **opened later** is no longer degraded, and
    /// one that opened and later failed is — the ledger records the latest
    /// attempt, because that is the one the answer rests on.
    ///
    /// Load-bearing for a fan-out under descriptor pressure: `workspace status`
    /// walks every member four times, and a member that opened on the first walk
    /// and failed on the third has contributed nothing to the third walk's
    /// figures.
    #[test]
    fn the_ledger_records_the_latest_attempt_not_the_first() {
        reset_spies();
        // A registry over one member, whose engine fails only on the FIRST start.
        thread_local! {
            static FAIL_NEXT: Cell<bool> = const { Cell::new(true) };
        }
        struct FlakyEngine;
        impl MemberEngine for FlakyEngine {
            type Watcher = ();
            fn start(
                _root: &Path,
                _read_connections: usize,
                _worker_pool: SharedWorkerPool,
            ) -> Result<Arc<Self>> {
                if FAIL_NEXT.with(Cell::get) {
                    FAIL_NEXT.with(|c| c.set(false));
                    anyhow::bail!("Too many open files (os error 24)");
                }
                Ok(Arc::new(FlakyEngine))
            }
            fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
                Ok(())
            }
        }

        let registry = EngineRegistry::<FlakyEngine>::with_budget(
            fed(&["a"]),
            RegistryMode::Lazy,
            roomy_budget(),
        );
        assert!(registry.engine_for("a").is_err(), "the first attempt fails");
        assert!(open_state(&registry.open_states(), "a").is_degraded());

        assert!(registry.engine_for("a").is_ok(), "the retry succeeds");
        assert_eq!(
            open_state(&registry.open_states(), "a"),
            MemberOpenState::Opened,
            "a member that opened on retry is not degraded — the answer used it"
        );
    }

    /// Reading the open states constructs **no** engine and opens **no**
    /// connection ([NFR-PE-10]): it is a ledger read, which is what keeps the
    /// degraded roll-up off `workspace status`'s walk budget.
    #[test]
    fn reading_the_open_states_constructs_nothing() {
        reset_spies();
        let registry = lazy(&["a", "b", "c"]);
        registry.engine_for("a").unwrap();
        let before = (starts(), registry.engine_starts(), peak_connections());

        for _ in 0..5 {
            let _ = registry.open_states();
        }

        assert_eq!(
            (starts(), registry.engine_starts(), peak_connections()),
            before,
            "five readouts built no engine and opened no connection"
        );
    }

    /// Eviction drops the least-recently-touched engines beyond the capacity.
    #[test]
    fn evicts_least_recently_used_beyond_capacity() {
        reset_spies();
        let registry = serve(&["a", "b", "c"]); // touched a,b,c in warm order
        registry.engine_for("a").unwrap(); // now "a" is most-recently touched

        let evicted = registry.evict_to_capacity(1);
        assert_eq!(registry.resident_members(), ["a"], "keeps the most recent");
        let mut evicted_sorted = evicted;
        evicted_sorted.sort();
        assert_eq!(evicted_sorted, ["b", "c"], "the two idle members are evicted");
    }

    /// Eviction returns the evicted members least-recently-touched **first** —
    /// asserted with a non-alphabetical touch order, so a name-sorted (rather
    /// than recency-ordered) result would fail.
    #[test]
    fn eviction_returns_evicted_members_least_recently_touched_first() {
        let registry = lazy(&["a", "b", "c"]);
        // Touch out of alphabetical order: c (oldest), then a, then b (newest).
        registry.engine_for("c").unwrap();
        registry.engine_for("a").unwrap();
        registry.engine_for("b").unwrap();

        let evicted = registry.evict_to_capacity(1);
        assert_eq!(
            evicted,
            ["c", "a"],
            "keeps the most-recently-touched (b); evicts the rest LRU-first, \
             not name-sorted"
        );
        assert_eq!(registry.resident_members(), ["b"]);
    }

    /// Eviction under capacity is a no-op.
    #[test]
    fn eviction_under_capacity_evicts_nothing() {
        let registry = serve(&["a", "b"]);
        assert!(registry.evict_to_capacity(5).is_empty());
        assert_eq!(registry.resident_count(), 2);
    }

    /// After eviction, the next touch rebuilds the engine — and, under serve,
    /// re-spawns its watcher.
    #[test]
    fn touch_after_eviction_rebuilds_and_rewatches_under_serve() {
        reset_spies();
        let registry = serve(&["a", "b"]);
        assert_eq!((starts(), watches()), (2, 2));

        registry.evict_to_capacity(0);
        assert_eq!(registry.resident_count(), 0, "everything evicted");

        registry.engine_for("a").unwrap();
        assert_eq!(starts(), 3, "the evicted engine is rebuilt on next touch");
        assert_eq!(watches(), 3, "serve re-watches the rebuilt engine");
    }

    /// Under serve, a member whose engine fails to **start** is skipped during
    /// the eager warm (logged, not fatal) while the healthy members stay
    /// resident — the warm must not panic ([ADR-53] degrade-don't-abort).
    #[test]
    fn serve_warm_skips_a_failing_member_and_keeps_the_healthy_ones() {
        #[derive(Debug)]
        struct PickyEngine;
        impl MemberEngine for PickyEngine {
            type Watcher = ();
            fn start(
                root: &Path,
                _read_connections: usize,
                _worker_pool: SharedWorkerPool,
            ) -> Result<Arc<Self>> {
                if root.ends_with("b") {
                    anyhow::bail!("store is corrupt");
                }
                Ok(Arc::new(PickyEngine))
            }
            fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
                Ok(())
            }
        }

        // Eager warm over a workspace whose member "b" cannot start.
        let registry = EngineRegistry::<PickyEngine>::with_budget(
            fed(&["a", "b", "c"]),
            RegistryMode::Serve,
            roomy_budget(),
        );
        assert_eq!(
            registry.resident_members(),
            ["a", "c"],
            "the failing member is skipped; the healthy members stay resident"
        );
        assert_eq!(registry.resident_count(), 2);
    }

    /// Under serve, a member whose **watcher** fails to spawn degrades to a
    /// watcherless-but-resident engine — the touch still succeeds ([FR-SY-06]).
    #[test]
    fn serve_degrades_when_a_watcher_fails_to_spawn() {
        #[derive(Debug)]
        struct NoWatchEngine;
        impl MemberEngine for NoWatchEngine {
            type Watcher = ();
            fn start(
                _root: &Path,
                _read_connections: usize,
                _worker_pool: SharedWorkerPool,
            ) -> Result<Arc<Self>> {
                Ok(Arc::new(NoWatchEngine))
            }
            fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
                anyhow::bail!("OS watcher could not attach")
            }
        }

        let registry = EngineRegistry::<NoWatchEngine>::with_budget(
            fed(&["a", "b"]),
            RegistryMode::Serve,
            roomy_budget(),
        );
        // Watcher failure is non-fatal: both engines are still built and resident.
        assert_eq!(registry.resident_members(), ["a", "b"]);
        assert_eq!(registry.resident_count(), 2);
        // A subsequent scoped touch of a watcherless member still succeeds.
        assert!(registry.engine_for("a").is_ok());
    }

    /// Touching an unknown member is an error, not a silent build.
    #[test]
    fn unknown_member_is_an_error() {
        let registry = lazy(&["a"]);
        let err = registry.engine_for("nope").unwrap_err();
        assert!(err.to_string().contains("no such workspace member"));
    }

    /// `default_engine` prefers `[workspace] default`, else the first member —
    /// the default-member policy the shared single-root tools run against
    /// ([FR-WS-05]).
    #[test]
    fn default_engine_prefers_declared_default_then_first_member() {
        // No declared default → the first member in discovery order.
        let registry = lazy(&["a", "b", "c"]);
        registry.default_engine().unwrap();
        assert_eq!(registry.resident_members(), ["a"], "no default → first member");

        // A declared default wins over discovery order.
        let mut federation = fed(&["a", "b", "c"]);
        federation.default = Some("b".to_string());
        let registry =
            EngineRegistry::<SpyEngine>::with_budget(federation, RegistryMode::Lazy, roomy_budget());
        registry.default_engine().unwrap();
        assert_eq!(registry.resident_members(), ["b"], "declared default wins");
    }

    /// `new_serve_default` warms **only** the default member eagerly (watching
    /// it), leaving the rest lazy — the context-aware `serve --ui` policy
    /// ([FR-WS-06], [NFR-PE-10]): opening the workspace must not pay N× cold-start
    /// or start N watchers up front.
    #[test]
    fn serve_default_warms_and_watches_only_the_default_member() {
        reset_spies();
        // No declared default → the first member in discovery order is warmed.
        let registry = EngineRegistry::<SpyEngine>::serve_default_with_budget(
            fed(&["a", "b", "c"]),
            roomy_budget(),
        );
        assert_eq!(starts(), 1, "only the default member is built up front, not all N");
        assert_eq!(watches(), 1, "only the default member is watched up front");
        assert_eq!(registry.resident_members(), ["a"], "the first member is the eager default");

        // A touch of another member builds + watches it lazily (serve semantics).
        registry.engine_for("c").unwrap();
        assert_eq!((starts(), watches()), (2, 2), "a later member is built and watched on first touch");
    }

    /// `new_serve_default` honours a **declared** default over discovery order.
    #[test]
    fn serve_default_prefers_the_declared_default() {
        reset_spies();
        let mut federation = fed(&["a", "b", "c"]);
        federation.default = Some("b".to_string());
        let registry =
            EngineRegistry::<SpyEngine>::serve_default_with_budget(federation, roomy_budget());
        assert_eq!(registry.resident_members(), ["b"], "the declared default is the eager member");
        assert_eq!(starts(), 1, "still exactly one eager engine");
    }

    /// A default member that fails to warm **degrades** — `new_serve_default`
    /// still returns a usable registry (the failure is logged, not fatal) so the
    /// healthy members answer their cross-service queries ([ADR-53]).
    #[test]
    fn serve_default_degrades_when_the_default_fails_to_warm() {
        #[derive(Debug)]
        struct FailingEngine;
        impl MemberEngine for FailingEngine {
            type Watcher = ();
            fn start(
                _root: &Path,
                _read_connections: usize,
                _worker_pool: SharedWorkerPool,
            ) -> Result<Arc<Self>> {
                anyhow::bail!("store is corrupt")
            }
            fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
                Ok(())
            }
        }
        // Must not panic even though the default cannot start.
        let registry = EngineRegistry::<FailingEngine>::serve_default_with_budget(
            fed(&["a", "b"]),
            roomy_budget(),
        );
        assert_eq!(registry.resident_count(), 0, "the failed default left no resident");
        // The registry is still usable — a fan-out reports the members as degraded
        // rather than the whole workspace aborting.
        assert_eq!(registry.fan_out(|_, _| ()).len(), 2);
    }

    /// `default_engine` on a member-less workspace errors rather than panicking.
    #[test]
    fn default_engine_errors_on_an_empty_workspace() {
        let registry =
            EngineRegistry::<SpyEngine>::with_budget(fed(&[]), RegistryMode::Lazy, roomy_budget());
        assert!(
            registry.default_engine().is_err(),
            "a workspace with no members has no engine to answer"
        );
    }

    // ── the workspace connection budget (NFR-PE-11 / ADR-63) ───────────────

    /// A fan-out over an N-member workspace holds no more than the budgeted
    /// live read connections **at any instant** — asserted at N = 72 and
    /// N = 200 under the stock 256-descriptor envelope that failed 63 of 72
    /// members ([NFR-PE-11] acceptance, [BR-45]).
    ///
    /// The instrument is the spy's live-connection high-water mark, not the
    /// registry's own arithmetic: an engine the registry has evicted but a
    /// caller still holds is genuinely open, and only a drop-counting spy sees
    /// that.
    #[test]
    fn a_fan_out_holds_no_more_than_the_budgeted_live_connections() {
        let budget = stock_macos_budget();
        for members in [72, 200] {
            reset_spies();
            let registry =
                EngineRegistry::<SpyEngine>::with_budget(big_fed(members), RegistryMode::Lazy, budget);

            let results = registry.fan_out(|_, engine| engine.read_connections);
            assert_eq!(results.len(), members, "every member is still answered");
            assert!(
                results.iter().all(|scoped| scoped.value.is_ok()),
                "no member fails to open inside the budget"
            );

            assert!(
                peak_connections() <= budget.total_read_connections(),
                "N = {members}: peaked at {} live read connections, over the \
                 budgeted ceiling of {}",
                peak_connections(),
                budget.total_read_connections(),
            );
            assert!(
                registry.resident_count() <= budget.max_resident_members(),
                "N = {members}: {} members resident, over the budgeted {}",
                registry.resident_count(),
                budget.max_resident_members(),
            );
            assert!(registry.live_read_connections() <= budget.total_read_connections());
        }
    }

    /// The ceiling holds because room is made **before** a connection is opened:
    /// admitting the member that overflows the budget must not peak one member's
    /// worth above it, even transiently.
    #[test]
    fn admission_evicts_before_opening_the_new_connections() {
        let budget = stock_macos_budget();
        let cap = budget.max_resident_members();
        reset_spies();
        let registry =
            EngineRegistry::<SpyEngine>::with_budget(big_fed(cap + 1), RegistryMode::Lazy, budget);

        for member in registry.members().iter().map(|m| m.name.clone()).collect::<Vec<_>>() {
            registry.engine_for(&member).unwrap();
        }
        assert_eq!(
            peak_connections(),
            cap * budget.per_member_read_connections(),
            "the overflowing admission peaked above the ceiling — room was made \
             after the start, not before it"
        );
    }

    /// A full all-member walk touches each member once, so it must reconstruct
    /// **nothing**: the reconstruction count is reported as an assertion so
    /// eviction thrash fails loudly instead of costing silent latency
    /// ([NFR-PE-11] acceptance).
    #[test]
    fn one_all_member_walk_reconstructs_nothing() {
        let budget = stock_macos_budget();
        for members in [72, 200] {
            reset_spies();
            let registry =
                EngineRegistry::<SpyEngine>::with_budget(big_fed(members), RegistryMode::Lazy, budget);

            registry.fan_out(|_, _| ());

            assert_eq!(
                registry.reconstructions(),
                0,
                "N = {members}: a single walk rebuilt {} engine(s) it had already \
                 built — eviction thrash",
                registry.reconstructions(),
            );
            assert_eq!(
                starts(),
                members,
                "N = {members}: exactly one construction per member"
            );
            // Without this the assertions above hold for ANY budget — a single
            // walk touches each member once, so zero reconstructions is true even
            // at a cap of two. Pinning residency is what makes the test sensitive
            // to the budget actually being applied.
            assert_eq!(
                registry.resident_count(),
                budget.max_resident_members().min(members),
                "N = {members}: the walk must end holding exactly the budget's worth"
            );
            assert_eq!(registry.start_failures(), 0, "N = {members}: every member opened");
        }
    }

    /// A *second* sequential walk rebuilds **every** member — LRU's worst case,
    /// because a scan evicts exactly the members it is about to revisit. The
    /// counter reports the full cost rather than hiding it, which is what makes
    /// the pathology measurable and [ADR-63]'s open question (clock or FIFO
    /// instead?) answerable with numbers.
    #[test]
    fn a_repeated_walk_reports_every_reconstruction_it_costs() {
        let budget = stock_macos_budget();
        let members = 72;
        assert!(
            members > budget.max_resident_members(),
            "the fixture must exceed the budget for a rewalk to thrash at all"
        );
        reset_spies();
        let registry =
            EngineRegistry::<SpyEngine>::with_budget(big_fed(members), RegistryMode::Lazy, budget);

        registry.fan_out(|_, _| ());
        assert_eq!(registry.reconstructions(), 0, "the first walk is clean");

        registry.fan_out(|_, _| ());
        assert_eq!(
            registry.reconstructions(),
            members as u64,
            "a sequential rewalk under LRU rebuilds every member; a smaller \
             number means the walk changed, a larger one means double-building"
        );
        // Thrash costs cold starts; it must never cost the ceiling.
        assert!(peak_connections() <= budget.total_read_connections());
    }

    /// An LRU-evicted member engine, reconstructed on next touch, answers
    /// **identically** to a never-evicted one — the store is canonical and an
    /// engine holds no authoritative state ([NFR-PE-11] acceptance, [FR-DB-01]).
    #[test]
    fn an_evicted_engine_answers_identically_once_reconstructed() {
        let budget = stock_macos_budget();
        let members = budget.max_resident_members() * 4; // guarantees eviction
        reset_spies();
        let registry =
            EngineRegistry::<SpyEngine>::with_budget(big_fed(members), RegistryMode::Lazy, budget);

        let target = registry.members()[0].name.clone();
        let before = registry.engine_for(&target).unwrap();
        let answer_before = (before.root.clone(), before.read_connections);
        drop(before);

        // Touch every other member so the target is evicted, then touch it again.
        registry.fan_out(|_, _| ());
        assert!(
            !registry.resident_members().contains(&target),
            "the fixture must actually evict the target for this to prove anything"
        );

        let after = registry.engine_for(&target).unwrap();
        assert_eq!(
            (after.root.clone(), after.read_connections),
            answer_before,
            "a reconstructed engine answered differently from a never-evicted one"
        );
        assert!(
            registry.reconstructions() >= 1,
            "the target must have been genuinely rebuilt, not served from cache"
        );
    }

    /// Eviction is driven by the **budget**, not by a fixed member count: the
    /// same workspace on a host with a larger descriptor allowance keeps more
    /// members resident, with no code change ([NFR-PE-11] acceptance).
    #[test]
    fn eviction_is_driven_by_the_budget_not_a_fixed_member_count() {
        let members = 72;
        let mut residency = Vec::new();
        for budget in [stock_macos_budget(), ConnectionBudget::from_limits(65_536, 12)] {
            reset_spies();
            let registry =
                EngineRegistry::<SpyEngine>::with_budget(big_fed(members), RegistryMode::Lazy, budget);
            registry.fan_out(|_, _| ());
            assert!(peak_connections() <= budget.total_read_connections());
            residency.push(registry.resident_count());
        }
        assert!(
            residency[1] > residency[0],
            "a roomier host must keep more members resident ({} vs {})",
            residency[1],
            residency[0],
        );
        assert_eq!(
            residency[1], members,
            "a host with descriptors to spare evicts nobody"
        );
    }

    /// Every resident member engine is built with its **budgeted** share of
    /// read connections, not the per-core pool an engine sizes for itself.
    #[test]
    fn a_member_engine_opens_only_its_budgeted_share_of_connections() {
        let budget = stock_macos_budget();
        reset_spies();
        let registry =
            EngineRegistry::<SpyEngine>::with_budget(big_fed(8), RegistryMode::Lazy, budget);
        let engine = registry.engine_for("m000").unwrap();
        assert_eq!(engine.read_connections, budget.per_member_read_connections());
        assert_eq!(registry.budget(), budget, "the registry reports its budget");
    }

    /// The budget applies to the **CLI fan-out path** too, not only `serve` —
    /// the extension of [NFR-PE-10]'s serve-only eviction that [ADR-63] makes.
    #[test]
    fn the_budget_bounds_the_lazy_cli_path_as_well_as_serve() {
        let budget = stock_macos_budget();
        for mode in [RegistryMode::Lazy, RegistryMode::Serve] {
            reset_spies();
            // Whatever each mode warms up front, drive the all-member walk the
            // CLI workspace commands and the `/api/v1/workspace/*` handlers both
            // perform — that is the path the budget has to bound in either mode.
            let registry = EngineRegistry::<SpyEngine>::with_budget(big_fed(72), mode, budget);
            registry.fan_out(|_, _| ());

            assert_eq!(starts(), 72, "{mode:?} must reach every member");
            assert!(
                registry.resident_count() <= budget.max_resident_members(),
                "{mode:?} left {} members resident, over the budgeted {}",
                registry.resident_count(),
                budget.max_resident_members(),
            );
            assert!(
                peak_connections() <= budget.total_read_connections(),
                "{mode:?} peaked at {} live read connections, over the budgeted {}",
                peak_connections(),
                budget.total_read_connections(),
            );
        }
    }

    /// The eager warm is bounded by the budget, so opening a workspace never
    /// pays N cold starts to keep a handful ([NFR-PE-10]).
    #[test]
    fn the_eager_warm_stops_at_the_budget_rather_than_warming_every_member() {
        let budget = stock_macos_budget();
        reset_spies();
        let registry =
            EngineRegistry::<SpyEngine>::with_budget(big_fed(200), RegistryMode::Serve, budget);
        assert_eq!(
            starts(),
            budget.max_resident_members(),
            "warming 200 members to keep {} would make boot cost a function of \
             the member count",
            budget.max_resident_members(),
        );
        assert_eq!(registry.reconstructions(), 0, "the warm evicts nothing it built");
        // The deferred members are still reachable — they build on first touch.
        assert!(registry.engine_for("m199").is_ok());
    }

    /// An engine a caller still holds is **never** evicted, so no member ever has
    /// two live engines over one store.
    ///
    /// This is the shipped `serve` shape: the web surface resolves the default
    /// member's engine once and holds it for the process lifetime, so on a
    /// workspace larger than the budget the default is exactly what LRU would
    /// discard first — and rebuilding it would give that store a second writer
    /// actor while the surface kept reading a frozen hydration cache from the
    /// orphan.
    #[test]
    fn a_member_engine_a_caller_still_holds_is_never_evicted() {
        let budget = stock_macos_budget();
        let members = budget.max_resident_members() * 4; // far beyond the budget
        reset_spies();
        let registry =
            EngineRegistry::<SpyEngine>::with_budget(big_fed(members), RegistryMode::Lazy, budget);

        // A long-lived caller pins the first member, exactly as the serve
        // surface pins the workspace default.
        let pinned = registry.engine_for("m000").unwrap();
        registry.fan_out(|_, _| ());

        assert!(
            registry.resident_members().contains(&"m000".to_string()),
            "the held member was evicted; its next touch would build a second \
             engine over the same store"
        );
        assert_eq!(
            starts(),
            members,
            "exactly one engine per member — a held-then-evicted member would \
             show up here as an extra start"
        );
        assert_eq!(registry.reconstructions(), 0, "nothing was rebuilt");
        assert!(
            Arc::ptr_eq(&pinned, &registry.engine_for("m000").unwrap()),
            "the caller's engine and the registry's must remain the same instance"
        );
        // Pinning does not widen the ceiling: the held member occupies one of the
        // budgeted slots rather than sitting outside them.
        assert!(registry.resident_count() <= budget.max_resident_members());
        assert!(peak_connections() <= budget.total_read_connections());
    }

    /// `live_read_connections` tracks residency rather than reporting the budget
    /// it wishes were true — asserted with **equalities**, because a readout
    /// pinned only by `<= budget` would still pass if it always returned zero.
    #[test]
    fn live_read_connections_tracks_actual_residency() {
        let budget = stock_macos_budget();
        reset_spies();
        let registry =
            EngineRegistry::<SpyEngine>::with_budget(big_fed(72), RegistryMode::Lazy, budget);

        assert_eq!(registry.live_read_connections(), 0, "nothing resident yet");

        registry.engine_for("m000").unwrap();
        assert_eq!(
            registry.live_read_connections(),
            budget.per_member_read_connections(),
            "one resident member reports one member's worth of connections"
        );

        registry.fan_out(|_, _| ());
        assert_eq!(
            registry.live_read_connections(),
            registry.resident_count() * budget.per_member_read_connections(),
        );
        assert_eq!(registry.live_read_connections(), budget.total_read_connections());

        registry.evict_to_capacity(0);
        assert_eq!(
            registry.live_read_connections(),
            0,
            "evicting everything must be visible in the readout"
        );
    }

    /// Eviction stops the evicted member's watcher, so a serve workspace larger
    /// than the budget ends up watching the resident set — not all N members.
    ///
    /// This is the steady-state cost [NFR-PE-10] and [NFR-PE-11] exist to bound,
    /// and it is only observable by counting watcher *drops* against spawns.
    #[test]
    fn a_serve_warm_leaves_one_live_watcher_per_resident_member() {
        let budget = stock_macos_budget();
        let cap = budget.max_resident_members();
        reset_spies();
        let registry =
            EngineRegistry::<SpyEngine>::with_budget(big_fed(72), RegistryMode::Serve, budget);

        // The eager warm stops at the budget rather than building 72 to keep 8.
        assert_eq!(starts(), cap, "the warm builds the budget's worth, not all N");
        assert_eq!(watches(), cap, "watch-on-touch: each member built is watched");
        assert_eq!(live_watchers(), cap, "and each of them is still watching");

        // Now walk every member. Each is built and watched on its first touch,
        // and each eviction stops that member's watcher — so live watchers track
        // residency rather than accumulating one per member ever touched.
        registry.fan_out(|_, _| ());
        assert_eq!(watches(), 72, "every member built is watched, warm or lazy");
        assert_eq!(
            live_watchers(),
            registry.resident_count(),
            "a watcher lives exactly as long as its member's residency; \
             {} spawned, {} still running, {} resident",
            watches(),
            live_watchers(),
            registry.resident_count(),
        );
        assert!(
            live_watchers() <= cap,
            "{} watchers still running, over the budgeted {cap} residents",
            live_watchers(),
        );
    }

    /// A member that fails to start under a **binding** budget does not corrupt
    /// residency accounting: the failure is reported, the healthy members stay
    /// answerable, and the failed member is not counted as a reconstruction when
    /// it later succeeds.
    ///
    /// The pre-existing degrade tests all run under a roomy budget, where
    /// admission eviction never fires — so without this the interaction between
    /// "make room, then start" and a start that fails is untested.
    #[test]
    fn a_failing_member_under_a_binding_budget_degrades_without_corrupting_residency() {
        #[derive(Debug)]
        struct FlakyEngine {
            root: PathBuf,
        }
        // "b" fails the first time it is asked and succeeds afterwards.
        thread_local! {
            static B_ATTEMPTS: Cell<usize> = const { Cell::new(0) };
        }
        impl MemberEngine for FlakyEngine {
            type Watcher = ();
            fn start(
                root: &Path,
                _read_connections: usize,
                _worker_pool: SharedWorkerPool,
            ) -> Result<Arc<Self>> {
                if root.ends_with("b") {
                    let attempt = B_ATTEMPTS.with(|c| {
                        c.set(c.get() + 1);
                        c.get()
                    });
                    if attempt == 1 {
                        anyhow::bail!("store is briefly unavailable");
                    }
                }
                Ok(Arc::new(FlakyEngine {
                    root: root.to_path_buf(),
                }))
            }
            fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
                Ok(())
            }
        }

        // A budget that holds only two members, so admission genuinely evicts.
        let budget = ConnectionBudget::from_limits(78, 12);
        assert_eq!(budget.max_resident_members(), 2, "the fixture must bind");
        let registry = EngineRegistry::<FlakyEngine>::with_budget(
            fed(&["a", "b", "c"]),
            RegistryMode::Lazy,
            budget,
        );

        assert!(registry.engine_for("a").is_ok());
        assert!(
            registry.engine_for("b").is_err(),
            "the first touch of b surfaces the start failure"
        );
        assert_eq!(
            registry.reconstructions(),
            0,
            "a failed start is not recorded, so b's later success is a first \
             construction rather than a phantom reconstruction"
        );
        assert_eq!(
            registry.start_failures(),
            1,
            "the failed start is counted registry-wide, not only in the fan-out \
             read-model that happened to surface it"
        );
        assert!(registry.engine_for("b").is_ok(), "b recovers on its next touch");
        assert_eq!(registry.reconstructions(), 0);
        assert_eq!(registry.start_failures(), 1, "a later success does not erase it");
        assert!(
            registry.resident_count() <= budget.max_resident_members(),
            "a failing member must not push residency over the budget"
        );
        // The healthy members still answer.
        let results = registry.fan_out(|_, engine| engine.root.clone());
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|scoped| scoped.value.is_ok()));
    }

    // ── the single-root invariant via Backing (FR-WS-03 / ADR-52) ──────────

    /// With no workspace, `Backing::resolve` yields `Single` — one engine, no
    /// registry, no fan-out watchers: the single-root path bypasses the registry
    /// entirely.
    #[test]
    fn backing_single_bypasses_the_registry() {
        reset_spies();
        let backing = Backing::<SpyEngine>::resolve(None, RegistryMode::Serve, || {
            SpyEngine::start(
                Path::new("/solo"),
                single_root_pool(),
                SharedWorkerPool::with_threads(1).unwrap(),
            )
            .unwrap()
        });

        assert!(!backing.is_federated());
        assert!(backing.as_federated().is_none(), "no registry is allocated");
        assert!(backing.as_single().is_some(), "the single-root engine is used");
        assert_eq!(starts(), 1, "exactly the one single-root engine is built");
        assert_eq!(watches(), 0, "the single-root path spawns no registry watcher");
        // The engine's pool size is NOT asserted here: this test supplies it to
        // the `single` thunk itself, so any assertion on it would restate its own
        // input. That the single-root path keeps `RuntimeConfig::default()`'s
        // core-sized pool is a property of the real call sites, pinned against a
        // real engine in `tests/single_root_pool_invariant.rs`.
    }

    /// With a workspace, `Backing::resolve` yields `Federated` and never builds a
    /// single-root engine (the `single` thunk is not invoked).
    #[test]
    fn backing_federated_when_a_workspace_is_present() {
        reset_spies();
        let backing = Backing::<SpyEngine>::resolve(Some(fed(&["a", "b"])), RegistryMode::Serve, || {
            panic!("the single-root engine must not be built on the federated path")
        });

        assert!(backing.is_federated());
        let registry = backing.as_federated().expect("federated backing");
        assert_eq!(registry.resident_members(), ["a", "b"]);
        assert_eq!((starts(), watches()), (2, 2), "the workspace members are warmed");
    }

    // ── the shared worker pool (S-325, NFR-PE-11, NFR-PE-08, ADR-63) ───────

    /// Every resident member engine runs on **one** pool, so a workspace's
    /// thread cost is the budget's, not `members × cores` ([NFR-PE-11]).
    #[test]
    fn resident_member_engines_share_one_worker_pool() {
        reset_spies();
        let registry = lazy(&["a", "b", "c"]);
        let engines: Vec<Arc<SpyEngine>> = ["a", "b", "c"]
            .iter()
            .map(|m| registry.engine_for(m).unwrap())
            .collect();

        assert_eq!(starts(), 3, "the fixture must actually build three engines");
        for other in &engines[1..] {
            assert!(
                SharedWorkerPool::ptr_eq(&engines[0].worker_pool, &other.worker_pool),
                "member engines were handed different worker pools; the thread                  cost is still a multiple of the member count"
            );
        }
        assert_eq!(
            registry.shared_worker_threads(),
            registry.budget().worker_threads(),
            "the shared pool must be sized by the budget"
        );
    }

    /// The pool is sized by the **host**, not by the workspace: 200 members
    /// share the same pool of the same size three do ([NFR-PE-11]'s "threads
    /// track C rather than N × C").
    #[test]
    fn the_shared_pool_is_sized_by_the_host_not_the_member_count() {
        reset_spies();
        let small = EngineRegistry::<SpyEngine>::with_budget(
            big_fed(3),
            RegistryMode::Lazy,
            stock_macos_budget(),
        );
        let large = EngineRegistry::<SpyEngine>::with_budget(
            big_fed(200),
            RegistryMode::Lazy,
            stock_macos_budget(),
        );
        let mut resident_pools: Vec<SharedWorkerPool> = Vec::new();
        for registry in [&small, &large] {
            for member in registry.members().to_vec() {
                let engine = registry.engine_for(&member.name).unwrap();
                if registry.resident_members().contains(&member.name) {
                    resident_pools.push(engine.worker_pool.clone());
                }
            }
        }
        assert_eq!(
            large.shared_worker_threads(),
            small.shared_worker_threads(),
            "a 200-member workspace ran more worker threads than a 3-member one"
        );
        // `shared_worker_threads` upgrades ONE weak handle, so on its own it
        // reports a single pool's size and cannot see a second, third or 200th
        // pool — the very failure this test is named for. Compare the handles the
        // engines actually hold, per registry, so "one pool" is asserted rather
        // than inferred from a number that would look identical either way.
        for registry in [&small, &large] {
            let pools: Vec<SharedWorkerPool> = registry
                .resident_members()
                .iter()
                .map(|name| registry.engine_for(name).unwrap().worker_pool.clone())
                .collect();
            assert!(
                pools.len() > 1,
                "the fixture needs more than one resident engine to compare pools"
            );
            assert!(
                pools
                    .windows(2)
                    .all(|pair| SharedWorkerPool::ptr_eq(&pair[0], &pair[1])),
                "{} resident engines of this registry hold different worker \
                 pools; the thread cost is still a multiple of residency",
                pools.len(),
            );
        }
        assert!(
            resident_pools.len() > 2,
            "the walk must leave engines to compare"
        );
        assert_eq!(
            large.shared_worker_threads(),
            stock_macos_budget().worker_threads(),
        );
        assert!(
            large.resident_count() < large.members().len(),
            "the fixture must exceed the budget, or nothing was evicted"
        );
    }

    /// Sharing composes with eviction: a member evicted and rebuilt rejoins the
    /// **same** pool rather than building a second one, as long as any resident
    /// still holds it.
    #[test]
    fn an_evicted_member_rejoins_the_same_pool_when_rebuilt() {
        reset_spies();
        // A budget that holds two of three members, so touching the third
        // evicts the first while the second keeps the pool alive.
        let budget = ConnectionBudget::from_limits(78, 12);
        assert_eq!(budget.max_resident_members(), 2, "fixture assumption");
        let registry =
            EngineRegistry::<SpyEngine>::with_budget(fed(&["a", "b", "c"]), RegistryMode::Lazy, budget);

        let first = registry.engine_for("a").unwrap();
        let pool = first.worker_pool.clone();
        drop(first); // so LRU may evict it — a held engine is never evicted

        registry.engine_for("b").unwrap();
        registry.engine_for("c").unwrap();
        assert!(
            !registry.resident_members().contains(&"a".to_string()),
            "a was expected to be evicted; residency = {:?}",
            registry.resident_members()
        );

        let rebuilt = registry.engine_for("a").unwrap();
        assert!(
            registry.reconstructions() > 0,
            "nothing was rebuilt, so this proves nothing"
        );
        assert!(
            SharedWorkerPool::ptr_eq(&pool, &rebuilt.worker_pool),
            "reconstruction built a SECOND worker pool instead of rejoining the              workspace's"
        );
    }

    /// The shared pool is torn down when the last resident engine goes: nothing
    /// keeps `worker_threads` workers alive for a workspace with nothing
    /// resident ([ADR-63]'s teardown clause).
    #[test]
    fn the_shared_pool_is_torn_down_with_the_last_resident() {
        reset_spies();
        let registry = lazy(&["a", "b"]);
        registry.engine_for("a").unwrap();
        registry.engine_for("b").unwrap();
        assert_eq!(registry.shared_worker_threads(), registry.budget().worker_threads());

        registry.evict_to_capacity(1);
        assert_eq!(
            registry.shared_worker_threads(),
            registry.budget().worker_threads(),
            "one resident still holds the pool, so it must stay up"
        );

        registry.evict_to_capacity(0);
        assert_eq!(registry.resident_count(), 0);
        assert_eq!(
            registry.shared_worker_threads(),
            0,
            "the pool outlived the last resident engine; its workers are orphans"
        );

        // …and the next admission brings a pool back up rather than failing.
        registry.engine_for("a").unwrap();
        assert_eq!(registry.shared_worker_threads(), registry.budget().worker_threads());
    }

    /// The pool's lifetime follows the **engines**, not the registry's map: a
    /// member a caller still holds is skipped by eviction and keeps the pool up;
    /// once released and evicted, the pool goes with it.
    #[test]
    fn the_pool_lives_exactly_as_long_as_its_engines() {
        reset_spies();
        let registry = lazy(&["a"]);
        let held = registry.engine_for("a").unwrap();

        registry.evict_to_capacity(0);
        assert_eq!(
            registry.resident_count(),
            1,
            "a held engine is never evicted (its connections would stay open \
             anyway), so it is still resident here"
        );
        assert_eq!(
            registry.shared_worker_threads(),
            registry.budget().worker_threads(),
            "a held engine's jobs would have nowhere to run"
        );

        drop(held);
        registry.evict_to_capacity(0);
        assert_eq!(registry.resident_count(), 0);
        assert_eq!(
            registry.shared_worker_threads(),
            0,
            "the pool outlived every engine that could submit to it"
        );
    }

    /// A hit on an **already-resident** member does not wait for another
    /// member's in-flight admission.
    ///
    /// The registry serialises admission deliberately — that is what keeps the
    /// live count inside the budget and stops two touches building two engines
    /// over one store. What it must *not* do is hold that same lock across the
    /// hit path: a fan-out over a large workspace spends its whole duration in
    /// cold starts and teardowns, and under one lock every unrelated request for
    /// a resident member — the serve surface's default member above all — waited
    /// for it (recorded against [S-324] as a deferred finding).
    ///
    /// A deadlock or a regression here shows up as the `recv_timeout` below
    /// failing, not as a hung suite.
    ///
    /// [S-324]: ../../../docs/planning/journal.md#s-324-workspace-wide-read-connection-budget-with-lru-member-engine-eviction
    #[test]
    fn a_hit_does_not_wait_for_another_members_admission() {
        use std::sync::{Condvar, OnceLock};

        /// Whether the blocking start has been entered, and whether it may
        /// return. Process-global because `MemberEngine::start` is an associated
        /// function with nowhere to hang per-registry state; only this test uses
        /// it.
        #[derive(Default)]
        struct Gate {
            entered: bool,
            released: bool,
        }
        static GATE: OnceLock<(Mutex<Gate>, Condvar)> = OnceLock::new();
        fn gate() -> &'static (Mutex<Gate>, Condvar) {
            GATE.get_or_init(|| (Mutex::new(Gate::default()), Condvar::new()))
        }

        /// Starts instantly for every member but `slow`, whose start parks until
        /// the test releases it — standing in for the cold start and teardown a
        /// real admission performs.
        struct GatedEngine;
        impl MemberEngine for GatedEngine {
            type Watcher = ();

            fn start(
                root: &Path,
                _read_connections: usize,
                _worker_pool: SharedWorkerPool,
            ) -> Result<Arc<Self>> {
                if root.ends_with("slow") {
                    let (lock, cvar) = gate();
                    let mut state = lock.lock().unwrap();
                    state.entered = true;
                    cvar.notify_all();
                    while !state.released {
                        state = cvar.wait(state).unwrap();
                    }
                }
                Ok(Arc::new(GatedEngine))
            }

            fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
                Ok(())
            }
        }

        // The gate is process-global (an associated fn has nowhere to hang
        // per-registry state), so reset it rather than inherit whatever a
        // previous user left. Without this a second test using `GatedEngine` —
        // or any harness that re-runs this one in-process — would get an
        // already-released gate and a vacuous pass.
        {
            let (lock, _) = gate();
            *lock.lock().unwrap() = Gate::default();
        }

        // The gate is process-global (an associated fn has nowhere to hang
        // per-registry state), so reset it rather than inherit whatever a
        // previous user left. Without this a second test using `GatedEngine` —
        // or any harness re-running this one in-process — would find an
        // already-released gate and pass vacuously.
        {
            let (lock, _) = gate();
            *lock.lock().unwrap() = Gate::default();
        }

        let registry = EngineRegistry::<GatedEngine>::with_budget(
            fed(&["fast", "slow"]),
            RegistryMode::Lazy,
            roomy_budget(),
        );
        // `fast` is resident before anything blocks, so the touch below is a hit.
        registry.engine_for("fast").expect("fast member starts");

        let (hit_tx, hit_rx) = std::sync::mpsc::channel::<()>();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                registry.engine_for("slow").expect("slow member starts");
            });

            // Wait until the slow admission is genuinely inside `start`.
            let (lock, cvar) = gate();
            let mut state = lock.lock().unwrap();
            while !state.entered {
                state = cvar.wait(state).unwrap();
            }
            drop(state);

            let hit_registry = &registry;
            scope.spawn(move || {
                hit_registry
                    .engine_for("fast")
                    .expect("fast member is resident");
                hit_tx.send(()).expect("report the hit completed");
            });
            let served = hit_rx.recv_timeout(std::time::Duration::from_secs(10));

            // Release the blocked admission before asserting, so a failure ends
            // the test instead of wedging the scope's join.
            let (lock, cvar) = gate();
            lock.lock().unwrap().released = true;
            cvar.notify_all();

            served.expect(
                "a hit on a resident member blocked behind another member's \
                 admission; the fan-out serialises unrelated requests again",
            );
        });
    }
}
