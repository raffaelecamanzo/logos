//! The core execution runtime — Logos's entire in-process concurrency layer
//! ([execution-runtime], [ADR-02], [ADR-03]).
//!
//! [`Runtime`] owns three cooperating pieces and nothing else does concurrency:
//!
//! 1. A **single-writer actor** ([`writer`]) — one thread owns the sole RW
//!    connection; every mutation is one transaction per batch with atomic
//!    rollback ([ADR-02], [NFR-RA-07], [NFR-RA-10]).
//! 2. A **read-only WAL pool** ([`reader`]) — `N` snapshot connections that are
//!    never blocked by the writer ([NFR-PE-01]).
//! 3. A **`rayon` worker pool** — one pool for both grammar extraction and core
//!    CPU jobs (the [AQ-04] resolution); the dedicated core-pool is deferred
//!    until profiling shows extraction starving navigation latency (see the
//!    [execution-runtime] Notes). The runtime builds its own by default and
//!    accepts an **injected** [`SharedWorkerPool`] instead, so the resident
//!    member engines of a workspace cost the host's core count in threads rather
//!    than `members × cores` ([NFR-PE-11], [ADR-63]).
//!
//! # The async→sync bridge ([ADR-03])
//!
//! [`submit_read`](Runtime::submit_read) and [`submit_write`](Runtime::submit_write)
//! are ordinary **blocking** calls: they submit work and await the result on a
//! channel. The core stays fully synchronous and `rayon`-friendly; `tokio` lives
//! only at the MCP edge and reaches the core by calling these from a
//! `spawn_blocking` context. The core owns its own concurrency policy rather than
//! borrowing `tokio`'s blocking pool ([NFR-MA-02]).
//!
//! # Lifecycle ([ADR-04])
//!
//! A [`Runtime`] is expensive to build (open + migrate the writer store, open
//! `N` reader connections, spawn the worker pool) and is meant to be **held for
//! the process lifetime** by a long-lived [`Engine`](crate::Engine): cold-start
//! pays this once ([NFR-PE-05]) and every later call reuses the live pools.
//! Dropping the `Runtime` tears everything down cleanly — the writer thread is
//! joined and all connections are closed.
//!
//! [execution-runtime]: ../../../docs/specs/architecture/components/execution-runtime.md
//! [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
//! [ADR-03]: ../../../docs/specs/architecture/decisions/ADR-03.md
//! [ADR-04]: ../../../docs/specs/architecture/decisions/ADR-04.md
//! [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
//! [NFR-RA-10]: ../../../docs/specs/requirements/NFR-RA-10.md
//! [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
//! [NFR-PE-05]: ../../../docs/specs/requirements/NFR-PE-05.md
//! [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
//! [NFR-MA-02]: ../../../docs/specs/requirements/NFR-MA-02.md
//! [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md
//! [AQ-04]: ../../../docs/specs/architecture.md#14-open-questions

mod reader;
mod writer;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};

use anyhow::{Context, Result};

use crate::graph_store::{BatchWriter, GraphStore, SqliteGraphStore};

use reader::ReaderPool;
use writer::WriterActor;

#[cfg(test)]
mod tests;

/// Live `rayon` worker threads across **every** pool this process has built
/// through [`Runtime`] — the quantity [NFR-PE-11] bounds ([ADR-63]).
///
/// Maintained by the pool builder's start/exit handlers, so it counts threads
/// that actually exist rather than threads that were asked for. A dropped pool
/// terminates its workers asynchronously (`rayon` signals rather than joins), so
/// a reader watching a teardown should poll this to zero rather than sample it
/// once.
static LIVE_WORKER_THREADS: AtomicUsize = AtomicUsize::new(0);

/// How many `rayon` worker threads Logos is running in this process right now.
///
/// The instrument behind [NFR-PE-11]'s "total worker threads track the host core
/// count, not `members × cores`": a workspace fan-out can be measured against the
/// host rather than against the member count, and a torn-down pool can be shown
/// to have left nothing behind. Counts only pools built here — the process-global
/// `rayon` pool, which Logos never uses, is invisible to it.
///
/// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
pub fn live_worker_threads() -> usize {
    LIVE_WORKER_THREADS.load(Ordering::Relaxed)
}

/// A `rayon` worker pool **several runtimes share** ([NFR-PE-11], [ADR-63]).
///
/// [ADR-02] sizes a runtime's pool to the host core count, which is right under
/// its premise of one [`Engine`](crate::Engine) per process. A workspace holds
/// one engine per resident member ([FR-WS-03]), so that default costs
/// `members × cores` threads — ~864 on the measured 12-core / 72-member
/// workspace. Handing every resident member *one* pool makes the thread cost a
/// function of the host instead.
///
/// The handle is an [`Arc`], so the pool lives exactly as long as the runtimes
/// holding it: the last resident engine to go tears it down, and nothing owns
/// worker threads on behalf of a workspace that has none resident. The registry
/// that hands the pool out therefore keeps only a [`Weak`] reference (see
/// [`downgrade`](Self::downgrade)) — enough to re-share a live pool with the next
/// member admitted, never enough to keep one alive.
///
/// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
/// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
/// [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
/// [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md
#[derive(Clone, Debug)]
pub struct SharedWorkerPool(Arc<rayon::ThreadPool>);

impl SharedWorkerPool {
    /// Build a shareable pool of `threads` workers.
    ///
    /// # Errors
    /// Returns an error if `rayon` cannot build the pool.
    pub fn with_threads(threads: usize) -> Result<Self> {
        Self::named(threads, "logos-workspace")
    }

    /// Build a pool of `threads` workers whose threads are named `<prefix>-<i>`.
    ///
    /// The prefix says who the pool belongs to when a debugger or a profiler
    /// lists the process's threads: `logos-core` for the private pool a runtime
    /// builds for itself, `logos-workspace` for the one a workspace's resident
    /// members share.
    fn named(threads: usize, prefix: &'static str) -> Result<Self> {
        Ok(Self(Arc::new(build_worker_pool(threads, prefix)?)))
    }

    /// Worker threads in this pool.
    pub fn threads(&self) -> usize {
        self.0.current_num_threads()
    }

    /// The pool itself, to `install` jobs on.
    pub fn pool(&self) -> &rayon::ThreadPool {
        &self.0
    }

    /// Whether `a` and `b` are the **same** pool.
    ///
    /// Reference identity, not equality of size: "these two engines share one
    /// pool" and "these two engines happen to have equally-sized pools" are
    /// exactly the distinction [NFR-PE-11] turns on, and only the first bounds
    /// the thread count.
    ///
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }

    /// A non-owning handle to this pool — what a registry keeps between
    /// admissions so re-sharing never outlives residency.
    pub fn downgrade(&self) -> WeakWorkerPool {
        WeakWorkerPool(Arc::downgrade(&self.0))
    }
}

/// A non-owning [`SharedWorkerPool`] handle: it re-shares a pool that is still
/// alive and yields nothing once the last resident engine has dropped it.
#[derive(Clone, Debug, Default)]
pub struct WeakWorkerPool(Weak<rayon::ThreadPool>);

impl WeakWorkerPool {
    /// The shared pool, if any runtime still holds it.
    pub fn upgrade(&self) -> Option<SharedWorkerPool> {
        self.0.upgrade().map(SharedWorkerPool)
    }
}

/// Build one `rayon` pool of `threads` workers, named `<prefix>-<i>` and
/// accounted for in [`live_worker_threads`].
///
/// The single construction site for every pool Logos owns — private and shared
/// alike — so the two cannot drift apart in sizing policy or in instrumentation.
fn build_worker_pool(threads: usize, prefix: &'static str) -> Result<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(move |i| format!("{prefix}-{i}"))
        .start_handler(|_| {
            LIVE_WORKER_THREADS.fetch_add(1, Ordering::Relaxed);
        })
        .exit_handler(|_| {
            LIVE_WORKER_THREADS.fetch_sub(1, Ordering::Relaxed);
        })
        .build()
        .with_context(|| format!("building the {prefix} worker pool ({threads} threads)"))
}

/// Tunable shapes for the runtime's pools.
///
/// The defaults encode the locked architecture resolutions: reader pool sized to
/// the core count ([AQ-02]), one worker pool — private to this runtime, also
/// sized to the core count — serving both extraction and core jobs ([AQ-04]),
/// and a bounded write queue for bounded-block backpressure on
/// correctness-bearing writes ([AQ-01]). All are tunable so a future dogfood /
/// perf-hardening pass can right-size them without touching call sites, and
/// [`worker_pool`](Self::worker_pool) additionally lets a *caller* supply the
/// pool instead of having one built ([NFR-PE-11], [ADR-63]).
///
/// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
/// [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md
///
/// [AQ-01]: ../../../docs/specs/architecture.md#14-open-questions
/// [AQ-02]: ../../../docs/specs/architecture.md#14-open-questions
/// [AQ-04]: ../../../docs/specs/architecture.md#14-open-questions
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Number of read-only WAL connections — the maximum read concurrency.
    pub reader_pool_size: usize,
    /// Worker threads in the `rayon` pool this runtime builds for itself.
    ///
    /// Ignored when [`worker_pool`](Self::worker_pool) injects one: an injected
    /// pool was sized by whoever owns it, and re-deriving a size here would make
    /// the sharing a suggestion rather than a fact.
    pub worker_threads: usize,
    /// An **injected** `rayon` pool to run this runtime's CPU jobs on, shared
    /// with every other runtime given the same handle ([NFR-PE-11], [ADR-63]).
    ///
    /// `None` — the default, and the whole single-root path — builds a private
    /// pool of [`worker_threads`](Self::worker_threads) workers exactly as
    /// before. The pool is *injected, never discovered*: a runtime given no pool
    /// has no way to find one, so a workspace's sharing policy cannot leak into
    /// a plain repo ([FR-WS-03], [ADR-52]).
    ///
    /// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    /// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
    /// [ADR-63]: ../../../docs/specs/architecture/decisions/ADR-63.md
    pub worker_pool: Option<SharedWorkerPool>,
    /// In-flight write-job backlog before [`submit_write`](Runtime::submit_write)
    /// blocks (bounded-block backpressure).
    pub write_queue_capacity: usize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self {
            reader_pool_size: cores,
            worker_threads: cores,
            worker_pool: None,
            write_queue_capacity: 256,
        }
    }
}

/// The owner of all in-process concurrency (see the module docs).
///
/// `Runtime` is `Send + Sync`: the writer's RW connection lives *inside* the
/// writer thread (never in this struct), reader connections live inside an mpmc
/// channel, and `rayon::ThreadPool` is itself `Send + Sync` — so a long-lived
/// `Engine` holding a `Runtime` can be shared behind an `Arc` across the MCP
/// surface's blocking tasks.
pub struct Runtime {
    writer: WriterActor,
    readers: ReaderPool,
    /// The worker pool this runtime submits CPU jobs to. A [`SharedWorkerPool`]
    /// because it may be **shared** with the other resident member engines of a
    /// workspace ([NFR-PE-11], [ADR-63]); a private pool is simply one with a
    /// single owner, so the two cases differ in ownership and in nothing else.
    pool: SharedWorkerPool,
    db_path: PathBuf,
}

impl Runtime {
    /// Open the runtime over `db_path` with [`RuntimeConfig::default`].
    ///
    /// # Errors
    /// See [`open_with_config`](Self::open_with_config).
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_config(db_path, RuntimeConfig::default())
    }

    /// Open the runtime over `db_path` with an explicit `config`.
    ///
    /// Startup order matters: the **writer opens (and migrates) the store
    /// first**, so the read-only pool can attach to an existing, fully migrated
    /// database. Then the reader pool and the shared worker pool come up. By the
    /// time this returns the engine is ready to serve reads and writes.
    ///
    /// # Errors
    /// Returns an error if the writer store cannot be opened/migrated, a reader
    /// connection cannot be opened, or the worker pool cannot be built.
    pub fn open_with_config(db_path: impl AsRef<Path>, config: RuntimeConfig) -> Result<Self> {
        let db_path = db_path.as_ref().to_path_buf();

        // 1. Writer first — creates the file and runs migrations (RW connection).
        let store = SqliteGraphStore::open(&db_path)
            .with_context(|| format!("opening the writer store at {}", db_path.display()))?;
        let writer = WriterActor::spawn(store, config.write_queue_capacity);

        // 2. Read-only pool over the now-migrated database.
        let readers = ReaderPool::open(&db_path, config.reader_pool_size)?;

        // 3. The CPU pool for extraction + core jobs (AQ-04) — the injected one
        //    if this runtime was given a share of a workspace's ([NFR-PE-11],
        //    [ADR-63]), otherwise a private pool built exactly as before. Named
        //    threads aid debugging; a private pool is sized to cores and capped
        //    there to limit contention with tokio on a small-core baseline
        //    (AR-02).
        let pool = match config.worker_pool {
            Some(shared) => shared,
            None => SharedWorkerPool::named(config.worker_threads, "logos-core")?,
        };

        Ok(Self {
            writer,
            readers,
            pool,
            db_path,
        })
    }

    /// Submit a write batch to the single writer and block until it completes.
    ///
    /// All mutations funnel here, serialized through one thread, one transaction
    /// per batch with atomic rollback ([ADR-02], [NFR-RA-07]). `job` receives a
    /// [`BatchWriter`]: return `Ok` to commit, `Err` (or panic) to roll back.
    ///
    /// # Errors
    /// Returns the job's error after rollback, or a runtime error if the writer
    /// is gone / the job panicked.
    pub fn submit_write<T, F>(&self, job: F) -> Result<T>
    where
        F: FnOnce(&BatchWriter<'_>) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.writer.submit(job)
    }

    /// Run a read against a pooled read-only connection and return its result.
    ///
    /// Never blocked by an in-flight write ([NFR-PE-01]); blocks only if every
    /// reader is currently checked out. `job` runs on the calling thread, so
    /// concurrent callers read in true parallel up to the pool size.
    ///
    /// # Errors
    /// Returns the read closure's error, or a runtime error if the pool is gone.
    pub fn submit_read<T>(&self, job: impl FnOnce(&dyn GraphStore) -> Result<T>) -> Result<T> {
        self.readers.with_connection(job)
    }

    /// The `rayon` worker pool for data-parallel core CPU jobs ([AQ-04]) —
    /// private to this runtime, or shared with the workspace's other resident
    /// member engines when one was injected ([NFR-PE-11], [ADR-63]).
    ///
    /// The pipeline ([S-010]) installs extraction work here via
    /// `worker_pool().install(|| extract_files(...))` so all CPU parallelism
    /// shares one core-owned pool rather than spawning competing ones. Note: graph
    /// hydration ([S-009]) reads through [`submit_read`](Runtime::submit_read) and
    /// builds the petgraph view synchronously on the calling thread — it does not
    /// use this pool.
    ///
    /// [S-010]: ../../../docs/planning/journal.md#s-010-indexing-and-incremental-sync-pipeline
    pub fn worker_pool(&self) -> &rayon::ThreadPool {
        self.pool.pool()
    }

    /// Whether this runtime's worker pool is the **same** pool as `other`'s —
    /// the observable that says a workspace's engines share one pool rather than
    /// each owning an identically-sized one ([NFR-PE-11]).
    ///
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    pub fn shares_worker_pool_with(&self, other: &Runtime) -> bool {
        SharedWorkerPool::ptr_eq(&self.pool, &other.pool)
    }

    /// Maximum number of concurrent readers (the reader pool size).
    pub fn reader_pool_size(&self) -> usize {
        self.readers.size()
    }

    /// The on-disk path of the canonical store this runtime serves.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }
}
