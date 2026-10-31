//! The workspace reference resolution's pooling (S-637, FR-RS-49): what a
//! workspace re-reads of each member's provably external and unbound rows, and
//! the members it states.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::*;
use crate::graph_store::{DeclaredNameSources, UnboundCodeTarget, UnboundInvocation};
use crate::model::EdgeKind;
use crate::resolve::package_key::PackageLayout;

fn universe(names: &[&str]) -> NameUniverse {
    NameUniverse::build(
        &DeclaredNameSources {
            names: names.iter().map(|n| (*n).to_string()).collect(),
            aliases: Vec::new(),
            files: Vec::new(),
        },
        &PackageLayout::default(),
    )
}

fn calls(target: &str, rows: u64) -> UnboundCodeTarget {
    UnboundCodeTarget {
        language: Some("java".to_string()),
        kind: i64::from(EdgeKind::Calls.as_i32()),
        target: target.to_string(),
        rows,
    }
}

fn fqn(dotted: &str) -> Vec<String> {
    dotted.split('.').map(str::to_string).collect()
}

/// A roster entry: a member read with its own figure, or unread with a reason.
type Row = (&'static str, Result<InternalResolution, &'static str>);

/// [`pool`] over owned roster rows.
fn run(
    roster: &[Row],
    facts: BTreeMap<String, MemberReferenceFacts>,
    declared: &[&str],
    bound_sites: &BTreeSet<BoundSite>,
) -> WorkspaceReferenceResolution {
    let reads: Vec<MemberRead<'_>> =
        roster.iter().map(|(member, own)| MemberRead { member, own: own.as_ref().map_err(|why| *why) }).collect();
    let declared: HashSet<Vec<String>> = declared.iter().map(|name| fqn(name)).collect();
    pool(&reads, facts, &declared, bound_sites)
}

const CALL_SITE: &str = "local fetch_order";

/// `app` calls into `lib`: `Widget.render` (named nowhere in `app`, declared
/// by `lib`), `com.lib.Client.create` (the binder's `external-type`, a type
/// `lib` declares), `serde.to_string` (no member declares it), and two HTTP
/// client calls, one of which the bridge bound to `lib`. `lib` declares
/// `com.lib.Client` ([`DECLARED`]). Groups count rows, so the external-type
/// group and the bound call site are two rows each — a count read as one per
/// group is visible.
fn app_and_lib() -> (Vec<Row>, BTreeMap<String, MemberReferenceFacts>) {
    let freshness = vec![
        // 12 code refs: 4 bound; 2 + 1 named nowhere + 2 external-type external;
        // 2 + 1 unbound invocation call sites.
        ("app", Ok(InternalResolution::measured(12, 4, 5))),
        ("lib", Ok(InternalResolution::measured(6, 6, 0))),
    ];
    let app = MemberReferenceFacts {
        universe: universe(&["Main", "fetch_order"]),
        named_nowhere: vec![calls("Widget.render", 2), calls("serde.to_string", 1)],
        external_type: vec![(
            "Client.create".to_string(),
            BTreeMap::from([(vec![fqn("com.lib.Client")], 2)]),
        )],
        unbound_invocations: vec![
            UnboundInvocation { source_symbol: CALL_SITE.to_string(), target: "GET /orders/{id}".to_string(), rows: 2 },
            UnboundInvocation { source_symbol: CALL_SITE.to_string(), target: "GET /nowhere".to_string(), rows: 1 },
        ],
    };
    let lib = MemberReferenceFacts { universe: universe(&["Widget", "render", "Client"]), ..Default::default() };
    (freshness, BTreeMap::from([("app".to_string(), app), ("lib".to_string(), lib)]))
}

/// The types the two members declare.
const DECLARED: [&str; 2] = ["com.app.Main", "com.lib.Client"];

fn bound(member: &str, symbol: &str, target: &str) -> BTreeSet<BoundSite> {
    BTreeSet::from([(member.to_string(), symbol.to_string(), target.to_string())])
}

#[test]
fn a_reference_another_member_declares_is_internal_and_a_bridge_bound_one_resolved() {
    let (freshness, facts) = app_and_lib();
    let pooled = run(&freshness, facts, &DECLARED, &bound("app", CALL_SITE, "GET /orders/{id}"));
    assert_eq!(
        pooled.cross_member,
        CrossMemberReferences { named_by_another_member: 2, type_in_another_member: 2, bridge_bound: 2 }
    );
    let figure = pooled.internal_resolution;
    // 18 code references; only `serde.to_string` stays external; 10 bound + 2
    // bound across members.
    assert_eq!(
        (figure.code_references, figure.provably_external, figure.internal_candidates, figure.resolved),
        (18, 1, 17, 12)
    );
    assert_eq!(figure.lower_bound, Some(12.0 / 17.0));
    assert_eq!(
        pooled.summary,
        "lower bound 0.706 (12 of 17 internal candidates resolved; 18 code references, 1 provably external), \
         summed over 2 of 2 members; 4 counted internal as another member's, 2 bound across members"
    );
}

#[test]
fn a_site_bound_for_another_member_or_target_resolves_nothing_here() {
    for (member, target) in [("lib", "GET /orders/{id}"), ("app", "GET /orders")] {
        let (freshness, facts) = app_and_lib();
        let pooled = run(&freshness, facts, &DECLARED, &bound(member, CALL_SITE, target));
        assert_eq!(pooled.cross_member.bridge_bound, 0, "{member} {target}");
        assert_eq!(pooled.internal_resolution.resolved, 10);
    }
}

#[test]
fn a_member_alone_keeps_its_external_rows_external() {
    let (mut freshness, mut facts) = app_and_lib();
    freshness.truncate(1);
    facts.remove("lib");
    let pooled = run(&freshness, facts, &["com.app.Main"], &BTreeSet::new());
    assert_eq!(pooled.cross_member, CrossMemberReferences::default());
    assert_eq!(pooled.internal_resolution, InternalResolution::measured(12, 4, 5), "its own figure, unchanged");
}

#[test]
fn the_figure_is_pooled_never_a_mean_of_member_ratios() {
    let freshness: Vec<Row> = vec![
        ("small", Ok(InternalResolution::measured(1, 1, 0))),
        ("large", Ok(InternalResolution::measured(99, 0, 0))),
    ];
    let facts = BTreeMap::from([
        ("small".to_string(), MemberReferenceFacts::default()),
        ("large".to_string(), MemberReferenceFacts::default()),
    ]);
    let pooled = run(&freshness, facts, &DECLARED, &BTreeSet::new());
    // The mean of 1.0 and 0.0 would read 0.5.
    assert_eq!(pooled.internal_resolution.lower_bound, Some(0.01));
}

#[test]
fn an_unread_member_is_named_with_its_reason_and_lends_no_names() {
    let (mut freshness, mut facts) = app_and_lib();
    freshness[1] = ("lib", Err("the store could not be opened"));
    facts.remove("lib");
    // An unread member's types are not in the declared set either.
    let pooled = run(&freshness, facts, &["com.app.Main"], &BTreeSet::new());
    assert_eq!(
        pooled.members,
        ReferenceMembersRead {
            members: 2,
            read: 1,
            unread: vec!["lib".to_string()],
            unread_reasons: BTreeMap::from([("lib".to_string(), "the store could not be opened".to_string())]),
        }
    );
    // `lib`'s names and types were not read, so nothing of `app` re-reads.
    assert_eq!(pooled.cross_member, CrossMemberReferences::default());
    assert_eq!(pooled.internal_resolution.code_references, 12);
    assert!(pooled.summary.ends_with("summed over 1 of 2 members; 0 counted internal as another member's, 0 bound across members; not read: lib"), "{}", pooled.summary);
}

#[test]
fn a_status_read_without_its_facts_is_named_rather_than_summed() {
    let (freshness, mut facts) = app_and_lib();
    facts.remove("lib");
    let pooled = run(&freshness, facts, &DECLARED, &BTreeSet::new());
    assert_eq!(pooled.members.unread, ["lib"]);
    assert_eq!(pooled.members.unread_reasons["lib"], FACTS_NOT_READ);
    assert_eq!(pooled.internal_resolution.code_references, 12, "lib's 6 are not summed");
}

#[test]
fn no_member_read_or_no_internal_candidate_reads_absent_never_a_ratio() {
    let pooled = run(&[("app", Err("failed"))], BTreeMap::new(), &[], &BTreeSet::new());
    assert_eq!(pooled.internal_resolution.lower_bound, None);
    assert_eq!(pooled.internal_resolution.absence, Some(ReferenceFigureAbsence::NoReferencesRecorded));
    assert_eq!(pooled.summary, "no figure: no code reference is recorded, summed over 0 of 1 members; not read: app");

    let freshness: Vec<Row> = vec![("app", Ok(InternalResolution::measured(3, 0, 3)))];
    let facts = BTreeMap::from([(
        "app".to_string(),
        MemberReferenceFacts { named_nowhere: vec![calls("serde.to_string", 3)], ..Default::default() },
    )]);
    let pooled = run(&freshness, facts, &DECLARED, &BTreeSet::new());
    assert_eq!(
        pooled.internal_resolution.absence,
        Some(ReferenceFigureAbsence::NoInternalCandidates { provably_external: 3 })
    );
    assert_eq!(pooled.summary, "no figure: all 3 code references are provably external, summed over 1 of 1 members");
}
