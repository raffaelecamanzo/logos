//! Bounded background index **warm** for a workspace ([FR-WS-14], [BR-44]).
//!
//! `logos init --workspace` ([FR-WS-02]) must return without blocking on
//! indexing, and the warm must survive the parent's exit — so it cannot be an
//! in-process thread pool. It used to be one detached `logos index` child *per
//! approved member*, which is unbounded by construction: 84 approved members
//! meant 84 concurrent rayon-parallel indexers, breaching [NFR-PE-06]'s 1 GB
//! RSS cap ~84× and defeating [NFR-PE-08]'s no-contention premise.
//!
//! This module owns the two pieces of that correction that are business logic,
//! not surface concerns:
//!
//! - [`effective_concurrency`] — the **effective-bound resolution** seam. K
//!   defaults to `max(1, cores / 4)` capped at [`CONCURRENCY_CAP`]; the
//!   `configured` argument is the per-workspace override channel. Today no
//!   caller has one to pass (they pass `None`); the `[workspace.warm]
//!   concurrency` manifest key of [FR-WS-01] lands behind this signature
//!   without the queue below changing at all.
//! - [`warm_queue`] — the bounded queue itself: at most K member indexes in
//!   flight, the next starting as each finishes, whatever N is.
//!
//! # Why the queue takes its worker as a parameter
//! Spawning and awaiting an `index` child process is surface work (the `cli`
//! crate owns `current_exe()` re-invocation), and launching real concurrent
//! indexers to test a *bound* would oversubscribe the machine running the
//! suite ([NFR-PE-08]). Taking `index_member` as a closure keeps the scheduling
//! decisions — the property under test — assertable against a **stubbed**
//! spawn, and keeps this module free of `std::process`.
//!
//! # Properties the queue guarantees
//! - **Hard ceiling.** Exactly `min(K, N)` worker threads exist and each runs
//!   `index_member` synchronously, so in-flight indexes never exceed K for any
//!   N ([BR-44]).
//! - **Per-member isolation.** A member whose worker returns `Err` is recorded
//!   degraded and that worker takes the next queued member; the queue neither
//!   stalls nor aborts.
//! - **Queue-bounded lifetime.** Workers exit the instant the cursor is
//!   exhausted and [`warm_queue`] returns when the last one joins, so a
//!   supervisor built on it exits on drain and never idle-waits.
//! - **No store lock.** Nothing here opens a store, a runtime, or a
//!   connection — the caller's `index_member` owns a child process that holds
//!   its own lock for its own lifetime. When [`warm_queue`] returns, no
//!   `.logos` lock is held on its behalf.
//!
//! Correctness never depends on any of this: a member the queue never reached
//! — because the supervisor was killed, or never spawned at all — indexes
//! correctly on first query through the lazy `ensure_indexed` fallback
//! ([FR-IX-07]). A deferred warm is the designed fallback, not a failure
//! ([BR-44]).
//!
//! [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
//! [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
//! [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
//! [FR-IX-07]: ../../../docs/specs/requirements/FR-IX-07.md
//! [NFR-PE-06]: ../../../docs/specs/requirements/NFR-PE-06.md
//! [NFR-PE-08]: ../../../docs/specs/requirements/NFR-PE-08.md
//! [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::Serialize;

/// Hard cap on the core-derived default warm concurrency ([FR-WS-14]).
///
/// A 4-core laptop and a 64-core builder must both behave sanely, so the
/// core-derived value is capped rather than scaling with the host: warming is
/// a *background* courtesy whose whole point is to stay out of the way.
///
/// [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
pub const CONCURRENCY_CAP: usize = 4;

/// The core-derived default bound: `max(1, cores / 4)`, capped at
/// [`CONCURRENCY_CAP`] ([FR-WS-14]).
///
/// `cores / 4` (not `cores`) because each member index is itself
/// rayon-parallel across the extraction pool ([NFR-PE-08]) — K concurrent
/// members already means K auto-sized pools competing for the same cores.
///
/// [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
/// [NFR-PE-08]: ../../../docs/specs/requirements/NFR-PE-08.md
#[must_use]
pub fn default_concurrency() -> usize {
    let cores = std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get);
    (cores / 4).clamp(1, CONCURRENCY_CAP)
}

/// Resolve the effective warm bound K ([FR-WS-14], [BR-44]).
///
/// `configured` is the per-workspace override seam: `None` takes
/// [`default_concurrency`], `Some(k)` takes `k` (floored at 1 — a zero bound
/// would stall the queue forever, so it can never be honoured literally).
/// Every caller passes `None` today; [FR-WS-01]'s `[workspace.warm]
/// concurrency` key becomes the `Some` source without the supervisor or
/// [`warm_queue`] changing. Range validation of a manifest value belongs at
/// parse time with an actionable message, not silently here.
///
/// Whatever this returns is a **hard** ceiling: no member count, `--yes`, or
/// manifest value may put more indexes in flight than the resolved K
/// ([BR-44]).
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
/// [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
/// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[must_use]
pub fn effective_concurrency(configured: Option<usize>) -> usize {
    configured.map_or_else(default_concurrency, |k| k.max(1))
}

/// One member's outcome in a [`WarmSummary`].
///
/// `degraded` carries the reason a member's index or spawn failed, mirroring
/// the `Degraded { reason }` posture enablement already reports per member
/// ([NFR-CC-04]) — a member that simply was never reached is *deferred*, not
/// degraded, and never appears here at all ([BR-44]).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberWarm {
    /// The member repository root the queue warmed.
    pub root: String,
    /// `None` on success; the failure reason when the index or spawn failed.
    pub degraded: Option<String>,
}

/// What one bounded warm pass did ([FR-WS-14]).
///
/// Ordered by queue position, not completion order, so the readout is
/// deterministic regardless of how the workers interleaved.
///
/// [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WarmSummary {
    /// The resolved bound K the pass honoured.
    pub concurrency: usize,
    /// Every queued member, in queue order.
    pub members: Vec<MemberWarm>,
}

impl WarmSummary {
    /// Members whose index or spawn failed.
    #[must_use]
    pub fn degraded_count(&self) -> usize {
        self.members.iter().filter(|m| m.degraded.is_some()).count()
    }

    /// Members indexed successfully.
    #[must_use]
    pub fn warmed_count(&self) -> usize {
        self.members.len() - self.degraded_count()
    }
}

/// Run `index_member` over `members` at a bounded concurrency of at most
/// `concurrency` ([FR-WS-14], [BR-44]).
///
/// Starts `min(concurrency, members.len())` workers, each pulling the next
/// queued member as it finishes the previous one, and returns once the last
/// member is done. A worker whose `index_member` returns `Err` records the
/// reason and continues — one bad member never stalls or aborts the queue.
///
/// `index_member` is called once per member, from a worker thread, and must
/// block until that member's index has finished (the whole bound rests on
/// that: an early return would let the worker take another member while the
/// previous index was still running). Report a failure as `Err(reason)`
/// rather than panicking: a panicking worker forfeits its slot (the rest of
/// the queue still drains through the remaining workers) and the panic
/// resurfaces from the scope, so the whole summary is lost rather than one
/// member's outcome. Nothing is swallowed either way.
#[must_use]
pub fn warm_queue<F>(members: &[PathBuf], concurrency: usize, index_member: F) -> WarmSummary
where
    F: Fn(&Path) -> Result<(), String> + Sync,
{
    let bound = concurrency.max(1);
    if members.is_empty() {
        return WarmSummary {
            concurrency: bound,
            members: Vec::new(),
        };
    }

    let cursor = AtomicUsize::new(0);
    let done: Mutex<Vec<(usize, MemberWarm)>> = Mutex::new(Vec::with_capacity(members.len()));

    // `min(bound, N)`: never start a worker there is no member for. The bound
    // is what `bound` says; the thread count is only ever ≤ it.
    std::thread::scope(|scope| {
        for _ in 0..bound.min(members.len()) {
            scope.spawn(|| {
                while let Some(i) = take_next(&cursor, members.len()) {
                    let root = &members[i];
                    let outcome = MemberWarm {
                        root: root.display().to_string(),
                        degraded: index_member(root).err(),
                    };
                    // A poisoned collector is still a usable Vec, and the
                    // module's contract is per-member degradation: one
                    // member's panic must not lose every other member's
                    // outcome (the `registry::lock_residents` convention).
                    done.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push((i, outcome));
                }
            });
        }
    });

    let mut collected = done
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    collected.sort_unstable_by_key(|(i, _)| *i);

    WarmSummary {
        concurrency: bound,
        members: collected.into_iter().map(|(_, m)| m).collect(),
    }
}

/// Claim the next queue position, or `None` once the queue is drained.
///
/// `Relaxed` suffices: the only cross-thread invariant is that each index is
/// handed out exactly once, which `fetch_add`'s atomicity gives on its own —
/// no other memory is ordered against it (each worker touches only its own
/// member and the separately-locked collector).
fn take_next(cursor: &AtomicUsize, len: usize) -> Option<usize> {
    let i = cursor.fetch_add(1, Ordering::Relaxed);
    (i < len).then_some(i)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::AtomicBool;
    use std::sync::Barrier;
    use std::time::Duration;

    fn roots(n: usize) -> Vec<PathBuf> {
        (0..n).map(|i| PathBuf::from(format!("m{i}"))).collect()
    }

    /// A stubbed `index_member` that records the maximum number of concurrent
    /// calls it ever saw — the whole bound assertion, with no real indexer
    /// launched ([NFR-PE-08]).
    #[derive(Default)]
    struct SpawnStub {
        in_flight: AtomicUsize,
        peak: AtomicUsize,
        calls: AtomicUsize,
    }

    impl SpawnStub {
        fn enter(&self) {
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            self.calls.fetch_add(1, Ordering::SeqCst);
        }

        fn leave(&self) {
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
        }

        /// Hold the slot long enough that a broken bound would overlap.
        fn work(&self) {
            self.enter();
            std::thread::sleep(Duration::from_millis(5));
            self.leave();
        }
    }

    // ── effective_concurrency: the bound-resolution seam (FR-WS-14) ────────

    #[test]
    fn default_concurrency_is_at_least_one_and_never_above_the_cap() {
        let k = default_concurrency();
        assert!(
            (1..=CONCURRENCY_CAP).contains(&k),
            "max(1, cores/4) capped at {CONCURRENCY_CAP}, got {k}"
        );
    }

    #[test]
    fn no_override_resolves_to_the_core_derived_default() {
        assert_eq!(effective_concurrency(None), default_concurrency());
    }

    #[test]
    fn an_override_is_honoured_verbatim() {
        assert_eq!(effective_concurrency(Some(2)), 2);
        // Above the core-derived cap: the cap bounds the DEFAULT, and a
        // deliberate per-workspace override is the documented escape hatch —
        // range validation belongs at manifest parse time, not silently here.
        assert_eq!(effective_concurrency(Some(9)), 9);
    }

    #[test]
    fn a_zero_override_floors_at_one_rather_than_stalling_the_queue() {
        assert_eq!(
            effective_concurrency(Some(0)),
            1,
            "a zero bound could never drain a queue"
        );
    }

    // ── warm_queue: the hard ceiling, against a stubbed spawn (BR-44) ──────

    #[test]
    fn in_flight_never_exceeds_the_bound_for_n_far_above_k() {
        let members = roots(40);
        let stub = SpawnStub::default();

        let summary = warm_queue(&members, 3, |_| {
            stub.work();
            Ok(())
        });

        assert_eq!(summary.concurrency, 3);
        assert_eq!(stub.calls.load(Ordering::SeqCst), 40, "every member is warmed");
        assert_eq!(
            summary.members.len(),
            40,
            "every member is reported, in queue order"
        );
        assert!(
            stub.peak.load(Ordering::SeqCst) <= 3,
            "the bound is a hard ceiling regardless of N: peak {} > 3",
            stub.peak.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn a_bound_of_one_serialises_the_queue_completely() {
        let members = roots(6);
        let stub = SpawnStub::default();

        let _ = warm_queue(&members, 1, |_| {
            stub.work();
            Ok(())
        });

        assert_eq!(stub.peak.load(Ordering::SeqCst), 1, "K = 1 means no overlap");
    }

    /// The bound is a *ceiling*, not a target: it must not force N ≥ K workers
    /// into existence when there are fewer members than slots.
    #[test]
    fn fewer_members_than_the_bound_warms_each_member_exactly_once() {
        let members = roots(2);
        let stub = SpawnStub::default();

        let summary = warm_queue(&members, 4, |_| {
            stub.work();
            Ok(())
        });

        assert_eq!(stub.calls.load(Ordering::SeqCst), 2);
        assert_eq!(summary.concurrency, 4, "the resolved bound is reported as-is");
        assert_eq!(summary.warmed_count(), 2);
    }

    /// Not just "≤ K" — the queue must actually *use* its slots, or a bound
    /// implemented as head-of-line blocking would pass every ceiling test
    /// while warming serially.
    #[test]
    fn the_queue_actually_runs_k_members_concurrently() {
        let members = roots(8);
        let barrier = Barrier::new(3);
        let tripped = AtomicBool::new(false);

        let _ = warm_queue(&members, 3, |_| {
            // The first three workers can only get past this if all three are
            // genuinely in flight at once; later members find it already
            // tripped and pass straight through.
            if !tripped.load(Ordering::SeqCst) {
                barrier.wait();
                tripped.store(true, Ordering::SeqCst);
            }
            Ok(())
        });
        // Reaching here at all proves the barrier was satisfied — a serial
        // implementation would deadlock rather than fail an assertion.
        assert!(tripped.load(Ordering::SeqCst));
    }

    // ── warm_queue: per-member failure isolation (FR-WS-14) ────────────────

    #[test]
    fn a_failing_member_is_recorded_degraded_and_the_queue_advances() {
        let members = roots(5);
        let stub = SpawnStub::default();

        let summary = warm_queue(&members, 2, |root| {
            stub.enter();
            let failing = root.ends_with("m2");
            stub.leave();
            if failing {
                Err("store is corrupt".to_string())
            } else {
                Ok(())
            }
        });

        assert_eq!(
            stub.calls.load(Ordering::SeqCst),
            5,
            "the queue neither stalls nor aborts on a failing member"
        );
        assert_eq!(summary.degraded_count(), 1);
        assert_eq!(summary.warmed_count(), 4);
        assert_eq!(
            summary.members[2].degraded.as_deref(),
            Some("store is corrupt"),
            "the reason is recorded against the member that failed"
        );
        assert!(summary.members[3].degraded.is_none());
    }

    #[test]
    fn every_member_failing_still_drains_the_queue() {
        let members = roots(4);
        let summary = warm_queue(&members, 2, |_| Err("spawn failed".to_string()));

        assert_eq!(summary.members.len(), 4);
        assert_eq!(summary.degraded_count(), 4);
        assert_eq!(summary.warmed_count(), 0);
    }

    // ── warm_queue: shape and edges ────────────────────────────────────────

    #[test]
    fn outcomes_are_reported_in_queue_order_not_completion_order() {
        let members = roots(12);
        // Reverse-staggered sleeps: later members finish first, so a
        // completion-ordered collector would scramble the readout.
        let summary = warm_queue(&members, 4, |root| {
            let name = root.display().to_string();
            let i: u64 = name.trim_start_matches('m').parse().unwrap();
            std::thread::sleep(Duration::from_millis(12 - i));
            Err(name)
        });

        let reasons: Vec<&str> = summary
            .members
            .iter()
            .map(|m| m.degraded.as_deref().unwrap())
            .collect();
        assert_eq!(reasons, (0..12).map(|i| format!("m{i}")).collect::<Vec<_>>());
    }

    #[test]
    fn an_empty_queue_is_a_noop_that_spawns_nothing() {
        let stub = SpawnStub::default();
        let summary = warm_queue(&[], 4, |_| {
            stub.work();
            Ok(())
        });

        assert_eq!(stub.calls.load(Ordering::SeqCst), 0);
        assert!(summary.members.is_empty());
        assert_eq!(summary.degraded_count(), 0);
    }

    /// `warm_queue` returning is the supervisor's exit signal, so it must not
    /// linger once the last member is done — no idle-wait, no lingering
    /// worker.
    #[test]
    fn warm_queue_returns_as_soon_as_the_last_member_finishes() {
        let members = roots(4);
        let started = std::time::Instant::now();
        let _ = warm_queue(&members, 4, |_| {
            std::thread::sleep(Duration::from_millis(10));
            Ok(())
        });
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "returns on drain, never idle-waits: {:?}",
            started.elapsed()
        );
    }
}
