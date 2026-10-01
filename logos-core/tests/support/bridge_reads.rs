//! **The narrowed bridge read changes no answer** (S-484, [FR-WS-05],
//! [NFR-PE-10]) — one assertion every federation fixture runs over its own
//! workspace.
//!
//! The three bridge-backed read-models (`route-providers`, `callers`,
//! `impact`) are answered twice and compared serialized, ignoring only the
//! `member_reads` field S-484 added:
//!
//! - **cold** — over a fresh bridge, whose first read computes the edge set
//!   and residue from every member's surfaces. That computation is the
//!   unchanged code, and it is what the old all-member stamp check returned
//!   on a hit as well as on a miss, so these are today's answers;
//! - **narrowed** — over a warm bridge after every member engine has been
//!   evicted, so the stamp check states each member's stamp without starting
//!   it. Each such answer is proved to have taken that path: the bridge read
//!   it was built from read no member and started no engine.
//!
//! The symbols asked about are every endpoint of the fixture's own edges plus
//! one that matches nothing, each unscoped and scoped to every member, so the
//! helper needs nothing from the fixture but its registry.
//!
//! [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
//! [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md

use std::collections::BTreeSet;

use logos_core::federation::{query, ContractBridge, EngineRegistry, MemberReads};
use logos_core::Engine;
use serde_json::Value;

/// Assert the three bridge-backed answers over `registry` are identical, cold
/// and narrowed, ignoring only `member_reads` — and that every narrowed read
/// really read nothing.
pub fn assert_narrowed_read_changes_no_answer(registry: &EngineRegistry<Engine>) {
    let cold = ContractBridge::new();
    let mut symbols: BTreeSet<String> = query::reachability_inputs(&cold, registry)
        .edges
        .iter()
        .flat_map(|edge| [edge.from.symbol.as_str().to_string(), edge.to.symbol.as_str().to_string()])
        .collect();
    symbols.insert("local no_such_symbol".to_string());
    let repos: Vec<Option<&str>> = std::iter::once(None)
        .chain(registry.members().iter().map(|member| Some(member.name.as_str())))
        .collect();
    let expected = answers(registry, &cold, &symbols, &repos, false);

    // Warm the bridge over the workspace as the narrowed reads will find it:
    // every engine evicted. A member whose engine advanced its stamp while
    // resident (a fresh engine's navigation prologue can) makes the first such
    // read a miss — as it was when the old check reopened it at the restart
    // stamp — and the second the hit every compared answer below is served.
    let warm = ContractBridge::new();
    for _ in 0..2 {
        registry.evict_to_capacity(0);
        let _ = query::reachability_inputs(&warm, registry);
    }

    let narrowed = answers(registry, &warm, &symbols, &repos, true);
    assert_eq!(narrowed.len(), expected.len());
    for (narrowed, expected) in narrowed.iter().zip(&expected) {
        assert_eq!(narrowed, expected, "the narrowed read changed an answer");
    }
}

/// Every answer over `bridge`, serialized without `member_reads`.
///
/// With `narrowed`, every member engine is evicted before each answer, so each
/// one's stamp check runs narrowed — and each is proved to have: the bridge
/// read it was built from read no member and started no engine. Without that
/// guard the comparison could pass over a stamp check that quietly opened
/// everything, as the old one did. (A member whose last open failed is
/// attempted again, so it may be named unread; it is never read.)
fn answers(
    registry: &EngineRegistry<Engine>,
    bridge: &ContractBridge,
    symbols: &BTreeSet<String>,
    repos: &[Option<&str>],
    narrowed: bool,
) -> Vec<Value> {
    let guard = |reads: &MemberReads, starts: u64| {
        if narrowed {
            assert!(reads.read.is_empty(), "guard the guard: the narrowed read read a member: {reads:?}");
            assert_eq!(registry.engine_starts(), starts, "guard the guard: the narrowed read started an engine");
        }
    };
    let evicted = || {
        if narrowed {
            registry.evict_to_capacity(0);
        }
        registry.engine_starts()
    };
    let mut out = Vec::new();
    for repo in repos {
        let starts = evicted();
        let read = query::bridge_read(bridge, registry);
        guard(&read.reads, starts);
        out.push(without_reads(query::xservice_route_providers(&read, *repo)));
        for symbol in symbols {
            let starts = evicted();
            let inputs = query::reachability_inputs(bridge, registry);
            guard(&inputs.reads, starts);
            out.push(without_reads(query::xservice_callers(registry, &inputs, symbol, None, *repo)));
            let starts = evicted();
            let inputs = query::reachability_inputs(bridge, registry);
            guard(&inputs.reads, starts);
            out.push(without_reads(query::xservice_impact(registry, &inputs, symbol, None, *repo)));
        }
    }
    out
}

/// `answer` serialized, with the one field the comparison ignores removed —
/// after checking it is there, so a renamed field cannot make the strip a
/// no-op that hides it.
fn without_reads(answer: impl serde::Serialize) -> Value {
    let mut value = serde_json::to_value(answer).expect("the read-model serializes");
    let object = value.as_object_mut().expect("a read-model is an object");
    assert!(object.remove("member_reads").is_some(), "every bridge-backed answer names its member reads");
    value
}
