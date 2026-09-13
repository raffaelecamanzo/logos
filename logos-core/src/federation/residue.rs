//! The **unresolved egress residue** a cross-service reachability answer carries
//! (S-401, [CR-125], [FR-WS-05], [BR-53]).
//!
//! [`cross_service_coverage`](super::coverage::cross_service_coverage) already
//! classifies every cross-boundary reference, and `workspace status` already
//! reports the classification. None of it reached the per-query surface: an
//! `xservice callers` answer with no cross-service row rendered exactly like one
//! over an estate whose outbound calls were all unresolvable, and the first is
//! the reading a developer acts on ([CR-125] §2).
//!
//! # What the residue is
//! The **invocation**-intake references — captured outbound call sites
//! ([FR-WS-08]–[FR-WS-10]) — that produced no edge. Its denominator is the
//! `bound + ambiguous + unbound` population
//! [`egress_resolution`](super::coverage::CrossServiceCoverage::egress_resolution)
//! is computed over, so a residue read beside a `workspace status` reconciles
//! against it arithmetically rather than by eye, and
//! `unresolved_sites == measured_sites - bound` by construction.
//!
//! `no-provider-in-workspace` references are bucketed **separately** and are not
//! in the residue count, exactly as [FR-WS-05] already excludes them from every
//! ratio: a call to something this workspace does not contain is outside the
//! boundary, not an unresolved site inside it.
//!
//! # One assembly point ([CR-125] §4.4)
//! [`egress_residue`] is the only producer, and every surface — MCP, CLI and the
//! web API — reaches it through [`super::query`]'s read-models, so the two
//! renderings cannot disagree about a figure neither of them computes. It is the
//! same discipline [ADR-52] applies to the bind/refuse classifier, applied to the
//! refusal's *report*.
//!
//! # Advisory only, never a gate input ([ADR-53], [FR-WS-05])
//! Reachable only through an [`EngineRegistry`](super::registry::EngineRegistry),
//! which exists only under a workspace manifest. Nothing here can move a member's
//! gated signal, and no gate reads it.
//!
//! [BR-53]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [CR-125]: ../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
//! [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
//! [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
//! [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
//! [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md

use std::collections::BTreeMap;

use serde::Serialize;

use super::bridge::{BridgeIntake, MemberContracts};
use super::coverage::{cross_service_coverage, CoverageState, ReferenceCoverage, UnboundReason};
use super::registry::{AnswerScope, MemberEngine};

/// One refusal reason with the number of unresolved egress sites carrying it
/// ([FR-WS-05]).
///
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResidueReason {
    /// The reason, in the [FR-WS-05] wire vocabulary (`base-url-runtime`, …).
    ///
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    pub reason: UnboundReason,
    /// Captured outbound sites in scope that did not resolve for this reason.
    pub sites: u64,
}

/// One member's unresolved egress, repo-qualified ([FR-WS-03]).
///
/// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberEgressResidue {
    /// The member the captured sites belong to (its workspace-relative name).
    pub member: String,
    /// Captured outbound sites — `bound + ambiguous + unbound`, the denominator
    /// [`egress_resolution`](super::coverage::CrossServiceCoverage::egress_resolution)
    /// is taken over.
    pub measured_sites: u64,
    /// Of those, the ones that produced no edge.
    pub unresolved_sites: u64,
    /// Sites whose provider is not in this workspace — bucketed apart, never
    /// folded into [`unresolved_sites`](Self::unresolved_sites) ([FR-WS-05]).
    ///
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    pub no_provider_in_workspace: u64,
    /// [`unresolved_sites`](Self::unresolved_sites) split by reason; the entries
    /// sum to it exactly.
    pub by_reason: Vec<ResidueReason>,
}

/// The workspace's unresolved egress, per member — **assembled once**
/// ([CR-125] §4.4).
///
/// Not itself a rendered read-model: a query renders
/// [`EgressResidue`](EgressResidue), the projection of this onto the members that
/// query's answer was computed over ([`beside`](Self::beside)).
///
/// [CR-125]: ../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
/// Deliberately **not** `Default`, for the reason
/// [`CrossServiceCoverage`](super::coverage::CrossServiceCoverage) gives for the
/// same field: a derived `covers_all_members: false` is a *lie* over the empty
/// workspace a defaulted value describes. It would also be a second public
/// constructor beside [`egress_residue`], and a surface handed a defaulted value
/// would silently render no residue at all with nothing failing to compile
/// ([CR-125] §4.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEgressResidue {
    /// One row per member that contributed a captured outbound site, in the
    /// member order [`cross_service_coverage`] produced.
    pub members: Vec<MemberEgressResidue>,
    /// Whether every roster member contributed to the classification behind these
    /// rows ([FR-WS-16], [NFR-CC-04]) — carried through so a residue computed over
    /// a partly-degraded workspace never reads as a whole one.
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub covers_all_members: bool,
}

/// The unresolved egress residue **in one answer's scope**, as a reachability
/// answer renders it ([FR-WS-05], [BR-53]).
///
/// Serialized by every surface alike — the MCP tool result, the CLI's human and
/// `--json` renderings, and the web API — because it is a field of the answer,
/// not something a surface composes ([CR-125] §4.4).
///
/// [BR-53]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [CR-125]: ../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EgressResidue {
    /// The `--repo` member this residue was scoped to, when the query named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// How many members contributed captured egress to this residue.
    ///
    /// **The spread of the unresolved sites, never the workspace's roster size.**
    /// Counts exactly the members contributing at least one unresolved site, so
    /// it is `≤` the number of members the query fanned across and reconciles
    /// with the `across N members` the composed line renders. A member that made
    /// no captured outbound call, that resolved all of it, or whose only rows are
    /// out-of-workspace contributes nothing here. Stated because the readings are
    /// easy to conflate, and a residue implying a roster size — or claiming one
    /// site is spread across two members — is the over-read this whole block
    /// exists to prevent ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub members_in_scope: u64,
    /// Captured outbound sites in scope — the denominator.
    pub measured_sites: u64,
    /// Of those, the ones that produced no edge. Never zero: a zero residue is
    /// reported by **omitting** this block, so an answer over a fully-resolved
    /// scope renders exactly as it did before [CR-125].
    ///
    /// [CR-125]: ../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
    pub unresolved_sites: u64,
    /// Sites in scope whose provider is not in this workspace — reported beside
    /// the residue, never inside it ([FR-WS-05]).
    ///
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    pub no_provider_in_workspace: u64,
    /// The per-reason breakdown, most sites first then reason ascending
    /// ([NFR-RA-06]). Sums to [`unresolved_sites`](Self::unresolved_sites).
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub by_reason: Vec<ResidueReason>,
    /// Whether the classification behind this residue covered every roster member
    /// ([FR-WS-16]).
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    pub covers_all_members: bool,
    /// The answer and its residue **as one line** — the structural form of
    /// [BR-53], modelled on
    /// [`resolved_edges_summary`](super::coverage::CrossServiceCoverage::resolved_edges_summary)
    /// ([CR-111] §4.4): one composed field that the human and the `--json`
    /// rendering both serialize, so neither can regress while the other stays
    /// honest.
    ///
    /// e.g. `"no resolved cross-service callers; 141 of 146 captured outbound
    /// sites in scope did not resolve across 84 members (base-url-runtime 120,
    /// ambiguous 21); 31 more have no provider in this workspace"`.
    ///
    /// [BR-53]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [CR-111]: ../../../docs/requests/CR-111-bound-ratio-carries-its-denominator.md
    pub summary: String,
}

/// What a reachability answer resolved, for the one line [`EgressResidue::summary`]
/// composes ([BR-53]).
///
/// A count and the noun it counts, taken from the answer itself rather than
/// inferred — `xservice callers` resolves *cross-service callers*, `xservice
/// impact` resolves *cross-service impacts*, and an answer that resolved none of
/// them is the case the residue exists to stop reading as an absence.
///
/// [BR-53]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnswerReach {
    /// How many cross-service rows the answer resolved. **Workspace-wide**: the
    /// cross-service tier matches on the queried symbol alone and `--repo` does
    /// not narrow it, which is why [`EgressResidue::compose`] says so whenever a
    /// scope is in force.
    pub resolved: usize,
    /// What one of those rows is, singular — the composed line pluralises it
    /// itself, because it reads "1 resolved cross-service caller" at arity one.
    pub noun: &'static str,
}

impl AnswerReach {
    /// The answer's own half of the composed line.
    ///
    /// `scoped` qualifies the count as **workspace-wide**, which it always is:
    /// `--repo` narrows the answer's per-member fan-out and the residue, but not
    /// the cross-service tier this counts. Saying nothing would leave the line
    /// joining a workspace-wide numerator to a member-scoped residue with no
    /// sign that the two halves cover different populations ([NFR-CC-04]).
    fn phrase(self, scoped: bool) -> String {
        let where_ = if scoped { " workspace-wide" } else { "" };
        match self.resolved {
            0 => format!("no resolved {}s{where_}", self.noun),
            1 => format!("1 resolved {}{where_}", self.noun),
            n => format!("{n} resolved {}s{where_}", self.noun),
        }
    }
}

impl WorkspaceEgressResidue {
    /// Project this onto one answer's scope, or `None` when that scope's residue
    /// is **zero** ([FR-WS-05]).
    ///
    /// `None` is the whole of the zero case: the caller's field is skipped when
    /// absent, so an answer over a fully-resolved scope serializes byte-for-byte
    /// as it did before [CR-125]. Silence is honest exactly once, and this is it.
    ///
    /// # The scope rule ([FR-WS-05])
    /// `repo = Some(member)` reports that member's residue alone — the same
    /// narrowing `--repo` applies to the answer's own fan-out. `repo = None`
    /// reports every member the query fanned across. The residue therefore covers
    /// exactly the members the answer was computed over: never wider (it would
    /// report sites the answer never considered) and never narrower (that is the
    /// silence this criterion removes).
    ///
    /// A member in scope that contributes no **unresolved** site — because it
    /// made no captured outbound call, resolved all of it, or holds only
    /// out-of-workspace rows — is absent from
    /// [`members_in_scope`](EgressResidue::members_in_scope), which counts the
    /// spread of the sites rather than the roster.
    ///
    /// [CR-125]: ../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    pub fn beside(&self, repo: Option<&str>, reach: AnswerReach) -> Option<EgressResidue> {
        let rows: Vec<&MemberEgressResidue> = self
            .members
            .iter()
            .filter(|row| repo.is_none_or(|member| row.member == member))
            .collect();

        let unresolved_sites: u64 = rows.iter().map(|r| r.unresolved_sites).sum();
        if unresolved_sites == 0 {
            return None;
        }
        let measured_sites: u64 = rows.iter().map(|r| r.measured_sites).sum();
        let no_provider_in_workspace: u64 = rows.iter().map(|r| r.no_provider_in_workspace).sum();

        let mut totals: BTreeMap<UnboundReason, u64> = BTreeMap::new();
        for entry in rows.iter().flat_map(|r| r.by_reason.iter()) {
            *totals.entry(entry.reason).or_default() += entry.sites;
        }
        let by_reason = rank_reasons(totals);

        let residue = EgressResidue {
            scope: repo.map(str::to_string),
            // The members the unresolved sites are actually spread across — NOT
            // every member with captured egress. A member that resolved all of
            // its egress, or whose only rows are out-of-workspace, contributed
            // nothing to `unresolved_sites` and must not be counted in the span
            // the composed line reports them "across".
            members_in_scope: rows.iter().filter(|r| r.unresolved_sites > 0).count() as u64,
            measured_sites,
            unresolved_sites,
            no_provider_in_workspace,
            by_reason,
            covers_all_members: self.covers_all_members,
            summary: String::new(),
        };
        Some(EgressResidue {
            summary: residue.compose(reach),
            ..residue
        })
    }
}

impl EgressResidue {
    /// Compose [BR-53]'s one line: what the answer resolved, then what it could
    /// not reach, then the separately-bucketed out-of-workspace remainder.
    ///
    /// [BR-53]: ../../../docs/specs/software-spec.md#327-workspace-federation
    fn compose(&self, reach: AnswerReach) -> String {
        let reasons: Vec<String> = self
            .by_reason
            .iter()
            .map(|entry| format!("{} {}", entry.reason.as_str(), entry.sites))
            .collect();
        // The residue's own half names the members it covers, because `--repo`
        // narrows it and not the resolved count it sits beside.
        let scope = match self.scope.as_deref() {
            Some(member) => format!("in {member}"),
            None => "in scope".to_string(),
        };
        let mut line = format!(
            "{}; {} of {} did not resolve across {} ({})",
            reach.phrase(self.scope.is_some()),
            self.unresolved_sites,
            plural(self.measured_sites, &format!("captured outbound site {scope}")),
            plural(self.members_in_scope, "member"),
            reasons.join(", "),
        );
        if self.no_provider_in_workspace > 0 {
            let has = if self.no_provider_in_workspace == 1 { "has" } else { "have" };
            line.push_str(&format!(
                "; {} more {has} no provider in this workspace",
                self.no_provider_in_workspace
            ));
        }
        if !self.covers_all_members {
            line.push_str("; computed over fewer than all workspace members");
        }
        line
    }
}

/// `n member` / `n members` — the composed line reads as English at both
/// arities, including when the noun carries a trailing qualifier
/// (`"captured outbound site in api"` → `"2 captured outbound sites in api"`),
/// where the plural `s` belongs on the head word and not at the end.
fn plural(n: u64, noun: &str) -> String {
    if n == 1 {
        return format!("{n} {noun}");
    }
    match noun.split_once(' ') {
        // Multi-word: the head is everything up to the qualifier. The nouns this
        // composes are all `<adjectives> <head> <qualifier>` with the head last
        // before the qualifier, so split on the qualifier's preposition.
        Some(_) => match noun.rsplit_once(" in ") {
            Some((head, qualifier)) => format!("{n} {head}s in {qualifier}"),
            None => format!("{n} {noun}s"),
        },
        None => format!("{n} {noun}s"),
    }
}

/// Order a reason tally most-sites-first, ties broken by the wire token
/// ([NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn rank_reasons(totals: BTreeMap<UnboundReason, u64>) -> Vec<ResidueReason> {
    let mut out: Vec<ResidueReason> = totals
        .into_iter()
        .map(|(reason, sites)| ResidueReason { reason, sites })
        .collect();
    out.sort_by(|a, b| b.sites.cmp(&a.sites).then_with(|| a.reason.as_str().cmp(b.reason.as_str())));
    out
}

/// Project a classified [`CrossServiceCoverage`] onto its per-member egress
/// residue ([CR-125]).
///
/// Pure, so the residue is testable without a workspace — and, more to the point,
/// it is a projection of the **same classification** the bridge's edges come from
/// ([ADR-52]'s one classifier), which is what stops the answer and its residue
/// drifting apart.
///
/// Only [`BridgeIntake::Invocation`] rows are egress: a contract-surface row is a
/// *declared* endpoint, not an outbound call, and counting declarations here would
/// reproduce the pooled numerator [CR-120] retired.
///
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
/// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
/// [CR-125]: ../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
fn residue_from(
    references: &[ReferenceCoverage],
    covers_all_members: bool,
) -> WorkspaceEgressResidue {
    // Insertion-ordered per member so the rows keep `cross_service_coverage`'s own
    // member order; the per-reason tallies inside each row are ordered by
    // `rank_reasons`.
    let mut rows: Vec<MemberEgressResidue> = Vec::new();
    let mut tallies: Vec<BTreeMap<UnboundReason, u64>> = Vec::new();

    for reference in references {
        if reference.intake != BridgeIntake::Invocation {
            continue;
        }
        let at = match rows.iter().position(|row| row.member == reference.from.member) {
            Some(at) => at,
            None => {
                rows.push(MemberEgressResidue {
                    member: reference.from.member.clone(),
                    measured_sites: 0,
                    unresolved_sites: 0,
                    no_provider_in_workspace: 0,
                    by_reason: Vec::new(),
                });
                tallies.push(BTreeMap::new());
                rows.len() - 1
            }
        };
        match reference.state {
            // Bound: no residue, but it is in the denominator — the rate's
            // numerator and the residue are the two halves of one population.
            CoverageState::Bound => rows[at].measured_sites += 1,
            // Outside the workspace boundary: bucketed apart from both, exactly as
            // FR-WS-05 excludes it from every ratio.
            CoverageState::Unbound {
                reason: UnboundReason::NoProviderInWorkspace,
            } => rows[at].no_provider_in_workspace += 1,
            CoverageState::Unbound { reason } => {
                rows[at].measured_sites += 1;
                rows[at].unresolved_sites += 1;
                *tallies[at].entry(reason).or_default() += 1;
            }
        }
    }

    for (row, tally) in rows.iter_mut().zip(tallies) {
        row.by_reason = rank_reasons(tally);
    }
    WorkspaceEgressResidue {
        members: rows,
        covers_all_members,
    }
}

/// The workspace's unresolved egress residue, assembled once per answer
/// ([CR-125], [FR-WS-05]).
///
/// Takes the [`AnswerScope`] rather than the registry for the reason every
/// workspace read-model does: a member that will not open is attempted and
/// announced **once** across the walks of one answer ([FR-WS-16], [NFR-PE-10]).
///
/// [CR-125]: ../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
pub fn egress_residue<E>(answer: &AnswerScope<'_, E>) -> WorkspaceEgressResidue
where
    E: MemberEngine + MemberContracts,
{
    let coverage = cross_service_coverage(answer);
    residue_from(&coverage.references, coverage.covers_all_members)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::federation::bridge::BridgeEndpoint;
    use crate::model::LogosSymbol;
    use crate::resolve::binding::Provenance;

    /// The two nouns the reachability verbs answer with ([BR-53]).
    const CALLERS: AnswerReach = AnswerReach {
        resolved: 0,
        noun: "cross-service caller",
    };

    fn reference(
        member: &str,
        intake: BridgeIntake,
        state: CoverageState,
    ) -> ReferenceCoverage {
        ReferenceCoverage {
            relation: "route".to_string(),
            from: BridgeEndpoint {
                member: member.to_string(),
                // The smallest valid SCIP symbol the federation fixtures use.
                symbol: LogosSymbol::parse(&format!("local {member}_call")).unwrap(),
            },
            bucket: state.bucket(),
            state,
            to: None,
            intake,
            candidates: None,
            provenance: Provenance::Literal,
        }
    }

    fn unbound(member: &str, reason: UnboundReason) -> ReferenceCoverage {
        reference(
            member,
            BridgeIntake::Invocation,
            CoverageState::Unbound { reason },
        )
    }

    fn bound(member: &str) -> ReferenceCoverage {
        reference(member, BridgeIntake::Invocation, CoverageState::Bound)
    }

    /// **The defect [CR-125] is filed for.** An answer that resolved nothing over
    /// a non-zero residue renders as *unresolved, naming the count* — never as a
    /// bare empty set, which is the reading a developer acts on.
    ///
    /// [CR-125]: ../../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
    #[test]
    fn an_empty_answer_over_a_non_zero_residue_names_the_count_and_its_reasons() {
        let refs = [
            unbound("api", UnboundReason::BaseUrlRuntime),
            unbound("api", UnboundReason::BaseUrlRuntime),
            unbound("web", UnboundReason::Ambiguous),
        ];
        let residue = residue_from(&refs, true)
            .beside(None, CALLERS)
            .expect("a non-zero residue is reported");

        assert_eq!(residue.unresolved_sites, 3);
        assert_eq!(residue.measured_sites, 3);
        assert_eq!(residue.members_in_scope, 2);
        assert_eq!(
            residue.summary,
            "no resolved cross-service callers; 3 of 3 captured outbound sites in \
             scope did not resolve across 2 members (base-url-runtime 2, ambiguous 1)",
            "the count and every reason behind it are in the one composed line (BR-53)"
        );
    }

    /// The zero case is the **one** case in which silence is honest ([BR-53]
    /// exception): nothing is added, so the answer serializes exactly as it did
    /// before [CR-125].
    ///
    /// [CR-125]: ../../../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
    #[test]
    fn a_zero_residue_adds_nothing_to_the_answer() {
        let refs = [bound("api"), bound("web")];
        assert!(
            residue_from(&refs, true).beside(None, CALLERS).is_none(),
            "every captured site resolved, so the answer stands unqualified"
        );
        assert!(
            residue_from(&[], true).beside(None, CALLERS).is_none(),
            "no captured egress at all is likewise no residue"
        );
    }

    /// A resolved answer still carries its residue, and says what it resolved —
    /// the criterion is about the residue, not about emptiness ([FR-WS-05]).
    #[test]
    fn a_resolved_answer_still_carries_its_residue() {
        let refs = [bound("api"), unbound("api", UnboundReason::PathNotComposed)];
        let residue = residue_from(&refs, true)
            .beside(
                None,
                AnswerReach {
                    resolved: 1,
                    noun: "cross-service callers",
                },
            )
            .expect("one unresolved site is a residue");
        assert!(
            residue.summary.starts_with("1 resolved cross-service callers; 1 of 2 "),
            "got {:?}",
            residue.summary
        );
    }

    /// The residue's denominator **is** the egress-resolution denominator
    /// (`bound + ambiguous + unbound`), so `unresolved == measured - bound` and a
    /// reader reconciles the two figures arithmetically rather than by eye.
    #[test]
    fn the_residue_reconciles_against_the_egress_resolution_denominator() {
        let refs = [
            bound("api"),
            bound("api"),
            unbound("api", UnboundReason::BaseUrlRuntime),
            unbound("api", UnboundReason::Ambiguous),
            // Bucketed apart: not in either half of the rate's population.
            unbound("api", UnboundReason::NoProviderInWorkspace),
        ];
        let residue = residue_from(&refs, true).beside(None, CALLERS).unwrap();

        assert_eq!(residue.measured_sites, 4, "bound + ambiguous + unbound");
        assert_eq!(residue.unresolved_sites, 2, "measured minus the 2 bound");
        assert_eq!(residue.no_provider_in_workspace, 1);
        assert_eq!(
            residue.by_reason.iter().map(|r| r.sites).sum::<u64>(),
            residue.unresolved_sites,
            "the breakdown sums to the count it breaks down"
        );
        assert!(
            !residue
                .by_reason
                .iter()
                .any(|r| r.reason == UnboundReason::NoProviderInWorkspace),
            "an out-of-workspace provider is not an unresolved site inside it"
        );
        assert!(
            residue.summary.ends_with("; 1 more has no provider in this workspace"),
            "the separately-bucketed remainder is still reported: {:?}",
            residue.summary
        );
    }

    /// **`members_in_scope` is the spread of the unresolved sites, and a member
    /// that resolved its egress is not part of that spread** ([NFR-CC-04]).
    ///
    /// The reading this pins out is a line that contradicts itself on its face —
    /// *"1 of 3 … did not resolve across 2 members"* — one site cannot be spread
    /// across two. Both shapes that produce it are here: a member whose egress is
    /// entirely bound, and a member whose only rows are out-of-workspace (which
    /// are excluded from the residue's own denominator, so such a member
    /// contributes to neither half).
    #[test]
    fn only_members_carrying_an_unresolved_site_are_in_the_spread() {
        let refs = [
            bound("api"),
            unbound("api", UnboundReason::BaseUrlRuntime),
            // `web` captured egress and resolved all of it.
            bound("web"),
            // `ext` calls only things this workspace does not contain.
            unbound("ext", UnboundReason::NoProviderInWorkspace),
            unbound("ext", UnboundReason::NoProviderInWorkspace),
        ];
        let residue = residue_from(&refs, true).beside(None, CALLERS).unwrap();

        assert_eq!(residue.unresolved_sites, 1);
        assert_eq!(residue.measured_sites, 3, "bound api + unbound api + bound web");
        assert_eq!(residue.no_provider_in_workspace, 2);
        assert_eq!(
            residue.members_in_scope, 1,
            "only `api` carries the unresolved site: {residue:?}"
        );
        assert_eq!(
            residue.summary,
            "no resolved cross-service callers; 1 of 3 captured outbound sites in \
             scope did not resolve across 1 member (base-url-runtime 1); 2 more \
             have no provider in this workspace",
            "the span in the line is the span of the sites"
        );
    }

    /// The scope rule ([FR-WS-05]): `--repo` reports that member's residue alone,
    /// the same narrowing it applies to the answer's own fan-out.
    #[test]
    fn repo_scopes_the_residue_to_that_members_egress() {
        let refs = [
            unbound("api", UnboundReason::BaseUrlRuntime),
            unbound("web", UnboundReason::Ambiguous),
            unbound("web", UnboundReason::Ambiguous),
        ];
        let workspace = residue_from(&refs, true);

        let scoped = workspace.beside(Some("web"), CALLERS).unwrap();
        assert_eq!(scoped.scope.as_deref(), Some("web"));
        assert_eq!(scoped.members_in_scope, 1);
        assert_eq!(scoped.unresolved_sites, 2);
        assert_eq!(
            scoped.by_reason,
            [ResidueReason {
                reason: UnboundReason::Ambiguous,
                sites: 2
            }],
            "`api`'s base-url-runtime site is outside this answer's scope"
        );

        assert_eq!(
            workspace.beside(None, CALLERS).unwrap().unresolved_sites,
            3,
            "unscoped, every member the query fanned across is in scope"
        );
        assert!(
            workspace.beside(Some("absent"), CALLERS).is_none(),
            "a member with no captured egress has no residue to report"
        );
    }

    /// The scope filter is **exact member equality**, probed with its near miss:
    /// a workspace routinely holds `api` beside `api-gateway`, and a prefix or
    /// substring match there would report a neighbour's egress under the scoped
    /// member's name — a residue attributed to the wrong service, which is worse
    /// than no residue at all ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn the_scope_filter_does_not_admit_a_member_whose_name_merely_contains_it() {
        let refs = [
            unbound("api", UnboundReason::BaseUrlRuntime),
            unbound("api-gateway", UnboundReason::Ambiguous),
            unbound("legacy-api", UnboundReason::PathNotComposed),
        ];
        let workspace = residue_from(&refs, true);

        let scoped = workspace.beside(Some("api"), CALLERS).unwrap();
        assert_eq!(
            scoped.members_in_scope, 1,
            "`api-gateway` and `legacy-api` are different members, not this one"
        );
        assert_eq!(
            scoped.by_reason,
            [ResidueReason {
                reason: UnboundReason::BaseUrlRuntime,
                sites: 1
            }],
            "only `api`'s own site: {scoped:?}"
        );
        assert_eq!(
            workspace.beside(Some("api-gateway"), CALLERS).unwrap().by_reason,
            [ResidueReason {
                reason: UnboundReason::Ambiguous,
                sites: 1
            }],
            "and the neighbour scopes to its own, not to `api`'s"
        );
    }

    /// **Under `--repo` the line names which population each half covers**
    /// ([NFR-CC-04]).
    ///
    /// `--repo` narrows the answer's per-member fan-out and the residue, but
    /// **not** the cross-service tier, which matches on the queried symbol alone.
    /// So the resolved count beside a scoped residue is workspace-wide, and a
    /// line that said only "no resolved cross-service callers; 1 of 1 captured
    /// outbound sites **in scope**" would join two populations with nothing to
    /// tell them apart — the same silent conflation this whole block exists to
    /// remove, one level down.
    #[test]
    fn a_scoped_residue_says_which_population_each_half_of_the_line_covers() {
        let refs = [
            unbound("api", UnboundReason::BaseUrlRuntime),
            unbound("web", UnboundReason::Ambiguous),
        ];
        let scoped = residue_from(&refs, true)
            .beside(Some("api"), CALLERS)
            .unwrap();
        assert_eq!(
            scoped.summary,
            "no resolved cross-service callers workspace-wide; 1 of 1 captured \
             outbound site in api did not resolve across 1 member \
             (base-url-runtime 1)",
            "the resolved half is workspace-wide; the residue half names `api`"
        );

        // Unscoped, there is one population and the line says `in scope`.
        let unscoped = residue_from(&refs, true).beside(None, CALLERS).unwrap();
        assert_eq!(
            unscoped.summary,
            "no resolved cross-service callers; 2 of 2 captured outbound sites in \
             scope did not resolve across 2 members (ambiguous 1, base-url-runtime 1)",
            "unscoped there is one population, and the line says `in scope`"
        );
    }

    /// A **contract-surface** row is a declared endpoint, not an outbound call.
    /// Counting declarations as egress would rebuild the pooled numerator
    /// [CR-120] retired.
    ///
    /// [CR-120]: ../../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
    #[test]
    fn a_contract_surface_reference_is_not_egress() {
        let refs = [
            reference(
                "api",
                BridgeIntake::ContractSurface,
                CoverageState::Unbound {
                    reason: UnboundReason::PathNotComposed,
                },
            ),
            reference("api", BridgeIntake::ContractSurface, CoverageState::Bound),
        ];
        assert!(
            residue_from(&refs, true).beside(None, CALLERS).is_none(),
            "a declaration is not a call site"
        );
        assert!(
            residue_from(&refs, true).members.is_empty(),
            "and contributes no member row at all"
        );
    }

    /// Most sites first, ties broken by the wire token — deterministic output
    /// ([NFR-RA-06]).
    #[test]
    fn the_breakdown_is_ranked_most_sites_first_then_by_token() {
        let refs = [
            unbound("api", UnboundReason::TopicNotLiteral),
            unbound("api", UnboundReason::PathNotComposed),
            unbound("api", UnboundReason::PathNotComposed),
            unbound("api", UnboundReason::Ambiguous),
        ];
        let residue = residue_from(&refs, true).beside(None, CALLERS).unwrap();
        assert_eq!(
            residue
                .by_reason
                .iter()
                .map(|r| (r.reason.as_str(), r.sites))
                .collect::<Vec<_>>(),
            [
                ("path-not-composed", 2),
                ("ambiguous", 1),
                ("topic-not-literal", 1)
            ],
            "2 first; the two 1s in token order"
        );
    }

    /// A residue computed over a partly-degraded workspace never reads as one
    /// computed over the whole of it ([FR-WS-16], [NFR-CC-04]).
    #[test]
    fn a_partly_read_workspace_says_so_in_the_composed_line() {
        let refs = [unbound("api", UnboundReason::BaseUrlRuntime)];
        let residue = residue_from(&refs, false).beside(None, CALLERS).unwrap();
        assert!(!residue.covers_all_members);
        assert!(
            residue
                .summary
                .ends_with("; computed over fewer than all workspace members"),
            "got {:?}",
            residue.summary
        );
    }

    /// Every count slot reads as English at arity one — `1 member`, `1 captured
    /// outbound site`, and (below) `1 resolved cross-service caller` / `1 more
    /// has`. The line is read by humans, and three of the four slots were
    /// hard-plural before the Sprint 69 review.
    #[test]
    fn the_composed_line_reads_as_english_at_one_member() {
        let refs = [unbound("api", UnboundReason::BaseUrlRuntime)];
        let residue = residue_from(&refs, true).beside(None, CALLERS).unwrap();
        assert_eq!(
            residue.summary,
            "no resolved cross-service callers; 1 of 1 captured outbound site in \
             scope did not resolve across 1 member (base-url-runtime 1)"
        );
    }
}
