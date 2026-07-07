//! The single-writer actor ([ADR-02], [NFR-RA-10], [NFR-RA-07]).
//!
//! One dedicated OS thread owns the sole read-write [`SqliteGraphStore`] and is
//! the *only* code path that mutates `logos.db`. Every mutation — full index,
//! incremental sync, reconcile, annotation commit — is a job on a **bounded**
//! channel, executed strictly one at a time in submission order. Because there
//! is exactly one writer, write batches can never interleave and SQLite never
//! returns `SQLITE_BUSY` to a second would-be writer ([ADR-02]).
//!
//! Each job runs inside [`SqliteGraphStore::write_batch`], so it is one
//! transaction per batch with wholesale rollback on any error ([NFR-RA-07]).
//! A job that *panics* is caught at the loop boundary so the actor keeps serving
//! subsequent jobs — a write-job panic must not poison the writer (the
//! execution-runtime "Failure Modes" contract).
//!
//! [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
//! [NFR-RA-10]: ../../../docs/specs/requirements/NFR-RA-10.md
//! [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::thread::{self, JoinHandle, ThreadId};

use anyhow::{anyhow, Result};
use crossbeam_channel::{bounded, Sender};

use crate::graph_store::{BatchWriter, SqliteGraphStore};

/// A unit of work executed on the writer thread.
///
/// The job is monomorphic — it returns `()` — because each submission boxes its
/// own typed response channel inside the closure (see [`WriterActor::submit`]).
/// That keeps the queue a single concrete type while letting callers await an
/// arbitrary `Result<T>`.
type WriteTask = Box<dyn FnOnce(&mut SqliteGraphStore) + Send>;

/// Owns the writer thread and the sending half of its bounded job queue.
///
/// Dropping the actor closes the queue (the loop's `recv` then returns `Err`)
/// and joins the thread, so the sole RW connection is checkpointed and closed
/// cleanly on shutdown.
pub(crate) struct WriterActor {
    /// `Some` until [`Drop`] takes it to close the queue before joining.
    tx: Option<Sender<WriteTask>>,
    /// `Some` until [`Drop`] joins the writer thread.
    handle: Option<JoinHandle<()>>,
    /// The writer thread's id, used to reject a re-entrant `submit` from inside a
    /// running write job (which would otherwise deadlock — see [`submit`]).
    ///
    /// [`submit`]: WriterActor::submit
    thread_id: ThreadId,
}

impl WriterActor {
    /// Spawn the writer thread, handing it ownership of the RW `store`.
    ///
    /// `queue_capacity` bounds the in-flight write backlog: `submit` blocks once
    /// the queue is full (bounded-block backpressure for correctness-bearing
    /// index/explicit writes, [AQ-01]).
    ///
    /// [AQ-01]: ../../../docs/specs/architecture.md#14-open-questions
    pub(crate) fn spawn(mut store: SqliteGraphStore, queue_capacity: usize) -> Self {
        let (tx, rx) = bounded::<WriteTask>(queue_capacity);
        let handle = thread::Builder::new()
            .name("logos-writer".to_owned())
            .spawn(move || {
                // Serve jobs until every sender is dropped (Runtime shutdown).
                while let Ok(task) = rx.recv() {
                    // A panicking job unwinds only this closure: the in-flight
                    // transaction is rolled back by `Transaction`'s `Drop`, the
                    // job's response channel is dropped (so the caller observes
                    // a disconnect and surfaces an error), and the writer thread
                    // survives to serve the next job.
                    //
                    // `AssertUnwindSafe` is required because `&mut SqliteGraphStore`
                    // wraps a `rusqlite::Connection`, whose inner `RefCell` is
                    // `!RefUnwindSafe`. It is *sound* here because rusqlite borrows
                    // that `RefCell` only for the duration of individual API calls
                    // and never holds a `RefMut` across the user job: when the
                    // unwind reaches this boundary every internal borrow has been
                    // released and the connection is back in autocommit, fully
                    // reusable for the next job.
                    let _ = catch_unwind(AssertUnwindSafe(|| task(&mut store)));
                }
            })
            .expect("spawning the logos-writer thread");
        let thread_id = handle.thread().id();
        Self {
            tx: Some(tx),
            handle: Some(handle),
            thread_id,
        }
    }

    /// Submit a write batch and block until the writer has run it.
    ///
    /// `job` is handed a [`BatchWriter`] scoped to a fresh transaction; returning
    /// `Ok` commits the batch, returning `Err` (or panicking) rolls it back
    /// wholesale ([NFR-RA-07]). This is the submit-and-await bridge async surfaces
    /// use to reach the sync core ([ADR-03]): the call is a plain blocking call,
    /// safe to invoke from any thread (including a `tokio` `spawn_blocking`).
    ///
    /// # Errors
    /// Returns the job's own error after rollback, or a runtime error if the
    /// writer thread is gone, panicked while running the job, or `submit` was
    /// called re-entrantly from inside a write job (see below).
    pub(crate) fn submit<T, F>(&self, job: F) -> Result<T>
    where
        F: FnOnce(&BatchWriter<'_>) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        // Re-entrancy guard: a write job that captured a handle to this runtime
        // and called `submit` again would enqueue work the writer can never pick
        // up (it is busy running the current job) and then block forever awaiting
        // its response — a self-deadlock. Detect it and fail fast instead. Normal
        // callers are never the writer thread, so this never fires for them.
        if thread::current().id() == self.thread_id {
            return Err(anyhow!(
                "submit_write called re-entrantly from within a running write job — \
                 this would deadlock the single writer; restructure to one batch per submit"
            ));
        }

        // A rendezvous-ish 1-slot channel carries exactly one response back.
        let (resp_tx, resp_rx) = bounded::<Result<T>>(1);
        let task: WriteTask = Box::new(move |store: &mut SqliteGraphStore| {
            // One transaction per batch + atomic rollback on Err (NFR-RA-07).
            let result = store.write_batch(job);
            // If the caller stopped waiting, drop the result silently.
            let _ = resp_tx.send(result);
        });

        self.tx
            .as_ref()
            .ok_or_else(|| anyhow!("writer actor has shut down"))?
            .send(task)
            .map_err(|_| anyhow!("writer actor is no longer accepting jobs"))?;

        // Block until the writer runs our batch and reports back. A disconnect
        // means the job panicked (its `resp_tx` was dropped mid-run) — the actor
        // itself stays alive, but this batch did not complete.
        resp_rx.recv().map_err(|_| {
            anyhow!(
                "write batch did not complete — the writer dropped it (likely a panic in the job)"
            )
        })?
    }
}

impl Drop for WriterActor {
    fn drop(&mut self) {
        // Close the queue first: dropping the sole sender makes the loop's
        // `recv` return `Err`, so the thread breaks out and the RW connection is
        // dropped (WAL checkpointed) on its own stack.
        self.tx.take();
        if let Some(handle) = self.handle.take() {
            // A writer-thread panic is already contained by `catch_unwind` in
            // the loop, so a clean join is expected; ignore a poisoned join
            // rather than panicking inside `drop`.
            let _ = handle.join();
        }
    }
}
