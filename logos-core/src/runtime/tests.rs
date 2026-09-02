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

use super::{live_worker_threads, Runtime, RuntimeConfig, SharedWorkerPool};
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
            worker_pool: None,
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
            worker_pool: None,
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
            worker_pool: None,
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

// ── the injectable worker pool (S-325, NFR-PE-11, ADR-63) ────────────────────

/// Open a runtime over a fresh database with an explicit worker-pool config.
fn runtime_with_pool(
    dir: &TempDir,
    name: &str,
    worker_threads: usize,
    worker_pool: Option<SharedWorkerPool>,
) -> Runtime {
    Runtime::open_with_config(
        dir.path().join(name),
        RuntimeConfig {
            reader_pool_size: 1,
            worker_threads,
            worker_pool,
            write_queue_capacity: 8,
        },
    )
    .expect("runtime opens")
}

/// The pool is **injected, not discovered**: a runtime given none builds its own,
/// exactly as before this story — so the single-root path cannot accidentally
/// join a workspace's pool ([FR-WS-03], [ADR-52]).
#[test]
fn a_runtime_given_no_pool_builds_its_own() {
    let dir = TempDir::new().expect("temp dir");
    let a = runtime_with_pool(&dir, "a.db", 3, None);
    let b = runtime_with_pool(&dir, "b.db", 3, None);

    assert!(
        !a.shares_worker_pool_with(&b),
        "two runtimes given no pool must each build a private one; sharing by          default would make the pool discovered rather than injected"
    );
    assert_eq!(
        a.worker_pool().current_num_threads(),
        3,
        "a private pool is sized by RuntimeConfig::worker_threads"
    );
}

/// Runtimes handed the same pool run on that one pool — the whole mechanism
/// behind [NFR-PE-11]'s "threads track the host, not `members × cores`".
#[test]
fn runtimes_given_one_pool_share_it() {
    let dir = TempDir::new().expect("temp dir");
    let shared = SharedWorkerPool::with_threads(2).expect("pool builds");

    let a = runtime_with_pool(&dir, "a.db", 3, Some(shared.clone()));
    let b = runtime_with_pool(&dir, "b.db", 3, Some(shared.clone()));

    assert!(
        a.shares_worker_pool_with(&b),
        "runtimes injected with the same pool must submit to the same pool"
    );
    assert_eq!(
        a.worker_pool().current_num_threads(),
        2,
        "an injected pool keeps ITS size; `worker_threads` (3 here) is the size          of the pool a runtime would have built for itself, and must not be          re-derived over an injected one"
    );
    // A control against the reverse mistake: `shares_worker_pool_with` must be
    // able to say "no", or the assertion above is vacuous.
    let private = runtime_with_pool(&dir, "c.db", 3, None);
    assert!(!a.shares_worker_pool_with(&private));
}

/// Job submission semantics are unchanged: the same job, run on a private pool
/// and on a shared one, returns the same result through the same
/// `worker_pool().install(…)` call every core call site uses.
#[test]
fn a_shared_pool_runs_the_same_jobs_with_the_same_results() {
    use rayon::prelude::*;

    let dir = TempDir::new().expect("temp dir");
    let shared = SharedWorkerPool::with_threads(2).expect("pool builds");
    let witness_pool = shared.clone();
    let private = runtime_with_pool(&dir, "private.db", 2, None);
    let injected = runtime_with_pool(&dir, "injected.db", 2, Some(shared));

    // Assert the PREMISE first. Without it the comparison below passes just as
    // happily when the injection seam is severed and `injected` quietly gets a
    // private pool — 500_500 is what any working rayon pool returns, so the
    // equality alone says nothing about sharing.
    let witness = runtime_with_pool(&dir, "witness.db", 2, Some(witness_pool));
    assert!(
        injected.shares_worker_pool_with(&witness),
        "the injected runtime is not on the shared pool, so comparing its results \
         would not be comparing a shared pool against a private one"
    );
    assert!(!private.shares_worker_pool_with(&injected));

    let job = |runtime: &Runtime| -> u64 {
        runtime
            .worker_pool()
            .install(|| (1..=1000_u64).into_par_iter().sum())
    };
    assert_eq!(job(&private), 500_500);
    assert_eq!(job(&injected), job(&private));
}

/// Two members' jobs share one pool without deadlocking: a long-running job
/// occupying **every** worker makes another member's submission *queue*, and
/// releasing it lets that submission complete.
///
/// The queue step is what makes this a test of *sharing*: if the two runtimes
/// were on separate pools the second job would run immediately, so the "still
/// pending while saturated" assertion fails exactly when the injection seam is
/// severed. Without it the test passes just as happily on two private pools,
/// where a long job on one cannot contend with the other at all.
#[test]
fn a_long_job_on_a_shared_pool_queues_another_submission_without_deadlocking_it() {
    const WORKERS: usize = 2;
    let dir = TempDir::new().expect("temp dir");
    let shared = SharedWorkerPool::with_threads(WORKERS).expect("pool builds");
    let long = runtime_with_pool(&dir, "long.db", WORKERS, Some(shared.clone()));
    let short = runtime_with_pool(&dir, "short.db", WORKERS, Some(shared));
    assert!(
        long.shares_worker_pool_with(&short),
        "the premise: both members must be on the SAME pool, or nothing below \
         is about sharing"
    );

    let latch = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let occupied = Arc::new(AtomicUsize::new(0));

    // `spawn`, not `install`: queue one blocking job per worker without blocking
    // this thread, so the pool is saturated rather than merely busy.
    for _ in 0..WORKERS {
        let latch = Arc::clone(&latch);
        let occupied = Arc::clone(&occupied);
        long.worker_pool().spawn(move || {
            occupied.fetch_add(1, Ordering::SeqCst);
            let (lock, cvar) = &*latch;
            let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
            while !*released {
                released = cvar.wait(released).unwrap_or_else(|e| e.into_inner());
            }
        });
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while occupied.load(Ordering::SeqCst) < WORKERS && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert_eq!(
        occupied.load(Ordering::SeqCst),
        WORKERS,
        "the pool was never saturated, so the queueing assertion below would \
         prove nothing"
    );

    let (tx, rx) = mpsc::channel::<u64>();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let value = short.worker_pool().install(|| 7_u64);
            let _ = tx.send(value);
        });

        // Saturated: the other member's job has nowhere to run yet. Deterministic
        // rather than timing-sensitive — every worker is parked on the latch, so
        // the only way this could complete is a pool with a spare worker, i.e. a
        // pool that is not the one the long jobs are on.
        assert!(
            rx.recv_timeout(Duration::from_millis(250)).is_err(),
            "a second member's job ran while every worker of the shared pool was \
             occupied — the two runtimes are not sharing a pool"
        );

        // Releasing drains the queue: the pool queued the work, it did not
        // deadlock on it.
        let (lock, cvar) = &*latch;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cvar.notify_all();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10))
                .expect("the queued job must run once the long jobs release"),
            7
        );
    });
}

/// The thread gauge counts real workers: it rises by a pool's size when the pool
/// is built and falls back once the last handle is dropped, so
/// "no orphaned threads" is a measurement rather than an argument.
///
/// Relative rather than absolute, because `cargo` runs this binary's tests on
/// parallel threads of one process and the gauge is process-wide. The absolute
/// ceiling is asserted in `tests/workspace_connection_budget.rs`, whose binary
/// holds exactly one test.
#[test]
fn the_thread_gauge_follows_a_pools_lifetime() {
    const THREADS: usize = 3;
    let before = live_worker_threads();
    let pool = SharedWorkerPool::with_threads(THREADS).expect("pool builds");
    // Workers start asynchronously, so wait for them rather than sampling once.
    assert!(
        wait_until(|| live_worker_threads() >= before + THREADS),
        "the gauge never rose by the {THREADS} workers the pool was built with          (before {before}, now {})",
        live_worker_threads()
    );
    assert_eq!(pool.threads(), THREADS);

    drop(pool);
    // `rayon` signals termination rather than joining, so the workers exit
    // shortly after the last handle goes.
    assert!(
        wait_until(|| live_worker_threads() < before + THREADS),
        "the pool's workers outlived the pool (still {} live, was {before}          before it was built)",
        live_worker_threads()
    );
}

/// Poll `condition` until it holds, or give up after a generous deadline.
///
/// Used only where the property is genuinely asynchronous (thread start and
/// exit); everywhere else the tests assert directly.
fn wait_until(condition: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    condition()
}
