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
//! used to be its complement; but *being indexed right now* and *was attempted
//! and failed* are facts about a **process**, not about a store. So the
//! derivation takes them as an explicit [`WarmEvidence`] input:
//!
//! - **`warming` is omitted, never inferred** ([NFR-CC-04]). No live in-flight
//!   signal source exists, so the roll-up's `warming` key is *absent from the
//!   JSON* rather than a fabricated `0`, and [`MemberWarmState::Warming`] is
//!   unreachable — not by a convention a later edit can forget, but because
//!   nothing short of [`WarmEvidence::with_in_flight`] can put a member in the
//!   in-flight set, and no production caller calls it.
//! - **A failed member is `degraded`, never `deferred`** ([BR-44]). Three
//!   channels reach here durably and all map to `degraded`: a member whose
//!   engine could not be **started**, a member that started but whose
//!   **freshness read** failed, and a member whose **warm** was attempted and
//!   failed. The second takes care — [`Engine::status`](crate::Engine::status)
//!   is infallible and degrades to a *defaulted* [`StatusInfo`](crate::models::StatusInfo) whose
//!   `indexed: false` is indistinguishable from an honestly empty graph — so
//!   [`workspace_status`](super::query::workspace_status) fans
//!   [`Engine::try_status`](crate::Engine::try_status) and folds its `Err` into the per-member error
//!   channel instead.
//!
//! # The record this module reads ([FR-WS-17])
//! The third channel is the durable one, and it is owned here: [`WarmOutcomes`]
//! is the sidecar's schema, [`write_outcomes`] its atomic write and
//! [`read_outcomes`] its degrade-to-empty read. It sits at the **workspace
//! root** beside the manifest, keyed on [`Member::name`](super::Member::name),
//! and nothing is ever written inside a member's `.logos/` — [FR-WS-14]'s
//! no-member-store property stays literal.
//!
//! It records **outcomes**, not only failures, and that closes what used to be
//! this module's stated residual gap: a member the warm indexed successfully
//! that contains no supported-language file holds no indexed file, so index
//! presence alone reports it `deferred` — "never attempted" — permanently. A
//! recorded success says the warm *completed*, which is the fact `deferred` was
//! asserting the opposite of. One mechanism answers both halves ([FR-WS-17]).
//!
//! Where the record and the store disagree, the **store wins** ([BR-47]): a
//! member that demonstrably holds a graph is never reported degraded, so a
//! member indexed later by a re-run or by the lazy [FR-IX-07] fallback reads
//! `warm` again and no code path has to remember to clear anything. The record
//! is evidence, not a latch.
//!
//! A workspace with **no** record derives from index presence alone, exactly as
//! every build before [FR-WS-17] did — which is also what a malformed,
//! truncated or unreadable one degrades to ([NFR-RA-02]).
//!
//! # No engines are constructed to answer
//! Every function here is pure, over values a `workspace status` walk has
//! already gathered ([NFR-PE-10]): labelling N members costs zero engine
//! starts, zero connections, and zero filesystem reads, so the read-model's
//! resident-engine ceiling is exactly what the fan-out it rides on already
//! paid ([NFR-PE-11]).
//!
//! [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
//! [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
//! [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
//! [FR-IX-07]: ../../../docs/specs/requirements/FR-IX-07.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
//! [NFR-PE-11]: ../../../docs/specs/requirements/NFR-PE-11.md
//! [NFR-RA-02]: ../../../docs/specs/requirements/NFR-RA-02.md
//! [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [BR-47]: ../../../docs/specs/software-spec.md#327-workspace-federation

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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

// ── the durable warm-outcome record (FR-WS-17, BR-47) ──────────────────────

/// The warm-outcome sidecar's filename, at the **workspace root** beside
/// [`MANIFEST_FILENAME`](super::MANIFEST_FILENAME) ([FR-WS-17]).
///
/// At the workspace root and nowhere else: [FR-WS-14] makes "the supervisor
/// holds no member store" a *literal* property — the supervisor skips even
/// telemetry initialisation for it — so a per-member marker inside each
/// member's `.logos/` would have to amend that requirement rather than
/// implement this one ([CR-102] §5.1 CRA-01). One sidecar also costs one write
/// per pass instead of N, and keys on the workspace-relative member name every
/// federation read-model already joins on rather than on an absolute root.
///
/// Dot-prefixed and not the manifest's `.toml`: the manifest is a checked-in
/// file an operator edits, this is machine-written state about the last warm on
/// *this* machine. Nothing reads it but [`read_outcomes`], and its absence is a
/// first-class state (`deferred` by index presence alone), so it is safe to
/// delete and pointless to commit.
///
/// [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
/// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
/// [CR-102]: ../../../docs/requests/CR-102-warm-outcome-record-and-spec-corrections.md
pub const OUTCOME_FILENAME: &str = ".logos.workspace.warm.json";

/// The schema version [`write_outcomes`] stamps and [`read_outcomes`] requires.
///
/// Load-bearing rather than decorative: a record whose `version` is not this
/// one reads as **no record at all**, so a future schema that changes what a
/// member entry *means* degrades an old reader to index-presence derivation
/// instead of letting it misread new bytes under old rules ([NFR-RA-02]).
///
/// [NFR-RA-02]: ../../../docs/specs/requirements/NFR-RA-02.md
pub const OUTCOME_SCHEMA_VERSION: u32 = 1;

/// The largest sidecar [`read_outcomes`] will read into memory.
///
/// A backstop, not a limit anyone should meet: one member entry is well under
/// 200 bytes even with a verbose failure reason, so 8 MiB admits a roster two
/// orders of magnitude larger than the 86-member workspace this feature was
/// built for. Its purpose is that an implausible file at that path degrades to
/// no record rather than allocating whatever it finds on the path of every
/// `workspace status` ([NFR-RA-02]).
///
/// [NFR-RA-02]: ../../../docs/specs/requirements/NFR-RA-02.md
const MAX_RECORD_BYTES: u64 = 8 * 1024 * 1024;

/// What the warm did to one member ([FR-WS-17]).
///
/// Recording **success** as well as failure is the half that is easy to skip
/// and cannot be: a failure-only marker cannot describe a member the warm
/// indexed perfectly well that holds no supported-language file, and that
/// member is misreported `deferred` — "never attempted" — by the same
/// index-presence equation a failure-only record was meant to fix. One
/// mechanism answers both ([FR-WS-17] Notes).
///
/// Internally tagged on `outcome`, the same dialect
/// [`MemberWarmState`] and [`MemberOutcome`](super::enable::MemberOutcome)
/// speak.
///
/// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum WarmOutcome {
    /// The member's index ran to completion. Says nothing about whether it
    /// *found* anything — a member with no supported-language file succeeds
    /// here and holds no indexed file, which is exactly the case index presence
    /// alone gets wrong.
    Succeeded,
    /// The member's index or its spawn failed, with the reason verbatim.
    Failed {
        /// Why the warm failed, as the supervisor recorded it.
        reason: String,
    },
}

/// The sidecar itself: one warm outcome per member name ([FR-WS-17]).
///
/// Keyed by [`Member::name`](super::Member::name) — the workspace-relative path
/// every federation read-model joins on — deliberately **not** by the absolute
/// root [`WarmSummary`](super::warm::WarmSummary) carries, which is a
/// diagnostic label and not join-compatible with anything.
///
/// A [`BTreeMap`] so the serialised bytes are ordered and a re-write that
/// changed nothing produces an identical file.
///
/// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarmOutcomes {
    /// The schema version — [`OUTCOME_SCHEMA_VERSION`] on anything this code
    /// wrote, and the gate [`read_outcomes`] checks before believing a word of
    /// the rest.
    pub version: u32,
    /// Member name → what the warm did to it.
    #[serde(default)]
    pub members: BTreeMap<String, WarmOutcome>,
}

impl Default for WarmOutcomes {
    /// An empty record **at the current schema version** — the value
    /// [`read_outcomes`] returns for every degrade, and the base a merge starts
    /// from, so a written record can never carry a stale or zero version.
    fn default() -> Self {
        Self {
            version: OUTCOME_SCHEMA_VERSION,
            members: BTreeMap::new(),
        }
    }
}

impl WarmOutcomes {
    /// Whether the record names no member at all — the "no record" case, which
    /// derives exactly as it did before [FR-WS-17] existed.
    ///
    /// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

/// Where the sidecar lives for `workspace_root` — beside the manifest, never
/// inside a member ([FR-WS-17]).
///
/// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
#[must_use]
pub fn outcome_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(OUTCOME_FILENAME)
}

/// Write `outcomes` to `workspace_root`'s sidecar **atomically** ([FR-WS-17]).
///
/// The publish itself — sibling temp, `fsync`, `rename`, cleanup on failure —
/// is [`crate::fs_atomic::publish`], shared with `config::writeback` rather
/// than hand-rolled here. That sharing is not tidiness: this function first
/// carried its own copy, and the copy silently dropped the thread id from the
/// temp name, so two threads of one process publishing the same sidecar would
/// have computed the same temp path and published spliced bytes with `Ok(())`
/// returned to both. A concurrent reader sees the whole previous record or the
/// whole new one, never a partial one.
///
/// The version is stamped from [`OUTCOME_SCHEMA_VERSION`] rather than taken
/// from the argument, so no caller can publish a record under a version it does
/// not actually speak.
///
/// # Errors
/// Any I/O failure — an unwritable workspace root, a full disk. Every caller
/// discards it: the record is *evidence*, and a warm whose evidence could not
/// be filed still warmed the members. Losing it degrades `workspace status` to
/// index presence, which is the pre-[FR-WS-17] behaviour, not a fault
/// ([NFR-RA-02]).
///
/// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
/// [NFR-RA-02]: ../../../docs/specs/requirements/NFR-RA-02.md
pub fn write_outcomes(workspace_root: &Path, outcomes: &WarmOutcomes) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(&WarmOutcomes {
        version: OUTCOME_SCHEMA_VERSION,
        members: outcomes.members.clone(),
    })
    .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    crate::fs_atomic::publish(&outcome_path(workspace_root), &bytes, None)
}

/// Read `workspace_root`'s sidecar, degrading to an **empty** record on
/// anything at all ([FR-WS-17], [NFR-RA-02]).
///
/// Infallible by design, and that is the requirement rather than a convenience:
/// no `workspace status`, MCP call or HTTP read may fail because a piece of
/// *advisory* evidence is absent, unreadable, truncated, malformed, or written
/// under a schema version this build does not speak. Every one of those cases
/// returns [`WarmOutcomes::default`], which makes the derivation fall back to
/// index presence — precisely the behaviour of every build before the record
/// existed.
///
/// Note what is deliberately **not** here: no repair, no deletion, no warning.
/// A corrupt sidecar is left exactly as found — the next warm overwrites it
/// atomically anyway — because a read-model that silently rewrote a file on the
/// path of every `status` would be a far worse surprise than a stale one.
///
/// # The size guard
/// This runs on **every** `workspace status`, including the MCP tool and the
/// HTTP surface, and `fs::read` allocates whatever it finds. Reading an
/// implausibly large file at that path into memory is the one input that could
/// turn "degrade quietly" into an abort, which would break the very guarantee
/// this function exists to make. [`MAX_RECORD_BYTES`] is the cheap backstop:
/// orders of magnitude above any real roster, and an oversized file degrades
/// exactly like a malformed one.
///
/// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
/// [NFR-RA-02]: ../../../docs/specs/requirements/NFR-RA-02.md
#[must_use]
pub fn read_outcomes(workspace_root: &Path) -> WarmOutcomes {
    let path = outcome_path(workspace_root);
    let oversized = fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_RECORD_BYTES);
    if oversized {
        tracing::debug!(
            record = %path.display(),
            "warm-outcome record is implausibly large — ignoring it"
        );
        return WarmOutcomes::default();
    }
    let Ok(bytes) = fs::read(&path) else {
        return WarmOutcomes::default();
    };
    let Ok(outcomes) = serde_json::from_slice::<WarmOutcomes>(&bytes) else {
        tracing::debug!(
            record = %path.display(),
            "warm-outcome record is unreadable — warm state falls back to index presence"
        );
        return WarmOutcomes::default();
    };
    if outcomes.version == OUTCOME_SCHEMA_VERSION {
        outcomes
    } else {
        tracing::debug!(
            record = %path.display(),
            version = outcomes.version,
            expected = OUTCOME_SCHEMA_VERSION,
            "warm-outcome record speaks a schema version this build does not"
        );
        WarmOutcomes::default()
    }
}

/// The live warm facts a `workspace status` read has available — the seam that
/// keeps `warming` and warm-failure **inputs** rather than guesses
/// ([NFR-CC-04], [BR-44]).
///
/// The **warm** halves now have a real source: [`with_outcomes`](Self::with_outcomes)
/// over the durable sidecar [`read_outcomes`] returns ([FR-WS-17]). `warming`
/// still has none — no live supervisor signal exists — so it stays omitted, and
/// [`MemberWarmState::Warming`] stays unreachable in production, not by a
/// convention a later edit can forget but because nothing short of
/// [`with_in_flight`](Self::with_in_flight) can populate the in-flight set.
///
/// An **empty** record — no sidecar, or one that failed to read — leaves this
/// value equal to [`none`](Self::none), which is what makes a workspace without
/// a record derive exactly as it did before [FR-WS-17] ([FR-WS-17] AC5).
///
/// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
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
    /// Member name → why its warm was attempted and failed, from the durable
    /// record ([FR-WS-17], [BR-44]).
    ///
    /// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
    /// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
    failed: BTreeMap<String, String>,
    /// Member names whose warm was attempted and **succeeded**, from the same
    /// record.
    ///
    /// Carried separately from `failed` rather than derived as its complement,
    /// because the complement of "failed" over the roster is "failed or never
    /// recorded", and those are the two states this set exists to tell apart: a
    /// recorded success with an empty graph is `warm` (the warm completed and
    /// the member holds nothing indexable), whereas no record at all is
    /// `deferred` ([FR-WS-17], [BR-47]).
    ///
    /// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
    /// [BR-47]: ../../../docs/specs/software-spec.md#327-workspace-federation
    succeeded: BTreeSet<String>,
}

impl WarmEvidence {
    /// No evidence at all: `warming` is omitted and only a member that could
    /// not be opened reads `degraded` ([NFR-CC-04]). The value a workspace with
    /// no durable record derives from, byte-identical to the pre-[FR-WS-17]
    /// behaviour.
    ///
    /// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
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
    /// reason ([BR-44]).
    ///
    /// The failure half of [`with_outcomes`](Self::with_outcomes), kept as its
    /// own constructor so the derivation's precedence table can be exercised
    /// one channel at a time.
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

    /// Take both warm halves from a durable [`WarmOutcomes`] record — the
    /// production input [`read_outcomes`] supplies ([FR-WS-17], [BR-47]).
    ///
    /// An empty record (absent, unreadable, or malformed — [`read_outcomes`]
    /// returns the same value for all three) leaves this evidence equal to
    /// [`none`](Self::none), so nothing about the derivation, the roll-up or the
    /// wire format moves for a workspace that has no record ([FR-WS-17] AC5).
    ///
    /// Recording a success is **not** the same as declaring a live signal:
    /// `warming` stays absent, because a finished outcome says nothing about
    /// what is in flight *now* ([NFR-CC-04]).
    ///
    /// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [BR-47]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[must_use]
    pub fn with_outcomes(mut self, outcomes: &WarmOutcomes) -> Self {
        for (member, outcome) in &outcomes.members {
            match outcome {
                WarmOutcome::Succeeded => {
                    self.succeeded.insert(member.clone());
                }
                WarmOutcome::Failed { reason } => {
                    self.failed.insert(member.clone(), reason.clone());
                }
            }
        }
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

    /// Whether a durable record says `member`'s warm **completed**. The one
    /// fact that separates "indexed nothing because there was nothing to index"
    /// from "never attempted" ([BR-47]).
    ///
    /// [BR-47]: ../../../docs/specs/software-spec.md#327-workspace-federation
    fn succeeded(&self, member: &str) -> bool {
        self.succeeded.contains(member)
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
/// **Demonstrated index presence outranks the durable record** ([BR-47]): a
/// member whose graph holds indexed files is `warm` whatever the record says,
/// so a member indexed later — by a re-run, or lazily on first query
/// ([FR-IX-07]) — reads `warm` again with nothing having to *clear* a stale
/// failure. That is what makes the record safe to keep: it is evidence, not a
/// latch, and no code path anywhere has to remember to delete it.
///
/// Note the exact scope of that rule. It is index **presence** — `Ok(true)` —
/// that beats a record, not an `Err`: a member whose store could not be read at
/// all has demonstrated nothing, so the record still speaks for it. In order:
///
/// 1. `indexed == Ok(true)` combined with a recorded failure — the record is
///    stale, and index presence wins ([BR-47]).
/// 2. `evidence.failure(member)` — a durable record that this member's warm was
///    attempted and failed, for every other index reading.
/// 3. `indexed == Err(reason)` — the member could not be read at all.
/// 4. `evidence.is_warming(member)` — a live in-flight signal.
/// 5. `indexed == Ok(true)` — index presence.
/// 6. `evidence.succeeded(member)` — a durable record that the warm completed:
///    `warm`, not `deferred`, for a member that holds no supported-language
///    file and therefore no indexed file ([FR-WS-17]).
/// 7. `Ok(false)` with no record at all — `deferred`, never attempted.
///
/// In-flight outranks index presence deliberately: a full index persists in
/// bounded chunks ([FR-IX-08]), so a member being indexed right now can already
/// hold files — reporting it `warm` would claim a completed index that is still
/// running.
///
/// A recorded failure ranks **above** the in-flight signal, so a member a live
/// signal calls in-flight while the record says its last warm failed reports
/// `degraded` rather than `warming` — and above the unreadable-store channel,
/// so the *durable* reason, not the transient open error, is the one that
/// reaches the wire. Both orderings are unobservable today (nothing populates
/// the in-flight set) and both are pinned by the precedence table test, so
/// whichever way a future signal source wants them, the change is deliberate.
///
/// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
/// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
/// [FR-IX-07]: ../../../docs/specs/requirements/FR-IX-07.md
/// [FR-IX-08]: ../../../docs/specs/requirements/FR-IX-08.md
/// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [BR-47]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[must_use]
pub fn derive_state(
    member: &str,
    indexed: Result<bool, &str>,
    evidence: &WarmEvidence,
) -> MemberWarmState {
    // The BR-47 rule, and the only guard on the record: a member that
    // demonstrably holds a graph is never reported degraded, so a stale record
    // is outvoted by the store rather than having to be cleared.
    if !matches!(indexed, Ok(true)) {
        if let Some(reason) = evidence.failure(member) {
            return MemberWarmState::Degraded {
                reason: reason.to_string(),
            };
        }
    }
    match indexed {
        Err(reason) => MemberWarmState::Degraded {
            reason: reason.to_string(),
        },
        Ok(_) if evidence.is_warming(member) => MemberWarmState::Warming,
        Ok(true) => MemberWarmState::Warm,
        // A recorded success with an empty graph is a *completed* warm over a
        // member that holds nothing indexable — `warm`, not the "never
        // attempted" claim `deferred` makes ([FR-WS-17]).
        Ok(false) if evidence.succeeded(member) => MemberWarmState::Warm,
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

    /// A record built from `(member, Some(reason) => failed | None => succeeded)`
    /// pairs — the shape most of these assertions want, without a `BTreeMap`
    /// literal in each.
    fn outcomes<'a>(entries: impl IntoIterator<Item = (&'a str, Option<&'a str>)>) -> WarmOutcomes {
        WarmOutcomes {
            members: entries
                .into_iter()
                .map(|(member, reason)| {
                    let outcome = reason.map_or(WarmOutcome::Succeeded, |reason| {
                        WarmOutcome::Failed {
                            reason: reason.to_string(),
                        }
                    });
                    (member.to_string(), outcome)
                })
                .collect(),
            ..WarmOutcomes::default()
        }
    }

    /// The whole derivation as a table: index presence × evidence → label.
    /// A table rather than one test per state, because the *precedence* between
    /// the three inputs is the rule under test and only a table pins it.
    #[test]
    fn the_label_follows_failure_then_in_flight_then_index_presence() {
        let none = WarmEvidence::none();
        let warming = WarmEvidence::none().with_in_flight(["api"]);
        let failed = WarmEvidence::none().with_failures([("api", "store is corrupt")]);
        let succeeded = WarmEvidence::none().with_outcomes(&outcomes([("api", None)]));
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
            // BR-47: demonstrated index presence outranks the record. A member
            // that holds a graph is `warm` even against a stale failure, which
            // is what makes the record safe to never clear.
            ("indexed, warm failed", &failed, Ok(true), MemberWarmState::Warm),
            ("empty, warm failed", &failed, Ok(false), degraded.clone()),
            // A recorded SUCCESS with an empty graph is `warm`, not the "never
            // attempted" claim `deferred` makes: the warm completed and the
            // member holds nothing indexable (FR-WS-17).
            ("empty, warm succeeded", &succeeded, Ok(false), MemberWarmState::Warm),
            ("indexed, warm succeeded", &succeeded, Ok(true), MemberWarmState::Warm),
            // A success record does NOT rescue an unreadable store: the member
            // demonstrated nothing, so the open failure still speaks.
            (
                "unopenable, warm succeeded",
                &succeeded,
                Err("engine start failed"),
                MemberWarmState::Degraded {
                    reason: "engine start failed".to_string(),
                },
            ),
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
            // …but index presence outranks BOTH (BR-47): a member holding a
            // graph while a live signal says it is being re-indexed is
            // `warming`, and in no case `degraded`.
            (
                "in flight and warm failed, indexed",
                &both,
                Ok(true),
                MemberWarmState::Warming,
            ),
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
        // `web` is named by the failure record, so it is degraded — but only
        // while it has no index of its own to argue with ([BR-47]).
        assert_eq!(
            derive_state("web", Ok(false), &evidence),
            MemberWarmState::Degraded {
                reason: "spawn failed".to_string()
            }
        );
        assert_eq!(derive_state("web", Ok(true), &evidence), MemberWarmState::Warm);
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

    // ── FR-WS-17: the durable outcome record ───────────────────────────────

    /// [BR-47]'s truth table verbatim, as the story states it — the four cases
    /// the requirement enumerates, asserted as one block so the *rule* is
    /// reviewable against the spec rather than scattered across four tests.
    ///
    /// It overlaps the precedence table above on purpose: that one pins the
    /// ordering of every input pair including the contested and currently
    /// unobservable ones, this one pins the four combinations a user can
    /// actually produce today.
    ///
    /// [BR-47]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[test]
    fn br47_truth_table() {
        let failed = |reason: &str| MemberWarmState::Degraded {
            reason: reason.to_string(),
        };

        // 1. The warm succeeded but the member holds no supported-language
        //    file: `warm`, never `deferred`.
        let record = outcomes([("api", None)]);
        let evidence = WarmEvidence::none().with_outcomes(&record);
        assert_eq!(derive_state("api", Ok(false), &evidence), MemberWarmState::Warm);

        // 2. Neither record nor index: `deferred`.
        let evidence = WarmEvidence::none().with_outcomes(&WarmOutcomes::default());
        assert_eq!(
            derive_state("api", Ok(false), &evidence),
            MemberWarmState::Deferred
        );

        // 3. The graph holds indexed files: `warm`, EVEN AGAINST a stale record
        //    saying the warm failed.
        let record = outcomes([("api", Some("index failed: exit status: 2"))]);
        let evidence = WarmEvidence::none().with_outcomes(&record);
        assert_eq!(derive_state("api", Ok(true), &evidence), MemberWarmState::Warm);

        // 4. A recorded failure with no index: `degraded`, carrying the
        //    RECORDED reason verbatim.
        assert_eq!(
            derive_state("api", Ok(false), &evidence),
            failed("index failed: exit status: 2")
        );
    }

    /// The record round-trips through the sidecar: what was written is what is
    /// read back, keys and reasons intact.
    #[test]
    fn a_written_record_reads_back_identically() {
        let dir = tempfile::tempdir().expect("workspace root");
        let written = outcomes([
            ("api", None),
            ("services/web", Some("spawn failed: No such file")),
        ]);

        write_outcomes(dir.path(), &written).expect("write");
        let read = read_outcomes(dir.path());

        assert_eq!(read, written);
        assert_eq!(read.version, OUTCOME_SCHEMA_VERSION);
        assert!(
            dir.path().join(OUTCOME_FILENAME).is_file(),
            "the sidecar lives beside the manifest, under its own name"
        );
    }

    /// The version is stamped by the writer, not taken from the caller — so no
    /// caller can publish a record under a version it does not speak.
    #[test]
    fn the_writer_stamps_the_current_schema_version_whatever_it_was_handed() {
        let dir = tempfile::tempdir().expect("workspace root");
        let mut lying = outcomes([("api", None)]);
        lying.version = 99;

        write_outcomes(dir.path(), &lying).expect("write");

        assert_eq!(read_outcomes(dir.path()).version, OUTCOME_SCHEMA_VERSION);
    }

    /// **[NFR-RA-02]**: every way a record can be unusable degrades to the empty
    /// record — which is index-presence derivation, i.e. the pre-[FR-WS-17]
    /// behaviour — and none of them panics or errors.
    ///
    /// A table rather than four tests, because "all of these behave the same"
    /// IS the property: a future reader that grew a distinct error path for one
    /// of them fails here.
    ///
    /// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
    /// [NFR-RA-02]: ../../../docs/specs/requirements/NFR-RA-02.md
    #[test]
    fn an_unusable_record_degrades_to_the_empty_one_and_never_fails() {
        let good = serde_json::to_string(&outcomes([("api", None)])).unwrap();
        let truncated = &good[..good.len() / 2];

        for (case, bytes) in [
            ("malformed", "{ not json at all".to_string()),
            ("truncated", truncated.to_string()),
            ("empty file", String::new()),
            ("right shape, wrong types", r#"{"version":1,"members":[]}"#.to_string()),
            (
                "a future schema version",
                r#"{"version":2,"members":{"api":{"outcome":"succeeded"}}}"#.to_string(),
            ),
            (
                "an unknown outcome word",
                r#"{"version":1,"members":{"api":{"outcome":"exploded"}}}"#.to_string(),
            ),
        ] {
            let dir = tempfile::tempdir().expect("workspace root");
            std::fs::write(dir.path().join(OUTCOME_FILENAME), &bytes).expect("corrupt sidecar");

            let read = read_outcomes(dir.path());

            assert!(read.is_empty(), "{case} must yield no members");
            assert_eq!(
                WarmEvidence::none().with_outcomes(&read),
                WarmEvidence::none(),
                "{case} must derive exactly as a workspace with no record does"
            );
        }
    }

    /// An **absent** sidecar — the overwhelmingly common case, and the one
    /// [FR-WS-17] AC5 pins: identical derivation to a build without the record.
    ///
    /// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
    #[test]
    fn no_record_at_all_derives_exactly_as_before_the_record_existed() {
        let dir = tempfile::tempdir().expect("workspace root");

        let read = read_outcomes(dir.path());
        assert!(read.is_empty());
        assert_eq!(
            WarmEvidence::none().with_outcomes(&read),
            WarmEvidence::none(),
            "an empty record is not evidence"
        );

        // And the observable end of it: same labels, same roll-up, `warming`
        // still omitted rather than zeroed.
        let evidence = WarmEvidence::none().with_outcomes(&read);
        let states: Vec<MemberWarmState> = [("api", Ok(true)), ("web", Ok(false))]
            .into_iter()
            .map(|(m, indexed)| derive_state(m, indexed, &evidence))
            .collect();
        assert_eq!(states, [MemberWarmState::Warm, MemberWarmState::Deferred]);
        assert_eq!(rollup(&states, &evidence).warming, None);
    }

    /// A corrupt sidecar is left **exactly** as found: the read-model does not
    /// repair, delete or rewrite a file on the path of every `status`.
    #[test]
    fn reading_a_corrupt_record_leaves_the_file_untouched() {
        let dir = tempfile::tempdir().expect("workspace root");
        let path = dir.path().join(OUTCOME_FILENAME);
        std::fs::write(&path, "{ garbage").expect("corrupt sidecar");

        let _ = read_outcomes(dir.path());

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ garbage");
    }

    /// The write is **atomic**: a reader running concurrently with a long series
    /// of writes observes a whole record every time, never a partial one, and
    /// no temp file survives the pass.
    ///
    /// The mechanism is a sibling temp plus `rename`, so the assertion that
    /// matters is that no read ever lands on a half-written file. Replacing the
    /// write with a plain `File::create` + `write_all` on the target fails this
    /// reliably.
    #[test]
    fn a_concurrent_reader_never_observes_a_partial_record() {
        let dir = tempfile::tempdir().expect("workspace root");
        let root = dir.path().to_path_buf();
        // A big enough record that a non-atomic write would be split across
        // several `write` syscalls and caught mid-flight.
        let bulky: Vec<(String, Option<String>)> = (0..400)
            .map(|i| (format!("member-{i:04}"), Some("x".repeat(200))))
            .collect();
        let record = WarmOutcomes {
            version: OUTCOME_SCHEMA_VERSION,
            members: bulky
                .iter()
                .map(|(name, reason)| {
                    (
                        name.clone(),
                        WarmOutcome::Failed {
                            reason: reason.clone().unwrap(),
                        },
                    )
                })
                .collect(),
        };
        write_outcomes(&root, &record).expect("seed");

        let stop = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                let mut reads = 0_usize;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let read = read_outcomes(&root);
                    assert_eq!(
                        read.members.len(),
                        400,
                        "a reader saw a partial record: {} members",
                        read.members.len()
                    );
                    reads += 1;
                }
                reads
            });
            for _ in 0..50 {
                write_outcomes(&root, &record).expect("write");
            }
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            let reads = reader.join().expect("reader");
            assert!(reads > 0, "the reader must actually have run");
        });

        let leftovers: Vec<String> = std::fs::read_dir(&root)
            .expect("root")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left behind: {leftovers:?}");
    }

    /// A record naming members the workspace does not have, or omitting ones it
    /// does, changes nothing for the members it does not name — the record is
    /// per-member evidence, not a workspace-wide verdict.
    #[test]
    fn the_record_speaks_only_for_the_members_it_names() {
        let record = outcomes([("api", Some("index failed"))]);
        let evidence = WarmEvidence::none().with_outcomes(&record);

        assert_eq!(derive_state("web", Ok(false), &evidence), MemberWarmState::Deferred);
        assert_eq!(derive_state("web", Ok(true), &evidence), MemberWarmState::Warm);
        assert_eq!(
            derive_state("api", Ok(false), &evidence),
            MemberWarmState::Degraded {
                reason: "index failed".to_string()
            }
        );
    }

    /// The roll-up partition survives the new evidence source, and `warming` is
    /// still **omitted** — a finished outcome is not a live in-flight signal
    /// ([NFR-CC-04], [FR-WS-17] AC7).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn a_record_never_makes_the_rollup_report_warming() {
        let record = outcomes([
            ("api", Some("index failed")),
            ("web", None),
            ("svc", None),
        ]);
        let evidence = WarmEvidence::none().with_outcomes(&record);
        let states: Vec<MemberWarmState> = [("api", Ok(false)), ("web", Ok(false)), ("svc", Ok(true))]
            .into_iter()
            .map(|(m, indexed)| derive_state(m, indexed, &evidence))
            .collect();

        let rollup = rollup(&states, &evidence);

        assert_eq!(rollup.warming, None, "an outcome is not an in-flight signal");
        assert_eq!((rollup.members, rollup.warm, rollup.deferred, rollup.degraded), (3, 2, 0, 1));
        assert_eq!(
            rollup.warm + rollup.warming.unwrap_or(0) + rollup.deferred + rollup.degraded,
            rollup.members,
            "the counts must still partition the member set"
        );
        let value = serde_json::to_value(&rollup).unwrap();
        assert!(value.get("warming").is_none(), "the key stays absent: {value}");
    }

    /// The record's own wire shape, pinned against literals: a schema change is
    /// a failing test rather than a silently unreadable sidecar on every
    /// machine that upgraded.
    #[test]
    fn the_record_serialises_to_its_documented_shape() {
        let value = serde_json::to_value(outcomes([("api", None), ("web", Some("boom"))])).unwrap();

        assert_eq!(value["version"], OUTCOME_SCHEMA_VERSION);
        assert_eq!(value["members"]["api"]["outcome"], "succeeded");
        assert_eq!(
            value["members"]["api"].as_object().unwrap().len(),
            1,
            "a success is a bare label, no reason key"
        );
        assert_eq!(value["members"]["web"]["outcome"], "failed");
        assert_eq!(value["members"]["web"]["reason"], "boom");
    }


    /// An implausibly large file at the sidecar's path degrades like any other
    /// unusable record instead of being read into memory.
    ///
    /// [`read_outcomes`] runs on every `workspace status`, including the MCP
    /// tool and the HTTP surface, so an unbounded `fs::read` there is the one
    /// input that could turn "degrade quietly" into an abort — breaking the
    /// guarantee the function exists to make ([NFR-RA-02]).
    ///
    /// A **sparse** file, so the assertion costs no disk: the guard reads the
    /// length from metadata and never opens it.
    #[test]
    fn an_implausibly_large_record_is_ignored_rather_than_read_into_memory() {
        let dir = tempfile::tempdir().expect("workspace root");
        let path = outcome_path(dir.path());
        let file = std::fs::File::create(&path).expect("sidecar");
        file.set_len(MAX_RECORD_BYTES + 1).expect("sparse length");
        drop(file);

        let read = read_outcomes(dir.path());

        assert!(read.is_empty(), "an oversized record yields no members");
        assert_eq!(
            std::fs::metadata(&path).expect("sidecar").len(),
            MAX_RECORD_BYTES + 1,
            "and, like every other unusable record, it is left untouched"
        );
    }

    /// A record right **at** the cap is still read — the guard rejects only what
    /// exceeds it, so the boundary is not off by one in the direction that would
    /// silently discard a legitimate roster.
    #[test]
    fn a_record_at_the_size_cap_is_still_read() {
        let dir = tempfile::tempdir().expect("workspace root");
        let record = outcomes([("api", None)]);
        write_outcomes(dir.path(), &record).expect("write");
        assert!(
            std::fs::metadata(outcome_path(dir.path())).unwrap().len() <= MAX_RECORD_BYTES,
            "a real record is nowhere near the cap"
        );

        assert_eq!(read_outcomes(dir.path()), record);
    }

    /// A failed write returns the error and leaves no temp behind — the error
    /// path of [`write_outcomes`]' documented contract, which no test reached
    /// while every call site used `.expect("write")`.
    #[test]
    fn a_failed_write_returns_the_error_and_leaves_no_temp_behind() {
        let dir = tempfile::tempdir().expect("workspace root");
        // `rename` onto a directory cannot succeed for any user, on any
        // platform — deterministic where a permission bit is not.
        std::fs::create_dir(outcome_path(dir.path())).expect("obstruct the sidecar");

        assert!(write_outcomes(dir.path(), &outcomes([("api", None)])).is_err());

        let leftovers: Vec<String> = std::fs::read_dir(dir.path())
            .expect("root")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp left behind: {leftovers:?}");
    }

}
