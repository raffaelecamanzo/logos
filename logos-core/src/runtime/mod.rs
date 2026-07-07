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
//! 3. A **shared `rayon` worker pool** — one pool for both grammar extraction
//!    and core CPU jobs (the [AQ-04] resolution); the dedicated core-pool is
//!    deferred until profiling shows extraction starving navigation latency
//!    (see the [execution-runtime] Notes).
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
//! [NFR-MA-02]: ../../../docs/specs/requirements/NFR-MA-02.md
//! [AQ-04]: ../../../docs/specs/architecture.md#14-open-questions

mod reader;
mod writer;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::graph_store::{BatchWriter, GraphStore, SqliteGraphStore};

use reader::ReaderPool;
use writer::WriterActor;

#[cfg(test)]
mod tests;

/// Tunable shapes for the runtime's pools.
///
/// The defaults encode the locked architecture resolutions: reader pool sized to
/// the core count ([AQ-02]), one shared worker pool also sized to the core count
/// ([AQ-04]), and a bounded write queue for bounded-block backpressure on
/// correctness-bearing writes ([AQ-01]). All are tunable so a future dogfood /
/// perf-hardening pass can right-size them without touching call sites.
///
/// [AQ-01]: ../../../docs/specs/architecture.md#14-open-questions
/// [AQ-02]: ../../../docs/specs/architecture.md#14-open-questions
/// [AQ-04]: ../../../docs/specs/architecture.md#14-open-questions
#[derive(Debug, Clone, Copy)]
pub struct RuntimeConfig {
    /// Number of read-only WAL connections — the maximum read concurrency.
    pub reader_pool_size: usize,
    /// Worker threads in the shared `rayon` pool.
    pub worker_threads: usize,
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
    pool: rayon::ThreadPool,
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

        // 3. One shared CPU pool for extraction + core jobs (AQ-04). Named
        //    threads aid debugging; sized to cores and capped there to limit
        //    contention with tokio on a small-core baseline (AR-02).
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(config.worker_threads)
            .thread_name(|i| format!("logos-core-{i}"))
            .build()
            .context("building the shared core worker pool")?;

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

    /// The shared `rayon` worker pool for data-parallel core CPU jobs ([AQ-04]).
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
        &self.pool
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
