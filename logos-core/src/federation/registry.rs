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
//! all N. A [`RegistryMode::Serve`] registry is **eager**: it warms every member
//! and spawns one filesystem watcher per member, then leans on
//! [`evict_to_capacity`](EngineRegistry::evict_to_capacity) to bound the
//! steady-state resident set. A per-member start (or watch) failure **degrades**
//! — it is logged and skipped — rather than aborting the whole workspace.
//!
//! # Single-root invariant
//! The registry is never on the single-root path. [`Backing::resolve`] returns
//! [`Backing::Single`] — the one [`Engine`](crate::Engine) used exactly as today
//! — when discovery finds no workspace, and only allocates an [`EngineRegistry`]
//! when a workspace is present ([ADR-52]). Single-root behaviour is byte-for-byte
//! unchanged.
//!
//! [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
//! [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

use super::{Federation, Member};
use crate::Engine;

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
    type Watcher: Send;

    /// Build a long-lived engine rooted at a member's working-tree root — the
    /// read/write-capable flavour ([`Engine::start`](crate::Engine::start)).
    ///
    /// # Errors
    /// Propagates a store-open / migrate / runtime failure so the registry can
    /// report the member as degraded.
    fn start(root: &Path) -> Result<Arc<Self>>;

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

    fn start(root: &Path) -> Result<Arc<Self>> {
        Ok(Arc::new(Engine::start(root)?))
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
    /// `serve`: every member is warmed eagerly with one watcher each; idle
    /// engines are then evictable to bound the steady-state resident set.
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
    engine: Arc<E>,
    /// Held for the engine's residency; dropping it stops the watcher. `None`
    /// under [`RegistryMode::Lazy`] or when the watcher failed to spawn.
    _watcher: Option<E::Watcher>,
    /// Value of the registry's [`tick`](EngineRegistry::tick) at last touch.
    last_touch: u64,
}

/// The `root → Engine` registry over a workspace's members ([FR-WS-03],
/// [NFR-PE-10], [ADR-52]).
///
/// Shareable behind an [`Arc`] across request tasks (the interior state is a
/// [`Mutex`]), so the web surface and concurrent fan-out see one registry. Each
/// member engine submits to **its own** runtime pools, so touching one member
/// never advances another's state.
///
/// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
pub struct EngineRegistry<E: MemberEngine = Engine> {
    federation: Federation,
    mode: RegistryMode,
    residents: Mutex<HashMap<String, Resident<E>>>,
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
    pub fn new(federation: Federation, mode: RegistryMode) -> Self {
        let registry = Self::build(federation, mode);
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
    /// A default member that fails to warm **degrades** (logged, not fatal): the
    /// registry is still returned so the healthy members answer their
    /// `/api/v1/workspace/*` queries, mirroring [`warm_all`](Self::warm_all)'s
    /// per-member degrade contract ([ADR-53]).
    ///
    /// [FR-WS-06]: ../../../docs/specs/requirements/FR-WS-06.md
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    pub fn new_serve_default(federation: Federation) -> Self {
        let registry = Self::build(federation, RegistryMode::Serve);
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
    fn build(federation: Federation, mode: RegistryMode) -> Self {
        Self {
            federation,
            mode,
            residents: Mutex::new(HashMap::new()),
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

        // Hold the lock across the build so two concurrent touches of the same
        // member cannot each construct an engine (the second would waste a cold
        // start and orphan a watcher). Fan-out is member-sequential and serve
        // warm is one-shot, so this does not serialise steady-state reads —
        // those run on each engine's own pools after the Arc is cloned out.
        let mut residents = self.lock_residents();
        let tick = self.tick.fetch_add(1, Ordering::Relaxed);

        if let Some(resident) = residents.get_mut(member) {
            resident.last_touch = tick;
            return Ok(Arc::clone(&resident.engine));
        }

        let engine = E::start(&target.root)
            .with_context(|| format!("starting the engine for workspace member {member:?}"))?;
        let watcher = self.spawn_watcher(member, &engine);
        residents.insert(
            member.to_string(),
            Resident {
                engine: Arc::clone(&engine),
                _watcher: watcher,
                last_touch: tick,
            },
        );
        Ok(engine)
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
    /// `f` runs eagerly, once per member, in discovery order; because each
    /// member's engine is independent (its own pools and store), the per-member
    /// calls never interfere.
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
        let residents = self.lock_residents();
        let mut names: Vec<String> = residents.keys().cloned().collect();
        names.sort();
        names
    }

    /// The number of resident member engines.
    pub fn resident_count(&self) -> usize {
        self.lock_residents().len()
    }

    /// Evict least-recently-touched member engines until at most `cap` remain,
    /// bounding steady-state resource cost ([NFR-PE-10]).
    ///
    /// Dropping a resident drops its engine `Arc` and (under serve) stops its
    /// watcher. An evicted member is rebuilt — and re-watched, under serve — on
    /// its next touch. Returns the evicted member names, least-recently-touched
    /// first.
    ///
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    pub fn evict_to_capacity(&self, cap: usize) -> Vec<String> {
        let mut residents = self.lock_residents();
        if residents.len() <= cap {
            return Vec::new();
        }
        let mut by_recency: Vec<(String, u64)> = residents
            .iter()
            .map(|(name, resident)| (name.clone(), resident.last_touch))
            .collect();
        by_recency.sort_by_key(|(_, touch)| *touch); // least-recently-touched first
        let evict_count = residents.len() - cap;
        by_recency
            .into_iter()
            .take(evict_count)
            .map(|(name, _)| {
                residents.remove(&name); // drops the engine Arc + stops the watcher
                name
            })
            .collect()
    }

    /// Lock the resident map, **recovering** a poisoned lock rather than
    /// propagating the poison.
    ///
    /// The map is a plain engine cache; a poisoned view is still usable. The
    /// registry is shared behind an [`Arc`] across serve request tasks, and the
    /// module's contract is per-member degradation — so a single member's panic
    /// (e.g. inside a build held under this lock) must not brick every
    /// subsequent `engine_for` / `fan_out` for the healthy members. Recovering
    /// the guard keeps that all-or-nothing failure from happening ([ADR-53]).
    ///
    /// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
    fn lock_residents(&self) -> std::sync::MutexGuard<'_, HashMap<String, Resident<E>>> {
        self.residents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Eagerly build (and, under serve, watch) every member. Degraded members
    /// are logged and skipped.
    fn warm_all(&self) {
        for member in &self.federation.members {
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
    Federated(EngineRegistry<E>),
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
            Some(federation) => Backing::Federated(EngineRegistry::new(federation, mode)),
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
    pub fn as_federated(&self) -> Option<&EngineRegistry<E>> {
        match self {
            Backing::Federated(registry) => Some(registry),
            Backing::Single(_) => None,
        }
    }

    /// Whether this backing federates a workspace.
    pub fn is_federated(&self) -> bool {
        matches!(self, Backing::Federated(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::Cell;
    use std::path::PathBuf;

    // Per-test-thread construction spies. Each `#[test]` runs on its own thread,
    // so thread-local counters are isolated per test — no cross-test bleed — and
    // let us assert exactly how many engines/watchers a registry built.
    thread_local! {
        static STARTS: Cell<usize> = const { Cell::new(0) };
        static WATCHES: Cell<usize> = const { Cell::new(0) };
    }

    fn reset_spies() {
        STARTS.with(|c| c.set(0));
        WATCHES.with(|c| c.set(0));
    }
    fn starts() -> usize {
        STARTS.with(Cell::get)
    }
    fn watches() -> usize {
        WATCHES.with(Cell::get)
    }

    /// A fake member engine that records its root and counts constructions,
    /// standing in for a real [`Engine`] so lazy/eager/eviction policy is
    /// testable without any on-disk store.
    #[derive(Debug)]
    struct SpyEngine {
        root: PathBuf,
    }
    struct SpyWatcher;

    impl MemberEngine for SpyEngine {
        type Watcher = SpyWatcher;

        fn start(root: &Path) -> Result<Arc<Self>> {
            STARTS.with(|c| c.set(c.get() + 1));
            Ok(Arc::new(SpyEngine {
                root: root.to_path_buf(),
            }))
        }

        fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
            WATCHES.with(|c| c.set(c.get() + 1));
            Ok(SpyWatcher)
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
        }
    }

    fn lazy(names: &[&str]) -> EngineRegistry<SpyEngine> {
        EngineRegistry::new(fed(names), RegistryMode::Lazy)
    }
    fn serve(names: &[&str]) -> EngineRegistry<SpyEngine> {
        EngineRegistry::new(fed(names), RegistryMode::Serve)
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
            fn start(_root: &Path) -> Result<Arc<Self>> {
                anyhow::bail!("store is corrupt")
            }
            fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
                Ok(())
            }
        }
        let registry = EngineRegistry::<FailingEngine>::new(fed(&["a", "b"]), RegistryMode::Lazy);
        let results = registry.fan_out(|_, _| ());
        assert_eq!(results.len(), 2, "every member is still reported");
        assert!(
            results.iter().all(|s| s.value.is_err()),
            "a failing member surfaces as Err, not a panic or a dropped member"
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
            fn start(root: &Path) -> Result<Arc<Self>> {
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
        let registry = EngineRegistry::<PickyEngine>::new(fed(&["a", "b", "c"]), RegistryMode::Serve);
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
            fn start(_root: &Path) -> Result<Arc<Self>> {
                Ok(Arc::new(NoWatchEngine))
            }
            fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
                anyhow::bail!("OS watcher could not attach")
            }
        }

        let registry = EngineRegistry::<NoWatchEngine>::new(fed(&["a", "b"]), RegistryMode::Serve);
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
        let registry = EngineRegistry::<SpyEngine>::new(federation, RegistryMode::Lazy);
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
        let registry = EngineRegistry::<SpyEngine>::new_serve_default(fed(&["a", "b", "c"]));
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
        let registry = EngineRegistry::<SpyEngine>::new_serve_default(federation);
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
            fn start(_root: &Path) -> Result<Arc<Self>> {
                anyhow::bail!("store is corrupt")
            }
            fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
                Ok(())
            }
        }
        // Must not panic even though the default cannot start.
        let registry = EngineRegistry::<FailingEngine>::new_serve_default(fed(&["a", "b"]));
        assert_eq!(registry.resident_count(), 0, "the failed default left no resident");
        // The registry is still usable — a fan-out reports the members as degraded
        // rather than the whole workspace aborting.
        assert_eq!(registry.fan_out(|_, _| ()).len(), 2);
    }

    /// `default_engine` on a member-less workspace errors rather than panicking.
    #[test]
    fn default_engine_errors_on_an_empty_workspace() {
        let registry = EngineRegistry::<SpyEngine>::new(fed(&[]), RegistryMode::Lazy);
        assert!(
            registry.default_engine().is_err(),
            "a workspace with no members has no engine to answer"
        );
    }

    // ── the single-root invariant via Backing (FR-WS-03 / ADR-52) ──────────

    /// With no workspace, `Backing::resolve` yields `Single` — one engine, no
    /// registry, no fan-out watchers: the single-root path bypasses the registry
    /// entirely.
    #[test]
    fn backing_single_bypasses_the_registry() {
        reset_spies();
        let backing = Backing::<SpyEngine>::resolve(None, RegistryMode::Serve, || {
            SpyEngine::start(Path::new("/solo")).unwrap()
        });

        assert!(!backing.is_federated());
        assert!(backing.as_federated().is_none(), "no registry is allocated");
        assert!(backing.as_single().is_some(), "the single-root engine is used");
        assert_eq!(starts(), 1, "exactly the one single-root engine is built");
        assert_eq!(watches(), 0, "the single-root path spawns no registry watcher");
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
}
