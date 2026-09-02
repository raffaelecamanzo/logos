//! [`live_worker_threads`] counts real worker threads, and a dropped pool leaves
//! none behind (S-325, [NFR-PE-11], [ADR-63]).
//!
//! # Why this file holds exactly one test
//! The gauge is a **process-global** counter and `cargo` runs a binary's
//! `#[test]`s on parallel threads of one process, so a sibling test building or
//! dropping a pool moves it under this one. As a `logos-core` unit test this was
//! observably flaky — a concurrent `federation::registry` test's pool was live
//! when the baseline was sampled, so the target was never reachable — and it
//! could equally have passed on a sibling's threads rather than its own. One
//! test in its own binary makes the count unambiguous, which is the whole point
//! of an instrument. `workspace_connection_budget.rs` and
//! `workspace_shared_worker_pool.rs` are single-test binaries for the same
//! reason.
//!
//! [NFR-PE-11]: ../../docs/specs/requirements/NFR-PE-11.md
//! [ADR-63]: ../../docs/specs/architecture/decisions/ADR-63.md

use std::time::{Duration, Instant};

use logos_core::{live_worker_threads, SharedWorkerPool};

/// Poll `condition` until it holds or a generous deadline passes. `rayon` starts
/// and terminates workers asynchronously, so the gauge is eventually — not
/// immediately — consistent with the pools that exist.
fn wait_until(condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    condition()
}

/// The gauge rises by exactly a pool's worth of workers when the pool is built
/// and returns to **zero** when the last handle is dropped.
///
/// The teardown assertion is `== 0`, not "fewer than before": a pool that leaked
/// two of its three workers would satisfy the weaker form, and "no orphaned
/// threads" is the claim [ADR-63] actually makes.
#[test]
fn the_gauge_rises_with_a_pool_and_returns_to_zero_when_it_is_dropped() {
    const THREADS: usize = 3;

    assert_eq!(
        live_worker_threads(),
        0,
        "this binary must start with no Logos worker threads, or the counts below \
         are not this test's"
    );

    let pool = SharedWorkerPool::with_threads(THREADS).expect("pool builds");
    assert!(
        wait_until(|| live_worker_threads() == THREADS),
        "the gauge reads {} for a {THREADS}-worker pool",
        live_worker_threads()
    );
    assert_eq!(pool.threads(), THREADS);

    // A second pool adds its own workers — the gauge is a process total, not a
    // per-pool readout, which is what makes it able to catch `members × cores`.
    let second = SharedWorkerPool::with_threads(THREADS).expect("pool builds");
    assert!(
        wait_until(|| live_worker_threads() == 2 * THREADS),
        "two {THREADS}-worker pools read {} live workers",
        live_worker_threads()
    );

    drop(second);
    assert!(
        wait_until(|| live_worker_threads() == THREADS),
        "dropping one of two pools left {} live workers",
        live_worker_threads()
    );

    drop(pool);
    // `rayon` signals termination rather than joining, so the workers exit
    // shortly after the last handle goes — polled, not sampled.
    assert!(
        wait_until(|| live_worker_threads() == 0),
        "{} worker(s) outlived every pool that owned them",
        live_worker_threads()
    );
}
