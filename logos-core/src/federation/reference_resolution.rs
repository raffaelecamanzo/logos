//! The **workspace reference resolution** ([FR-RS-49], S-637, [CR-212] §3.2 E):
//! one figure over every member's code references, counting a reference into
//! another member as internal.
//!
//! Each member's own figure ([`InternalResolution`], on its `status` row) is
//! computed over its own graph, so a call into a type only a sibling declares
//! is **provably external** there: nothing that member declares could be what
//! it names. The workspace holds every member's names, and re-reads exactly
//! those rows:
//!
//! - a row the name test (i) classified external whose target a name **any
//!   member** declares could be — the union of the members' name universes,
//!   which is "another member's" for a row its own universe already refused —
//!   is an internal candidate ([`CrossMemberReferences::named_by_another_member`]);
//! - a row the binder classified `external-type` (ii) whose type another member
//!   declares is an internal candidate
//!   ([`CrossMemberReferences::type_in_another_member`]) — the same decision,
//!   by the same predicate ([`CallResidue::declared_elsewhere`]), that moves it
//!   to `type-in-another-member` in the member's call residue;
//! - an unbound invocation call site the coverage tier **bound across
//!   members** — a bridge edge's consumer end ([FR-WS-04]) — is resolved
//!   ([`CrossMemberReferences::bridge_bound`]). Such a row is never provably
//!   external (an invocation arm binds through its own matcher, not a name),
//!   so it is already an internal candidate. An asserted `[[links]]` edge is a
//!   claim, not evidence ([BR-75]), and resolves nothing here.
//!
//! # Pooled, never a mean ([BR-56])
//! The figure is computed from the members' populations **summed**, then
//! divided — never a mean of member ratios, which would weigh a ten-reference
//! member as heavily as a hundred-thousand-reference one. It is a
//! workspace-level figure under its own name, from workspace-level evidence.
//!
//! # Its members are stated ([NFR-CC-04])
//! A member whose `status` could not be read contributes nothing and is
//! **named** in [`ReferenceMembersRead::unread`] with its reason; the figure
//! states how many members it summed over. An unread member's names are not in
//! the union either, so a reference into it stays provably external — the
//! under-count a lower bound may make.
//!
//! [FR-RS-49]: ../../../docs/specs/requirements/FR-RS-49.md
//! [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
//! [CR-212]: ../../../docs/requests/CR-212-reference-resolution-is-stated-over-the-references-that-could-be-internal.md
//! [BR-56]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [BR-75]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::Serialize;

use crate::model::LogosSymbol;
use crate::models::{CallResidue, InternalResolution, ReferenceFigureAbsence};
use crate::resolve::reference_classes::{MemberReferenceFacts, NameUniverse};

use super::coverage::BoundSite;

/// The `reference_resolution` section of `workspace status` ([FR-RS-49],
/// S-637): the pooled figure, what the workspace counted differently from its
/// members, and the members it summed over.
///
/// [FR-RS-49]: ../../../docs/specs/requirements/FR-RS-49.md
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkspaceReferenceResolution {
    /// Resolved code references over internal candidates, summed over the
    /// members read — a lower bound, with its population and its absence, the
    /// shape of each member's own figure.
    pub internal_resolution: InternalResolution,
    /// The rows the workspace classified differently from their own member.
    pub cross_member: CrossMemberReferences,
    /// The members summed over, and the ones that could not be read.
    pub members: ReferenceMembersRead,
    /// The figure, its population and its members in one line, so no surface
    /// renders the ratio bare.
    pub summary: String,
}

/// What the workspace figure counts differently from the members' own
/// ([`WorkspaceReferenceResolution`]). Each is a count of ledger rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CrossMemberReferences {
    /// Rows provably external in their own member by the name test that a name
    /// another member declares could be: internal candidates here.
    pub named_by_another_member: u64,
    /// Rows their own member's binder proved `external-type` whose type
    /// another member declares: internal candidates here.
    pub type_in_another_member: u64,
    /// Invocation call sites unbound in their own member that the bridge bound
    /// to another member: resolved here.
    pub bridge_bound: u64,
}

/// The members the figure summed over ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReferenceMembersRead {
    /// The roster size.
    pub members: u64,
    /// Members whose reference populations were summed.
    pub read: u64,
    /// Roster members that could not be read, by name, in roster order.
    /// Absent when none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unread: Vec<String>,
    /// Why each [`unread`](Self::unread) member was not read. Exactly the
    /// unread members; absent when none.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub unread_reasons: BTreeMap<String, String>,
}

/// Why a member whose status was read has no reference facts — which the one
/// read producing both cannot do, and is stated rather than assumed.
const FACTS_NOT_READ: &str = "its reference classes were not read";

/// One roster member's status read, as [`pool`] takes it: its own figure, or
/// why its status was not read.
pub(crate) struct MemberRead<'a> {
    /// The member's name.
    pub(crate) member: &'a str,
    /// Its own `internal_resolution`, or the reason it has none.
    pub(crate) own: Result<&'a InternalResolution, &'a str>,
}

/// Pool the members' reference populations into the workspace figure.
///
/// `roster` is every member's status read, in roster order; `facts` the
/// reference facts read beside each status, by member; `declared` every
/// fully-qualified type name a member read declares; `bound_sites` the
/// invocation call sites the coverage tier bound across members.
pub(crate) fn pool(
    roster: &[MemberRead<'_>],
    mut facts: BTreeMap<String, MemberReferenceFacts>,
    declared: &HashSet<Vec<String>>,
    bound_sites: &BTreeSet<BoundSite>,
) -> WorkspaceReferenceResolution {
    let mut members = ReferenceMembersRead { members: roster.len() as u64, ..ReferenceMembersRead::default() };
    let mut read: Vec<(&str, &InternalResolution, MemberReferenceFacts)> = Vec::new();
    for row in roster {
        match (row.own, facts.remove(row.member)) {
            (Ok(own), Some(member_facts)) => read.push((row.member, own, member_facts)),
            (own, _) => {
                members.unread.push(row.member.to_string());
                members
                    .unread_reasons
                    .insert(row.member.to_string(), own.err().unwrap_or(FACTS_NOT_READ).to_string());
            }
        }
    }
    members.read = read.len() as u64;

    let mut union = NameUniverse::default();
    for (_, _, member_facts) in &mut read {
        union.absorb(std::mem::take(&mut member_facts.universe));
    }

    let (mut code_references, mut resolved, mut provably_external) = (0, 0, 0);
    let mut cross_member = CrossMemberReferences::default();
    for (member, own, member_facts) in &read {
        code_references += own.code_references;
        resolved += own.resolved;
        provably_external += own.provably_external;
        let read_again = reread(member, member_facts, &union, declared, bound_sites);
        cross_member.named_by_another_member += read_again.named_by_another_member;
        cross_member.type_in_another_member += read_again.type_in_another_member;
        cross_member.bridge_bound += read_again.bridge_bound;
    }
    let internal_resolution = InternalResolution::measured(
        code_references,
        resolved + cross_member.bridge_bound,
        // Each re-read row is one its own member counted external, so this
        // never saturates; saturating keeps a broken invariant from wrapping.
        provably_external
            .saturating_sub(cross_member.named_by_another_member + cross_member.type_in_another_member),
    );
    WorkspaceReferenceResolution {
        summary: summarize(&internal_resolution, &cross_member, &members),
        internal_resolution,
        cross_member,
        members,
    }
}

/// One member's provably external and unbound rows, re-read against the
/// workspace: the counts [`CrossMemberReferences`] sums.
fn reread(
    member: &str,
    facts: &MemberReferenceFacts,
    union: &NameUniverse,
    declared: &HashSet<Vec<String>>,
    bound_sites: &BTreeSet<BoundSite>,
) -> CrossMemberReferences {
    let named_by_another_member = facts
        .named_nowhere
        .iter()
        .filter(|group| union.could_be_internal(group.language.as_deref(), group.kind, &group.target))
        .map(|group| group.rows)
        .sum();
    let elsewhere = |fqn: &[String]| declared.contains(fqn);
    let type_in_another_member = facts
        .external_type
        .iter()
        .flat_map(|(_, by_candidates)| by_candidates)
        .filter(|(candidates, _)| CallResidue::declared_elsewhere(candidates, &elsewhere))
        .map(|(_, rows)| rows)
        .sum();
    // The coverage tier keys a site on the parsed symbol, so the ledger's text
    // is read the same way; one that does not parse names no endpoint, and the
    // bridge skipped it too.
    let bridge_bound = facts
        .unbound_invocations
        .iter()
        .filter(|site| {
            LogosSymbol::parse(&site.source_symbol).is_ok_and(|symbol| {
                bound_sites.contains(&(member.to_string(), symbol.as_str().to_string(), site.target.clone()))
            })
        })
        .map(|site| site.rows)
        .sum();
    CrossMemberReferences { named_by_another_member, type_in_another_member, bridge_bound }
}

/// The [`WorkspaceReferenceResolution::summary`] line.
fn summarize(figure: &InternalResolution, cross: &CrossMemberReferences, members: &ReferenceMembersRead) -> String {
    let over = format!("summed over {} of {} members", members.read, members.members);
    let mut line = match (figure.lower_bound, figure.absence) {
        (Some(ratio), _) => format!(
            "lower bound {ratio:.3} ({} of {} internal candidates resolved; {} code references, {} provably \
             external), {over}; {} counted internal as another member's, {} bound across members",
            figure.resolved,
            figure.internal_candidates,
            figure.code_references,
            figure.provably_external,
            cross.named_by_another_member + cross.type_in_another_member,
            cross.bridge_bound,
        ),
        (None, Some(ReferenceFigureAbsence::NoInternalCandidates { provably_external })) => format!(
            "no figure: all {provably_external} code references are provably external, {over}"
        ),
        (None, _) => format!("no figure: no code reference is recorded, {over}"),
    };
    if !members.unread.is_empty() {
        line.push_str(&format!("; not read: {}", members.unread.join(", ")));
    }
    line
}

#[cfg(test)]
#[path = "reference_resolution_tests.rs"]
mod tests;
