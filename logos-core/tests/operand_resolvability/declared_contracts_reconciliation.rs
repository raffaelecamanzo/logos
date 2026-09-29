//! **S-458's reconciliation** — the shipped declared-contract relation
//! (`federation::declared_contracts`, published on the coverage answer)
//! reproduces [S-456]'s declared-contract half **by name** over the reference
//! estate: the same ordered `(holder, contract)` pairs, labelled the same way,
//! and the same per-operation ties resolved by document identity.
//!
//! This is a reconciliation, never a floor ([S-456] recorded the figures; a
//! census figure is not an acceptance floor). It exists so the product's
//! identity score and external grouping cannot drift from the gate harness's
//! without a failure — the risk the sprint names as "a second identity-score /
//! external-grouping derivation diverging from S-456's".
//!
//! The two derivations differ deliberately in one place, and the comparison is
//! made where they agree: the harness pools a declared `documentation` holder's
//! copies into the copy graph, the product keeps them out. Neither counts such a
//! holder's pairs, so the counted pairs are compared, never the uncounted rows.
//!
//! Estate-gated like its sibling: it skips without `LOGOS_REF_WORKSPACE`, and it
//! opens member engines, so point it at a **private copy** whose stores this
//! binary has already reconciled (the harness refuses any other).
//!
//! [S-456]: ../../../docs/planning/journal.md#s-456-measure-vendored-spec-contracts-the-external-join-and-own-spec-ties

use std::collections::BTreeSet;

use logos_core::federation::{
    cross_service_coverage, discover, ContractTarget, EngineRegistry, RegistryMode,
};
use logos_core::Engine;

use super::vendored_spec_contracts::{judgement, Contract, Holder, TieClass};

/// The product's counted pairs as `(holder, label)`, labelled the harness's way
/// (`member <name>` / `external <display name>`).
fn product_pairs(root: &std::path::Path) -> (BTreeSet<(String, String)>, usize) {
    let federation = discover(root).expect("manifest parses").expect("a workspace");
    let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);
    let coverage = cross_service_coverage(&registry.answer());
    let relation = coverage.declared_contracts.expect("the estate declares contracts");
    let pairs = relation
        .contracts
        .iter()
        .map(|c| {
            let label = match &c.target {
                ContractTarget::Member { member, .. } => format!("member {member}"),
                ContractTarget::External { name, .. } => format!("external {name}"),
            };
            (c.holder.clone(), label)
        })
        .collect();
    (pairs, relation.resolved_ties.len())
}

#[test]
fn the_shipped_relation_reproduces_s456s_declared_contract_half_by_name() {
    let Some(root) = crate::corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to a private copy of the reference \
             workspace> to reconcile S-458's relation with S-456's harness."
        );
        return;
    };
    // The harness first: its registry is dropped before the product opens one.
    let (_, j) = judgement(&root);
    let harness: BTreeSet<(String, String)> = j
        .declared
        .pairs()
        .into_iter()
        .map(|(holder, contract)| (holder, j.declared.contract_label(&contract)))
        .collect();
    // The ties the harness's identity pair names: rows of that holder's document
    // whose tied list includes the identified member.
    let identity: Vec<(String, String, String)> = j
        .declared
        .rows
        .iter()
        .filter(|r| r.standing == Holder::Eligible)
        .filter_map(|r| match &r.contract {
            Contract::Member(m) => Some((r.holder.clone(), r.path.clone(), m.clone())),
            Contract::External(_) => None,
        })
        .collect();
    let harness_ties = j
        .ties
        .iter()
        .filter(|(row, class)| {
            *class != TieClass::Undecided
                && identity.iter().any(|(h, p, m)| &row.holder == h && &row.document == p && row.tied.contains(m))
        })
        .count();

    let (product, product_ties) = product_pairs(&root);
    println!("S-458 reconciliation over {}:", root.display());
    for (holder, label) in &product {
        println!("  {holder:<26} -> {label}");
    }
    println!("  resolved ties: product {product_ties}, harness {harness_ties}");

    assert_eq!(product, harness, "the relation must reproduce S-456's counted pairs by name");
    assert_eq!(product_ties, harness_ties, "the identity pair's resolved ties");
}
