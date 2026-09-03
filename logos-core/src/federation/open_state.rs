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
//! # "Degraded" here is not [ADR-14]'s "degraded"
//! [`Severity::Degraded`](crate::error::Severity) is an **error-boundary** class
//! that maps to exit **0** — a fault that must not abort the command. This
//! module's `degraded` is a **result-level** class that maps to exit **1** via
//! [FR-CL-03]: the command ran fine, and its answer is incomplete. Two
//! orthogonal axes that happen to share a word; neither is a special case of the
//! other.
//!
//! [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
//! [FR-CL-03]: ../../../docs/specs/requirements/FR-CL-03.md
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
    /// The process was **refused when it opened a store that is there** — the
    /// store and its graph are intact, and a re-index is not the remedy.
    ///
    /// Two host conditions produce this with an *identical* symptom: the
    /// descriptor table is exhausted (`RLIMIT_NOFILE` — the [CR-100] failure), or
    /// the file permissions on the member's `.logos/` refuse this user. Nothing
    /// in the diagnostic separates them, because a refused `open(2)` reaches
    /// SQLite as the same `CANTOPEN` either way — so
    /// [`message`](Self::message) names **both** rather than asserting the one
    /// that happened to be true the first time this was seen ([NFR-CC-04]).
    ///
    /// The variant and its wire key (`host-resource-limit`) are kept exactly as
    /// they are. Telling the two conditions apart for real needs a readability
    /// probe before classification *and* a second variant — a new key on every
    /// degraded member row, which [CR-102] deliberately did not spend to
    /// separate two conditions that share one remedy shape ("the store is fine,
    /// look at the host"). See [`classify`] for that non-choice in full.
    ///
    /// [CR-100]: ../../../docs/requests/CR-100-workspace-resource-budget.md
    /// [CR-102]: ../../../docs/requests/CR-102-warm-outcome-record-and-spec-corrections.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    HostResourceLimit,
    /// Something that is **not a regular file** occupies the member's canonical
    /// store path (`<root>/.logos/logos.db`) — a directory, a socket, a dangling
    /// symlink. Nothing can open it and no re-index can fix it; the path has to
    /// be cleared first.
    ///
    /// Deliberately **not** used for a merely *absent* store: a member that was
    /// never indexed opens perfectly well, because
    /// [`Engine::start`](crate::Engine::start) creates the store on open. An
    /// absent file at failure time therefore carries no information — it is
    /// equally consistent with descriptor exhaustion *during creation*, which is
    /// exactly the failure this vocabulary exists to stop misreporting. That case
    /// claims no cause at all (see [`classify`]).
    StoreObstructed,
}

impl DegradedCause {
    /// The plain-language statement of this cause, for the human diagnostic and
    /// for the row's `degraded_reason`.
    ///
    /// Stated as what is *known* **first**, then the remedy, because the whole
    /// reason the cause exists is that the raw SQLite wording sends a reader to
    /// the wrong remedy ([FR-WS-16]). Where one symptom admits two causes the
    /// remedy names **both** — asserting the narrower one is the same class of
    /// misdiagnosis, merely relocated ([NFR-CC-04], [CR-102]).
    ///
    /// [CR-102]: ../../../docs/requests/CR-102-warm-outcome-record-and-spec-corrections.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::HostResourceLimit => {
                "not a damaged store: the member's store is present and its graph intact, \
                 so a re-index is not the remedy — what failed is this process's attempt \
                 to open the file. Two conditions produce that identically and the \
                 diagnostic separates neither, so both are named: the file permissions on \
                 the member's `.logos/` directory and store, which must be readable by the \
                 user running logos, and a host resource limit on file descriptors \
                 (RLIMIT_NOFILE) — check the permissions, then raise `ulimit -n` or query \
                 fewer members at once"
            }
            Self::StoreObstructed => {
                "the member's `.logos/logos.db` path is occupied by something that is not a \
                 regular file — clear that path, then run `logos index` in that member"
            }
        }
    }
}

/// Descriptor-exhaustion evidence, in every spelling a layer between the kernel
/// and here might use.
///
/// `EMFILE` renders as "Too many open files" and `ENFILE` as "Too many open
/// files in system", so that one prefix covers both. The bare errno forms are
/// listed too: a wrapper that reports `io::Error` without its `strerror` text
/// yields only "os error 24" / "os error 23", and missing the very failure this
/// vocabulary exists for would leave the operator with no remedy at all. The
/// closing parenthesis is part of the needle so `os error 24` cannot match
/// `os error 240`.
///
/// Matched case-insensitively, so a wrapper that re-capitalises still classifies.
const DESCRIPTOR_EXHAUSTION: [&str; 3] = [
    "too many open files",
    "os error 24)",
    "os error 23)",
];

/// SQLite's `SQLITE_CANTOPEN`, in the two forms `rusqlite` surfaces it.
///
/// Deliberately **ambiguous on its own**: SQLite reports it both for a database
/// file that is not there and for one it could not `open(2)`. Which of the two
/// it was is settled by whether the store file exists — see [`classify`].
const CANNOT_OPEN: [&str; 2] = ["unable to open database file", "error code 14"];

/// What is at the member's canonical store path — the fact that disambiguates
/// SQLite's `CANTOPEN` ([`classify`]).
///
/// A dedicated enum rather than a `bool` so a call site cannot silently swap the
/// polarity of the one input the classification turns on. **Three** states, not
/// two, because "no file" and "a file of the wrong kind" license opposite
/// conclusions: the first is uninformative and the second is decisive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreFile {
    /// A regular file exists at `<root>/.logos/logos.db`. A `CANTOPEN` over it
    /// means the *process* was refused, not that the data is missing.
    Present,
    /// Nothing at all is at that path. **Uninformative**: the store is created on
    /// open, so an absent file is equally consistent with a never-indexed member
    /// and with descriptor exhaustion during creation.
    Absent,
    /// Something that is not a regular file occupies the path — a directory, a
    /// socket, a dangling symlink. Decisive, and no re-index can fix it.
    Obstructed,
}

/// Classify an open failure from its diagnostic and the store's presence
/// ([FR-WS-16]).
///
/// `None` — no cause is claimed — is a first-class outcome: the verbatim
/// diagnostic is then the whole of what a reader is told, rather than being
/// dressed up with a guessed cause ([NFR-CC-04]).
///
/// # The one inference, stated — and the case it deliberately refuses
/// Descriptor exhaustion in the diagnostic is direct evidence and needs no
/// inference. `CANTOPEN` needs one, because SQLite reports it both when the file
/// is absent and when the `open(2)` itself was refused. The store path settles
/// it in two of three states and, crucially, **not** in the third:
///
/// - **`Present`** — a regular store file rules out "missing data", so the
///   refusal came from the process's ability to open a file, which under a
///   workspace is descriptor-bound ([NFR-PE-11]) ⇒ [`HostResourceLimit`].
/// - **`Obstructed`** — a non-regular file at the path cannot be opened by
///   anything ⇒ [`StoreObstructed`], with the remedy being to clear the path.
/// - **`Absent`** — **no cause is claimed.** The store is *created* on open
///   ([`Engine::start`](crate::Engine::start)), so a never-indexed member opens
///   fine; an absent file at failure time means only that the create-open did
///   not get far enough, which is exactly what descriptor exhaustion looks like.
///   Reading it as "no store, go re-index" would send an operator whose real
///   problem is `ulimit -n` to a command that cannot help — the same class of
///   misdiagnosis [FR-WS-16] exists to remove, merely relocated. The verbatim
///   diagnostic stands alone instead ([NFR-CC-04]).
///
/// # A present store admits two causes, and the remedy names both
/// A *present* store this process is not **permitted** to read classifies as
/// [`HostResourceLimit`] as well, because a refused `open(2)` reaches SQLite as
/// the same `CANTOPEN` whether the descriptor table is full or the permission
/// bits say no. The shipped remedy asserted the narrower cause and advised
/// `ulimit -n`; on the [CR-102] acceptance run over the real 84-member
/// workspace the descriptor limit was provably *not* what failed, so that advice
/// was wrong on the one run that exercised it.
///
/// [`DegradedCause::message`] therefore states what **is** known first — the
/// store is intact, and a re-index is not the remedy — and then names **both**
/// candidates: the file permissions on the member's `.logos/`, and the
/// descriptor limit. Claiming either alone is exactly the over-assertion
/// [NFR-CC-04] forbids.
///
/// Separating them for real is a **deliberate non-choice**. It needs a
/// readability probe on the store path *before* classification and a second
/// [`DegradedCause`] variant — a new wire key on every degraded member row, paid
/// by every consumer, to split two conditions that share one remedy shape ("the
/// store is fine, look at the host"). [CR-102] widened the remedy text instead
/// and recorded the probe as the larger option not taken; if it is ever built,
/// the probe is what licenses the new variant, not the other way round.
///
/// [CR-102]: ../../../docs/requests/CR-102-warm-outcome-record-and-spec-corrections.md
///
/// [`HostResourceLimit`]: DegradedCause::HostResourceLimit
/// [`StoreObstructed`]: DegradedCause::StoreObstructed
/// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
#[must_use]
pub fn classify(diagnostic: &str, store: StoreFile) -> Option<DegradedCause> {
    let lower = diagnostic.to_ascii_lowercase();
    let names = |needles: &[&str]| needles.iter().any(|needle| lower.contains(needle));

    if names(&DESCRIPTOR_EXHAUSTION) {
        return Some(DegradedCause::HostResourceLimit);
    }
    if names(&CANNOT_OPEN) {
        return match store {
            StoreFile::Present => Some(DegradedCause::HostResourceLimit),
            StoreFile::Obstructed => Some(DegradedCause::StoreObstructed),
            // Uninformative — see the doc above. Claiming nothing is the honest
            // answer, and the verbatim diagnostic is what the reader gets.
            StoreFile::Absent => None,
        };
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
        /// no cause was identified — what to *tell* the operator.
        ///
        /// Additive to — never a replacement for — the row's existing `error`
        /// field, which keeps the raw engine diagnostic exactly as it was.
        degraded_reason: String,
        /// The verbatim engine diagnostic, **always**.
        ///
        /// Carried here rather than left to the row's `error` field because
        /// `error` does not always hold it. Two reachable cases:
        ///
        /// - a member that **opened** for the freshness walk and failed on a
        ///   later one has `result: {…}` and `error: null`, so a classified
        ///   `degraded_reason` would be the only text and the raw diagnostic
        ///   would exist nowhere in the payload;
        /// - `workspace reachability` and `workspace check` have no member table
        ///   at all, so this is the only structured place their degraded members'
        ///   diagnostics can live.
        ///
        /// Classifying a cause must never *destroy* evidence — the sentence is a
        /// reading of the diagnostic, not a replacement for it ([NFR-CC-04]).
        degraded_diagnostic: String,
    },
}

impl MemberOpenState {
    /// The degraded state for a failed open, classifying `diagnostic` against
    /// what is at the store path ([FR-WS-16]).
    ///
    /// The verbatim `diagnostic` is retained whether or not a cause is
    /// identified: the classified sentence is a *reading* of it, never a
    /// substitute ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[must_use]
    pub fn degraded(diagnostic: &str, store: StoreFile) -> Self {
        let cause = classify(diagnostic, store);
        Self::Degraded {
            degraded_cause: cause,
            degraded_reason: cause
                .map_or_else(|| diagnostic.to_string(), |cause| cause.message().to_string()),
            degraded_diagnostic: diagnostic.to_string(),
        }
    }

    /// The reason to show an operator, when this member failed to open.
    ///
    /// The renderer [`DegradedRollup::notice`] uses so the stderr diagnostic can
    /// state each member's cause — the only degraded channel `workspace check`
    /// and `workspace reachability` have, since neither payload carries a member
    /// table ([FR-WS-16]).
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Degraded {
                degraded_reason, ..
            } => Some(degraded_reason),
            _ => None,
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
    /// Names only. The per-member cause, reason and verbatim diagnostic stay on
    /// the member table's own rows, so this is a roll-up and not a second table.
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    pub degraded_members: Vec<String>,
    /// Whether every member in the roster was opened.
    ///
    /// `false` marks the figures derived from the **member rows** — those rows
    /// themselves and the warm roll-up folded from them — as covering fewer than
    /// all members, so a partial answer never reads as a complete one
    /// ([NFR-CC-04]).
    ///
    /// It is **not** the whole payload's completeness marker. A read-model
    /// computed from its own separate walk carries its own — see
    /// `CrossServiceCoverage::covers_all_members`, which is also `false` when a
    /// member *opened fine* and its contract-surface read failed. That is not an
    /// open failure, so it moves neither this marker nor
    /// [`all_opened`](Self::all_opened); a consumer must read the marker
    /// belonging to the figure it is rendering.
    ///
    /// Distinct from [`all_opened`](Self::all_opened): this is `false` for a
    /// member laziness never *attempted* too, which is a coverage fact and not a
    /// failure — which is precisely why the exit code is derived from
    /// `all_opened` and not from here ([BR-45]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [BR-45]: ../../../docs/specs/software-spec.md#327-workspace-federation
    pub covers_all_members: bool,
}

impl DegradedRollup {
    /// Whether every member the workspace declares was opened — the predicate the
    /// **exit code** is derived from ([FR-WS-16], [FR-CL-03]).
    ///
    /// Deliberately not [`covers_all_members`](Self::covers_all_members), which
    /// is also `false` for a member laziness never *attempted*: that is a
    /// coverage fact, not a failure, and gating the exit code on it would fail a
    /// perfectly healthy scoped command ([BR-45]). The two differ exactly in that
    /// case, which is why the choice between them lives here in the core beside
    /// the doc that explains it, rather than in a CLI adapter picking one of two
    /// adjacent fields ([ADR-01]).
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [FR-CL-03]: ../../../docs/specs/requirements/FR-CL-03.md
    /// [BR-45]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
    #[must_use]
    pub const fn all_opened(&self) -> bool {
        self.degraded_members.is_empty()
    }

    /// The human diagnostic naming each degraded member **and its cause**, or
    /// `None` when nothing degraded ([FR-WS-16]).
    ///
    /// Takes the rows rather than reading the roll-up's names, because the cause
    /// lives on the row. That matters most where there is nothing else: this is
    /// the **only** degraded channel `workspace check` and `workspace
    /// reachability` have — neither payload carries a member table (`check`
    /// serialises a bare `Option`, and wrapping it would destroy the
    /// honest-`null` contract [NFR-CC-04] gave it). Naming members without their
    /// cause would leave those two commands exiting 1 with no diagnosis, which is
    /// the misdiagnosis [FR-WS-16] exists to remove, merely made silent.
    ///
    /// [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[must_use]
    pub fn notice<'a>(&self, opens: impl IntoIterator<Item = &'a MemberOpen>) -> Option<String> {
        if self.degraded_members.is_empty() {
            return None;
        }
        let mut lines = format!(
            "warning: {} of {} workspace members could not be opened and are reported \
             degraded. Every figure in this answer covers only the {} that opened \
             (FR-WS-16).",
            self.degraded_members.len(),
            self.members,
            self.opened,
        );
        for open in opens {
            if let Some(reason) = open.state.reason() {
                lines.push_str(&format!("\n  {}: {reason}", open.member));
            }
        }
        Some(lines)
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

    /// The whole classification as a table: diagnostic × store path → cause. A
    /// table because the *precedence* between the two inputs is the rule under
    /// test — direct descriptor evidence must outrank whatever the store path
    /// says, and an **absent** store must claim nothing at all.
    #[test]
    fn the_cause_follows_descriptor_evidence_then_the_store_path() {
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
                "cantopen over an OBSTRUCTED store path",
                OBSERVED,
                StoreFile::Obstructed,
                Some(DegradedCause::StoreObstructed),
            ),
            (
                "cantopen by SQLite's own wording only, path obstructed",
                "applying the read-only connection contract: unable to open database file",
                StoreFile::Obstructed,
                Some(DegradedCause::StoreObstructed),
            ),
            // **The regression this arm exists for.** An absent store file is
            // NOT evidence of a missing store: the store is created on open, so
            // a never-indexed member opens fine and an absent file at failure
            // time is equally consistent with descriptor exhaustion partway
            // through creating it. Claiming `store-obstructed` here would send an
            // operator whose real problem is `ulimit -n` to a `logos index` that
            // cannot help — the same misdiagnosis FR-WS-16 removes, relocated.
            (
                "cantopen over an ABSENT store claims nothing",
                OBSERVED,
                StoreFile::Absent,
                None,
            ),
            (
                "cantopen by SQLite's wording only, store absent",
                "applying the read-only connection contract: unable to open database file",
                StoreFile::Absent,
                None,
            ),
            // The errno-only rendering: a layer that drops `strerror` still
            // classifies, so the fd failure never loses its remedy.
            (
                "errno-only EMFILE, store absent",
                "opening read-only pool connection 3/12: os error 24)",
                StoreFile::Absent,
                Some(DegradedCause::HostResourceLimit),
            ),
            (
                "errno-only ENFILE",
                "opening the writer store: os error 23)",
                StoreFile::Obstructed,
                Some(DegradedCause::HostResourceLimit),
            ),
            (
                "a nearby errno is not descriptor exhaustion",
                "opening the writer store: permission denied (os error 240)",
                StoreFile::Present,
                None,
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
            degraded_diagnostic,
        } = &state
        else {
            panic!("a failed open is degraded: {state:?}");
        };

        assert_eq!(*degraded_cause, Some(DegradedCause::HostResourceLimit));
        // Classifying a cause must not DESTROY the evidence it read: the
        // verbatim diagnostic is retained beside the sentence ([NFR-CC-04]).
        assert_eq!(
            degraded_diagnostic, OBSERVED,
            "the raw diagnostic survives classification"
        );
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

    /// [CR-102]: a present-but-unopenable store states what is **known** before
    /// it advises, and then names **both** candidate causes — file permissions
    /// and the descriptor limit ([FR-WS-16], [NFR-CC-04]).
    ///
    /// The regression this pins is the shipped remedy, which advised `ulimit -n`
    /// alone. On the run that exercised it the descriptor limit was provably not
    /// what failed, so the one piece of advice the operator got was wrong —
    /// while everything the derivation actually *establishes* (the store is
    /// there, a re-index will not help) was true and went unsaid. Order is part
    /// of the assertion: what is known comes first, because that is the half a
    /// reader can rely on.
    ///
    /// [CR-102]: ../../../docs/requests/CR-102-warm-outcome-record-and-spec-corrections.md
    #[test]
    fn a_present_but_unopenable_store_states_what_is_known_then_names_both_causes() {
        let reason = MemberOpenState::degraded(OBSERVED, StoreFile::Present)
            .reason()
            .expect("a failed open has a reason")
            .to_string();

        // What is known, and where it sits: the store is intact and a re-index
        // is not the remedy, stated before any advice.
        let known = reason
            .find("re-index is not the remedy")
            .unwrap_or_else(|| panic!("the reason states what is known: {reason}"));
        assert!(
            reason.contains("store is present"),
            "the intact store is stated as a fact: {reason}"
        );

        // Both candidates, each named — and both AFTER what is known.
        let permissions = reason
            .find("permissions")
            .unwrap_or_else(|| panic!("file permissions are named: {reason}"));
        let descriptors = reason
            .find("RLIMIT_NOFILE")
            .unwrap_or_else(|| panic!("the descriptor limit is named: {reason}"));
        assert!(
            known < permissions && known < descriptors,
            "what is known is stated BEFORE the two candidates: {reason}"
        );
        assert!(
            reason.contains("ulimit -n"),
            "the descriptor candidate keeps its actionable remedy: {reason}"
        );

        // And neither candidate is asserted as THE cause — the sentence says
        // both fit ([NFR-CC-04]).
        assert!(
            reason.contains("both are named"),
            "the reason says both causes fit rather than picking one: {reason}"
        );

        // The wire contract is untouched: same variant, same kebab-case key, and
        // the verbatim diagnostic still rides its own field.
        let state = MemberOpenState::degraded(OBSERVED, StoreFile::Present);
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(
            value["degraded_cause"], "host-resource-limit",
            "no variant added and no wire key renamed: {value}"
        );
        assert_eq!(value["degraded_diagnostic"], OBSERVED);
    }

    /// The widened remedy is confined to the ambiguous cause: an **obstructed**
    /// store path still gets its own, unambiguous remedy, and an unclassified
    /// failure still gets the verbatim diagnostic and nothing invented.
    #[test]
    fn the_other_outcomes_keep_their_own_narrower_remedies() {
        let obstructed = MemberOpenState::degraded(OBSERVED, StoreFile::Obstructed)
            .reason()
            .expect("a failed open has a reason")
            .to_string();
        assert!(
            obstructed.contains("not a regular file") && obstructed.contains("clear that path"),
            "an obstructed path has ONE cause and keeps its own remedy: {obstructed}"
        );
        assert!(
            !obstructed.contains("RLIMIT_NOFILE") && !obstructed.contains("permissions"),
            "and does not inherit the ambiguous cause's two candidates: {obstructed}"
        );

        let unclassified = MemberOpenState::degraded("database disk image is malformed", StoreFile::Present)
            .reason()
            .expect("a failed open has a reason")
            .to_string();
        assert_eq!(
            unclassified, "database disk image is malformed",
            "no cause, no remedy invented — the diagnostic stands alone"
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
            degraded_diagnostic,
        } = &state
        else {
            panic!("a failed open is degraded: {state:?}");
        };
        assert_eq!(*degraded_cause, None, "no cause is invented");
        assert_eq!(degraded_reason, raw, "the diagnostic stands as it is");
        assert_eq!(degraded_diagnostic, raw, "and is carried on its own key too");

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
        let rows = vec![opened("api"), opened("web")];
        let rollup = rollup(&rows);

        assert_eq!(rollup.opened, 2);
        assert!(rollup.degraded_members.is_empty());
        assert!(rollup.covers_all_members);
        assert_eq!(rollup.notice(&rows), None, "nothing to warn about");
    }

    /// A workspace whose members were merely never **attempted** is not
    /// covered-all either — but it is not degraded, so nothing is named and the
    /// exit code is untouched ([BR-45], [NFR-PE-10]).
    #[test]
    fn an_unattempted_workspace_is_partial_but_never_degraded() {
        let rows = vec![opened("api"), not_attempted("web"), not_attempted("svc")];
        let rollup = rollup(&rows);

        assert!(rollup.degraded_members.is_empty(), "laziness names nobody");
        assert_eq!(rollup.notice(&rows), None, "and warns about nobody");
        assert!(
            rollup.all_opened(),
            "and `all_opened` — the exit-code predicate — is TRUE, unlike \
             `covers_all_members`: laziness must never fail a healthy command (BR-45)"
        );
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
        assert!(rollup.all_opened());
        assert_eq!(rollup.notice(std::iter::empty()), None);
    }

    /// The human notice names **every** degraded member, **its cause**, and the
    /// coverage shortfall — an exit code with no named member is what
    /// [FR-WS-16] is replacing, and a named member with no cause is what it is
    /// replacing for `workspace check` and `workspace reachability`, whose
    /// payloads carry no member table at all.
    #[test]
    fn the_notice_names_every_degraded_member_with_its_cause_and_the_shortfall() {
        let rows = vec![
            opened("api"),
            degraded("web", OBSERVED, StoreFile::Present),
            degraded("svc", "database disk image is malformed", StoreFile::Present),
        ];
        let notice = rollup(&rows).notice(&rows).expect("two members degraded");

        assert!(notice.contains("web") && notice.contains("svc"), "{notice}");
        assert!(notice.contains("2 of 3"), "{notice}");
        assert!(
            notice.contains("only the 1 that opened"),
            "the notice states the reduced coverage, grammatically: {notice}"
        );
        // Each member's own reason rides its own line — the classified sentence
        // for the one that classified, the verbatim diagnostic for the one that
        // did not.
        assert!(
            notice.contains("web: not a damaged store"),
            "the classified cause reaches the human channel, leading with what is \
             known: {notice}"
        );
        assert!(
            notice.contains("svc: database disk image is malformed"),
            "and an unclassified failure carries its diagnostic there: {notice}"
        );
        assert!(
            !notice.contains("api"),
            "a healthy member is not named: {notice}"
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

    /// Every [`DegradedCause`]'s **serialized value**, pinned against literals.
    ///
    /// The SPA declares these as a closed union (`DegradedCause` in
    /// `web/ui/src/api/types.ts`), and nothing else in Rust asserts the wire
    /// spelling: dropping the `rename_all` attribute would emit
    /// `"HostResourceLimit"`, leave every Rust test green, and silently break
    /// that union. A hand-maintained cross-language contract needs pinning on
    /// the side that produces it.
    #[test]
    fn every_degraded_cause_has_a_stable_kebab_case_wire_value() {
        for (cause, wire) in [
            (DegradedCause::HostResourceLimit, "host-resource-limit"),
            (DegradedCause::StoreObstructed, "store-obstructed"),
        ] {
            assert_eq!(serde_json::to_value(cause).unwrap(), wire);
        }

        // And it reaches the member row under the `degraded_cause` key, not just
        // as a bare enum.
        let obstructed = MemberOpenState::degraded(OBSERVED, StoreFile::Obstructed);
        let value = serde_json::to_value(&obstructed).unwrap();
        assert_eq!(value["degraded_cause"], "store-obstructed", "{value}");
        assert_eq!(
            value["degraded_diagnostic"], OBSERVED,
            "the verbatim diagnostic rides its own key: {value}"
        );
    }

    /// [FR-WS-16] AC3, the arm the whole story is *for*: a store path occupied by
    /// something that is not a regular file names that fact and its real remedy —
    /// and a *never-indexed* member under the same diagnostic claims **no** cause
    /// at all rather than the re-index that cannot help it.
    #[test]
    fn an_obstructed_path_states_its_remedy_and_an_absent_store_claims_nothing() {
        let obstructed = MemberOpenState::degraded(OBSERVED, StoreFile::Obstructed);
        let reason = obstructed.reason().expect("a degraded member has a reason");
        assert!(
            reason.contains("not a") && reason.contains("regular file"),
            "the reason states what is actually wrong: {reason}"
        );
        assert!(
            reason.contains("clear that path") && reason.contains("logos index"),
            "and the remedy is clear-then-reindex, not reindex alone: {reason}"
        );

        // The absent case: no cause, and specifically NOT the obstructed remedy.
        let absent = MemberOpenState::degraded(OBSERVED, StoreFile::Absent);
        let value = serde_json::to_value(&absent).unwrap();
        assert!(
            value.get("degraded_cause").is_none(),
            "an absent store licenses no cause: {value}"
        );
        assert_eq!(
            absent.reason(),
            Some(OBSERVED),
            "the verbatim diagnostic is what the operator gets"
        );
        assert!(
            !absent.reason().unwrap().contains("logos index"),
            "and it must NOT send an fd-exhausted operator to a re-index"
        );
    }
}
