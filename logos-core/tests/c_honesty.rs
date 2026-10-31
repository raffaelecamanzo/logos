//! C honesty-fixture integration tests (S-056, CR-009, the never-fabricate
//! posture of [NFR-CC-04]/[NFR-RA-05]), driven end-to-end through the public
//! [`Engine`] façade against real pure-C fixtures.
//!
//! C is the set's deliberate honesty fixture: it ships no frameworks
//! capability, and is **not** class-applicable. The contract these
//! tests pin is that every such absence surfaces as `n/a` — never a fabricated
//! clean score:
//!
//! - a pure-C repo (functions **and** a `struct`) reports Cohesion as `None`
//!   (n/a drop-out), because C emits no `Class` node, while the aggregate
//!   signal still computes over the remaining dimensions ([FR-QM-11],
//!   [NFR-CC-04], [ADR-21]);
//! - its bodied `struct` is a `Struct` (S-582, [FR-EX-31]), so Focus
//!   ([FR-QM-12]) counts it as it counts a Go type with no methods — by its
//!   span alone, a method count of zero never making it god.

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

/// A pure-C fixture containing both functions and a `struct`. Cohesion
/// (production classes) must drop out as `n/a`, while the remaining dimensions
/// still compute a real aggregate ([FR-QM-11], [NFR-CC-04], [ADR-21]). The
/// struct is a Struct (S-582), so Focus applies over it, one container that
/// its span leaves short of god ([FR-QM-12]).
#[test]
fn pure_c_repo_reports_cohesion_as_na_and_focus_over_its_struct() {
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
    let model = metrics::snapshot(rt, &view, None, metrics::Thresholds::default(), &engine.registry().expect("a started engine holds its registry").member_access_extensions())
        .expect("snapshot runs")
        .metrics;

    assert!(!model.empty, "an indexed C repo is not the empty sentinel");
    assert!(
        model.cohesion.is_none(),
        "C is not class-applicable — Cohesion is n/a, never a fabricated score"
    );
    let focus = model.focus.as_ref().expect("the bodied `struct` is a Focus container");
    assert_eq!(focus.raw, 0.0, "one container, not god: no methods, a five-line span");
    // The honesty payoff (ADR-21): n/a dimensions drop out of the denominator,
    // they do not collapse the signal — the remaining dimensions still aggregate.
    assert!(
        model.aggregate_signal.is_some(),
        "the aggregate is computed over the remaining applicable dimensions"
    );
}
