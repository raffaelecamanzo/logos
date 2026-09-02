//! Process open-file-limit hygiene ([NFR-PE-11], [ADR-63]).
//!
//! A workspace's live database connections are bounded by
//! [`ConnectionBudget`](crate::federation::ConnectionBudget), which derives that
//! bound from this process's `RLIMIT_NOFILE` **soft** limit. Raising the soft
//! limit toward the hard limit therefore widens the budget — but it is a
//! *secondary* defence only: [ADR-63] rejects "raise the limit and stop there"
//! precisely because any fixed raise fails at some N. The guarantee rests on the
//! budget; this module just stops the stock 256-descriptor macOS default from
//! being the binding constraint when the kernel would happily allow more.
//!
//! Every operation here is **best-effort and silent**: a platform that refuses
//! the raise, or has no such limit at all, simply keeps whatever it had. Nothing
//! in the read path may observe a failure here as an error.
//!
//! [NFR-PE-11]: ../../docs/specs/requirements/NFR-PE-11.md
//! [ADR-63]: ../../docs/specs/architecture/decisions/ADR-63.md

use std::sync::Once;

/// Ceiling applied to a *reported* soft limit.
///
/// A soft limit of `RLIM_INFINITY` (common inside containers) is true but
/// useless as a budget input — it would derive an effectively unbounded resident
/// set, reinstating the very `members × cores` growth the budget exists to
/// remove. Reporting a large finite number instead keeps the budget arithmetic
/// meaningful without pretending the kernel is stricter than it is.
const REPORTED_SOFT_LIMIT_CEILING: u64 = 1_048_576;

/// Fallback targets tried, in order, when raising straight to the hard limit is
/// refused.
///
/// macOS reports `RLIM_INFINITY` as the `RLIMIT_NOFILE` hard limit but rejects
/// `setrlimit` above `kern.maxfilesperproc`, so an unconditional raise-to-hard
/// fails there with `EINVAL`. These two rungs — a generous modern value and the
/// legacy `OPEN_MAX` — cover that case without probing sysctl.
#[cfg(unix)]
const RAISE_LADDER: [u64; 2] = [65_536, 10_240];

/// Raise this process's `RLIMIT_NOFILE` soft limit toward its hard limit.
///
/// Called once, from `main`, before anything opens a file — [ADR-63] scopes the
/// raise to process startup, and nothing in the library changes a process-global
/// limit behind an embedder's back. The `Once` guard makes a second call harmless
/// rather than expected.
///
/// Best-effort and silent on failure. Returns the soft limit in force afterwards
/// — clamped, per [`open_file_soft_limit`] — or `None` where the platform has no
/// such limit.
///
/// This is deliberately **not** a fallible API. A caller must not be able to
/// treat "could not raise" as a condition worth reporting — that is the
/// [ADR-63] posture that the budget, not the limit, carries the guarantee.
///
/// [ADR-63]: ../../docs/specs/architecture/decisions/ADR-63.md
pub fn raise_open_file_limit() -> Option<u64> {
    static RAISED: Once = Once::new();
    RAISED.call_once(|| {
        raise_once();
    });
    open_file_soft_limit()
}

/// This process's current `RLIMIT_NOFILE` soft limit, clamped to
/// `REPORTED_SOFT_LIMIT_CEILING`.
///
/// `None` where the platform has no per-process descriptor limit, or where the
/// query itself failed — in both cases the caller falls back to a conservative
/// default rather than guessing high.
pub fn open_file_soft_limit() -> Option<u64> {
    limits().map(|(soft, _hard)| clamp_reported(soft))
}

/// Clamp a raw soft limit to [`REPORTED_SOFT_LIMIT_CEILING`].
///
/// A named function rather than an inline `.min()` so the clamp can be asserted
/// on an input this host will never report — every real soft limit (256, 1024,
/// 65 536) is far below the ceiling, so an inline clamp is indistinguishable
/// from no clamp at all in a test.
fn clamp_reported(soft: u64) -> u64 {
    soft.min(REPORTED_SOFT_LIMIT_CEILING)
}

#[cfg(unix)]
fn raise_once() {
    let Some((soft, hard)) = raw_limits() else {
        return;
    };
    for target in raise_candidates(soft, hard) {
        if set_soft_limit(target, hard) {
            return;
        }
    }
}

/// The targets to attempt, most generous first: the hard limit itself, then the
/// fallbacks for kernels that refuse it — keeping only rungs that are both an
/// actual increase and permitted by the hard limit.
///
/// Split out as a pure function because the raise itself is `Once`-guarded and
/// mutates a process-global limit, which makes the *ordering* — the part that
/// carries the macOS `RLIM_INFINITY` handling — otherwise untestable.
#[cfg(unix)]
fn raise_candidates(soft: libc::rlim_t, hard: libc::rlim_t) -> Vec<libc::rlim_t> {
    let ladder = RAISE_LADDER
        .iter()
        .filter_map(|target| libc::rlim_t::try_from(*target).ok());
    std::iter::once(hard)
        .chain(ladder)
        .filter(|target| *target > soft && *target <= hard)
        .collect()
}

#[cfg(not(unix))]
fn raise_once() {}

/// `(soft, hard)` for `RLIMIT_NOFILE` in the platform's own `rlim_t`, or `None`
/// if the query failed.
#[cfg(unix)]
fn raw_limits() -> Option<(libc::rlim_t, libc::rlim_t)> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `getrlimit` only writes a fully-initialised `rlimit` through the
    // unique pointer to our own stack slot. The pointer neither escapes nor
    // aliases, and the resource constant is the libc-typed `RLIMIT_NOFILE`.
    let rc = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
    (rc == 0).then_some((limit.rlim_cur, limit.rlim_max))
}

/// `(soft, hard)` for `RLIMIT_NOFILE`, or `None` where the platform has no such
/// limit (or the query failed).
///
/// The conversions are a no-op wherever `rlim_t` is already `u64` — which is
/// every target we ship — but `rlim_t` is platform-defined and narrower on some,
/// so widening explicitly is what keeps this compiling there.
#[allow(clippy::useless_conversion)]
#[cfg(unix)]
fn limits() -> Option<(u64, u64)> {
    let (soft, hard) = raw_limits()?;
    Some((u64::try_from(soft).ok()?, u64::try_from(hard).ok()?))
}

#[cfg(not(unix))]
fn limits() -> Option<(u64, u64)> {
    None
}

/// Set the soft limit to `soft` keeping `hard`; `true` if the kernel accepted it.
#[cfg(unix)]
fn set_soft_limit(soft: libc::rlim_t, hard: libc::rlim_t) -> bool {
    let limit = libc::rlimit {
        rlim_cur: soft,
        rlim_max: hard,
    };
    // SAFETY: `setrlimit` only reads the fully-initialised `rlimit` behind the
    // pointer to our own stack slot; the pointer neither escapes nor aliases.
    unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The raise is best-effort: it never panics, and on a platform with a
    /// descriptor limit it leaves the soft limit at least where it found it —
    /// a failed raise must never *lower* what the process already had.
    #[test]
    fn raising_never_lowers_the_soft_limit() {
        let before = open_file_soft_limit();
        let after = raise_open_file_limit();
        match (before, after) {
            (Some(before), Some(after)) => assert!(
                after >= before,
                "the best-effort raise lowered the soft limit ({before} → {after})"
            ),
            (None, None) => {} // no per-process limit on this platform
            (before, after) => panic!("the limit appeared or vanished: {before:?} → {after:?}"),
        }
    }

    /// The raise is idempotent — calling it repeatedly reports a stable limit,
    /// so a library entry point and `main` may both call it.
    #[test]
    fn raising_is_idempotent() {
        let first = raise_open_file_limit();
        let second = raise_open_file_limit();
        assert_eq!(first, second, "a repeated raise must report the same limit");
    }

    /// The macOS shape: a hard limit of `RLIM_INFINITY` is attempted first and,
    /// when the kernel refuses it, the ladder supplies finite rungs in
    /// descending order. This is the case the module exists for, and it is
    /// unreachable through `raise_open_file_limit` because the raise is
    /// `Once`-guarded and this host's limits are whatever they are.
    #[cfg(unix)]
    #[test]
    fn the_raise_ladder_tries_the_hard_limit_then_descending_fallbacks() {
        let infinite = libc::RLIM_INFINITY;
        assert_eq!(
            raise_candidates(256, infinite),
            vec![infinite, 65_536, 10_240],
            "an unbounded hard limit must still offer finite fallbacks"
        );
        // A finite hard limit filters out the rungs above it.
        assert_eq!(raise_candidates(256, 20_000), vec![20_000, 10_240]);
        // Nothing to do when the soft limit already is the hard limit.
        assert!(
            raise_candidates(65_536, 65_536).is_empty(),
            "an already-raised process must attempt no setrlimit at all"
        );
        // Every candidate is a genuine increase within the hard limit.
        for (soft, hard) in [(0, 100_000), (10_240, 65_536), (1_024, infinite)] {
            for target in raise_candidates(soft, hard) {
                assert!(target > soft && target <= hard);
            }
        }
    }

    /// The soft limit never exceeds the hard limit, and is reported clamped so
    /// an `RLIM_INFINITY` soft limit cannot derive an unbounded budget.
    #[test]
    fn the_reported_soft_limit_is_clamped_and_within_the_hard_limit() {
        let Some((soft, hard)) = limits() else {
            return; // no per-process limit on this platform
        };
        assert!(soft <= hard, "soft limit {soft} exceeds hard limit {hard}");
        let reported = open_file_soft_limit().expect("a platform with limits reports one");
        assert!(
            reported <= REPORTED_SOFT_LIMIT_CEILING,
            "an unbounded soft limit must be reported clamped, got {reported}"
        );
    }

    /// The clamp itself, on the input the whole constant exists for: a container
    /// reporting `RLIM_INFINITY`. Real hosts report 256 / 1024 / 65 536, all far
    /// below the ceiling, so nothing else in the suite can tell the clamp from
    /// its absence.
    #[test]
    fn an_unbounded_soft_limit_is_reported_as_the_ceiling() {
        assert_eq!(clamp_reported(u64::MAX), REPORTED_SOFT_LIMIT_CEILING);
        assert_eq!(clamp_reported(REPORTED_SOFT_LIMIT_CEILING + 1), REPORTED_SOFT_LIMIT_CEILING);
        assert_eq!(clamp_reported(256), 256, "an ordinary limit passes through");
    }
}
