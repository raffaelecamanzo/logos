//! **S-459's reconciliation** — the shipped external join
//! (`federation::external_join`, published as `bound_external` on the coverage
//! answer) reproduces [S-456]'s invocation-to-external half **by name** over the
//! reference estate: every row the harness counted exact under a committed base
//! path is a row the product binds, to the external's own copy, under the same
//! base path.
//!
//! This is a reconciliation, never a floor ([S-456] recorded the figures; a
//! census figure is not an acceptance floor). It exists so the product's join
//! cannot drift from the gate harness's without a failure.
//!
//! The two differ deliberately in one place, and the comparison names it rather
//! than hiding it: the harness walked **hidden** directories for overlays, the
//! product opens only files the discovery walk admitted ([FR-WS-19],
//! [ADR-68] point 3). So a row whose only overriding overlay sits under a hidden
//! directory — `notification-adapter`'s `.helm/values.yaml` — is the harness's
//! overshoot and the product's bind under application configuration. Every row
//! the product binds beyond the harness's exact set must be exactly that: a row
//! the harness's own application-config-only counterfactual reads exact.
//!
//! Estate-gated like its siblings: it skips without `LOGOS_REF_WORKSPACE`, and it
//! opens member engines, so point it at a **private copy** whose stores this
//! binary has already reconciled (the harness refuses any other).
//!
//! [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
//! [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
//! [S-456]: ../../../docs/planning/journal.md#s-456-measure-vendored-spec-contracts-the-external-join-and-own-spec-ties

use std::collections::{BTreeMap, BTreeSet};

use logos_core::federation::{
    cross_service_coverage, discover, BaseOrigin, EngineRegistry, JoinOutcome, RegistryMode,
};
use logos_core::Engine;

use super::vendored_spec_contracts::{judgement, CallClass};

/// `(member, symbol)` → `(base path, origin, operation)` for every row the
/// shipped join binds, plus its headline `(bound, denominator)`.
type Bound = BTreeMap<(String, String), (String, BaseOrigin, String)>;

fn product_bound(root: &std::path::Path) -> (Bound, (u64, u64)) {
    let federation = discover(root).expect("manifest parses").expect("a workspace");
    let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);
    let coverage = cross_service_coverage(&registry.answer());
    let join = coverage.bound_external.expect("the estate declares named externals");
    let bound = join
        .rows
        .iter()
        .filter_map(|row| match &row.outcome {
            JoinOutcome::BoundExternal(b) => Some((
                (row.from.member.clone(), row.from.symbol.as_str().to_string()),
                (b.base.path.clone(), b.base.origin, b.operation.clone()),
            )),
            JoinOutcome::Refused(_) => None,
        })
        .collect();
    (bound, (join.headline.bound_external, join.headline.no_provider_rows))
}

#[test]
fn the_shipped_join_reproduces_s456s_invocation_half_by_name() {
    let Some(root) = crate::corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to a private copy of the reference \
             workspace> to reconcile S-459's join with S-456's harness."
        );
        return;
    };
    // The harness first: its registry is dropped before the product opens one.
    let (_, j) = judgement(&root);
    let key = |c: &super::vendored_spec_contracts::JudgedCall| (c.row.member.clone(), c.row.symbol.clone());
    let harness_exact: BTreeMap<(String, String), String> = j
        .calls
        .iter()
        .filter_map(|c| match &c.class {
            CallClass::Exact { base, .. } => Some((key(c), base.clone())),
            _ => None,
        })
        .collect();
    let harness_app_only_exact: BTreeSet<(String, String)> =
        j.calls.iter().filter(|c| matches!(c.app_only, CallClass::Exact { .. })).map(key).collect();

    let (product, (bound, rows)) = product_bound(&root);
    println!("S-459 reconciliation over {}:", root.display());
    println!("  product: {bound} of {rows} no-provider REST rows bound; harness exact {}", harness_exact.len());
    for ((member, symbol), (base, origin, operation)) in &product {
        let mark = if harness_exact.contains_key(&(member.clone(), symbol.clone())) { " " } else { "+" };
        println!("  {mark} {member:<22} {operation:<48} base {base:?} ({origin:?})  {symbol}");
    }

    for (row, base) in &harness_exact {
        let Some((product_base, _, _)) = product.get(row) else {
            panic!("the harness's exact row {row:?} is not bound by the product");
        };
        assert_eq!(product_base, base, "{row:?} binds under a different base path");
    }
    for row in product.keys().filter(|row| !harness_exact.contains_key(*row)) {
        assert!(
            harness_app_only_exact.contains(row),
            "{row:?} is bound by the product but the harness reads it exact under neither \
             reading — not the hidden-overlay difference this reconciliation admits"
        );
        assert_eq!(product[row].1, BaseOrigin::ApplicationConfig, "{row:?}");
    }
    assert_eq!(bound as usize, product.len());
}
