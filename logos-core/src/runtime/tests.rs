//! Unit tests for the execution runtime ([S-008], [ADR-02], [ADR-03]).
//!
//! These drive the [`Runtime`] through its public submit API against a real
//! on-disk WAL database (a `tempfile` dir), exercising the four properties the
//! story's acceptance criteria call out: serialized writes, atomic rollback,
//! reads never blocked by an in-flight write, and a panic-tolerant writer.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use tempfile::TempDir;

use super::{Runtime, RuntimeConfig};
use crate::graph_store::{BatchWriter, NewNode};
use crate::model::{LogosSymbol, NodeKind};

/// Open a runtime over a fresh on-disk database in a temp dir.
///
/// Returns the `TempDir` too so the caller keeps it alive for the test's
/// duration (dropping it deletes the database file out from under the pool).
fn runtime() -> (Runtime, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let runtime = Runtime::open(dir.path().join("logos.db")).expect("runtime opens");
    (runtime, dir)
}

/// Insert one `function` node named `name` inside the batch (a self-contained
/// write unit used to populate the store from a write job).
fn insert_function(w: &BatchWriter<'_>, symbol: &str, name: &str) -> Result<()> {
    let sym = LogosSymbol::parse(symbol)?;
    let symbol_id = w.upsert_symbol(&sym)?;
    w.insert_node(&NewNode::plain(symbol_id, NodeKind::Function, name))?;
    Ok(())
}

/// How many nodes match `name` via the read pool.
fn count_by_name(runtime: &Runtime, name: &str) -> usize {
    runtime
        .submit_read(|store| Ok(store.search(name, None, 100)?.len()))
        .expect("read succeeds")
}

#[test]
fn write_then_read_roundtrips_through_the_pools() {
    let (runtime, _dir) = runtime();

    runtime
        .submit_write(|w| insert_function(w, "local roundtrip", "roundtrip_target"))
        .expect("write batch commits");

    // A committed write is visible to the read-only pool (WAL: a fresh read
    // transaction sees the latest committed state).
    assert_eq!(count_by_name(&runtime, "roundtrip_target"), 1);
}

#[test]
fn submit_write_returns_the_jobs_value() {
    let (runtime, _dir) = runtime();
    let answer = runtime
        .submit_write(|w| {
            insert_function(w, "local valued", "valued")?;
            Ok(42_u32)
        })
        .expect("write batch commits");
    assert_eq!(answer, 42);
}

#[test]
fn live_writer_connection_reports_the_bulk_load_pragmas() {
    // FR-DB-02 / CR-057: inspecting the *live* single-writer connection (the RW
    // connection owned by the writer actor thread) shows the bulk-load pragmas
    // set. We read them through a no-op write batch, the only seam onto that
    // connection.
    let (runtime, _dir) = runtime();

    let (cache_size, mmap_size, temp_store) = runtime
        .submit_write(|w| {
            Ok((
                w.pragma_i64("cache_size")?,
                w.pragma_i64("mmap_size")?,
                w.pragma_i64("temp_store")?,
            ))
        })
        .expect("pragma read batch commits");

    // -65536 KiB (64 MiB), 256 MiB, MEMORY (2) — the constants set in
    // `configure_connection`. Asserted by value here (the graph_store unit test
    // owns the exact-constant equality); this proves they reach the live actor.
    assert_eq!(cache_size, -65_536, "live writer cache_size (FR-DB-02)");
    assert_eq!(mmap_size, 268_435_456, "live writer mmap_size (FR-DB-02)");
    assert_eq!(temp_store, 2, "live writer temp_store = MEMORY (FR-DB-02)");
}

#[test]
fn failed_write_batch_rolls_back_atomically() {
    let (runtime, _dir) = runtime();

    // The job inserts a node, then fails: the whole batch must roll back so the
    // node never lands (NFR-RA-07).
    let result: Result<()> = runtime.submit_write(|w| {
        insert_function(w, "local doomed", "doomed_node")?;
        Err(anyhow!("deliberate failure after a partial write"))
    });
    assert!(result.is_err(), "the failing batch must surface its error");
    assert_eq!(
        count_by_name(&runtime, "doomed_node"),
        0,
        "a rolled-back batch leaves no partial state"
    );

    // The runtime stays consistent and usable: a subsequent write commits.
    runtime
        .submit_write(|w| insert_function(w, "local survivor", "survivor_node"))
        .expect("the writer keeps serving after a rolled-back batch");
    assert_eq!(count_by_name(&runtime, "survivor_node"), 1);
}

#[test]
fn reentrant_write_from_a_job_is_rejected_not_deadlocked() {
    let (runtime, _dir) = runtime();
    let runtime = Arc::new(runtime);

    // A job that captures the runtime and submits another write runs on the
    // writer thread; the inner submit must fail fast rather than deadlock the
    // single writer waiting on itself.
    let inner = Arc::clone(&runtime);
    let outer: Result<()> = runtime.submit_write(move |_w| {
        let reentrant: Result<()> =
            inner.submit_write(|w| insert_function(w, "local reentrant", "reentrant_node"));
        assert!(
            reentrant.is_err(),
            "a re-entrant submit_write must be rejected, not block forever"
        );
        Ok(())
    });
    assert!(outer.is_ok(), "the outer batch still commits");
    // The rejected re-entrant insert never landed.
    assert_eq!(count_by_name(&runtime, "reentrant_node"), 0);
}

#[test]
fn writer_survives_a_panicking_job() {
    let (runtime, _dir) = runtime();

    // A job that panics must not poison the writer (execution-runtime Failure
    // Modes). The submit call surfaces an error rather than unwinding the caller.
    let panicked: Result<()> = runtime.submit_write(|_w| panic!("boom inside a write job"));
    assert!(
        panicked.is_err(),
        "a panicking job is reported as an error to the caller"
    );

    // The actor is still alive: a normal write+read still works.
    runtime
        .submit_write(|w| insert_function(w, "local afterpanic", "after_panic"))
        .expect("the writer survived the panic and keeps serving");
    assert_eq!(count_by_name(&runtime, "after_panic"), 1);
}

#[test]
fn reads_are_not_blocked_by_an_in_flight_write() {
    let (runtime, _dir) = runtime();
    let runtime = Arc::new(runtime);

    // Channels to choreograph: the write job announces it has started (and is
    // therefore mid-transaction), then parks until released.
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();

    std::thread::scope(|scope| {
        let writer_rt = Arc::clone(&runtime);
        let write_handle = scope.spawn(move || {
            writer_rt
                .submit_write(move |w| {
                    insert_function(w, "local blocker", "blocker")?;
                    started_tx.send(()).expect("announce write started");
                    // Hold the transaction open until the test releases us.
                    release_rx.recv().expect("await release");
                    Ok(())
                })
                .expect("blocking write commits once released");
        });

        // Wait until the writer is provably mid-transaction.
        started_rx.recv().expect("writer started");

        // A read must complete *now*, without waiting for the write to finish —
        // this is the WAL "reads never blocked by the writer" guarantee
        // (NFR-PE-01). If reads were serialized behind writes, this would hang
        // until the release below and the test would time out.
        let count = count_by_name(&runtime, "anything");
        assert_eq!(count, 0, "the read ran against the snapshot and returned");

        // Now let the write finish and join.
        release_tx.send(()).expect("release the writer");
        write_handle.join().expect("writer thread joins cleanly");
    });

    // After release+commit the blocker is visible.
    assert_eq!(count_by_name(&runtime, "blocker"), 1);
}

#[test]
fn concurrent_writes_are_serialized_no_interleaving() {
    // Small bespoke pool so the test is deterministic regardless of host cores.
    let dir = TempDir::new().expect("temp dir");
    let runtime = Runtime::open_with_config(
        dir.path().join("logos.db"),
        RuntimeConfig {
            reader_pool_size: 4,
            worker_threads: 4,
            write_queue_capacity: 64,
        },
    )
    .expect("runtime opens");
    let runtime = Arc::new(runtime);

    const WRITERS: usize = 8;
    const PER_WRITER: usize = 25;

    // `in_writer` proves serialization directly: if two jobs ever ran at once,
    // one would observe the flag already set. `peak`/`completed` are sanity
    // counters.
    let in_writer = Arc::new(AtomicBool::new(false));
    let interleavings = Arc::new(AtomicUsize::new(0));

    std::thread::scope(|scope| {
        for t in 0..WRITERS {
            let rt = Arc::clone(&runtime);
            let in_writer = Arc::clone(&in_writer);
            let interleavings = Arc::clone(&interleavings);
            scope.spawn(move || {
                for i in 0..PER_WRITER {
                    let in_writer = Arc::clone(&in_writer);
                    let interleavings = Arc::clone(&interleavings);
                    rt.submit_write(move |w| {
                        // Entry: the flag must currently be false (no other job
                        // is executing). swap returns the previous value.
                        if in_writer.swap(true, Ordering::SeqCst) {
                            interleavings.fetch_add(1, Ordering::SeqCst);
                        }
                        let symbol = format!("local w{t}_{i}");
                        let name = format!("n_{t}_{i}");
                        insert_function(w, &symbol, &name)?;
                        in_writer.store(false, Ordering::SeqCst);
                        Ok(())
                    })
                    .expect("each write commits");
                }
            });
        }
    });

    assert_eq!(
        interleavings.load(Ordering::SeqCst),
        0,
        "the single writer must never run two batches concurrently"
    );

    // Every one of the WRITERS * PER_WRITER batches committed exactly once.
    let total = runtime
        .submit_read(|store| Ok(store.search("n_", None, 10_000)?.len()))
        .expect("read total");
    assert_eq!(
        total,
        WRITERS * PER_WRITER,
        "every batch committed exactly once"
    );
}

#[test]
fn many_reads_run_concurrently_up_to_the_pool_size() {
    const POOL: usize = 4;
    let dir = TempDir::new().expect("temp dir");
    let runtime = Runtime::open_with_config(
        dir.path().join("logos.db"),
        RuntimeConfig {
            reader_pool_size: POOL,
            worker_threads: 2,
            write_queue_capacity: 8,
        },
    )
    .expect("runtime opens");
    let runtime = Arc::new(runtime);
    assert_eq!(runtime.reader_pool_size(), POOL);

    // POOL readers must be able to be in-flight simultaneously: each read parks
    // on a barrier that only trips once all POOL of them have checked out a
    // connection. If the pool served fewer than POOL at once this would deadlock
    // (caught by the test's overall timeout), proving genuine read concurrency.
    let barrier = Arc::new(std::sync::Barrier::new(POOL));
    std::thread::scope(|scope| {
        for _ in 0..POOL {
            let rt = Arc::clone(&runtime);
            let barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                rt.submit_read(|store| {
                    barrier.wait();
                    // Touch the connection so the read is real work.
                    let _ = store.search("noop", None, 1)?;
                    Ok(())
                })
                .expect("concurrent read succeeds");
            });
        }
    });
}

#[test]
fn cold_start_is_within_the_pe05_budget() {
    use std::time::Instant;

    let dir = TempDir::new().expect("temp dir");
    let db = dir.path().join("logos.db");

    let start = Instant::now();
    let runtime = Runtime::open(&db).expect("runtime opens");
    let elapsed = start.elapsed();

    // Keep the runtime live so the open cost includes spawning the writer thread
    // and opening every reader connection, not a partially-initialized shell.
    assert_eq!(runtime.db_path(), db.as_path());

    // NFR-PE-05: cold start ≤ 200 ms. The runtime open is the store/pool half of
    // that budget (registry build is the other half, measured at the Engine
    // level). Assert generously within budget; print the real number so a
    // regression is visible in test output even before it breaches.
    assert!(
        elapsed < Duration::from_millis(200),
        "runtime cold start took {elapsed:?}, exceeding the NFR-PE-05 ≤200ms budget"
    );
}

#[test]
fn reader_pool_size_zero_is_rejected() {
    let dir = TempDir::new().expect("temp dir");
    let err = Runtime::open_with_config(
        dir.path().join("logos.db"),
        RuntimeConfig {
            reader_pool_size: 0,
            worker_threads: 1,
            write_queue_capacity: 1,
        },
    );
    assert!(err.is_err(), "a zero-size reader pool must be rejected");
}

#[test]
fn worker_pool_runs_parallel_jobs() {
    let (runtime, _dir) = runtime();
    // The shared rayon pool (AQ-04) executes data-parallel work for the core.
    let sum: u64 = runtime.worker_pool().install(|| {
        use rayon::prelude::*;
        (1..=1000_u64).into_par_iter().sum()
    });
    assert_eq!(sum, 500_500);
}
