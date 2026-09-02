//! The **open-state** axis of a workspace member: was its store opened, and if
//! not, why ([FR-WS-16], [BR-45], [NFR-CC-04]).
//!
//! # Two axes, deliberately separable
//! [`warm_state`](super::warm_state) already labels a member `warm` /
//! `warming` / `deferred` / `degraded`. That axis is about **index presence** —
//! whether the member's graph holds any indexed file. This module is about
//! **store openability** — whether the process managed to open the member's
//! store at all. The two answer different questions and a reader needs both:
//! an `open_state: degraded` member has no index presence to report, and a
//! `warm_state: deferred` member is perfectly openable.
//!
//! They are therefore **separate fields on one row**, never one merged
//! vocabulary. `warm_state` keeps exactly the four values [FR-WS-15] gave it
//! and this module adds `open_state` beside it; neither is derived from the
//! other. (A member that could not be opened does read `degraded` on *both*
//! axes today, because an unopenable store is also an unreadable index — that
//! is an agreement between two independent derivations, not a shared one.)
//!
//! # `degraded` means attempted **and failed** ([BR-45])
//! Three member outcomes are distinguishable here, and conflating any two of
//! them is the defect [FR-WS-16] exists to prevent:
//!
//! | Open state | Means | Alarming? |
//! |---|---|---|
//! | `opened` | the store was opened — **including** a member later LRU-evicted to stay inside the [budget](super::budget) ([NFR-PE-11]) | no |
//! | `not-attempted` | the answer never needed this member, so nothing was opened ([NFR-PE-10] laziness) | no |
//! | `degraded` | opening was **attempted and failed**, with the cause | yes |
//!
//! Eviction is the sharp one. An evicted member's engine is torn down and
//! rebuilt on next touch, and the rebuild is indistinguishable from a
//! never-evicted one ([FR-DB-01]) — so eviction is a *success* that happened to
//! be reclaimed, and it must never surface as a failure. The
//! [registry](super::registry) therefore records the outcome of each **open
//! attempt**, not residency: an evicted member's ledger entry stays `opened`
//! because its open succeeded, and residency is not consulted here at all.
//!
//! # Nothing is inferred
//! [`DegradedCause`] is an [`Option`]: absent when the diagnostic carries no
//! evidence identifying a cause, rather than defaulted to the most likely one
//! ([NFR-CC-04]). The verbatim engine error still rides the row's existing
//! `error` field, untouched, so a reader who wants the raw diagnostic always
//! has it.
//!
//! [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
//! [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
//! [FR-DB-01]: ../../../docs/specs/requirements/FR-DB-01.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
//! [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
//! [BR-45]: ../../../docs/specs/software-spec.md#327-workspace-federation

use serde::Serialize;

/// Why a member's store could not be opened, when the diagnostic identifies a
/// cause ([FR-WS-16]).
///
/// The point of naming a cause at all is that SQLite's own wording misdiagnoses
/// the failure that motivated [FR-WS-16]: "applying the read-only connection
/// contract ([FR-DB-02]): unable to open database file" reads as a missing or
/// corrupt store, and in the observed case all 72 stores were present and
/// correctly checkpointed — what ran out was the process descriptor table,
/// which that message never mentions.
///
/// [FR-DB-02]: ../../../docs/specs/requirements/FR-DB-02.md
/// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DegradedCause {
    /// A **host** resource limit refused the connection: the process could not
    /// obtain the file descriptors the member's read pool needs
    /// (`RLIMIT_NOFILE`). The member's store is present and its graph intact —
    /// the fix is a wider budget or a higher `ulimit -n`, never a re-index.
    HostResourceLimit,
    /// The member has **no store file** at its canonical path
    /// (`<root>/.logos/logos.db`): never indexed, removed, or something other
    /// than a regular file sits there. `logos index` in that member is the fix.
    StoreUnavailable,
}

impl DegradedCause {
    /// The plain-language statement of this cause, for the human diagnostic and
    /// for the row's `degraded_reason`.
    ///
    /// Stated as what is *known* plus the remedy, because the whole reason the
    /// cause exists is that the raw SQLite wording sends a reader to the wrong
    /// remedy ([FR-WS-16]).
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::HostResourceLimit => {
                "a host resource limit, not a damaged store: the store is present but the \
                 process could not obtain the file descriptors its connections need \
                 (RLIMIT_NOFILE) — raise `ulimit -n`, or query fewer members at once"
            }
            Self::StoreUnavailable => {
                "no store at the member's `.logos/logos.db` — run `logos index` in that member"
            }
        }
    }
}

/// Descriptor-exhaustion evidence, in the words the OS uses for it.
///
/// `EMFILE` renders as "Too many open files" and `ENFILE` as "Too many open
/// files in system", so this one prefix covers both, and both are unambiguous.
/// Matched case-insensitively so a wrapper that re-capitalises the errno text
/// still classifies.
const DESCRIPTOR_EXHAUSTION: &str = "too many open files";

/// SQLite's `SQLITE_CANTOPEN`, in the two forms `rusqlite` surfaces it.
///
/// Deliberately **ambiguous on its own**: SQLite reports it both for a database
/// file that is not there and for one it could not `open(2)`. Which of the two
/// it was is settled by whether the store file exists — see [`classify`].
const CANNOT_OPEN: [&str; 2] = ["unable to open database file", "error code 14"];

/// Whether the member's store file is where it should be — the fact that
/// disambiguates SQLite's `CANTOPEN` ([`classify`]).
///
/// A dedicated enum rather than a `bool` so a call site cannot silently swap the
/// polarity of the one input the classification turns on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreFile {
    /// A regular file exists at `<root>/.logos/logos.db`.
    Present,
    /// Nothing, or something that is not a regular file, is at that path.
    Absent,
}

/// Classify an open failure from its diagnostic and the store's presence
/// ([FR-WS-16]).
///
/// `None` — no cause is claimed — is a first-class outcome: the verbatim
/// diagnostic is then the whole of what a reader is told, rather than being
/// dressed up with a guessed cause ([NFR-CC-04]).
///
/// # The one inference, stated
/// Descriptor exhaustion in the diagnostic is direct evidence and needs no
/// inference. `CANTOPEN` over a store that **is** present does: SQLite reports
/// `CANTOPEN` when the file is absent *or* when the `open(2)` itself was
/// refused, and a present file rules out the first — so the refusal came from
/// the process's ability to open a file, which under a workspace is
/// descriptor-bound ([NFR-PE-11]). The blind spot is a store the process is not
/// permitted to read, which classifies as [`HostResourceLimit`] too. That is
/// wrong about *which* host resource, and still right about the thing
/// [FR-WS-16] is fixing: the store is intact and a re-index is not the remedy.
///
/// [`HostResourceLimit`]: DegradedCause::HostResourceLimit
/// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
#[must_use]
pub fn classify(diagnostic: &str, store: StoreFile) -> Option<DegradedCause> {
    let lower = diagnostic.to_ascii_lowercase();

    if lower.contains(DESCRIPTOR_EXHAUSTION) {
        return Some(DegradedCause::HostResourceLimit);
    }
    if CANNOT_OPEN.iter().any(|needle| lower.contains(needle)) {
        return Some(match store {
            StoreFile::Present => DegradedCause::HostResourceLimit,
            StoreFile::Absent => DegradedCause::StoreUnavailable,
        });
    }
    None
}

/// One member's open state as a workspace read-model reports it ([FR-WS-16]).
///
/// Serialized **internally tagged** on `open_state`, matching
/// [`MemberWarmState`](super::warm_state::MemberWarmState)'s dialect so the two
/// axes read as two fields of one row rather than two payloads. The degraded
/// arm's fields are prefixed `degraded_` because the row this flattens into
/// already carries an `error` (the verbatim engine diagnostic) and a `reason`
/// (the warm axis'): three distinct facts that must stay three distinct keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "open_state", rename_all = "kebab-case")]
pub enum MemberOpenState {
    /// The member's store was opened. Includes a member whose engine was later
    /// evicted to stay inside the budget — eviction reclaims a **success**
    /// ([NFR-PE-11], [BR-45]).
    ///
    /// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
    /// [BR-45]: ../../../docs/specs/software-spec.md#327-workspace-federation
    Opened,
    /// Opening was never attempted: the answer did not need this member
    /// ([NFR-PE-10] laziness). Not a failure, and never counted as one.
    ///
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    NotAttempted,
    /// Opening was attempted and **failed**.
    Degraded {
        /// The classified cause, **absent** when the diagnostic identifies none
        /// rather than defaulted to the likeliest ([NFR-CC-04]).
        ///
        /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
        #[serde(skip_serializing_if = "Option::is_none")]
        degraded_cause: Option<DegradedCause>,
        /// The cause's plain-language statement, or the verbatim diagnostic when
        /// no cause was identified.
        ///
        /// Additive to — never a replacement for — the row's existing `error`
        /// field, which keeps the raw engine diagnostic exactly as it was.
        degraded_reason: String,
    },
}

impl MemberOpenState {
    /// The degraded state for a failed open, classifying `diagnostic` against
    /// the store's presence ([FR-WS-16]).
    #[must_use]
    pub fn degraded(diagnostic: &str, store: StoreFile) -> Self {
        let cause = classify(diagnostic, store);
        Self::Degraded {
            degraded_cause: cause,
            degraded_reason: cause
                .map_or_else(|| diagnostic.to_string(), |cause| cause.message().to_string()),
        }
    }

    /// Whether this member was attempted and failed — the one state that counts
    /// toward the degraded roll-up and the non-zero exit ([FR-WS-16]).
    #[must_use]
    pub const fn is_degraded(&self) -> bool {
        matches!(self, Self::Degraded { .. })
    }
}

/// One member's name paired with its open state — the registry's per-member
/// answer ([FR-WS-16], [FR-WS-03]).
///
/// Deliberately **not** `Serialize`: the open state reaches the wire folded into
/// the read-model's own member row (see
/// [`MemberStatus`](super::query::MemberStatus)), and a second serialisable
/// per-member shape is exactly the competing member table [FR-WS-16] forbids.
///
/// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
/// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberOpen {
    /// The member's name (its workspace-relative path).
    pub member: String,
    /// Whether that member's store was opened.
    pub state: MemberOpenState,
}

/// The workspace-wide degraded roll-up ([FR-WS-16], [NFR-CC-04]).
///
/// A projection of the per-member rows it accompanies, exactly as
/// [`WarmRollup`](super::warm_state::WarmRollup) is — so the roll-up can never
/// disagree with the member table, and the two roll-ups compose in one payload
/// instead of describing the workspace twice.
///
/// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DegradedRollup {
    /// Members in the workspace roster — the denominator the counts partition.
    pub members: usize,
    /// Members whose store was opened, evicted-and-reclaimed included.
    pub opened: usize,
    /// Members opening was never attempted for — outside the answer's scope,
    /// not a failure ([NFR-PE-10]).
    ///
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    pub not_attempted: usize,
    /// Members attempted and failed, **named**, in roster order ([FR-WS-16]).
    ///
    /// Names only. The per-member cause and the verbatim `error` stay on the
    /// member table's own rows, so this is a roll-up and not a second table.
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    pub degraded_members: Vec<String>,
    /// Whether every member in the roster was opened.
    ///
    /// `false` marks **every** figure computed beside this roll-up — the warm
    /// roll-up, the coverage summary, the topic inventory — as covering fewer
    /// than all members, so a partial answer never reads as a complete one
    /// ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub covers_all_members: bool,
}

impl DegradedRollup {
    /// The human diagnostic naming the degraded members and their causes, or
    /// `None` when nothing degraded ([FR-WS-16]).
    ///
    /// Rendered from the roll-up rather than from the payload so every workspace
    /// command can name its degraded members on stderr, including the ones whose
    /// read-model has no member table to fold into (`workspace check` serialises
    /// a bare `Option`, and wrapping it would destroy the honest-`null` contract
    /// [NFR-CC-04] gave it).
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[must_use]
    pub fn notice(&self) -> Option<String> {
        if self.degraded_members.is_empty() {
            return None;
        }
        Some(format!(
            "warning: {} of {} workspace members could not be opened and are reported \
             degraded: {}. Every figure in this answer covers only the {} members that \
             opened (FR-WS-16).",
            self.degraded_members.len(),
            self.members,
            self.degraded_members.join(", "),
            self.opened,
        ))
    }
}

/// Roll the per-member open states up across the workspace ([FR-WS-16]).
///
/// Order-preserving: [`degraded_members`](DegradedRollup::degraded_members)
/// comes out in the order the rows arrive, which the registry supplies in
/// manifest order ([NFR-RA-06]).
///
/// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
#[must_use]
pub fn rollup<'a>(opens: impl IntoIterator<Item = &'a MemberOpen>) -> DegradedRollup {
    let mut rollup = DegradedRollup {
        members: 0,
        opened: 0,
        not_attempted: 0,
        degraded_members: Vec::new(),
        covers_all_members: true,
    };
    for open in opens {
        rollup.members += 1;
        match &open.state {
            MemberOpenState::Opened => rollup.opened += 1,
            MemberOpenState::NotAttempted => rollup.not_attempted += 1,
            MemberOpenState::Degraded { .. } => rollup.degraded_members.push(open.member.clone()),
        }
    }
    rollup.covers_all_members = rollup.opened == rollup.members;
    rollup
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The verbatim diagnostic from [CR-100]'s observed failure — 63 of 72
    /// members, every store present and correctly checkpointed, exit 0.
    ///
    /// [CR-100]: ../../../docs/requests/CR-100-workspace-resource-budget.md
    const OBSERVED: &str = "starting the engine for workspace member \"filters-api\": starting \
                            the execution runtime for root .../filters-api: opening read-only \
                            pool connection 3/12: applying the read-only connection contract \
                            (FR-DB-02): unable to open database file: Error code 14";

    fn opened(member: &str) -> MemberOpen {
        MemberOpen {
            member: member.to_string(),
            state: MemberOpenState::Opened,
        }
    }

    fn not_attempted(member: &str) -> MemberOpen {
        MemberOpen {
            member: member.to_string(),
            state: MemberOpenState::NotAttempted,
        }
    }

    fn degraded(member: &str, diagnostic: &str, store: StoreFile) -> MemberOpen {
        MemberOpen {
            member: member.to_string(),
            state: MemberOpenState::degraded(diagnostic, store),
        }
    }

    /// The whole classification as a table: diagnostic × store presence →
    /// cause. A table because the *precedence* between the two inputs is the
    /// rule under test — direct descriptor evidence must outrank the
    /// store-presence inference.
    #[test]
    fn the_cause_follows_descriptor_evidence_then_store_presence() {
        for (case, diagnostic, store, expected) in [
            // Direct evidence: unambiguous, whatever the store looks like.
            (
                "emfile, store present",
                "opening pool connection: Too many open files (os error 24)",
                StoreFile::Present,
                Some(DegradedCause::HostResourceLimit),
            ),
            (
                "emfile outranks an absent store",
                "opening pool connection: Too many open files (os error 24)",
                StoreFile::Absent,
                Some(DegradedCause::HostResourceLimit),
            ),
            (
                "enfile, system-wide",
                "Too many open files in system (os error 23)",
                StoreFile::Present,
                Some(DegradedCause::HostResourceLimit),
            ),
            // CANTOPEN: ambiguous alone, settled by the store.
            (
                "cantopen over a present store",
                OBSERVED,
                StoreFile::Present,
                Some(DegradedCause::HostResourceLimit),
            ),
            (
                "cantopen over an absent store",
                OBSERVED,
                StoreFile::Absent,
                Some(DegradedCause::StoreUnavailable),
            ),
            (
                "cantopen by SQLite's own wording only",
                "applying the read-only connection contract: unable to open database file",
                StoreFile::Absent,
                Some(DegradedCause::StoreUnavailable),
            ),
            // No evidence at all: no cause is claimed (NFR-CC-04).
            (
                "an unrelated failure claims no cause",
                "building the workspace's shared worker pool: no threads available",
                StoreFile::Present,
                None,
            ),
            (
                "a corrupt store claims no cause either",
                "database disk image is malformed",
                StoreFile::Present,
                None,
            ),
        ] {
            assert_eq!(
                classify(diagnostic, store),
                expected,
                "{case}: {diagnostic}"
            );
        }
    }

    /// [FR-WS-16] AC3: the observed fd-exhaustion diagnostic yields a
    /// **host-resource** message, and specifically NOT the store-corruption
    /// reading its raw SQLite wording invites.
    #[test]
    fn the_observed_fd_exhaustion_reads_as_a_host_limit_not_a_broken_store() {
        let state = MemberOpenState::degraded(OBSERVED, StoreFile::Present);
        let MemberOpenState::Degraded {
            degraded_cause,
            degraded_reason,
        } = &state
        else {
            panic!("a failed open is degraded: {state:?}");
        };

        assert_eq!(*degraded_cause, Some(DegradedCause::HostResourceLimit));
        assert!(
            degraded_reason.contains("host resource limit")
                && degraded_reason.contains("not a damaged store"),
            "the reason states the host cause: {degraded_reason}"
        );
        for misleading in ["unable to open database file", "FR-DB-02", "Error code 14"] {
            assert!(
                !degraded_reason.contains(misleading),
                "the reason must not repeat the misdiagnosing wording {misleading:?}: \
                 {degraded_reason}"
            );
        }
        assert!(
            !degraded_reason.contains("logos index"),
            "a host limit must not send the reader to a re-index: {degraded_reason}"
        );
    }

    /// An unclassifiable failure carries the **verbatim** diagnostic rather than
    /// a fabricated cause ([NFR-CC-04]).
    #[test]
    fn an_unclassified_failure_carries_the_verbatim_diagnostic_and_no_cause() {
        let raw = "database disk image is malformed";
        let state = MemberOpenState::degraded(raw, StoreFile::Present);
        let MemberOpenState::Degraded {
            degraded_cause,
            degraded_reason,
        } = &state
        else {
            panic!("a failed open is degraded: {state:?}");
        };
        assert_eq!(*degraded_cause, None, "no cause is invented");
        assert_eq!(degraded_reason, raw, "the diagnostic stands as it is");

        let value = serde_json::to_value(&state).unwrap();
        assert!(
            value.get("degraded_cause").is_none(),
            "an unidentified cause is an ABSENT key, never a default: {value}"
        );
    }

    /// [BR-45]: a member that opened — evicted afterwards or not — and a member
    /// never attempted are both **non**-degraded, and neither reaches the
    /// roll-up's named set.
    #[test]
    fn only_an_attempted_and_failed_open_is_degraded() {
        assert!(!MemberOpenState::Opened.is_degraded());
        assert!(!MemberOpenState::NotAttempted.is_degraded());
        assert!(MemberOpenState::degraded(OBSERVED, StoreFile::Present).is_degraded());
    }

    /// The roll-up partitions the roster and names only the failures, in the
    /// order the rows arrive ([FR-WS-16]).
    #[test]
    fn the_rollup_partitions_the_roster_and_names_only_the_failures() {
        let rows = vec![
            opened("api"),
            degraded("web", OBSERVED, StoreFile::Present),
            not_attempted("svc"),
            degraded("old", "database disk image is malformed", StoreFile::Present),
        ];
        let rollup = rollup(&rows);

        assert_eq!(rollup.members, 4);
        assert_eq!(rollup.opened, 1);
        assert_eq!(rollup.not_attempted, 1);
        assert_eq!(
            rollup.degraded_members,
            ["web", "old"],
            "named in row order, not sorted into a different one"
        );
        assert_eq!(
            rollup.opened + rollup.not_attempted + rollup.degraded_members.len(),
            rollup.members,
            "the three counts partition the roster"
        );
        assert!(
            !rollup.covers_all_members,
            "one member unopened ⇒ every figure beside this roll-up is partial"
        );
    }

    /// A fully-opened workspace covers all members and produces no notice —
    /// the case that must keep exiting 0 ([FR-WS-16] AC1).
    #[test]
    fn a_fully_opened_workspace_covers_all_members_and_says_nothing() {
        let rollup = rollup(&[opened("api"), opened("web")]);

        assert_eq!(rollup.opened, 2);
        assert!(rollup.degraded_members.is_empty());
        assert!(rollup.covers_all_members);
        assert_eq!(rollup.notice(), None, "nothing to warn about");
    }

    /// A workspace whose members were merely never **attempted** is not
    /// covered-all either — but it is not degraded, so nothing is named and the
    /// exit code is untouched ([BR-45], [NFR-PE-10]).
    #[test]
    fn an_unattempted_workspace_is_partial_but_never_degraded() {
        let rollup = rollup(&[opened("api"), not_attempted("web"), not_attempted("svc")]);

        assert!(rollup.degraded_members.is_empty(), "laziness names nobody");
        assert_eq!(rollup.notice(), None, "and warns about nobody");
        assert!(
            !rollup.covers_all_members,
            "the figures still cover only 1 of 3 members"
        );
    }

    /// The empty workspace: no members, nothing degraded, covers all of nothing.
    #[test]
    fn an_empty_workspace_covers_all_members() {
        let rollup = rollup(std::iter::empty());

        assert_eq!(rollup.members, 0);
        assert!(rollup.covers_all_members, "0 of 0 is covered");
        assert_eq!(rollup.notice(), None);
    }

    /// The human notice names **every** degraded member and the coverage
    /// shortfall — an exit code with no named member is what [FR-WS-16] is
    /// replacing.
    #[test]
    fn the_notice_names_every_degraded_member_and_the_shortfall() {
        let rows = vec![
            opened("api"),
            degraded("web", OBSERVED, StoreFile::Present),
            degraded("svc", OBSERVED, StoreFile::Present),
        ];
        let notice = rollup(&rows).notice().expect("two members degraded");

        assert!(notice.contains("web") && notice.contains("svc"), "{notice}");
        assert!(notice.contains("2 of 3"), "{notice}");
        assert!(
            notice.contains("only the 1 members that opened"),
            "the notice states the reduced coverage: {notice}"
        );
    }

    /// The wire shape: `open_state` is the tag, and the degraded arm's keys are
    /// prefixed so they cannot collide with the `error` / `reason` the member
    /// row already carries ([FR-WS-15] deferred #15 stays open either way).
    ///
    /// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
    #[test]
    fn the_wire_shape_tags_open_state_and_never_shadows_error_or_reason() {
        for (state, tag) in [
            (MemberOpenState::Opened, "opened"),
            (MemberOpenState::NotAttempted, "not-attempted"),
            (
                MemberOpenState::degraded(OBSERVED, StoreFile::Present),
                "degraded",
            ),
        ] {
            let value = serde_json::to_value(&state).unwrap();
            assert_eq!(value["open_state"], tag, "{state:?} tags as {tag}");
            for reserved in ["error", "reason"] {
                assert!(
                    value.get(reserved).is_none(),
                    "{tag} must not carry a {reserved:?} key — the row's own is a different \
                     fact: {value}"
                );
            }
        }
    }
}
