//! The user-facing **warm-state vocabulary** `workspace status` labels each
//! member with, and its derivation ([FR-WS-15], [BR-44], [NFR-CC-04]).
//!
//! [`warm`](super::warm) owns the bounded warm itself; this module owns what a
//! *reader* is told about it afterwards. The two are deliberately separate
//! values: [`WarmSummary`](super::warm::WarmSummary) is an in-process readout of
//! one supervisor pass, keyed on absolute member roots and never serialized,
//! whereas [`MemberWarmState`] is a wire label keyed on the workspace-relative
//! [`Member::name`](super::Member::name) every federation read-model joins on.
//!
//! # Why the derivation takes evidence rather than gathering it
//! Only one of the four states is derivable from the store alone. `warm` is
//! "this member's graph holds at least one indexed file"
//! ([`StatusInfo::indexed`](crate::models::StatusInfo::indexed)) and `deferred`
//! is its complement; but *being indexed right now* and *was attempted and
//! failed* are facts about a **process**, not about a store, and no such fact is
//! recorded durably today — the bounded supervisor reports a failed member on a
//! stderr that the real detached spawn sends to `/dev/null`
//! (`cli::workspace_init::run_supervisor`).
//!
//! So the derivation takes those two facts as an explicit [`WarmEvidence`]
//! input, and today's only caller passes [`WarmEvidence::none`]:
//!
//! - **`warming` is omitted, never inferred** ([NFR-CC-04]). With no in-flight
//!   signal the roll-up's `warming` key is *absent from the JSON* rather than a
//!   fabricated `0`, and [`MemberWarmState::Warming`] is unreachable — not by a
//!   convention a later edit can forget, but because nothing short of
//!   [`WarmEvidence::with_in_flight`] can put a member in the in-flight set,
//!   and no caller calls it yet.
//! - **A failed member is `degraded`, never `deferred`** ([BR-44]). Two failure
//!   channels reach here durably, and both map to `degraded`: a member whose
//!   engine could not be **started**, and a member that started but whose
//!   **freshness read** failed. The second one takes care — [`Engine::status`](crate::Engine::status)
//!   is infallible and degrades to a *defaulted* [`StatusInfo`](crate::models::StatusInfo) whose
//!   `indexed: false` is indistinguishable from an honestly empty graph — so
//!   [`workspace_status`](super::query::workspace_status) fans
//!   [`Engine::try_status`](crate::Engine::try_status) and folds its `Err` into the per-member error
//!   channel instead.
//!
//!   What is **not** yet distinguishable is a warm that was attempted and
//!   failed while the member's store still opens and reads cleanly: that is
//!   indistinguishable from a member the queue never reached, and
//!   [`WarmEvidence::with_failures`] is the seam that closes the gap the moment
//!   a durable per-member warm-failure record exists. Adding it is a change of
//!   *input*, not of this module's shape or of the read-model's wire format.
//!
//! # One residual gap, stated rather than papered over
//! `warm` is index **presence**, per [FR-WS-15]. A member the warm reached and
//! indexed successfully but which contains no supported-language files holds no
//! indexed file, so it reports `deferred` — "never attempted" — permanently. The
//! requirement itself equates `warm` with index presence, so this is not fixed
//! here; naming it is the honest alternative to a heuristic that would guess at
//! *why* a graph is empty ([NFR-CC-04]). It belongs to the same human decision
//! as the warm-failure producer above.
//!
//! # No engines are constructed to answer
//! Every function here is pure, over values a `workspace status` walk has
//! already gathered ([NFR-PE-10]): labelling N members costs zero engine
//! starts, zero connections, and zero filesystem reads, so the read-model's
//! resident-engine ceiling is exactly what the fan-out it rides on already
//! paid ([NFR-PE-11]).
//!
//! [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
//! [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
//! [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

/// One member's warm state as `workspace status` reports it ([FR-WS-15]).
///
/// Serialized **internally tagged** on `warm_state`, so a member row reads
/// `{"member": "api", …, "warm_state": "warm"}` and a degraded one carries its
/// reason alongside the label — the same shape
/// [`MemberOutcome`](super::enable::MemberOutcome) already uses for enablement,
/// so the two per-member readouts of a workspace speak one dialect.
///
/// The four states are exhaustive and mutually exclusive:
///
/// | State | Means | Alarming? |
/// |---|---|---|
/// | `warm` | the member's graph holds at least one indexed file | no |
/// | `warming` | its index is running right now | no |
/// | `deferred` | never attempted; indexes lazily on first query ([FR-IX-07]) | **no** ([BR-44]) |
/// | `degraded` | attempted and **failed**; carries the reason | yes |
///
/// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
/// [FR-IX-07]: ../../../docs/specs/requirements/FR-IX-07.md
/// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "warm_state", rename_all = "lowercase")]
pub enum MemberWarmState {
    /// The member's graph holds at least one indexed file.
    Warm,
    /// The member's index is in flight right now. Only ever produced from a
    /// live signal declared through [`WarmEvidence::with_in_flight`] — never
    /// inferred ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    Warming,
    /// The member has no index yet and none was attempted. An honest,
    /// **non-alarming** state: the member indexes lazily on its first query
    /// ([FR-IX-07]), so a deferred warm is the designed fallback rather than a
    /// failure ([BR-44]).
    ///
    /// [FR-IX-07]: ../../../docs/specs/requirements/FR-IX-07.md
    /// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
    Deferred,
    /// The member was attempted and failed, with the reason. Distinct from
    /// [`Deferred`](Self::Deferred) by BR-44: never reported for a member that
    /// was merely never reached.
    Degraded {
        /// Why the attempt failed, verbatim.
        reason: String,
    },
}

/// The live warm facts a `workspace status` read has available — the seam that
/// keeps `warming` and warm-failure **inputs** rather than guesses
/// ([NFR-CC-04], [BR-44]).
///
/// Today's only caller passes [`none`](Self::none): no durable per-member warm
/// record exists, so `warming` is omitted and only an unopenable member reads
/// `degraded`. When the supervisor gains one (a small per-member marker it
/// writes, say), the change is `WarmEvidence::none()` → a populated value at the
/// call site; neither [`derive_state`] nor [`rollup`] nor the wire format
/// moves.
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WarmEvidence {
    /// Member names whose index is in flight right now, or `None` when no
    /// trustworthy live signal source exists at all.
    ///
    /// `None` and `Some(empty)` are deliberately different: `None` means "we
    /// cannot know", which omits `warming` from the roll-up entirely, whereas
    /// `Some(empty)` means "a source answered: nothing is warming", which
    /// reports `warming: 0` ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    in_flight: Option<BTreeSet<String>>,
    /// Member name → why its warm was attempted and failed. Empty today; the
    /// input a durable per-member warm-failure record feeds ([BR-44]).
    ///
    /// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
    failed: BTreeMap<String, String>,
}

impl WarmEvidence {
    /// No live signal at all: `warming` is omitted and only a member that could
    /// not be opened reads `degraded` ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Declare a trustworthy in-flight signal listing the members warming right
    /// now — the future supervisor input.
    ///
    /// Calling this at all is the assertion that a source answered, so the
    /// roll-up reports `warming` from here on, `0` included.
    #[must_use]
    pub fn with_in_flight<I, S>(mut self, members: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.in_flight = Some(members.into_iter().map(Into::into).collect());
        self
    }

    /// Declare the members whose warm was attempted and **failed**, with each
    /// reason — the future durable warm-failure input ([BR-44]).
    ///
    /// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[must_use]
    pub fn with_failures<I, S, R>(mut self, failures: I) -> Self
    where
        I: IntoIterator<Item = (S, R)>,
        S: Into<String>,
        R: Into<String>,
    {
        self.failed = failures
            .into_iter()
            .map(|(member, reason)| (member.into(), reason.into()))
            .collect();
        self
    }

    /// Whether a live `warming` signal exists — the one condition under which
    /// the roll-up may carry a `warming` count at all ([NFR-CC-04]).
    ///
    /// Private: [`rollup`] is the only consumer, and the seam an external caller
    /// needs is the three constructors, not this predicate.
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[must_use]
    fn reports_warming(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Whether `member`'s index is in flight per the live signal. Always `false`
    /// without one, which is what makes [`MemberWarmState::Warming`]
    /// unreachable today rather than merely unused.
    fn is_warming(&self, member: &str) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|warming| warming.contains(member))
    }

    /// Why `member`'s warm failed, if a durable record says it did.
    fn failure(&self, member: &str) -> Option<&str> {
        self.failed.get(member).map(String::as_str)
    }
}

/// Derive one member's warm state from its index presence and the available
/// [`WarmEvidence`] ([FR-WS-15], [BR-44]).
///
/// `indexed` is what the member's own status walk already found:
/// `Ok(true)` — its graph holds at least one indexed file; `Ok(false)` — the
/// read **succeeded** and the graph is empty; `Err(reason)` — the member could
/// not be opened, or could not be read, which is an attempt that **failed** and
/// therefore `degraded`, never `deferred` ([BR-44]).
///
/// The caller owes this distinction: an `Ok(false)` that actually came from a
/// *failed* read would be labeled `deferred` here, which is why
/// [`workspace_status`](super::query::workspace_status) fans the fallible
/// [`Engine::try_status`](crate::Engine::try_status) rather than the degrading [`Engine::status`](crate::Engine::status).
///
/// # Precedence
/// There are **four** inputs, not three, because failure arrives on two
/// channels. In order:
///
/// 1. `evidence.failure(member)` — a durable record that this member's warm was
///    attempted and failed.
/// 2. `indexed == Err(reason)` — the member could not be read at all.
/// 3. `evidence.is_warming(member)` — a live in-flight signal.
/// 4. `indexed == Ok(_)` — index presence.
///
/// In-flight outranks index presence deliberately: a full index persists in
/// bounded chunks ([FR-IX-08]), so a member being indexed right now can already
/// hold files — reporting it `warm` would claim a completed index that is still
/// running. Recorded failure outranks both: a member whose warm failed part-way
/// has exactly that partial index, which is precisely why it must not read
/// `warm`.
///
/// The two failure channels rank **above** the in-flight signal, so a member
/// that a live signal calls in-flight while its store cannot be read reports
/// `degraded`, not `warming`. That ordering is unobservable today (nothing
/// populates the in-flight set) and is pinned by the precedence table test so
/// whichever way a future signal source wants it, the change is deliberate. Of
/// the two failure channels the *durable record* wins, so its reason — not the
/// transient open error — is the one that reaches the wire.
///
/// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
/// [FR-IX-08]: ../../../docs/specs/requirements/FR-IX-08.md
/// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[must_use]
pub fn derive_state(
    member: &str,
    indexed: Result<bool, &str>,
    evidence: &WarmEvidence,
) -> MemberWarmState {
    if let Some(reason) = evidence.failure(member) {
        return MemberWarmState::Degraded {
            reason: reason.to_string(),
        };
    }
    match indexed {
        Err(reason) => MemberWarmState::Degraded {
            reason: reason.to_string(),
        },
        Ok(_) if evidence.is_warming(member) => MemberWarmState::Warming,
        Ok(true) => MemberWarmState::Warm,
        Ok(false) => MemberWarmState::Deferred,
    }
}

/// The workspace-wide warm roll-up ([FR-WS-15]).
///
/// `warming` is [`Option`] on purpose: absent from the JSON when no live signal
/// source exists, so a reader is never handed a `0` that means "we did not
/// look" ([NFR-CC-04]). The three unconditional counts plus `warming`
/// (when present) always sum to [`members`](Self::members).
///
/// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WarmRollup {
    /// Members in the workspace — the denominator the counts below partition.
    pub members: usize,
    /// Members whose graph holds at least one indexed file.
    pub warm: usize,
    /// Members being indexed right now. **Absent** when no trustworthy live
    /// signal source exists, rather than reported as `0` ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warming: Option<usize>,
    /// Members with no index yet and none attempted — lazily indexed on first
    /// query ([FR-IX-07]), not an error ([BR-44]).
    ///
    /// [FR-IX-07]: ../../../docs/specs/requirements/FR-IX-07.md
    /// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
    pub deferred: usize,
    /// Members whose attempt failed.
    pub degraded: usize,
}

/// Roll the per-member states up across the workspace ([FR-WS-15]).
///
/// `evidence` is taken again — rather than inferred from the states — because
/// "nothing is warming" and "we cannot know what is warming" produce the same
/// (empty) set of `Warming` labels and must not produce the same roll-up
/// ([NFR-CC-04]).
///
/// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[must_use]
pub fn rollup<'a>(
    states: impl IntoIterator<Item = &'a MemberWarmState>,
    evidence: &WarmEvidence,
) -> WarmRollup {
    let mut rollup = WarmRollup {
        members: 0,
        warm: 0,
        warming: evidence.reports_warming().then_some(0),
        deferred: 0,
        degraded: 0,
    };
    for state in states {
        rollup.members += 1;
        match state {
            MemberWarmState::Warm => rollup.warm += 1,
            // `get_or_insert` rather than a bare `+= 1` on the Option: a
            // `Warming` label can only come from evidence that reports warming,
            // so the counter is already `Some` — but an input built by hand
            // must still be counted rather than silently dropped.
            MemberWarmState::Warming => *rollup.warming.get_or_insert(0) += 1,
            MemberWarmState::Deferred => rollup.deferred += 1,
            MemberWarmState::Degraded { .. } => rollup.degraded += 1,
        }
    }
    rollup
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole derivation as a table: index presence × evidence → label.
    /// A table rather than one test per state, because the *precedence* between
    /// the three inputs is the rule under test and only a table pins it.
    #[test]
    fn the_label_follows_failure_then_in_flight_then_index_presence() {
        let none = WarmEvidence::none();
        let warming = WarmEvidence::none().with_in_flight(["api"]);
        let failed = WarmEvidence::none().with_failures([("api", "store is corrupt")]);
        // Both signals on the SAME member — the cells where precedence is
        // actually contested rather than merely stated.
        let both = WarmEvidence::none()
            .with_in_flight(["api"])
            .with_failures([("api", "store is corrupt")]);
        let degraded = MemberWarmState::Degraded {
            reason: "store is corrupt".to_string(),
        };

        for (case, evidence, indexed, expected) in [
            ("indexed, no evidence", &none, Ok(true), MemberWarmState::Warm),
            ("empty, no evidence", &none, Ok(false), MemberWarmState::Deferred),
            (
                "unopenable, no evidence",
                &none,
                Err("engine start failed"),
                MemberWarmState::Degraded {
                    reason: "engine start failed".to_string(),
                },
            ),
            // In-flight outranks index presence: a chunk-persisting index
            // (FR-IX-08) is already `indexed` while still running.
            ("indexed, in flight", &warming, Ok(true), MemberWarmState::Warming),
            ("empty, in flight", &warming, Ok(false), MemberWarmState::Warming),
            // Recorded failure outranks everything, partial index included.
            ("indexed, warm failed", &failed, Ok(true), degraded.clone()),
            ("empty, warm failed", &failed, Ok(false), degraded.clone()),
            // ── the contested cells ────────────────────────────────────────
            // An unreadable member outranks a live in-flight signal: reordering
            // the `Err` arm below the `is_warming` guard fails here.
            (
                "unopenable, in flight",
                &warming,
                Err("engine start failed"),
                MemberWarmState::Degraded {
                    reason: "engine start failed".to_string(),
                },
            ),
            // Recorded failure outranks the in-flight signal on the same member.
            ("in flight and warm failed", &both, Ok(false), degraded.clone()),
            ("in flight and warm failed, indexed", &both, Ok(true), degraded.clone()),
            // Both failure channels at once: the DURABLE record's reason wins,
            // not the transient open error — observable, so pinned.
            (
                "unopenable and warm failed",
                &failed,
                Err("engine start failed"),
                degraded.clone(),
            ),
        ] {
            assert_eq!(derive_state("api", indexed, evidence), expected, "{case}");
        }
    }

    /// A member the evidence does not name is unaffected by it — the signals are
    /// per-member, not workspace-wide moods.
    #[test]
    fn evidence_applies_only_to_the_members_it_names() {
        let evidence = WarmEvidence::none()
            .with_in_flight(["api"])
            .with_failures([("web", "spawn failed")]);

        assert_eq!(derive_state("svc", Ok(true), &evidence), MemberWarmState::Warm);
        assert_eq!(derive_state("svc", Ok(false), &evidence), MemberWarmState::Deferred);
        assert_eq!(derive_state("api", Ok(false), &evidence), MemberWarmState::Warming);
        assert_eq!(
            derive_state("web", Ok(true), &evidence),
            MemberWarmState::Degraded {
                reason: "spawn failed".to_string()
            }
        );
    }

    /// [BR-44]'s distinction, stated as its own assertion: a member that failed
    /// is never `deferred`, whichever channel reported the failure.
    ///
    /// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[test]
    fn a_failed_member_is_never_deferred() {
        let unopenable = derive_state("api", Err("no such store"), &WarmEvidence::none());
        let warm_failed = derive_state(
            "api",
            Ok(false),
            &WarmEvidence::none().with_failures([("api", "index exited 2")]),
        );
        for state in [&unopenable, &warm_failed] {
            assert!(
                matches!(state, MemberWarmState::Degraded { .. }),
                "{state:?} must be degraded"
            );
            assert_ne!(*state, MemberWarmState::Deferred);
        }
    }

    // ── NFR-CC-04: `warming` is omitted, never inferred ────────────────────

    /// With no live signal source, no member can be labeled `warming` — for any
    /// index presence at all. The variant is unreachable, not merely unused.
    #[test]
    fn without_a_live_signal_no_member_is_ever_labeled_warming() {
        let evidence = WarmEvidence::none();
        assert!(!evidence.reports_warming());
        for indexed in [Ok(true), Ok(false), Err("boom")] {
            assert_ne!(derive_state("api", indexed, &evidence), MemberWarmState::Warming);
        }
    }

    /// The roll-up **omits the `warming` key entirely** without a signal source,
    /// rather than serialising a `0` a reader would take as "none are warming".
    #[test]
    fn the_rollup_omits_warming_without_a_signal_rather_than_defaulting_it() {
        let states = [MemberWarmState::Warm, MemberWarmState::Deferred];
        let rollup = rollup(&states, &WarmEvidence::none());

        assert_eq!(rollup.warming, None);
        let value = serde_json::to_value(&rollup).unwrap();
        assert!(
            value.get("warming").is_none(),
            "the key must be absent, not null or 0: {value}"
        );
        assert_eq!(value["warm"], 1);
        assert_eq!(value["deferred"], 1);
        assert_eq!(value["members"], 2);
    }

    /// The counterpart: a source that answered "nothing is warming" reports
    /// `warming: 0`. `None` and `Some(0)` are different claims and the wire
    /// keeps them different.
    #[test]
    fn a_live_signal_reporting_nothing_in_flight_still_carries_a_zero() {
        let states = [MemberWarmState::Warm];
        let evidence = WarmEvidence::none().with_in_flight(Vec::<String>::new());
        assert!(
            evidence.reports_warming(),
            "calling with_in_flight IS the assertion that a source answered"
        );
        let rollup = rollup(&states, &evidence);

        assert_eq!(rollup.warming, Some(0));
        let value = serde_json::to_value(&rollup).unwrap();
        assert_eq!(value["warming"], 0, "an answered zero IS reported: {value}");
    }

    /// The branch `rollup`'s own comment justifies: a `Warming` state arriving
    /// while the evidence reports no signal source must still be **counted**,
    /// never silently dropped — dropping it would break the documented
    /// partition (`warm + warming + deferred + degraded == members`) with no
    /// test noticing.
    ///
    /// Degrading `get_or_insert(0)` to `if let Some(w) = &mut rollup.warming`
    /// passes every other test in the repo and fails only this one.
    #[test]
    fn a_warming_row_is_counted_even_without_a_signal_source() {
        let rollup = rollup(
            &[MemberWarmState::Warming, MemberWarmState::Warm],
            &WarmEvidence::none(),
        );

        assert_eq!(rollup.warming, Some(1), "the row is counted, not dropped");
        assert_eq!(
            rollup.warm + rollup.warming.unwrap_or(0) + rollup.deferred + rollup.degraded,
            rollup.members,
            "no member may fall out of the partition"
        );
    }

    // ── the roll-up partitions the workspace ───────────────────────────────

    #[test]
    fn the_rollup_counts_partition_the_member_set() {
        let states = [
            MemberWarmState::Warm,
            MemberWarmState::Warm,
            MemberWarmState::Warming,
            MemberWarmState::Deferred,
            MemberWarmState::Degraded {
                reason: "boom".to_string(),
            },
        ];
        let evidence = WarmEvidence::none().with_in_flight(["c"]);
        let rollup = rollup(&states, &evidence);

        assert_eq!(rollup.members, 5);
        assert_eq!((rollup.warm, rollup.warming, rollup.deferred, rollup.degraded), (2, Some(1), 1, 1));
        assert_eq!(
            rollup.warm + rollup.warming.unwrap_or(0) + rollup.deferred + rollup.degraded,
            rollup.members,
            "the counts must partition the member set, never overlap or drop one"
        );
    }

    /// A fully warmed workspace: every member `warm`, nothing deferred, nothing
    /// warming ([FR-WS-15] AC3).
    ///
    /// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
    #[test]
    fn a_fully_warmed_workspace_reports_no_deferred_and_no_warming() {
        let evidence = WarmEvidence::none();
        let states: Vec<MemberWarmState> = ["api", "web", "svc"]
            .into_iter()
            .map(|m| derive_state(m, Ok(true), &evidence))
            .collect();
        let rollup = rollup(&states, &evidence);

        assert_eq!((rollup.members, rollup.warm), (3, 3));
        assert_eq!((rollup.deferred, rollup.degraded), (0, 0));
        assert_eq!(rollup.warming, None, "no live signal ⇒ no warming key at all");
    }

    #[test]
    fn an_empty_workspace_rolls_up_to_zeroes() {
        let rollup = rollup(std::iter::empty(), &WarmEvidence::none());
        assert_eq!((rollup.members, rollup.warm, rollup.deferred, rollup.degraded), (0, 0, 0, 0));
        assert_eq!(rollup.warming, None);
    }

    // ── wire shape ─────────────────────────────────────────────────────────

    /// The label rides an internally-tagged `warm_state` key, and `degraded`
    /// carries its reason beside it — the shape a member row flattens in.
    #[test]
    fn a_member_state_serialises_as_a_tagged_label_with_the_reason_beside_it() {
        let value = serde_json::to_value(MemberWarmState::Deferred).unwrap();
        assert_eq!(value["warm_state"], "deferred");
        assert_eq!(value.as_object().unwrap().len(), 1, "a plain label, nothing else");

        let value = serde_json::to_value(MemberWarmState::Degraded {
            reason: "store is corrupt".to_string(),
        })
        .unwrap();
        assert_eq!(value["warm_state"], "degraded");
        assert_eq!(value["reason"], "store is corrupt");
    }

    /// Every variant serialises to the word the vocabulary names it by — the
    /// tag list pinned against literals, so a `rename_all` change or a renamed
    /// variant is a failing test rather than a silently changed wire contract.
    #[test]
    fn every_variant_serialises_to_its_vocabulary_word() {
        for (state, word) in [
            (MemberWarmState::Warm, "warm"),
            (MemberWarmState::Warming, "warming"),
            (MemberWarmState::Deferred, "deferred"),
            (
                MemberWarmState::Degraded {
                    reason: "boom".to_string(),
                },
                "degraded",
            ),
        ] {
            let value = serde_json::to_value(&state).unwrap();
            assert_eq!(value["warm_state"], word, "{state:?}");
        }
    }
}
