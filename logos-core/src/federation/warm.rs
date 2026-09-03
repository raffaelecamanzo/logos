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
//!   `configured` argument is the per-workspace override channel, fed by the
//!   `[workspace.warm] concurrency` manifest key of [FR-WS-01] — registered and
//!   range-validated at parse time ([`MANIFEST_CONCURRENCY_MAX`]), so what
//!   arrives here is already known to be in range and is honoured verbatim.
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
//! Correctness never depends on any of this: a member the queue never
//! **reached** — because the supervisor was killed, or never spawned at all —
//! indexes correctly on first query through the lazy `ensure_indexed` fallback
//! ([FR-IX-07]). A deferred warm is the designed fallback, not a failure
//! ([BR-44]).
//!
//! One exception, stated precisely because the fallback is otherwise total: a
//! member killed **mid**-index is not covered. A full index persists in
//! bounded chunks (`PERSIST_CHUNK_FILES`, [FR-IX-08]), and `ensure_indexed`
//! no-ops whenever the store has *any* indexed file — so a member interrupted
//! after its first chunk keeps serving an incomplete graph until an aggregate
//! command (`sync`, `scan`, `check`) walks it. That hazard predates the bound
//! (the per-member fan-out had it too) and is not widened by K, but the
//! supervisor's longer lifetime widens the window, which is why the spawn asks
//! for its own process group.
//!
//! [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
//! [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
//! [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
//! [FR-IX-07]: ../../../docs/specs/requirements/FR-IX-07.md
//! [FR-IX-08]: ../../../docs/specs/requirements/FR-IX-08.md
//! [NFR-PE-06]: ../../../docs/specs/requirements/NFR-PE-06.md
//! [NFR-PE-08]: ../../../docs/specs/requirements/NFR-PE-08.md
//! [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Hard cap on the core-derived default warm concurrency ([FR-WS-14]).
///
/// A 4-core laptop and a 64-core builder must both behave sanely, so the
/// core-derived value is capped rather than scaling with the host: warming is
/// a *background* courtesy whose whole point is to stay out of the way.
///
/// [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
pub const CONCURRENCY_CAP: usize = 4;

/// The largest K a `[workspace.warm] concurrency` manifest key may declare
/// ([FR-WS-01], [BR-44]) — validated at parse time by
/// [`manifest::parse`](super::manifest::parse), never clamped here.
///
/// Distinct from [`CONCURRENCY_CAP`], and deliberately larger. `CONCURRENCY_CAP`
/// bounds the value this module *derives* for a host it knows nothing about, so
/// it is conservative; this bounds what an operator may *declare* for a
/// workspace they do know, so it must admit a genuine override on a large
/// builder — a manifest override capped at the default's own cap could only ever
/// lower K, which is not what FR-WS-01 promises.
///
/// It is a fixed constant rather than a host-core-derived one because a manifest
/// is a checked-in file shared across machines: a ceiling that moved with
/// `available_parallelism` would make the same `logos.workspace.toml` parse on
/// the builder and fail on the laptop.
///
/// The number is `4 ×` [`CONCURRENCY_CAP`]: high enough that no reasonable
/// deliberate override hits it, low enough to still reject the mistake this
/// whole bound exists to correct — `concurrency = 84`, one per member, which is
/// the unbounded fan-out under a new spelling. Each of the K is itself
/// rayon-parallel over the full core count ([NFR-PE-08]), so K buys `K × cores`
/// worker threads and up to `K ×` one member index's peak RSS ([NFR-PE-06]).
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
/// [NFR-PE-06]: ../../../docs/specs/requirements/NFR-PE-06.md
/// [NFR-PE-08]: ../../../docs/specs/requirements/NFR-PE-08.md
/// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
pub const MANIFEST_CONCURRENCY_MAX: usize = 4 * CONCURRENCY_CAP;

// The two ceilings must stay ordered, and the compiler is the right place to
// say so: the manifest maximum bounds what an operator may *declare*, the cap
// bounds what this module *derives*. Inverted by a future tuning, an override
// could only ever lower K — the key would silently stop being the escape hatch
// [FR-WS-01] promises while every parse test still passed.
const _: () = assert!(
    MANIFEST_CONCURRENCY_MAX > CONCURRENCY_CAP,
    "a manifest override capped at the core-derived cap could never raise K"
);

/// The core-derived default bound: `max(1, cores / 4)`, capped at
/// [`CONCURRENCY_CAP`] ([FR-WS-14]).
///
/// `cores / 4` (not `cores`) because each member index is itself
/// rayon-parallel: a child sizes both its extraction pool and its reader pool
/// to the *full* core count ([NFR-PE-08]), so K concurrent members means
/// `K × cores` worker threads, not `cores`. Dividing keeps that product within
/// a small multiple instead of `N ×` it.
///
/// Note what this therefore does and does not bound: K bounds the **process**
/// count, and with it the `1 + K` ceiling [FR-WS-14] promises. It does not by
/// itself bound per-child pool width or per-child RSS, so peak memory is up to
/// `K ×` one member index — [NFR-PE-06]'s 1 GB target is improved ~84× over
/// the old fan-out but not literally met at K = 4. Capping a child's pool width
/// is the separate lever that would close it, and is not this story's.
///
/// [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
/// [NFR-PE-06]: ../../../docs/specs/requirements/NFR-PE-06.md
/// [NFR-PE-08]: ../../../docs/specs/requirements/NFR-PE-08.md
#[must_use]
pub fn default_concurrency() -> usize {
    derive_concurrency(std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get))
}

/// The formula itself, over an explicit core count ([FR-WS-14]).
///
/// Split from [`default_concurrency`] so the *rule* — `max(1, cores / 4)`
/// capped at [`CONCURRENCY_CAP`] — is pinned by a table rather than by
/// whatever the test host happens to report. A range assertion over the live
/// core count cannot tell `cores / 4` from `cores / 2`, `cores / 8`, or a
/// constant, so it would leave the stated formula unverified.
///
/// [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
#[must_use]
fn derive_concurrency(cores: usize) -> usize {
    (cores / 4).clamp(1, CONCURRENCY_CAP)
}

/// Resolve the effective warm bound K ([FR-WS-14], [BR-44]).
///
/// `configured` is the per-workspace override seam: `None` takes
/// [`default_concurrency`], `Some(k)` takes `k` (floored at 1 — a zero bound
/// would stall the queue forever, so it can never be honoured literally).
/// [FR-WS-01]'s `[workspace.warm] concurrency` key is the `Some` source, read
/// off the manifest by [`Federation::warm_concurrency`](super::Federation#structfield.warm_concurrency)
/// and passed down by the parent that spawns the supervisor.
///
/// The floor is a **backstop, not the validation**: a manifest declaring `0` is
/// rejected at parse time with an actionable message ([`manifest::parse`](super::manifest::parse)),
/// precisely so it never reaches here to be silently floored. What the floor
/// covers is a programmatic caller — a future flag, a test — passing `0`
/// directly, where stalling the queue forever is the worse failure.
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
/// Not a serialized read-model: the summary is an in-process reporting value
/// with no wire shape. [FR-WS-15]/[S-323] derives the user-facing
/// `warm`/`deferred` labels from index presence, not from this, so a
/// `Serialize` derive here would publish a shape nothing projects — and
/// [`root`](Self::root) is a diagnostic label (an absolute path), deliberately
/// NOT the workspace-relative member *name* every federation read-model keys
/// on, so it is not join-compatible with `WorkspaceStatus` either.
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
/// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberWarm {
    /// The member repository root the queue warmed — a human-facing diagnostic
    /// label, not a join key.
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
/// previous index was still running). Report a failure as `Err(reason)` rather
/// than panicking: a panicking worker forfeits its slot — the remaining
/// workers keep draining, though at K = 1 there are none — and either way the
/// panic resurfaces when the scope ends, so the whole summary is lost rather
/// than one member's outcome. Nothing is swallowed, but the scope re-raises a
/// generic payload, so the worker's own message survives only on stderr.
///
/// The returned summary is a readout, not a result to check: this function
/// exists for its side effect, so discarding it is a legitimate call shape.
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
                    // House convention (`registry::lock_residents`). Note it
                    // cannot actually fire here: `index_member` runs OUTSIDE
                    // the lock, the only code under it is a `Vec::push`, and a
                    // worker panic re-raises from the scope rather than
                    // returning a poisoned summary.
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

    /// The stated rule, pinned independently of the host: `max(1, cores / 4)`
    /// capped at the cap. `cores / 2`, `cores / 8` and any constant all pass a
    /// mere range check on the live core count, so the formula gets a table.
    #[test]
    fn the_derived_bound_follows_max_one_cores_over_four_capped() {
        for (cores, expected) in [
            (0, 1),
            (1, 1),
            (2, 1),
            (3, 1),
            (4, 1),
            (7, 1),
            (8, 2),
            (12, 3),
            (15, 3),
            (16, 4),
            (17, 4),
            (64, 4),
            (256, 4),
        ] {
            assert_eq!(
                derive_concurrency(cores),
                expected,
                "{cores} cores must derive K = {expected}"
            );
        }
    }

    /// The core-derived default must itself be a value a manifest could legally
    /// declare — otherwise the default and the documented range disagree, and
    /// an operator writing down what Logos already chose would be rejected.
    ///
    /// (The `MANIFEST_CONCURRENCY_MAX > CONCURRENCY_CAP` half of the invariant
    /// is a `const` assertion beside the constants — the compiler proves it.)
    #[test]
    fn the_derived_default_is_a_legal_manifest_value() {
        assert!(
            (1..=MANIFEST_CONCURRENCY_MAX).contains(&default_concurrency()),
            "derived {} outside the declarable range 1..={MANIFEST_CONCURRENCY_MAX}",
            default_concurrency()
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

        warm_queue(&members, 1, |_| {
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
        assert_eq!(summary.members.len() - summary.degraded_count(), 2);
    }

    /// Not just "≤ K" — the queue must actually *use* its slots, or a bound
    /// implemented as head-of-line blocking would pass every ceiling test
    /// while warming serially.
    ///
    /// Deliberately a **deadline**, not a `Barrier`: a barrier makes a serial
    /// regression hang `cargo test` forever (there is no per-test timeout), and
    /// in this repo a wedged suite reads as a stuck session rather than as a red
    /// test. Here a serial implementation records `peak == 1` and fails the
    /// assertion within the deadline instead.
    #[test]
    fn the_queue_actually_runs_k_members_concurrently() {
        let members = roots(8);
        let stub = SpawnStub::default();
        // ONE deadline shared by the whole queue, not one per call: a serial
        // regression must fail in ~3s total, not 3s × N.
        let started = std::time::Instant::now();
        let deadline = Duration::from_secs(3);

        warm_queue(&members, 3, |_| {
            stub.enter();
            // Hold the slot until three were genuinely in flight at once — or
            // until the shared deadline proves they never will be. Keyed on the
            // monotonic `peak`, not on live `in_flight`: once the rendezvous has
            // happened every later member passes straight through, so the
            // healthy run costs milliseconds and only a regression pays the
            // deadline.
            while stub.peak.load(Ordering::SeqCst) < 3 && started.elapsed() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            stub.leave();
            Ok(())
        });

        assert_eq!(
            stub.peak.load(Ordering::SeqCst),
            3,
            "the queue must actually use its K slots, not serialise behind one"
        );
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
        assert_eq!(summary.members.len() - summary.degraded_count(), 4);
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
        assert_eq!(summary.members.len() - summary.degraded_count(), 0);
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

    /// The documented panic contract: a panicking worker's panic resurfaces
    /// when the scope ends, so the summary is lost rather than silently
    /// returned short. Pins that the panic is never swallowed — note the scope
    /// re-raises its own generic payload, so the worker's original message
    /// reaches only stderr (via the default panic hook), which is why
    /// `index_member` is documented to return `Err`, not to panic.
    #[test]
    #[should_panic(expected = "a scoped thread panicked")]
    fn a_panicking_worker_resurfaces_from_the_scope() {
        let members = roots(4);
        warm_queue(&members, 2, |root| {
            assert!(!root.ends_with("m2"), "member m2 exploded");
            Ok(())
        });
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
        warm_queue(&members, 4, |_| {
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
