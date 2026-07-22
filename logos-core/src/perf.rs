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
//! `logos-core/tests/perf_envelope.rs`; this module owns only the single number
//! that defines the envelope and the advisory both surfaces share.
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
/// roll-up's presence marker — a graph indexed before this feature carries
/// `indexed_loc` but no `test_loc`, so `status` reports the counts as absent
/// rather than a fabricated `0` ([NFR-CC-04]).
///
/// [FR-IX-12]: ../../../docs/specs/requirements/FR-IX-12.md
/// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
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

#[cfg(test)]
mod tests {
    use super::*;

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
