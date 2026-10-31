//! The performance envelope and its beyond-envelope advisory (S-024,
//! [NFR-PE-09]).
//!
//! Logos tunes its latency budgets — sub-100 ms navigation ([NFR-PE-01]),
//! ≤30 s cold index ([NFR-PE-02]), ≤250 ms single-file sync ([NFR-PE-03]),
//! ≤2 s warm scan ([NFR-PE-04]), ≤1 GB peak RSS ([NFR-PE-06]) — for a
//! *small/medium* target of roughly [`ENVELOPE_LOC`] lines of code. Beyond
//! that envelope the system stays **functionally correct** (no crash, no
//! wrong results) but the latency guarantees no longer hold ([NFR-PE-09]).
//!
//! The honest-expectations contract from [NFR-PE-09] is a one-line **advisory**
//! emitted by `index` and `status` when a repository materially exceeds the
//! envelope. [`index`](crate::pipeline::index) records the LOC it ingested under
//! [`INDEXED_LOC_KEY`] so [`status`](crate::Engine::status) can repeat the same
//! advisory without re-reading the working tree.
//!
//! The budgets themselves are asserted as release-gating fitness functions in
//! `logos-core/tests/perf_envelope.rs`; this module owns the single number
//! that defines the envelope, the advisory both surfaces share, and the one
//! wall-clock tolerance every timing assertion scales through ([`scaled`]).
//!
//! [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
//! [NFR-PE-02]: ../../../docs/specs/requirements/NFR-PE-02.md
//! [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
//! [NFR-PE-04]: ../../../docs/specs/requirements/NFR-PE-04.md
//! [NFR-PE-06]: ../../../docs/specs/requirements/NFR-PE-06.md
//! [NFR-PE-09]: ../../../docs/specs/requirements/NFR-PE-09.md

/// The small/medium target the latency budgets ([NFR-PE-01]..[NFR-PE-07]) are
/// tuned for: ~100k lines of code over a 5-language repo on the baseline laptop.
///
/// [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
/// [NFR-PE-07]: ../../../docs/specs/requirements/NFR-PE-07.md
pub const ENVELOPE_LOC: u64 = 100_000;

/// The `project_metadata` key under which a full [`index`](crate::pipeline::index)
/// records the LOC it ingested, so [`status`](crate::Engine::status) can emit the
/// same [NFR-PE-09] advisory without re-reading the tree.
///
/// It doubles as the **total** of the source/test physical-LOC roll-up
/// ([FR-IX-12]): the roll-up's total is that same ingested-LOC sum, never
/// re-counted, so the invariant `total = source + test` holds by construction.
///
/// [NFR-PE-09]: ../../../docs/specs/requirements/NFR-PE-09.md
/// [FR-IX-12]: ../../../docs/specs/requirements/FR-IX-12.md
pub const INDEXED_LOC_KEY: &str = "indexed_loc";

/// The `project_metadata` key under which a full [`index`](crate::pipeline::index)
/// records the **test** portion of the physical-LOC roll-up ([FR-IX-12]) — the
/// ingested-LOC sum restricted to files matched by the [FR-AN-05] test-path
/// conventions ([`crate::navigate::is_test_path`]).
///
/// Only the test bucket is persisted: [`status`](crate::Engine::status) reads the
/// total from [`INDEXED_LOC_KEY`] and derives `source = total − test`, so the
/// source figure is never counted independently. This key's **presence** is the
/// roll-up's presence marker, and it reads absent in two ways, both of them
/// honest empty states rather than a fabricated `0` ([NFR-CC-04]): a graph
/// indexed before this feature carries `indexed_loc` but no `test_loc`; and a
/// full index that leaves the store empty **clears both keys**, because a
/// roll-up beside `indexed: false` would describe a graph that no longer exists
/// ([CR-134]). A full index that persists nothing while a graph survives leaves
/// both keys alone, so the figures keep describing the graph they were counted
/// over — see [`crate::pipeline`]'s `IndexPersistence`.
///
/// [FR-IX-12]: ../../../docs/specs/requirements/FR-IX-12.md
/// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [CR-134]: ../../../docs/requests/CR-134-a-zero-persist-index-fabricates-a-zero-loc-rollup.md
pub const TEST_LOC_KEY: &str = "test_loc";

/// The one-line advisory emitted when an indexed repository *materially* exceeds
/// the performance envelope ([NFR-PE-09]) — `None` inside it.
///
/// "Materially" is a 10% margin over [`ENVELOPE_LOC`]: a repo sitting right at
/// ~100k LOC is still on-budget, so it does not trip the advisory; only a repo
/// meaningfully past the envelope (>110k LOC) earns the honest-expectations
/// note. The advisory is degradation channel only — it never turns a successful
/// index/status into a failure ([ADR-14]).
///
/// [NFR-PE-09]: ../../../docs/specs/requirements/NFR-PE-09.md
/// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
pub(crate) fn envelope_advisory(indexed_loc: u64) -> Option<String> {
    // The "materially exceeds" trigger: the envelope plus a 10% margin.
    let trigger = ENVELOPE_LOC + ENVELOPE_LOC / 10;
    (indexed_loc > trigger).then(|| {
        format!(
            "this repository is ~{indexed_loc} LOC, beyond the ~{ENVELOPE_LOC} LOC \
             performance envelope (NFR-PE-09): results stay correct but the latency \
             budgets (sub-100ms navigation, \u{2264}30s index) no longer apply"
        )
    })
}

/// The environment variable that widens every wall-clock band ([`tolerance`]).
pub const TOLERANCE_ENV: &str = "LOGOS_PERF_TOLERANCE";

/// The multiplier a band carries when [`TOLERANCE_ENV`] is unset or unusable:
/// 1.15 since 2026-09-21 (the Sprint 74 human review measured a loaded host
/// breaching a raw budget with no code change).
pub const DEFAULT_TOLERANCE: f64 = 1.15;

/// The largest multiplier [`TOLERANCE_ENV`] may carry. A band 1000× its budget
/// asserts nothing, and a still larger one overflows [`Duration::mul_f64`].
pub const MAX_TOLERANCE: f64 = 1000.0;

/// The one reading of [`TOLERANCE_ENV`] — a pure function of the raw value, so
/// the rule is testable without touching the process environment.
///
/// A value that is not a finite number in `1.0..=`[`MAX_TOLERANCE`] is ignored: a
/// budget is never tightened by accident, and an infinite or enormous
/// multiplier would panic in [`Duration::mul_f64`].
pub(crate) fn tolerance_from(raw: Option<&str>) -> f64 {
    raw.and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| (1.0..=MAX_TOLERANCE).contains(v))
        .unwrap_or(DEFAULT_TOLERANCE)
}

/// The wall-clock tolerance multiplier: [`TOLERANCE_ENV`] when it holds a finite
/// number in `1.0..=`[`MAX_TOLERANCE`], else [`DEFAULT_TOLERANCE`]. Set `LOGOS_PERF_TOLERANCE=1.0` to
/// measure against the raw budgets.
///
/// This is the only source that reads the variable. Every test that compares an
/// elapsed time with a duration scales the duration through [`scaled`] (or reads
/// this), and `scripts/check-timing-assertions.py` fails the gate on one that
/// does not. It lives in the library, not a test file, so the unit tests of
/// `logos-core` and the integration tests of every package reach it as
/// `logos_core::perf`; production code never calls it.
pub fn tolerance() -> f64 {
    tolerance_from(std::env::var(TOLERANCE_ENV).ok().as_deref())
}

/// `budget` widened by [`tolerance`] — the form an elapsed-time bound takes:
/// `assert!(elapsed < perf::scaled(Duration::from_secs(5)))`.
pub fn scaled(budget: std::time::Duration) -> std::time::Duration {
    budget.mul_f64(tolerance())
}

/// [`scaled`] for a budget stated in milliseconds, the unit the NFR budgets use.
pub fn scaled_ms(budget_ms: u64) -> std::time::Duration {
    scaled(std::time::Duration::from_millis(budget_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_or_unusable_tolerance_is_the_default() {
        let ignored = [
            None,
            Some(""),
            Some("fast"),
            Some("0.5"),
            Some("0"),
            Some("-2"),
            Some("NaN"),
            Some("inf"),
            Some("1e300"),
            Some("1000.5"),
        ];
        for raw in ignored {
            assert_eq!(tolerance_from(raw), DEFAULT_TOLERANCE, "{raw:?} is ignored");
        }
    }

    #[test]
    fn a_tolerance_of_one_or_more_is_read() {
        assert_eq!(
            tolerance_from(Some("1.0")),
            1.0,
            "1.0 measures the raw budget"
        );
        assert_eq!(tolerance_from(Some("3")), 3.0);
        assert_eq!(
            tolerance_from(Some(" 2.5\n")),
            2.5,
            "whitespace is not part of the number"
        );
    }

    #[test]
    fn the_largest_tolerance_is_read_and_scales_without_panicking() {
        assert_eq!(tolerance_from(Some("1000")), MAX_TOLERANCE);
        let day = std::time::Duration::from_secs(86_400);
        assert_eq!(day.mul_f64(tolerance_from(Some("1000"))).as_secs(), 86_400_000);
    }

    #[test]
    fn the_environment_is_what_tolerance_and_scaled_read() {
        // `tolerance()` is the pure reading applied to the live variable (CI sets
        // it to 3, a developer may leave it unset), so whatever this process
        // holds, the three entry points agree with it and never tighten a budget.
        let raw = std::env::var(TOLERANCE_ENV).ok();
        let want = tolerance_from(raw.as_deref());
        assert_eq!(tolerance(), want);
        let budget = std::time::Duration::from_millis(200);
        assert_eq!(scaled(budget), budget.mul_f64(want));
        assert_eq!(scaled_ms(200), scaled(budget));
        assert!(scaled_ms(200) >= budget);
        if raw.is_none() {
            assert_eq!(scaled_ms(200), std::time::Duration::from_millis(230));
        }
    }

    #[test]
    fn a_budget_scales_by_the_tolerance_and_the_default_band_is_230_ms() {
        let band = std::time::Duration::from_millis(200);
        // `scaled` reads the process environment; this asserts the arithmetic
        // it applies, through the same pure reading it uses.
        assert_eq!(
            band.mul_f64(tolerance_from(None)),
            std::time::Duration::from_millis(230)
        );
        assert_eq!(
            band.mul_f64(tolerance_from(Some("3"))),
            std::time::Duration::from_millis(600)
        );
        assert!(scaled(band) >= band, "a budget is never tightened");
    }

    #[test]
    fn inside_the_envelope_is_silent() {
        // At and just under the envelope — and within the 10% margin — no advisory.
        assert!(envelope_advisory(0).is_none());
        assert!(envelope_advisory(ENVELOPE_LOC).is_none());
        assert!(
            envelope_advisory(ENVELOPE_LOC + ENVELOPE_LOC / 10).is_none(),
            "the 10% margin is still on-budget (not yet *material*)"
        );
    }

    #[test]
    fn materially_beyond_the_envelope_advises() {
        let loc = ENVELOPE_LOC * 2;
        let advisory = envelope_advisory(loc).expect("a 2x-envelope repo trips the advisory");
        // The advisory is honest and actionable: it names the figure, the
        // envelope, and what is lost (the latency budgets) — never a crash.
        assert!(advisory.contains(&loc.to_string()), "states the actual LOC");
        assert!(advisory.contains("NFR-PE-09"), "cites the contract");
        assert!(
            advisory.contains("correct"),
            "reassures correctness is retained: {advisory}"
        );
    }
}
