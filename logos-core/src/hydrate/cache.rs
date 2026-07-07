//! The bounded hydration cache the long-lived Engine owns ([ADR-04], [ADR-05],
//! [NFR-PE-07]).
//!
//! Hydrating a petgraph view is O(nodes + edges); the cache amortises that across
//! repeated aggregate runs so two runs with no intervening change reuse the same
//! `Arc<GraphView>` ([NFR-PE-07]). It is keyed by `(scope, granularity,
//! last_sync_at)`:
//!
//! - **`scope` + `granularity`** select *which* view (the architecture's
//!   `(scope, …)` key bundles the view selector; TK01 hydrates only whole-project
//!   views, so [`Scope`](super::Scope) has a single variant today).
//! - **`last_sync_at`** ([`SyncStamp`](super::SyncStamp)) is the temporal
//!   component: when the graph changes, the stamp advances and every cached view
//!   is invalidated ([ADR-04], [ADR-05]).
//!
//! # Invalidation is crisp, not lazy
//!
//! The Engine has exactly one current `last_sync_at` at a time ([ADR-04]: one
//! process, one root). So when [`get_or_build`](HydrationCache::get_or_build) is
//! called with a stamp newer than the one the cache holds, it **drops every
//! entry at the old stamp** before serving the new request. The cache therefore
//! only ever holds views at the current stamp; a stale view is never returned and
//! never lingers consuming RSS.
//!
//! # Bounding is parameterized ([AQ-02], [AA-04])
//!
//! [`HydrationConfig`] bounds the cache by **entry count and/or byte budget**,
//! either of which may bind first; eviction is least-recently-used. Both bounds
//! are exposed so the actual RSS ceiling — deferred to [AQ-02]/[AA-04] and tuned
//! during dogfood profiling — is a config change, not a code change. A single
//! view larger than the byte budget is still served (the cache keeps at least the
//! entry just built) and degrades gracefully under memory pressure ([NFR-PE-09]).
//!
//! [ADR-04]: ../../../docs/specs/architecture/decisions/ADR-04.md
//! [ADR-05]: ../../../docs/specs/architecture/decisions/ADR-05.md
//! [NFR-PE-07]: ../../../docs/specs/requirements/NFR-PE-07.md
//! [NFR-PE-09]: ../../../docs/specs/requirements/NFR-PE-09.md
//! [AQ-02]: ../../../docs/specs/architecture.md#14-open-questions
//! [AA-04]: ../../../docs/specs/architecture.md#24-assumptions

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::runtime::Runtime;

use super::view::{build_view, GraphView};
use super::{Granularity, Scope, SyncStamp};

/// 256 MiB — the **provisional** default byte budget. The real ≤1 GB-RSS-aware
/// ceiling is [AQ-02]/[AA-04], resolved during dogfood profiling; this is a safe
/// placeholder that keeps the cache bounded until then.
///
/// [AQ-02]: ../../../docs/specs/architecture.md#14-open-questions
/// [AA-04]: ../../../docs/specs/architecture.md#24-assumptions
const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;

/// Provisional default entry-count bound — comfortably above the four
/// granularities so all coexist; tightened or relaxed once [AQ-02] resolves.
const DEFAULT_MAX_ENTRIES: usize = 16;

/// How the hydration cache is bounded ([AQ-02], [AA-04]).
///
/// Both bounds are optional and independent: `None` disables that dimension, and
/// when both are set whichever binds first triggers LRU eviction. The defaults
/// are deliberately provisional placeholders (see [`DEFAULT_MAX_BYTES`]) — the
/// point of this type is that resolving the deferred RSS-budget decision is a
/// config change, not a rewrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HydrationConfig {
    /// Maximum number of cached views before LRU eviction; `None` = unbounded by
    /// count.
    pub max_entries: Option<usize>,
    /// Maximum total estimated bytes across cached views before LRU eviction;
    /// `None` = unbounded by bytes.
    pub max_bytes: Option<usize>,
}

impl Default for HydrationConfig {
    fn default() -> Self {
        Self {
            max_entries: Some(DEFAULT_MAX_ENTRIES),
            max_bytes: Some(DEFAULT_MAX_BYTES),
        }
    }
}

/// A point-in-time snapshot of cache effectiveness, surfaced in `stats`
/// ([NFR-PE-07] hydration-cache-hit fitness function).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HydrationStats {
    /// Lifetime cache hits (a requested view was already resident).
    pub hits: u64,
    /// Lifetime cache misses (a view had to be hydrated from SQLite).
    pub misses: u64,
    /// Views currently resident.
    pub entries: usize,
    /// Estimated bytes currently held across resident views.
    pub estimated_bytes: usize,
}

/// The cache key: the architecture's `(scope, last_sync_at)` extended with the
/// `granularity` selector so the four views never collide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CacheKey {
    scope: Scope,
    granularity: Granularity,
    stamp: SyncStamp,
}

/// One resident view plus its bookkeeping.
struct Entry {
    view: Arc<GraphView>,
    bytes: usize,
    /// Logical access time for LRU eviction (higher = more recent).
    last_access: u64,
}

struct Inner {
    entries: HashMap<CacheKey, Entry>,
    config: HydrationConfig,
    /// The stamp the resident entries belong to; `None` until the first build.
    current_stamp: Option<SyncStamp>,
    /// Monotonic LRU clock.
    tick: u64,
    total_bytes: usize,
    hits: u64,
    misses: u64,
}

/// The Engine-owned, bounded, `(scope, last_sync_at)`-keyed hydration cache.
///
/// `Send + Sync` via an internal `Mutex`, so a long-lived `Engine` behind an
/// `Arc` can hydrate from many blocking tasks. The expensive O(graph) build runs
/// **outside** the lock — the mutex is held only for the cheap map insert/evict —
/// so hydration never serializes unrelated reads.
pub struct HydrationCache {
    inner: Mutex<Inner>,
}

impl HydrationCache {
    /// A cache bounded by `config`.
    pub fn new(config: HydrationConfig) -> Self {
        Self {
            inner: Mutex::new(Inner {
                entries: HashMap::new(),
                config,
                current_stamp: None,
                tick: 0,
                total_bytes: 0,
                hits: 0,
                misses: 0,
            }),
        }
    }

    /// Return the cached view for `(scope, granularity, stamp)`, hydrating it from
    /// the runtime's read-only pool on a miss.
    ///
    /// If `stamp` is newer than the stamp the cache currently holds, every
    /// resident entry is invalidated first ([ADR-04]). The build runs against the
    /// RO pool ([ADR-02]) so it is never blocked by an in-flight write, and runs
    /// outside the cache lock.
    ///
    /// # Errors
    /// Propagates a read error from the RO pool (e.g. a corrupt discriminant
    /// surfaced by `all_nodes`/`all_edges`).
    pub fn get_or_build(
        &self,
        runtime: &Runtime,
        scope: Scope,
        granularity: Granularity,
        stamp: SyncStamp,
    ) -> Result<Arc<GraphView>> {
        let key = CacheKey {
            scope,
            granularity,
            stamp,
        };

        // Fast path: a hit at the current stamp.
        {
            let mut inner = self.inner.lock().expect("hydration cache mutex poisoned");
            inner.invalidate_if_advanced(stamp);
            if let Some(view) = inner.record_hit(&key) {
                return Ok(view);
            }
        }

        // Miss: fetch a snapshot from the RO pool and build the petgraph with the
        // cache lock released. Fetching holds one pooled connection only for the
        // read; the CPU-bound graph build runs unlocked.
        let (nodes, edges) =
            runtime.submit_read(|store| Ok((store.all_nodes()?, store.all_edges()?)))?;
        let view = Arc::new(build_view(granularity, &nodes, &edges));
        let bytes = view.estimated_bytes();

        // Re-lock to install. A concurrent caller may have built the same view
        // meanwhile; if so, reuse theirs so callers share one `Arc`.
        let mut inner = self.inner.lock().expect("hydration cache mutex poisoned");
        inner.invalidate_if_advanced(stamp);
        // If another caller advanced the cache to a newer generation while we were
        // building, our snapshot belongs to an older stamp: serve it to *this*
        // caller (it was a consistent read) but do not install it, so the cache is
        // never rolled back to a stale generation.
        if inner.current_stamp != Some(stamp) {
            inner.misses += 1;
            return Ok(view);
        }
        if let Some(view) = inner.record_hit(&key) {
            return Ok(view);
        }
        inner.insert(key, Arc::clone(&view), bytes);
        inner.misses += 1;
        inner.evict_to_bounds();
        Ok(view)
    }

    /// A snapshot of hit/miss counters and current residency.
    pub fn stats(&self) -> HydrationStats {
        let inner = self.inner.lock().expect("hydration cache mutex poisoned");
        HydrationStats {
            hits: inner.hits,
            misses: inner.misses,
            entries: inner.entries.len(),
            estimated_bytes: inner.total_bytes,
        }
    }
}

impl Default for HydrationCache {
    fn default() -> Self {
        Self::new(HydrationConfig::default())
    }
}

impl Inner {
    /// Drop all entries if `stamp` is newer than what the cache holds — the
    /// `last_sync_at`-advance invalidation ([ADR-04], [ADR-05]).
    ///
    /// Only a genuine **advance** clears the cache. A regressed stamp (which the
    /// single-owner Engine never produces, but a concurrent build that straddled
    /// an advance could present on re-lock) is ignored, so the cache is never
    /// rolled back to a stale generation by an out-of-order caller.
    fn invalidate_if_advanced(&mut self, stamp: SyncStamp) {
        match self.current_stamp {
            // Same generation — resident entries are valid.
            Some(current) if current == stamp => {}
            // Regressed stamp — leave the newer generation intact.
            Some(current) if current > stamp => {}
            // Genuine advance — drop every now-stale entry.
            Some(_) => {
                self.entries.clear();
                self.total_bytes = 0;
                self.current_stamp = Some(stamp);
            }
            // First build — nothing to invalidate.
            None => self.current_stamp = Some(stamp),
        }
    }

    /// Bump LRU recency and the hit counter for `key`, returning the view if
    /// resident.
    fn record_hit(&mut self, key: &CacheKey) -> Option<Arc<GraphView>> {
        self.tick += 1;
        let tick = self.tick;
        let entry = self.entries.get_mut(key)?;
        entry.last_access = tick;
        self.hits += 1;
        Some(Arc::clone(&entry.view))
    }

    /// Install a freshly built view as the most-recently-used entry.
    fn insert(&mut self, key: CacheKey, view: Arc<GraphView>, bytes: usize) {
        self.tick += 1;
        let tick = self.tick;
        self.total_bytes += bytes;
        self.entries.insert(
            key,
            Entry {
                view,
                bytes,
                last_access: tick,
            },
        );
    }

    /// Evict least-recently-used entries until both bounds are satisfied, always
    /// keeping at least one entry (so an over-budget single view is still served
    /// — graceful degradation, [NFR-PE-09]).
    fn evict_to_bounds(&mut self) {
        while self.over_bounds() && self.entries.len() > 1 {
            let Some(victim) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_access)
                .map(|(k, _)| *k)
            else {
                break;
            };
            if let Some(entry) = self.entries.remove(&victim) {
                self.total_bytes -= entry.bytes;
            }
        }
    }

    /// Whether either configured bound is currently exceeded.
    fn over_bounds(&self) -> bool {
        let over_entries = self
            .config
            .max_entries
            .is_some_and(|max| self.entries.len() > max);
        let over_bytes = self
            .config
            .max_bytes
            .is_some_and(|max| self.total_bytes > max);
        over_entries || over_bytes
    }
}
