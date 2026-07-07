//! C honesty-fixture integration tests (S-056, CR-009, the never-fabricate
//! posture of [NFR-CC-04]/[NFR-RA-05]), driven end-to-end through the public
//! [`Engine`] façade against real pure-C fixtures.
//!
//! C is the set's deliberate honesty fixture: it ships no test convention, no
//! smell query, no frameworks capability, and is **not** class-applicable. The
//! contract these tests pin is that every such absence surfaces as `n/a` — never
//! a fabricated clean score:
//!
//! - a pure-C repo (functions **and** a `struct`) reports Cohesion and Focus as
//!   `None` (n/a drop-out), because C emits neither `Class` nor `Struct` nodes,
//!   while the aggregate signal still computes over the remaining dimensions
//!   ([FR-QM-11], [FR-QM-12], [NFR-CC-04], [ADR-21]);
//! - a pure-C repo is never scored "clean" for test smells — it produces zero
//!   smell findings and is never fabricated into the analyzed-and-clean set
//!   ([FR-CV-08], [NFR-RA-05]).

#![cfg(feature = "lang-c")]

use std::fs;
use std::path::Path;

use logos_core::{metrics, Engine, Granularity};
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A pure-C fixture containing both functions and a `struct` — the struct is
/// present precisely to prove C never lets a method-less aggregate fabricate a
/// Focus score. Cohesion (production classes) and Focus (Class ∪ Struct
/// containers) must both drop out as `n/a`, while the remaining dimensions still
/// compute a real aggregate ([FR-QM-11], [FR-QM-12], [NFR-CC-04], [ADR-21]).
#[test]
fn pure_c_repo_reports_cohesion_and_focus_as_na() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/store.c",
        "\
struct Account {
    int id;
    int balance;
};

int deposit(int balance, int amount) {
    if (amount > 0) {
        return balance + amount;
    }
    return balance;
}

int withdraw(int balance, int amount) {
    return deposit(balance, -amount);
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    let view = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("dependency view hydrates");
    let rt = engine.runtime().unwrap();
    let (_, model) =
        metrics::snapshot(rt, &view, None, metrics::Thresholds::default()).expect("snapshot runs");

    assert!(!model.empty, "an indexed C repo is not the empty sentinel");
    assert!(
        model.cohesion.is_none(),
        "C is not class-applicable — Cohesion is n/a, never a fabricated score"
    );
    assert!(
        model.focus.is_none(),
        "C emits no Class/Struct container — Focus is n/a even with a `struct` present"
    );
    // The honesty payoff (ADR-21): n/a dimensions drop out of the denominator,
    // they do not collapse the signal — the remaining dimensions still aggregate.
    assert!(
        model.aggregate_signal.is_some(),
        "the aggregate is computed over the remaining applicable dimensions"
    );
}

/// A pure-C repo is never scored "clean" for test smells. C ships no smell query
/// and no test convention, so its files are simply not smell-analyzed — they
/// produce zero findings and C is never fabricated into an analyzed-and-clean
/// verdict ([FR-CV-08], [NFR-RA-05]).
#[test]
fn pure_c_repo_is_never_scored_clean_for_smells() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "tests/store_test.c",
        "\
int run_tests(void) {
    /* a test-ish function with no assertion — but C has no smell query, so
       this is honestly not analyzed, never flagged and never cleared. */
    int x = 1 + 1;
    return x;
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let report = engine.test_gaps(None, true).expect("test_gaps runs");

    assert!(
        report.smells.findings.is_empty(),
        "a pure-C repo yields zero smell findings — nothing fabricated"
    );
    assert!(
        !report.smells.not_analyzed.iter().any(|l| l == "c"),
        "C declares no test convention, so it is not in the analyzed-vs-n/a set \
         either — it is simply never smell-scored, honestly"
    );
}
