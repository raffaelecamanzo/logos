//! **S-392 — the one-hop parameter-forwarding residue** ([CR-121] CRA-05 and
//! CRA-06, [FR-WS-23], [FR-WS-10], [NFR-CC-04]), **and S-416 — the same residue
//! at two frames** ([CR-131] CRA-03, cluster C1).
//!
//! Two gates live here, over one population and one set of call sites. Read the
//! S-392 sections below first: they define the population, the classifier and
//! the one-frame bound, and S-416's figure is an INCREMENT over them, asserted
//! to reproduce them exactly. The S-416 material starts at
//! [`TWO_FRAME_DECLARED_FLOOR`] and the second frame itself is the "Pass 3"
//! section.
//!
//! **The two bounds are different and neither is dataflow.** S-392 measures ONE
//! frame; S-416 measures TWO — positional, intra-module, main-tree, refusing on
//! disagreement, with no recursion and no fixpoint in either. A third frame is
//! refused by construction and counted as its own residue. Wherever a sentence
//! below says "one level" or "one hop" it is describing S-392's bound, which
//! that story's figures are still stated over; it is not a claim about this
//! module as a whole. [CR-131] §7 records "the two-frame bound is read as a
//! licence for general dataflow" as a risk against [ADR-64], so: a pass licenses
//! two frames on this idiom and nothing wider.
//!
//! # The question
//!
//! [CR-121] CRA-05 asserts that *"the topic operand is syntactically present at
//! the wrapper's call sites, so one hop suffices for the production publish
//! sites"*, and it is recorded **unvalidated**. [FR-WS-23] is created GATED on
//! this measurement; if the measurement falsifies it, the requirement is marked
//! WITHDRAWN in place and [S-393] and [S-394] stay unplanned.
//!
//! The hop this module measures is exactly the one [FR-WS-23] fixes in its
//! acceptance criteria — **one level, one build module, no recursion, no
//! fixpoint**: a topic operand that is a bare parameter of a project-local
//! publish helper resolves by reading the corresponding positional argument at
//! that method's call sites, and only when every in-module call site agrees.
//!
//! # Why this population and no other
//!
//! [FR-WS-10]'s withdrawal note, and the `brokers.scm` header-form comment that
//! S-370 wrote beside it, already enumerate the estate: **54** header-form
//! publish sites across 52 files, **13 of them in `src/main`**, spanning 12
//! members; 19 pass an identifier and 35 read a `@ConfigurationProperties`
//! getter. S-365 then measured that **0 of those 13** production sites resolve
//! without the hop, and that all 38 arm-level admits are IT test classes.
//!
//! So production source is the population, and the split is not bookkeeping:
//! averaging the two trees is precisely how [CR-117] looked validated at arm
//! level (38 of 54, 70%) while being false on the population its acceptance
//! criterion was written over. Every figure here is reported per
//! [`Tree`](super::configuration_agreement::Tree), and the verdict is read off
//! the production row.
//!
//! # The floors, each declared before its run
//!
//! S-392: see [`DECLARED_FLOOR`] and [`ONE_HOP_FLOOR`]. The declaration was
//! written to `docs/planning/sprints/.pending/S-392-T1-floor.txt` at
//! 2026-09-12T12:33:41Z, before any of this module existed, and is reproduced
//! here byte-for-byte because that directory is gitignored.
//!
//! S-416: see [`TWO_FRAME_DECLARED_FLOOR`] and [`TWO_FRAME_FLOOR`]. That one is
//! **tracked in the first place** — committed on its own, before any of S-416's
//! measurement code existed, so the commit carrying it is the timestamp and no
//! copy step can go wrong. The figure is not re-derived: [CR-131] §3.2 C1 states
//! it as 7 of 13 "on S-392's own metric", and re-deriving a floor for a
//! re-proposal is how a gate gets quietly lowered to fit the second attempt.
//!
//! # What it does NOT mirror
//!
//! The parent harness already owns two pieces of judgement this module must not
//! re-spell, because a hand-written twin that later diverges is how a
//! measurement goes quietly wrong:
//!
//! - **What counts as a publish site** is
//!   [`collect_header_publishes`](super::configuration_agreement::collect_header_publishes).
//!   This module runs its own query only to recover the *AST node* for a site
//!   that function already recognised, and
//!   [`Findings::sites_without_a_node`] is asserted zero — so the two lists are
//!   the same list, not two readings of one corpus.
//! - **What counts as a bare parameter** is
//!   [`resolve_key`](super::configuration_agreement::resolve_key) returning
//!   [`Refusal::MethodParameter`], the same classifier that produced S-365's
//!   `broker/main 13` row. This module adds no predicate of its own; it only
//!   walks the tree for the parameter's *position*, which is arithmetic, not
//!   judgement.
//!
//! # Running it
//!
//! ```text
//! LOGOS_REF_WORKSPACE=~/source/pec-services \
//!   cargo test -p logos-core --test operand_resolvability -- forwarding --nocapture
//! ```
//!
//! The corpus is not in this repository, so the gate **skips** without it. The
//! fixtures below always run, so no column above rests on an unexercised path.
//!
//! [CR-117]: ../../../docs/requests/CR-117-broker-publish-capture-and-the-topic-key-namespace.md
//! [CR-121]: ../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
//! [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
//! [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
//! [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
//! [FR-WS-23]: ../../../docs/specs/requirements/FR-WS-23.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [S-393]: ../../../docs/planning/journal.md#s-393-a-parameter-passed-operand-resolves-one-hop-within-its-module
//! [S-394]: ../../../docs/planning/journal.md#s-394-config-resolved-topic-identity-so-a-publish-meets-a-subscribe

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

use logos_core::extract::{self, FileInput, SymbolContext};
use logos_core::model::EdgeKind;
use logos_core::plugin::LanguageRegistry;

use super::configuration_agreement::{
    collect_header_publishes, header_publish_query, judge, resolve_key, ConfigCorpus, KeyOutcome,
    CorpusLookup, Judge, PropertiesIndex, Refusal, Resolver, Tree, Verdict,
};
use super::{classify, folded_text, operand_name, operands, Unit, FOLD_DEPTH};

/// The recorded verdict, reproduced by the run and printed by it.
pub const RECORDED_FINDING: &str = include_str!("forwarding_finding.txt");

/// **The floor, as declared before the run** — a byte-for-byte copy of
/// `docs/planning/sprints/.pending/S-392-T1-floor.txt`, written at
/// 2026-09-12T12:33:41Z, before any of this module existed.
///
/// Copied here because the pending directory is **gitignored**: the original is
/// untracked and vanishes when the sprint's coordination directory is cleared,
/// which would leave a blocking gate's central evidence resting on a file mtime
/// that no longer exists. `include_str!` makes the declaration part of the
/// build, and [`fixtures::the_floor_is_the_one_declared_before_the_run`] parses
/// the figure out of this text and compares it to [`ONE_HOP_FLOOR`] — so the
/// constant cannot be edited to clear a future run without the declaration
/// being edited too, in a file whose whole purpose is to say it must not be.
pub const DECLARED_FLOOR: &str = include_str!("forwarding_floor.txt");

/// The materiality floor: **7 of the 13** production publish sites must resolve
/// at one hop.
///
/// One half of [CR-121] §6's "the 13 production publish sites resolve, and
/// `Producer` nodes rise from 0 to at least 13", rounded up — the same rule
/// S-384's identity floor applied to that section's other headline figure. 7 is
/// also a strict majority of 13, and the two derivations converging is why the
/// number is 7 rather than 6 or 8: below a majority, one hop is the *exception*
/// on this estate and [FR-WS-23] would be fixing a minority idiom into a `Must`.
///
/// [CR-121]: ../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
/// [FR-WS-23]: ../../../docs/specs/requirements/FR-WS-23.md
pub const ONE_HOP_FLOOR: usize = 7;

/// The enumerated production publish population, from [FR-WS-10]'s withdrawal
/// note and `plugins/java/queries/brokers.scm`'s header-form comment: 13
/// `src/main` sites of 54. A run that finds **fewer** is looking at a different
/// corpus, so it is VOID rather than falsifying — see
/// [`Findings::non_vacuity`].
///
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
pub const PRODUCTION_PUBLISH_SITES: usize = 13;

// ── S-416: the two-frame reading ────────────────────────────────────────────

/// **The two-frame floor, as declared before the run** — [CR-131] §3.2 C1's
/// blocking gate.
///
/// Unlike [`DECLARED_FLOOR`], which had to be copied into the tree because
/// `.pending/` is gitignored, this declaration is **tracked from the start**:
/// it was committed on its own, before any of S-416's measurement code existed,
/// so the commit that carries it is itself the timestamp.
/// [`fixtures::the_two_frame_floor_is_the_one_declared_before_the_run`] parses
/// the figure out of this text and compares it to [`TWO_FRAME_FLOOR`].
///
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
pub const TWO_FRAME_DECLARED_FLOOR: &str = include_str!("two_frame_forwarding_floor.txt");

/// The materiality floor for the two-frame reading: **7 of the 13** production
/// publish sites, on S-392's own metric.
///
/// The number is **not re-derived**. [CR-131] §3.2 C1 states it as "at least 7
/// of the 13 production publish sites resolve, *on S-392's own metric*", and
/// [`ONE_HOP_FLOOR`] already carries the three derivations. Re-deriving a floor
/// for a re-proposal is how a gate gets quietly lowered to fit what the second
/// attempt can reach — see `two_frame_forwarding_floor.txt`.
///
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
pub const TWO_FRAME_FLOOR: usize = 7;

/// S-392's seven named two-frame sites, reproduced from `forwarding_finding.txt`
/// verbatim: the module, and the `src/main` call site that decides each.
///
/// The reconciliation AC is stated over these by NAME, so they are a constant
/// and not a prose paragraph: a site S-392 named that this run cannot even see
/// is harness drift, and the run is VOID rather than falsifying. The join key is
/// the file's **basename and line**, which is how the finding wrote them and the
/// only part of the path that file records.
/// S-416's recorded verdict, reproduced by the run and printed by it.
pub const TWO_FRAME_RECORDED_FINDING: &str = include_str!("two_frame_forwarding_finding.txt");

/// The decisive figure: production publish sites resolved at two frames with
/// agreement over `src/main` call sites only. **Above [`TWO_FRAME_FLOOR`].**
pub const RECORDED_TWO_FRAME_MAIN_ONLY: usize = 8;

/// The same at two frames with every in-module call site admitted. Zero: the
/// second frame buys nothing without the main-tree rule, and the two relaxations
/// are reported as the dependent pair they are rather than as two independent
/// gains.
pub const RECORDED_TWO_FRAME_ALL_SITES: usize = 0;

/// What the SECOND FRAME itself bought under the decisive reading — resolved at
/// two frames, refused at one. The gate would have failed at 6 without it, so
/// this is the quantity the verdict actually turns on.
pub const RECORDED_BOUGHT_BY_FRAME_TWO: usize = 2;

/// How many of S-392's seven named two-frame sites resolve. Evidence for the
/// headline, never a second headline.
pub const RECORDED_NAMED_SITES_RESOLVED: usize = 2;

/// Production refusals at two frames, and how many of them look up a forwarding
/// name their own build module declares more than once.
///
/// The two being **equal** is the finding's central caveat: every refusal is
/// attributable to this harness binding frame two on `(name, arity)` rather than
/// to the estate, so the headline is a floor on what [CR-131] C2's
/// receiver-aware rule would resolve. If a future run separates them, the caveat
/// no longer covers the residue and the finding must be re-read.
///
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
pub const RECORDED_TWO_FRAME_REFUSALS: usize = 5;
pub const RECORDED_REFUSALS_FROM_NAME_AMBIGUITY: usize = 5;

pub const S392_NAMED_TWO_FRAME_SITES: [(&str, &str); 7] = [
    ("archive-listener-adapter", "ArchiveEventKafkaProducer.java:16"),
    ("deprecated-mailbox-core/manager", "DelayedMessageKafkaProducer.java:23"),
    ("pecserver-facade", "PecServerOperationKafkaProducer.java:23"),
    ("archive-manager", "DelayedMessageKafkaProducer.java:21"),
    ("mailbox-manager", "DelayedMessageKafkaProducer.java:23"),
    ("official-log-export-job-worker", "OfficialLogOutcomeKafkaProducer.java:17"),
    ("official-log-export-job-manager", "DelayedMessageKafkaProducer.java:19"),
];

/// The plugin whose expression shapes this module reads. A literal here and
/// nowhere in `logos-core`, for the reason the parent module's carve-out gives.
const JAVA: &str = "java";

/// How many blocking call sites are listed per candidate before the evidence
/// list is truncated. The cap is disclosed in the output when it bites.
const BLOCKER_SAMPLE: usize = 4;

// ── The recorded finding, pinned ────────────────────────────────────────────
//
// S-392 measured CRA-05 as FALSIFIED. These constants pin the figures so a
// change that flips the finding has to be DECIDED rather than absorbed into a
// green run. Re-open CR-121 §8 and re-decide the CR before relaxing any of
// them; do not edit one to make the suite pass.

/// Production publish sites resolved under FR-WS-23 AC1 as written — every call
/// site in the build module must agree.
pub const RECORDED_RESOLVED: usize = 0;

/// The same figure with agreement taken over `src/main` call sites only: the
/// most favourable reading of AC1 available. Still below [`ONE_HOP_FLOOR`],
/// which is what makes the falsification independent of the reading.
pub const RECORDED_RESOLVED_MAIN_ONLY: usize = 6;

/// Production publish sites whose topic is **two** call frames away, in
/// production source — the mechanism that decides this gate, and the one no
/// choice of floor or reading of AC1 can move.
pub const RECORDED_TWO_OR_MORE_HOPS: usize = 7;

/// Production publish sites refused **solely** by test-tree call sites —
/// Mockito stubs of the wrapper. The complement of the figure above.
pub const RECORDED_REFUSED_BY_TEST_ONLY: usize = 6;

// ── What one hop can land on ────────────────────────────────────────────────

/// What a positional argument at a call site resolves to, at **one** hop.
///
/// A single [`NeedsAnotherHop`](ArgValue::NeedsAnotherHop) among a method's call
/// sites refuses the whole method, because [FR-WS-23] admits a topic only when
/// *every* in-module call site supplies an agreeing, terminal operand. That
/// precedence is applied by [`decide`] and mirrored by the blocker ranking in
/// [`decide_every_candidate`]; it is **not** derived from the declaration order
/// below, and this type deliberately derives no `Ord` — a second spelling of the
/// precedence is a second thing to keep in step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgValue {
    /// A topic the source proves outright: a static string literal — `${…}`
    /// placeholders included, under the same grammatical-shape rule
    /// `brokers.scm` applies — **or a same-unit constant folded to one**
    /// (`arg_value` step 3). For the folded case the topic is precisely *not*
    /// "as written" at the call site, which is why this says "proves" rather
    /// than "is".
    Literal(String),
    /// A configuration key: a `@ConfigurationProperties` accessor or a
    /// `@Value`-annotated name that [FR-WS-19] resolves against committed
    /// sources.
    Key(String),
    /// The argument is **itself** a bare parameter of the caller. Resolving it
    /// needs a second hop, which [FR-WS-23] puts out of scope by construction.
    NeedsAnotherHop,
    /// Some other refusal — reported with its reason rather than bucketed.
    Unresolvable(Refusal),
    /// **S-416 only.** The argument was a bare parameter of the caller, the
    /// second frame was taken, and it refused — with a reason that is not "the
    /// argument is a parameter again". That case maps to
    /// [`NeedsAnotherHop`](ArgValue::NeedsAnotherHop) instead, so that [`decide`]
    /// applies exactly one precedence at either depth and the hop-count refusal
    /// keeps outranking an unresolvable operand at two frames as it does at one.
    ///
    /// Frame one never produces this variant, which is why adding it cannot move
    /// a single S-392 figure.
    FrameTwoRefused(FrameTwoRefusal),
}

impl ArgValue {
    /// The identity two call sites are compared on. `None` for a value that
    /// does not resolve at all, which never agrees with anything.
    fn identity(&self) -> Option<String> {
        match self {
            Self::Literal(text) => Some(format!("lit:{text}")),
            Self::Key(key) => Some(format!("key:{key}")),
            Self::NeedsAnotherHop | Self::Unresolvable(_) | Self::FrameTwoRefused(_) => None,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Literal(text) => format!("literal {text:?}"),
            Self::Key(key) => format!("config key {key}"),
            Self::NeedsAnotherHop => "a parameter of the caller (two or more hops)".into(),
            Self::Unresolvable(r) => format!("unresolvable: {}", r.label()),
            Self::FrameTwoRefused(t) => format!("second frame refused: {}", t.label()),
        }
    }
}

/// Where an argument that is itself a bare parameter comes from: the method
/// enclosing the call site, and the positional slot the parameter occupies in
/// **that** method's own signature.
///
/// This is the only new piece of arithmetic S-416 adds, and it is the same
/// arithmetic [`push_candidate`] already does for a publish site's operand —
/// [`declaring_scope`] then [`parameter_slot`], both reused rather than
/// re-spelled. A second frame is therefore the first frame applied once more to
/// a target the first frame named, not a new mechanism.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forward {
    /// The method enclosing the call site — the thing whose callers frame two
    /// reads.
    pub callee: Callee,
    /// The slot the forwarded parameter occupies in that method's signature.
    pub slot: usize,
    /// The class that declares it.
    ///
    /// [`Callee`] is all the `Calls` ledger's target text can express, and on
    /// this estate it is **ambiguous by construction**: S-392 recorded 7 wanted
    /// names declared twice inside one build module, every one of them "a base
    /// `KafkaProducer.sendMessage/3` together with a subclass override of the
    /// same signature". At one frame that ambiguity changed no verdict and
    /// S-392 checked all seven by hand. At two frames it is decisive, so frame
    /// two needs to know *which class* declares the method it is looking up —
    /// see [`dispatches_past`].
    pub declaring_class: String,
}

/// What the **second** frame proved about one forwarded argument.
///
/// Each variant is a distinct, countable fault, for the reason [`Residue`]'s
/// are: [NFR-CC-04] wants a diagnosis and not a bucket. `Residue` itself is
/// deliberately left alone — it is S-392's published vocabulary, its `ALL`
/// drives a census the recorded finding reproduces row for row, and widening it
/// would silently add a row to a table that is durable evidence.
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameTwo {
    /// The forwarding method's own callers agreed on one terminal value.
    Resolved(ArgValue),
    /// They did not, for a named reason.
    Refused(FrameTwoRefusal),
}

impl FrameTwo {
    pub fn label(&self) -> String {
        match self {
            Self::Resolved(v) => format!("resolved to {}", v.label()),
            Self::Refused(r) => r.label(),
        }
    }
}

/// Why a second frame refused.
///
/// Split from [`FrameTwo`] rather than folded into it so that a refusal cannot
/// carry a value: [`ArgValue::FrameTwoRefused`] takes one of these, and the type
/// therefore says outright that a refused frame contributes nothing to the
/// agreement. (It also breaks the `ArgValue` <-> `FrameTwo` cycle, which is how
/// the split came to be noticed.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameTwoRefusal {
    /// The argument is a bare parameter **again**: three or more frames, beyond
    /// the bound [CR-131] §3.2 C1 declares. This is the residue the two-frame
    /// bound turns on, exactly as [`Residue::TwoOrMoreHops`] is the one the
    /// one-frame bound turned on.
    ///
    /// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
    ThreeOrMoreFrames,
    /// The forwarding method is called nowhere the reading admits: nowhere at
    /// all, or — under the main-tree rule — only from the test tree.
    NoCallSite,
    /// Call sites exist, and every one of them is outside the build module the
    /// hop is fixed inside.
    OutOfModuleCaller,
    /// The forwarding method's callers resolve, and disagree. Emits per-site
    /// candidates and no edge, never an average.
    DisagreeingCallers,
    /// A caller's argument resolves to nothing at all — on this estate, a
    /// `Mockito.any()`.
    Unresolvable(Refusal),
    /// The forwarded argument could not be attributed to a method parameter slot
    /// at all: a `Foo::bar` method reference, which supplies no argument, or a
    /// constructor or lambda parameter, which is not a method call site. There
    /// is nothing to follow, so there is no second frame to take.
    NotFollowable,
}

impl FrameTwoRefusal {
    pub fn label(&self) -> String {
        match self {
            Self::ThreeOrMoreFrames => "the caller's caller passes a parameter (3+ frames)".into(),
            Self::NoCallSite => "the forwarding method has no call site the reading admits".into(),
            Self::OutOfModuleCaller => "every caller of the forwarding method is out of module".into(),
            Self::DisagreeingCallers => "the forwarding method's callers disagree".into(),
            Self::Unresolvable(r) => format!("a caller's argument resolves to nothing: {}", r.label()),
            Self::NotFollowable => "nothing to follow (method reference, or not a method)".into(),
        }
    }

    /// The order the **census** attributes a refusal in when more than one
    /// second frame blocked one candidate.
    ///
    /// This is a REPORTING rule and deliberately not a `derive(Ord)`: the
    /// decision itself is [`decide`]'s and lives in exactly one place. This
    /// says which of several blocking frames gets named in the census row, and
    /// it follows `decide`'s precedence so that the reason printed is the reason
    /// the verdict rests on — hop count first, then an operand that resolves to
    /// nothing, then a boundary, then a disagreement.
    fn census_rank(&self) -> u8 {
        match self {
            Self::ThreeOrMoreFrames => 0,
            Self::Unresolvable(_) => 1,
            Self::NotFollowable => 2,
            Self::NoCallSite => 3,
            Self::OutOfModuleCaller => 4,
            Self::DisagreeingCallers => 5,
        }
    }
}

/// Why a candidate did not resolve **at two frames**.
///
/// The first three variants are frame-one faults a second frame cannot help;
/// the next four are frame-two faults. They are kept apart because the whole
/// point of the gate is to say what the second frame bought, and a census that
/// pooled "no call site at frame one" with "no call site at frame two" could not
/// say it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TwoFrameResidue {
    /// Frame one: the operand is a constructor or lambda parameter.
    NotAMethodParameter,
    /// Frame one: the wrapper is called nowhere the reading admits.
    NoCallSiteAtFrameOne,
    /// Frame one: every call site of the wrapper is outside its build module.
    OutOfModuleAtFrameOne,
    /// Frame two: the argument is a bare parameter again.
    ThreeOrMoreFrames,
    /// Frame two: the forwarding method is called nowhere the reading admits.
    NoCallSiteAtFrameTwo,
    /// Frame two: every caller of the forwarding method is out of module.
    OutOfModuleCaller,
    /// Frame two: the forwarding method's callers disagree.
    DisagreeingCallers,
    /// Either frame: an admitted call site's argument resolves to nothing. On
    /// this estate this is the Mockito-stub mechanism, and the main-tree rule is
    /// what removes it.
    UnresolvableOperand,
    /// Frame one: the wrapper's own call sites disagree, after both frames.
    DisagreeAtFrameOne,
}

impl TwoFrameResidue {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotAMethodParameter => "operand is a constructor/lambda parameter",
            Self::NoCallSiteAtFrameOne => "the wrapper has no call site the reading admits",
            Self::OutOfModuleAtFrameOne => "every wrapper call site is outside the build module",
            Self::ThreeOrMoreFrames => "three or more frames",
            Self::NoCallSiteAtFrameTwo => "no call site for the forwarding method (frame 2)",
            Self::OutOfModuleCaller => "out-of-module caller (frame 2)",
            Self::DisagreeingCallers => "disagreeing callers (frame 2)",
            Self::UnresolvableOperand => "an admitted call site's argument resolves to nothing",
            Self::DisagreeAtFrameOne => "the wrapper's call sites disagree",
        }
    }

    pub const ALL: [Self; 9] = [
        Self::NotAMethodParameter,
        Self::NoCallSiteAtFrameOne,
        Self::OutOfModuleAtFrameOne,
        Self::ThreeOrMoreFrames,
        Self::NoCallSiteAtFrameTwo,
        Self::OutOfModuleCaller,
        Self::DisagreeingCallers,
        Self::UnresolvableOperand,
        Self::DisagreeAtFrameOne,
    ];
}

/// Why a candidate did **not** resolve at one hop. Each variant is a distinct,
/// countable fault, so the residue is a diagnosis rather than a bucket
/// ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Residue {
    /// The operand is a parameter of a constructor or a lambda, not of a
    /// method. [FR-WS-23] says "method", and a lambda's argument slot is not a
    /// call site the ledger records.
    NotAMethodParameter,
    /// The method is never called anywhere in the estate — dead, or reached
    /// only reflectively.
    NoCallSites,
    /// Call sites exist, but every one of them is outside the operand's own
    /// build module. [FR-WS-23] AC3 refuses these by name.
    OutOfModuleOnly,
    /// At least one in-module call site passes a parameter of its own: two or
    /// more hops. **This is the residue the bound turns on.**
    TwoOrMoreHops,
    /// At least one in-module call site's argument resolves to nothing at all.
    UnresolvableOperand,
    /// Every in-module call site resolves, but they do not agree. [FR-WS-23]
    /// AC2 emits per-site candidates and no edge; it is never averaged.
    Disagree,
}

impl Residue {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotAMethodParameter => "operand is a constructor/lambda parameter",
            Self::NoCallSites => "method has no call site anywhere",
            Self::OutOfModuleOnly => "every call site is outside the build module",
            Self::TwoOrMoreHops => "an in-module call site passes a parameter (2+ hops)",
            Self::UnresolvableOperand => "an in-module call site's argument resolves to nothing",
            Self::Disagree => "in-module call sites disagree",
        }
    }

    pub const ALL: [Self; 6] = [
        Self::NotAMethodParameter,
        Self::NoCallSites,
        Self::OutOfModuleOnly,
        Self::TwoOrMoreHops,
        Self::UnresolvableOperand,
        Self::Disagree,
    ];
}

/// One hop's outcome for one candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hop {
    /// Every in-module call site agreed on one terminal value.
    Resolved { value: ArgValue, call_sites: usize, caller_files: usize },
    Refused(Residue),
}

impl Hop {
    pub fn resolved(&self) -> bool {
        matches!(self, Self::Resolved { .. })
    }

    fn residue(&self) -> Option<Residue> {
        match self {
            Self::Refused(r) => Some(*r),
            Self::Resolved { .. } => None,
        }
    }
}

/// Which arm a candidate belongs to. The two are never summed: the broker arm
/// carries the floor, the client arm is a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Arm {
    /// A `setHeader(KafkaHeaders.TOPIC, …)` publish site (S-370).
    BrokerPublish,
    /// A gate-admitted client call whose path operand resolves to no key
    /// ([CR-115] CRA-01's 30-site production residue).
    ClientCall,
}

impl Arm {
    pub fn label(self) -> &'static str {
        match self {
            Self::BrokerPublish => "broker-publish",
            Self::ClientCall => "client-call",
        }
    }
}

/// The method a hop must look up: a bare name plus an arity, which is all the
/// `Calls` ledger's target text can express.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Callee {
    pub name: String,
    pub arity: usize,
}

/// One site whose operand is a bare parameter — the hop's input.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub arm: Arm,
    pub tree: Tree,
    pub member: String,
    /// The build module the operand's own method sits in — the scope
    /// [FR-WS-23] fixes the hop inside.
    pub module: String,
    pub file: String,
    pub line: u32,
    pub callee: Callee,
    /// The positional slot the operand occupies in its method's signature.
    pub slot: usize,
    pub operand: String,
    /// Set when the operand is a parameter of something that is not a method.
    pub blocked: Option<Residue>,
}

/// One observed call site of a wanted method.
#[derive(Debug, Clone)]
struct Observation {
    callee: Callee,
    module: String,
    tree: Tree,
    file: String,
    line: u32,
    /// The enclosing declaration, as `name@line`. The `Calls` ledger dedups on
    /// `(source declaration, target, form, kind, relation)`, so this is the
    /// grain at which a second call site disappears from it.
    declaration: Option<String>,
    /// The resolved value at each slot this measurement asked about.
    values: BTreeMap<usize, ArgValue>,
    /// **S-416.** For each slot whose value is
    /// [`ArgValue::NeedsAnotherHop`], what the second frame would have to look
    /// up: the method enclosing this call site, and the slot the forwarded
    /// parameter occupies in it.
    ///
    /// Absent when the argument is a parameter of something that is not a
    /// method (a constructor, a lambda) and for a method reference, which
    /// supplies no argument at all — both are [`FrameTwo::NotFollowable`].
    forwards: BTreeMap<usize, Forward>,
    /// **S-416.** The call's receiver is the `super` keyword.
    ///
    /// The only piece of receiver information this measurement reads, and it is
    /// read because it is the one thing decidable from the tokens alone: a
    /// `super.m(...)` inside `class C extends B` calls `B.m`, whatever else in
    /// the module happens to declare an `m` of the same arity. Anything more —
    /// which class an identifier receiver's declared type names — is type
    /// binding, which this harness does not do and does not claim.
    receiver_is_super: bool,
    /// For such a call, the simple name of the class it dispatches to: the one
    /// its enclosing class `extends`. `None` when there is no `extends` clause,
    /// in which case the call reaches `Object` and none of the methods this
    /// measurement looks up.
    super_dispatches_to: Option<String>,
    /// A `Foo::bar` method reference rather than an invocation.
    ///
    /// It counts toward the AGREEMENT — [FR-WS-23] AC1 says every call site —
    /// but not toward the LEDGER figures. The `Calls` ledger records a row for
    /// an invocation; a reference is a different construct, and folding the two
    /// together would make "call sites in files the ledger misses" mean two
    /// things at once and turn AC3's answer on the difference.
    is_reference: bool,
}

/// What the existing `Calls` ledger can and cannot answer — [CR-121] CRA-06,
/// which the CR records as unvalidated.
///
/// [CR-121]: ../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
#[derive(Debug, Default)]
pub struct LedgerAnswer {
    /// Files whose production `extract::extract` pass was read.
    pub files_extracted: usize,
    /// Every `Calls` row those files produced — the denominator, so a zero
    /// below is distinguishable from a walk that read nothing.
    pub calls_rows: usize,
    /// `Calls` rows whose target names one of the wanted methods.
    pub rows_naming_a_wanted_method: usize,
    /// Distinct `(caller declaration, wanted method)` pairs the ledger holds.
    pub caller_declarations: usize,
    /// In-module call sites this measurement observed **syntactically**.
    pub syntactic_call_sites: usize,
    /// Call sites the ledger's `(source, target, form, kind, relation)` dedup
    /// collapses away — the agreement rule cannot see them.
    pub collapsed_by_dedup: usize,
    /// Rows whose target text equals a value the hop resolved. The ledger has
    /// no operand field, so this is expected to be **zero**, and it is measured
    /// rather than asserted from reading the struct.
    pub rows_carrying_the_operand: usize,
    /// Wanted methods whose bare name is declared by two or more
    /// `method_declaration`s inside the same build module — the ambiguity a
    /// name-grained ledger target cannot resolve.
    pub ambiguous_by_name: usize,
    /// **Invocation** call sites the whole-estate walk saw that a ledger-driven
    /// walk would never have opened the file for — the ledger's own coverage
    /// gap. Method references are excluded and counted separately: the ledger
    /// records a `Calls` row for an invocation, so a reference missing from it
    /// is not a gap in the ledger's coverage of what it models.
    pub call_sites_the_ledger_misses: usize,
    /// `Foo::bar` references to a wanted method. They bind nothing and refuse
    /// the method under [FR-WS-23] AC1; reported because a reader who sees the
    /// row above at zero should know references were looked for and found.
    pub method_references: usize,
    /// A sample of the target texts seen, so the reader can check the grain.
    pub sample_targets: BTreeSet<String>,
}

/// [CR-121] CRA-06's verdict, in the two halves it actually has.
impl LedgerAnswer {
    /// Can the ledger answer *which declarations call this method*?
    pub fn answers_the_direction(&self) -> bool {
        self.caller_declarations > 0
    }

    /// Can the ledger answer *what argument that call passed*?
    pub fn answers_the_operand(&self) -> bool {
        self.rows_carrying_the_operand > 0
    }
}

/// [AC5] the measured cost of the hop, so the perf reconciliation uses a figure
/// rather than an estimate.
#[derive(Debug, Default, Clone)]
pub struct Cost {
    /// Pass 1: enumerating the candidate sites. Not the hop — the arm already
    /// does this today; recorded so the hop's share is readable.
    pub enumerate: Duration,
    /// Pass 2 over **every** Java file in the estate: the naive hop.
    pub hop_whole_estate: Duration,
    /// Pass 2 restricted to the files a ledger-driven implementation would
    /// open. The figure a real implementation should be held to.
    pub hop_targeted: Duration,
    /// Files parsed in the targeted pass.
    pub targeted_files: usize,
    /// Java files the whole-estate pass parsed.
    pub estate_files: usize,
    /// **S-416.** The second frame's own pass, over the files the `Calls` ledger
    /// names as callers of a forwarding method, with the same-name ambiguity
    /// census **switched off** — so this figure is parse-and-resolve and nothing
    /// else.
    pub frame_two: Duration,
    /// The same pass with the census switched back on. The difference between
    /// the two is the measurement scaffolding's share, MEASURED rather than
    /// disclosed in prose — which is what [CR-131] C1's cost criterion asks for.
    ///
    /// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
    pub frame_two_with_scaffolding: Duration,
    /// Files parsed in the second frame's pass — its denominator.
    pub frame_two_files: usize,
    /// Distinct forwarding methods the second frame had to look up — the other
    /// denominator the per-unit figure can be stated over.
    pub frame_two_callees: usize,
}

impl Cost {
    /// Microseconds of targeted hop per candidate — the per-site figure
    /// [NFR-PE-03] is phrased over.
    pub fn per_candidate_us(&self, candidates: usize) -> u128 {
        if candidates == 0 {
            return 0;
        }
        self.hop_targeted.as_micros() / candidates as u128
    }
}

/// Everything the run measured.
#[derive(Debug, Default)]
pub struct Findings {
    pub members: BTreeSet<String>,
    /// Java files walked, and files the broker detector reached.
    pub java_files: usize,
    pub publish_files: usize,
    /// Every header-form publish site, by tree — the denominator S-365 recorded
    /// as 13 main / 41 test.
    pub publish_sites: BTreeMap<Tree, usize>,
    /// Gate-admitted client-call sites carrying at least one parameter operand.
    pub client_sites: BTreeMap<Tree, usize>,
    /// A publish site `collect_header_publishes` recognised and this module's
    /// query did not reach. Asserted zero: it is the mirror's own policing.
    pub sites_without_a_node: usize,
    /// An operand this module classified as a bare parameter but could not give
    /// a positional slot — a varargs `spread_parameter`, whose name hangs off a
    /// declarator the binding walk reads as the TYPE, or a single-identifier
    /// lambda parameter, whose "parameter list" is one bare `identifier` with no
    /// children. Both used to `return` silently from `push_candidate`, shrinking
    /// the denominator the floor is stated over with nothing to show for it.
    ///
    /// **Defensive, and measured to be so**: on both shapes the parent's
    /// `resolve_key` declines to call the operand a bare parameter one step
    /// earlier, so neither reaches here (see
    /// [`fixtures::a_lambda_parameter_topic_is_excluded_before_the_slot_arithmetic`]).
    /// The counter is therefore a drift detector between the parent's classifier
    /// and this module's slot arithmetic — if they ever disagree about what a
    /// parameter is, the run is VOID rather than quietly short. Asserted zero,
    /// exactly as `sites_without_a_node` is.
    pub operands_without_a_slot: usize,
    pub candidates: Vec<Candidate>,
    /// Keyed by the candidate's index in [`Findings::candidates`].
    pub hops: BTreeMap<usize, Hop>,
    /// The same hops read under the **main-only** call-site rule, as a
    /// sensitivity: does admitting test call sites into the agreement change
    /// the verdict?
    pub hops_main_only: BTreeMap<usize, Hop>,
    /// **S-416.** The same candidates at TWO frames, every in-module call site
    /// admitted to the agreement — the strict reading of the new bound.
    pub hops_two_frame: BTreeMap<usize, Hop>,
    /// **S-416, the decisive cell.** Two frames, with agreement taken over
    /// `src/main` call sites only. [CR-131] §3.2 C1 states the floor over this
    /// reading and no other; the other three cells are reported beside it so a
    /// reader can see what each of the two relaxations bought.
    ///
    /// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
    pub hops_two_frame_main_only: BTreeMap<usize, Hop>,
    /// Per candidate index, the two-frame refusal reason under the decisive
    /// reading — the enumeration [CR-131] C1 asks for by name.
    ///
    /// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
    pub two_frame_residue: BTreeMap<usize, TwoFrameResidue>,
    /// Per candidate index, the in-module first-frame call sites that FORWARD a
    /// parameter, as `file:line [tree]` — untruncated, because this is the list
    /// S-392's seven named sites are reconciled against and a truncated join key
    /// is a silent miss.
    pub forwarding_sites: BTreeMap<usize, Vec<String>>,
    /// Per candidate index, what each second frame proved, under the decisive
    /// reading: the forwarding method looked up, and the outcome.
    pub frame_two_detail: BTreeMap<usize, Vec<String>>,
    /// Per candidate index, the largest number of main-tree
    /// `method_declaration`s sharing a forwarding method's **`(name, arity)`**
    /// inside the candidate's own build module.
    ///
    /// The honest limit on this measurement, measured rather than argued. Frame
    /// two binds on [`Callee`], `(name, arity)`, because that is all the `Calls`
    /// ledger's target text can express; [CR-131] C2 proposes the rule as "name,
    /// arity **and receiver-aware**", so wherever several declarations share one
    /// signature this harness pools their callers and the proposed rule would
    /// not. See [`Findings::pool_spans_several_overrides`] for what the count
    /// has to reach before that pooling can actually change an answer.
    ///
    /// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
    pub frame_two_name_declarations: BTreeMap<usize, usize>,
    /// Per candidate index, the forwarding methods whose SECOND frame resolved,
    /// as `module/name(arity)` — the chains behind the two-frame yield, for the
    /// generality caveat declared before the run.
    pub frame_two_chains: BTreeMap<usize, BTreeSet<String>>,
    /// Per candidate index, the in-module call sites the **main-tree rule
    /// excluded** from the agreement, with what each one's argument resolved to.
    ///
    /// On this estate these are the Mockito stubs. [CR-131] §3.2 C1's whole
    /// justification for the main-only reading is "a Mockito stub is not a
    /// publish", and a relaxation that silently drops sites cannot be audited:
    /// every site it removes is printed, named, so a reader can judge for
    /// themselves whether each really is a stub.
    ///
    /// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
    pub excluded_by_main_tree: BTreeMap<usize, Vec<String>>,
    pub ledger: LedgerAnswer,
    pub cost: Cost,
    /// Per candidate index, the in-module call sites the hop read, as
    /// `file:line` — the evidence a figure is asserted from rather than
    /// asserted at ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub caller_sites: BTreeMap<usize, Vec<String>>,
    /// Per candidate index, the in-module call sites that BLOCK the hop and
    /// what each one's argument resolved to — **at most [`BLOCKER_SAMPLE`]**,
    /// ranked by the precedence [`decide`] applies, with a trailing
    /// "… and N more" line when the cap bites. An undisclosed truncation of an
    /// evidence list is exactly the silence [NFR-CC-04] forbids. Without this a refusal reason
    /// names a class of fault but not the site that caused it, and the
    /// dominant cause on this estate — Mockito stubs in the test tree — would
    /// have been invisible in the figures.
    pub blockers: BTreeMap<usize, Vec<String>>,
    /// Distinct wrapper methods and members behind the resolved production
    /// count — the generality caveat the floor declared in advance.
    pub resolved_methods: BTreeSet<String>,
    pub resolved_members: BTreeSet<String>,
}

impl Findings {
    /// The decisive number: production broker-publish sites resolved at one hop.
    pub fn production_publish_resolved(&self) -> usize {
        self.resolved_in(Arm::BrokerPublish, Tree::Main, &self.hops)
    }

    /// The same figure under the main-only reading.
    pub fn production_publish_resolved_main_only(&self) -> usize {
        self.resolved_in(Arm::BrokerPublish, Tree::Main, &self.hops_main_only)
    }

    /// **S-416's decisive number**: production publish sites resolved at two
    /// frames with agreement over `src/main` call sites only.
    pub fn production_publish_resolved_two_frame_main_only(&self) -> usize {
        self.resolved_in(Arm::BrokerPublish, Tree::Main, &self.hops_two_frame_main_only)
    }

    /// Two frames, every in-module call site admitted — the strict reading.
    pub fn production_publish_resolved_two_frame(&self) -> usize {
        self.resolved_in(Arm::BrokerPublish, Tree::Main, &self.hops_two_frame)
    }

    /// The candidates the SECOND FRAME actually bought, under one reading:
    /// resolved at two frames and refused at one. Non-vacuity V6 is stated over
    /// this — a two-frame reading that equals its one-frame reading has measured
    /// a no-op, and a floor cleared by a no-op is cleared by the first frame.
    pub fn bought_by_the_second_frame(&self, arm: Arm, tree: Tree, main_only: bool) -> usize {
        let (one, two) = if main_only {
            (&self.hops_main_only, &self.hops_two_frame_main_only)
        } else {
            (&self.hops, &self.hops_two_frame)
        };
        self.candidates
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                c.arm == arm
                    && c.tree == tree
                    && two.get(i).is_some_and(Hop::resolved)
                    && !one.get(i).is_some_and(Hop::resolved)
            })
            .count()
    }

    /// The two-frame residue census for one arm and tree, under the decisive
    /// reading — every refusal with its own reason ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub fn two_frame_census(&self, arm: Arm, tree: Tree) -> BTreeMap<TwoFrameResidue, usize> {
        let mut out = BTreeMap::new();
        for (i, c) in self.candidates.iter().enumerate() {
            if c.arm != arm || c.tree != tree {
                continue;
            }
            if let Some(reason) = self.two_frame_residue.get(&i) {
                *out.entry(*reason).or_default() += 1;
            }
        }
        out
    }

    /// Can frame two's `(name, arity)` pool for this candidate mix the callers
    /// of **two different overrides**?
    ///
    /// Two declarations is NOT enough, and getting that wrong in either
    /// direction is the whole difficulty. On this estate a module writes a base
    /// `KafkaProducer.sendMessage/3` plus one subclass override per producer, so
    /// the count is `1 + overrides`:
    ///
    /// * **2** — base plus exactly one override. The only call that targets the
    ///   base is the override's own `super.sendMessage(…)`, and
    ///   [`dispatches_past`] already removes it, so every caller left in the
    ///   pool targets the one override. The pool is unambiguous and a resolution
    ///   drawn from it is sound.
    /// * **3 or more** — base plus two or more overrides. The pool spans them,
    ///   this harness cannot tell their callers apart, and a receiver-aware rule
    ///   would.
    ///
    /// So this is the discriminator that explains BOTH halves of the estate
    /// result, and [`Findings::refusals_from_an_ambiguous_pool`] and
    /// [`Findings::resolutions_on_an_unambiguous_pool`] are the two guards read
    /// off it. A candidate with no forward at all reports 0 and is not
    /// ambiguous — there is no second frame to pool anything.
    pub fn pool_spans_several_overrides(&self, i: usize) -> bool {
        self.frame_two_name_declarations.get(&i).is_some_and(|n| *n >= 3)
    }

    /// Production candidates REFUSED at two frames whose frame-two pool spans
    /// several overrides — the refusals attributable to this harness's
    /// `(name, arity)` binding rather than to the estate.
    ///
    /// One definition, read by both the report and the gate's assertion. They
    /// were two separate spellings of the same three filters, and the finding's
    /// central caveat rests on the printed row and the asserted number being
    /// the same quantity — which is exactly the twin this file's discipline
    /// exists to prevent.
    pub fn refusals_from_an_ambiguous_pool(&self, arm: Arm, tree: Tree) -> Vec<(usize, usize)> {
        self.candidates
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                c.arm == arm && c.tree == tree && self.two_frame_residue.contains_key(i)
            })
            .filter(|(i, _)| self.pool_spans_several_overrides(*i))
            .map(|(i, _)| (i, self.frame_two_name_declarations.get(&i).copied().unwrap_or_default()))
            .collect()
    }

    /// The symmetric guard, and the one this measurement did not have until the
    /// review asked for it: production candidates RESOLVED at two frames whose
    /// frame-two pool is unambiguous.
    ///
    /// The pooling argument cuts both ways. A pool that spans several overrides
    /// can refuse on a disagreement the estate does not write — that is the
    /// caveat the refusal guard measures — but it can equally **manufacture** a
    /// resolution, by drawing a value from a caller of a different class's
    /// same-signature method. A resolution drawn from an unambiguous pool cannot
    /// be manufactured that way, so this is what licenses reading the headline
    /// as sound rather than merely as a lower bound.
    pub fn resolutions_on_an_unambiguous_pool(&self, arm: Arm, tree: Tree) -> Vec<(usize, usize)> {
        self.candidates
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                c.arm == arm
                    && c.tree == tree
                    && self.hops_two_frame_main_only.get(i).is_some_and(Hop::resolved)
                    && !self.frame_two_chains.get(i).is_none_or(BTreeSet::is_empty)
            })
            .filter(|(i, _)| !self.pool_spans_several_overrides(*i))
            .map(|(i, _)| (i, self.frame_two_name_declarations.get(&i).copied().unwrap_or_default()))
            .collect()
    }

    /// Production candidates resolved at two frames whose resolution actually
    /// went through a second frame — the ones the guard above must cover.
    pub fn resolutions_through_a_second_frame(&self, arm: Arm, tree: Tree) -> Vec<usize> {
        self.candidates
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                c.arm == arm
                    && c.tree == tree
                    && self.hops_two_frame_main_only.get(i).is_some_and(Hop::resolved)
                    && !self.frame_two_chains.get(i).is_none_or(BTreeSet::is_empty)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The generality caveat `two_frame_forwarding_floor.txt` declared in
    /// advance, over the decisive reading: the distinct wrapper methods, the
    /// distinct members, and the distinct forwarding chains a SECOND frame
    /// actually resolved through.
    ///
    /// Reported whatever the verdict, because it was declared whatever the
    /// verdict — a pass resting on one wrapper method or one member licenses
    /// little, and that has to be visible beside the number rather than
    /// discovered afterwards.
    pub fn two_frame_generality(&self) -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
        let (mut methods, mut members, mut chains) =
            (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        for (i, c) in self.candidates.iter().enumerate() {
            if c.arm != Arm::BrokerPublish || c.tree != Tree::Main {
                continue;
            }
            if !self.hops_two_frame_main_only.get(&i).is_some_and(Hop::resolved) {
                continue;
            }
            methods.insert(format!("{}/{}", c.module, c.callee.name));
            members.insert(c.member.clone());
            chains.extend(self.frame_two_chains.get(&i).into_iter().flatten().cloned());
        }
        (methods, members, chains)
    }

    /// The candidate in `module` whose forwarding call sites include
    /// `basename:line` — the join S-392's seven named sites are reconciled
    /// through. `None` means the run cannot see a site S-392 named, which is
    /// harness drift and VOID.
    ///
    /// **The module is half the key, and it has to be.** The basename and line
    /// alone are NOT unique across the estate: two of the seven named sites are
    /// the byte-identical string `DelayedMessageKafkaProducer.java:23`, in
    /// `deprecated-mailbox-core/manager` and in `mailbox-manager`. Joining on
    /// the basename alone made both rows resolve to the FIRST match, so the
    /// reconciliation printed a `deprecated-mailbox-core/manager` candidate
    /// under the `mailbox-manager` heading, and — worse — the V5 guard would
    /// have counted 7 named sites found while only 6 distinct candidates backed
    /// them. A site that genuinely vanished could then have hidden behind its
    /// twin, which is precisely the drift V5 exists to catch.
    pub fn candidate_forwarding_at(&self, module: &str, site: &str) -> Option<usize> {
        self.forwarding_sites.iter().find_map(|(i, sites)| {
            (self.candidates.get(*i).is_some_and(|c| c.module == module)
                && sites.iter().any(|s| {
                    // `file:line [tree]` — join on the basename and line, which
                    // is the whole of what `forwarding_finding.txt` recorded.
                    s.split(" [").next().is_some_and(|fl| {
                        fl.rsplit('/').next().is_some_and(|base| base == site)
                    })
                }))
            .then_some(*i)
        })
    }

    /// Candidates that resolve when only `src/main` call sites are admitted to
    /// the agreement, and refuse when the whole build module is. The gap
    /// between the two readings, attributed.
    pub fn refused_only_by_test_call_sites(&self, arm: Arm, tree: Tree) -> usize {
        self.candidates
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                c.arm == arm
                    && c.tree == tree
                    && self.hops_main_only.get(i).is_some_and(Hop::resolved)
                    && !self.hops.get(i).is_some_and(Hop::resolved)
            })
            .count()
    }

    fn resolved_in(&self, arm: Arm, tree: Tree, hops: &BTreeMap<usize, Hop>) -> usize {
        self.candidates
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                c.arm == arm && c.tree == tree && hops.get(i).is_some_and(Hop::resolved)
            })
            .count()
    }

    /// **[AC2]'s agreement figure**: how many in-module call sites actually
    /// agreed — the sites that carried a resolved candidate, not the sites that
    /// were read. Reported with [`LedgerAnswer::syntactic_call_sites`] as its
    /// denominator, and under both readings, because under the criterion as
    /// written it is zero by construction and a figure nobody writes down is a
    /// figure nobody can check.
    pub fn agreeing_call_sites(
        &self,
        arm: Arm,
        tree: Tree,
        hops: &BTreeMap<usize, Hop>,
    ) -> usize {
        self.candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.arm == arm && c.tree == tree)
            .filter_map(|(i, _)| match hops.get(&i) {
                Some(Hop::Resolved { call_sites, .. }) => Some(*call_sites),
                _ => None,
            })
            .sum()
    }

    pub fn candidates_in(&self, arm: Arm, tree: Tree) -> usize {
        self.candidates.iter().filter(|c| c.arm == arm && c.tree == tree).count()
    }

    /// The residue census for one arm and tree.
    pub fn residue(&self, arm: Arm, tree: Tree) -> BTreeMap<Residue, usize> {
        let mut out = BTreeMap::new();
        for (i, c) in self.candidates.iter().enumerate() {
            if c.arm != arm || c.tree != tree {
                continue;
            }
            if let Some(r) = self.hops.get(&i).and_then(Hop::residue) {
                *out.entry(r).or_default() += 1;
            }
        }
        out
    }

    /// The AC2 headline: operands needing two or more hops, or reaching outside
    /// their module.
    pub fn beyond_the_bound(&self, arm: Arm, tree: Tree) -> usize {
        let census = self.residue(arm, tree);
        census.get(&Residue::TwoOrMoreHops).copied().unwrap_or_default()
            + census.get(&Residue::OutOfModuleOnly).copied().unwrap_or_default()
    }
}

// ── Tree-sitter helpers: position, not judgement ────────────────────────────

/// Every call site, with its argument list, **and every method reference**.
/// The method name is filtered in Rust, as everywhere else in this harness.
///
/// The reference arm is the one place this measurement could have OVER-read.
/// `list.forEach(producer::sendMessage)` reaches the wrapper and proves nothing
/// about its topic, so under [FR-WS-23] AC1 — *every* call site in the module
/// must agree — a module containing one must not report the wrapper resolved.
/// A reference supplies no argument list, so it is recorded as needing another
/// hop rather than being skipped. The estate writes none
/// (`grep "::sendMessage"` → 0), so this changes no recorded figure; it closes
/// the only direction in which a figure could have been inflated.
///
/// [FR-WS-23]: ../../../docs/specs/requirements/FR-WS-23.md
const CALL_QUERY: &str = r"
[
  (method_invocation
    name: (identifier) @call.name
    arguments: (argument_list) @call.args)
  (method_reference) @call.ref
]
";

/// Every method declaration, for the same-name ambiguity census.
const DECL_QUERY: &str = r"
(method_declaration
  name: (identifier) @decl.name)
";

/// **S-416.** Every method declaration WITH its parameter list, for the
/// frame-two signature census.
///
/// [`DECL_QUERY`] captures the bare name because S-392's `ambiguous_by_name`
/// figure is about exactly that: the `Calls` ledger's target text is a name and
/// nothing else. Frame two binds on `(name, arity)`, so its census has to be
/// keyed the same way — counting `sendMessage` across every arity would report
/// an ambiguity the lookup does not actually have.
const DECL_SIGNATURE_QUERY: &str = r"
(method_declaration
  name: (identifier) @decl.name
  parameters: (formal_parameters) @decl.params)
";

/// The named children of an argument or parameter list, **minus comments**.
///
/// Tree-sitter counts a comment as a named sibling, so an un-filtered
/// `named_children` shifts every slot after a documented argument by one. The
/// same fact is what costs `brokers.scm`'s anchored patterns a commented
/// argument list; here it would silently bind the *wrong* positional operand,
/// which is worse than not binding at all.
///
/// The predicate is `ends_with("comment")`, not `== "comment"`, following
/// `extract::shape`'s: tree-sitter-java spells them `line_comment` and
/// `block_comment`, and an exact match against `"comment"` filters neither.
/// That is not a hypothetical — it was the first version here, and
/// [`fixtures::a_comment_in_the_argument_list_does_not_shift_the_slot`] caught
/// it by changing the ARITY, so the call site matched no wanted method at all.
///
/// `receiver_parameter` is dropped for the same reason and a different one:
/// `formal_parameters` admits a leading explicit receiver (`void m(Foo this,
/// String topic)`), which occupies a parameter slot but is supplied by **no**
/// argument at any call site. Counting it shifts every later slot by one and
/// the wrapper then matches no call site at all.
fn slots<'t>(list: Node<'t>) -> Vec<Node<'t>> {
    let mut cursor = list.walk();
    list.named_children(&mut cursor)
        .filter(|n| !n.kind().ends_with("comment") && n.kind() != "receiver_parameter")
        .collect()
}

/// The declaration that declares `name` as a parameter, innermost first.
fn declaring_scope<'t>(node: Node<'t>, name: &str, src: &[u8]) -> Option<Node<'t>> {
    let mut current = node.parent();
    while let Some(scope) = current {
        if matches!(
            scope.kind(),
            "method_declaration" | "constructor_declaration" | "lambda_expression"
        ) && parameter_slot(scope, name, src).is_some()
        {
            return Some(scope);
        }
        current = scope.parent();
    }
    None
}

/// The positional slot `name` occupies in `scope`'s parameter list, and the
/// list's length.
fn parameter_slot(scope: Node<'_>, name: &str, src: &[u8]) -> Option<(usize, usize)> {
    let params = scope.child_by_field_name("parameters")?;
    let list = slots(params);
    let index = list.iter().position(|p| parameter_name(*p, src).as_deref() == Some(name))?;
    Some((index, list.len()))
}

/// A parameter's declared name, across the three shapes Java writes: a
/// `formal_parameter`, a varargs `spread_parameter` (whose name hangs off a
/// `variable_declarator`), and a lambda's bare `identifier`.
fn parameter_name(param: Node<'_>, src: &[u8]) -> Option<String> {
    if param.kind() == "identifier" {
        return param.utf8_text(src).ok().map(str::to_string);
    }
    if let Some(name) = param.child_by_field_name("name") {
        return name.utf8_text(src).ok().map(str::to_string);
    }
    let mut cursor = param.walk();
    let declarator = param
        .named_children(&mut cursor)
        .find(|c| c.kind() == "variable_declarator");
    drop(cursor);
    declarator
        .and_then(|d| d.child_by_field_name("name"))
        .and_then(|n| n.utf8_text(src).ok())
        .map(str::to_string)
}

/// What one positional argument resolves to at **one** hop.
///
/// The classification is entirely the parent harness's, and this function adds
/// no rule of its own — it only names the outcomes the hop cares about and
/// fixes the order they are asked in:
///
/// 1. [`resolve_key`](super::configuration_agreement::resolve_key) first,
///    because **a parameter is decided first** — that is the precedence
///    `resolve_key_at` itself applies, and for its reason: `Unit` is
///    file-scoped, so a method parameter shadowing a same-named field would
///    otherwise fold to the *field's* value. A bare parameter here is the
///    second hop [FR-WS-23] puts out of scope.
/// 2. A resolved configuration key is the topic's identity ([FR-WS-19]).
/// 3. Only then [`folded_text`](super::folded_text), which folds a literal
///    **and a same-unit constant** to the same depth [`classify`] reaches.
///    Restricting this step to `static_literal` was the first version here, and
///    it under-read the estate: a caller passing
///    `private static final String ARCHIVE_COMMANDS_TOPIC = "…"` — the
///    idiomatic shape of every IT test call site — was recorded as
///    *unresolvable* when the source proves its value outright. Every judgement
///    call in this harness is resolved the way that maximises the measured
///    yield, so that a small result is a robust falsification rather than an
///    artefact of a strict reading.
///
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
/// [FR-WS-23]: ../../../docs/specs/requirements/FR-WS-23.md
fn arg_value(node: Node<'_>, src: &[u8], unit: &Unit<'_>, resolver: Judge<'_>) -> ArgValue {
    match resolve_key(node, src, unit, resolver) {
        KeyOutcome::Resolved { key, .. } => ArgValue::Key(key),
        KeyOutcome::Unresolved(Refusal::MethodParameter) => ArgValue::NeedsAnotherHop,
        KeyOutcome::Unresolved(other) => match folded_text(node, src, unit, FOLD_DEPTH) {
            Some(text) => ArgValue::Literal(text),
            None => ArgValue::Unresolvable(other),
        },
    }
}

/// **S-416.** The second frame's lookup target for one forwarded argument.
///
/// Every step is one the first frame already takes, called on a different node:
/// [`operand_name`] for the identifier, [`declaring_scope`] for the declaration
/// that binds it, [`parameter_slot`] for its position. The only rule this
/// function adds is the one [`Candidate`] already applies to a publish site's
/// own operand — a constructor or lambda scope is **not** a method call site, so
/// there is nothing to follow and the second frame refuses.
///
/// `None` therefore means exactly [`FrameTwo::NotFollowable`], and the caller
/// spells it that way rather than dropping the site.
fn forward_from(argument: Node<'_>, src: &[u8]) -> Option<Forward> {
    let name = operand_name(argument, src)?;
    let scope = declaring_scope(argument, &name, src)?;
    if scope.kind() != "method_declaration" {
        return None;
    }
    let (slot, arity) = parameter_slot(scope, &name, src)?;
    let method = scope.child_by_field_name("name")?.utf8_text(src).ok()?.to_string();
    Some(Forward {
        callee: Callee { name: method, arity },
        slot,
        declaring_class: enclosing_type_name(scope, src)?,
    })
}

/// The simple name of the type declaration enclosing `node`.
///
/// `None` for a method in an **anonymous** class body — `new Thin() { … }`. An
/// anonymous class has no name for a `super.m()` written elsewhere to target, so
/// there is nothing to compare and [`dispatches_past`] keeps such a call site.
///
/// The walk therefore has to STOP at an anonymous body rather than pass through
/// it: a `class_body` whose parent is an `object_creation_expression` is where
/// the enclosing type ends. Without that stop it kept climbing to the outer
/// named class, and a `super.m()` inside an anonymous subclass was attributed to
/// whatever the OUTER class extends — see
/// [`fixtures::a_super_call_in_an_anonymous_subclass_is_not_attributed_to_the_outer_class`].
fn enclosing_type_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(scope) = current {
        if anonymous_body(scope) {
            return None;
        }
        if matches!(
            scope.kind(),
            "class_declaration" | "enum_declaration" | "record_declaration" | "interface_declaration"
        ) {
            return scope
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(src).ok())
                .map(str::to_string);
        }
        current = scope.parent();
    }
    None
}

/// `true` for the body of an anonymous class — `new Foo() { … }`.
fn anonymous_body(node: Node<'_>) -> bool {
    node.kind() == "class_body"
        && node.parent().is_some_and(|p| p.kind() == "object_creation_expression")
}

/// The simple name of the class a `super.m(…)` at `node` dispatches to: what the
/// enclosing type extends.
///
/// For an **anonymous** class the answer is the type being instantiated —
/// `new Thin() { … void m() { super.m(); } }` dispatches to `Thin` — which is
/// read off the `object_creation_expression`'s own type, not off the outer
/// class's `extends`. Climbing past the anonymous body was a real over-exclusion:
/// it made the receiver rule discard a genuine caller, and discarding callers
/// RAISES the resolved count, so the mistake was not on the safe side.
///
/// `None` when there is no `extends` clause at all — a `super.m()` there targets
/// `Object`, which declares none of the methods this measurement looks up.
fn enclosing_superclass_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(scope) = current {
        if anonymous_body(scope) {
            let created = scope.parent()?.child_by_field_name("type")?;
            return simple_type_name(created, src);
        }
        if scope.kind() == "class_declaration" {
            let extends = scope.child_by_field_name("superclass")?;
            let mut cursor = extends.walk();
            let first = extends.named_children(&mut cursor).next();
            drop(cursor);
            return first.and_then(|t| simple_type_name(t, src));
        }
        current = scope.parent();
    }
    None
}

/// A type node's simple name, through the two wrappers Java's grammar puts
/// around one in an `extends` clause: `Foo<K, V>` is a `generic_type` whose
/// first named child is the identifier, and `a.b.Foo` is a
/// `scoped_type_identifier` whose LAST is.
///
/// Both shapes appear on the estate — `extends KafkaProducer<ArchiveEventKafkaKey,
/// SpecificRecord>` is the generic one — and reading the wrong child yields
/// `None`, which silently over-excludes. The near miss is pinned by
/// [`fixtures::a_super_call_reaching_its_own_superclass_is_a_caller`].
fn simple_type_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" | "identifier" => node.utf8_text(src).ok().map(str::to_string),
        "generic_type" | "scoped_type_identifier" => {
            let mut cursor = node.walk();
            let children: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
            drop(cursor);
            let pick =
                if node.kind() == "generic_type" { children.first() } else { children.last() };
            pick.and_then(|n| simple_type_name(*n, src))
        }
        _ => None,
    }
}

/// The enclosing method or constructor, as `name@line` — the grain the `Calls`
/// ledger's `source` field holds, spelled from the tree because this harness
/// must not depend on the symbol builder's naming to count a collapse.
/// `None` for a call outside any method or constructor — a field initialiser or
/// a static block. Those are **not** interchangeable with each other: the
/// extract pass attributes each to the file-module symbol or to its own
/// declaration, so folding them under one `"<file scope>"` key would report
/// them as collapsing into a single ledger row when they do not. Excluding them
/// keeps `collapsed_by_dedup` an exact count of the collapses this measurement
/// can prove, rather than an upper bound presented as a figure.
fn enclosing_declaration(node: Node<'_>, src: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(scope) = current {
        if matches!(scope.kind(), "method_declaration" | "constructor_declaration") {
            let name = scope
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(src).ok())
                .unwrap_or("<anonymous>");
            return Some(format!("{name}@{}", scope.start_position().row + 1));
        }
        current = scope.parent();
    }
    None
}

/// The estate member a project-relative path belongs to: its first segment.
fn member_of(rel: &str) -> String {
    rel.split('/').next().unwrap_or_default().to_string()
}

// ── Pass 1: enumerate the sites whose operand is a bare parameter ───────────

/// One Java source, read once so the three passes measure computation rather
/// than disk.
struct Source {
    rel: String,
    language: String,
    text: String,
    module: String,
    tree: Tree,
}

/// Everything a pass needs besides the file. A struct for the reason the parent
/// harness gives for `ScanCtx`: the signature was already at its limit.
struct Estate<'a> {
    registry: &'a LanguageRegistry,
    /// The `ConfigLookup` view of the discovered corpus. Held rather than built per call
    /// because [`Resolver`] borrows it for `'a` — the same reason the parent
    /// harness gives `ScanCtx` a `lookup` field (S-382).
    lookup: CorpusLookup<'a>,
    properties: &'a PropertiesIndex,
    symbols: &'a SymbolContext,
}

impl<'a> Estate<'a> {
    /// S-382 split the old combined resolver into [`Resolver`] (corpus +
    /// module) and [`Judge`] (resolver + properties). This returns the `Judge`,
    /// which is what `resolve_key`, `judge` and `collect_header_publishes` all
    /// take.
    fn resolver(&'a self, module: &'a str) -> Judge<'a> {
        Judge {
            resolver: Resolver { corpus: &self.lookup, module },
            props: self.properties,
        }
    }
}

/// The `Calls` ledger, indexed the only way its target text allows: by the
/// method name. Built from the production `extract::extract` pass, never from
/// this module's own tree walk.
#[derive(Default)]
struct CallsLedger {
    /// name → the `(file, caller declaration)` pairs that call it.
    by_name: BTreeMap<String, BTreeSet<(String, String)>>,
    /// name → how many rows carried it, before any per-name grouping.
    rows_by_name: BTreeMap<String, usize>,
    rows: usize,
    files: usize,
    /// Every distinct target text, so "does a ledger field ever carry the
    /// operand?" is answered by looking rather than by reading the struct.
    all_targets: BTreeSet<String>,
    targets: BTreeSet<String>,
}

impl CallsLedger {
    /// The files the ledger names as callers of any of `names` — what a
    /// ledger-driven implementation would actually open. Written once and used
    /// by both the frame-one targeted pass and the frame-two pass, which had
    /// the same four-line chain twice.
    fn files_naming<'a>(&'a self, names: &BTreeSet<String>) -> BTreeSet<&'a str> {
        names
            .iter()
            .filter_map(|n| self.by_name.get(n))
            .flat_map(|set| set.iter().map(|(file, _)| file.as_str()))
            .collect()
    }

    fn absorb(&mut self, rel: &str, facts: &extract::Facts) {
        self.files += 1;
        for r in facts.refs.iter().filter(|r| r.kind == EdgeKind::Calls) {
            self.rows += 1;
            let name = r.target.rsplit("::").next().unwrap_or(&r.target).to_string();
            self.all_targets.insert(r.target.clone());
            if self.targets.len() < 12 {
                self.targets.insert(r.target.clone());
            }
            *self.rows_by_name.entry(name.clone()).or_default() += 1;
            self.by_name
                .entry(name)
                .or_default()
                .insert((rel.to_string(), r.source.as_str().to_string()));
        }
    }
}

/// One parsed file and everything a scanner reads from it. A struct for the
/// reason the parent harness gives for `ScanCtx`: five of these travelled
/// together and pushed `client_candidates` past the argument limit.
struct Parsed<'t, 'r> {
    plugin: &'t dyn logos_core::plugin::LanguagePlugin,
    root: Node<'t>,
    src: &'t [u8],
    unit: Unit<'t>,
    resolver: Judge<'r>,
}

/// Parse one Java file and record every publish and client-call site whose
/// operand is a bare parameter.
fn scan_for_candidates(
    source: &Source,
    estate: &Estate<'_>,
    ledger: &mut CallsLedger,
    f: &mut Findings,
) {
    let Some(plugin) = estate.registry.for_path(&source.rel) else { return };
    let mut parser = Parser::new();
    if parser.set_language(plugin.language()).is_err() {
        return;
    }
    let Some(parsed) = parser.parse(&source.text, None) else { return };
    let src = source.text.as_bytes();
    let unit = Unit::build(parsed.root_node(), src);
    let resolver = estate.resolver(&source.module);

    let facts = extract::extract(&FileInput::new(&source.rel, &source.text), plugin, estate.symbols);
    ledger.absorb(&source.rel, &facts);

    let scan = Parsed { plugin, root: parsed.root_node(), src, unit, resolver };
    publish_candidates(source, &scan, f);
    client_candidates(source, &scan, &facts, f);
}

/// The broker arm: every `setHeader(KafkaHeaders.TOPIC, …)` site whose topic is
/// a bare parameter.
///
/// `collect_header_publishes` is the authority on *what is a publish site*;
/// this function's own query exists only to recover the AST node for a site
/// that function already recognised, and every site it fails to recover is
/// counted into [`Findings::sites_without_a_node`] and asserted zero.
fn publish_candidates(source: &Source, scan: &Parsed<'_, '_>, f: &mut Findings) {
    let Parsed { plugin, root, src, ref unit, resolver } = *scan;
    let Some(query) = header_publish_query(plugin.language()) else { return };
    let authority =
        collect_header_publishes(&query, root, src, unit, &source.rel, resolver);
    if authority.is_empty() {
        return;
    }
    f.publish_files += 1;
    let nodes = topic_nodes(&query, root, src);
    for site in &authority {
        *f.publish_sites.entry(source.tree).or_default() += 1;
        let Some(topic) = nodes.get(&(site.line, site.text.clone())) else {
            f.sites_without_a_node += 1;
            continue;
        };
        // The population boundary, and it is guarded TWICE on purpose — stated
        // so neither guard is later removed as dead. Here, by asking the parent
        // harness's own classifier; and again in `push_candidate`, which needs
        // an enclosing scope that actually declares the operand as a parameter
        // before it can name a slot. A mutation of either alone does not widen
        // the denominator, which is why
        // `a_publish_site_whose_topic_is_not_a_parameter_is_not_a_forwarding_candidate`
        // pins the observable boundary rather than one of the two checks.
        if !matches!(
            resolve_key(*topic, src, unit, resolver),
            KeyOutcome::Unresolved(Refusal::MethodParameter)
        ) {
            continue;
        }
        push_candidate(Arm::BrokerPublish, source, *topic, src, site.line, f);
    }
}

/// The `(line, normalised text)` → topic-node map the authority list is joined
/// on. Both keys are built exactly as `collect_header_publishes` builds them.
fn topic_nodes<'t>(query: &Query, root: Node<'t>, src: &'t [u8]) -> BTreeMap<(u32, String), Node<'t>> {
    let names = query.capture_names();
    let mut out = BTreeMap::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, src);
    while let Some(m) = matches.next() {
        let mut method = None;
        let mut topic = None;
        for cap in m.captures {
            match names[cap.index as usize] {
                "publish.method" => method = Some(cap.node),
                "publish.topic" => topic = Some(cap.node),
                _ => {}
            }
        }
        let (Some(method), Some(topic)) = (method, topic) else { continue };
        let line = method.start_position().row as u32 + 1;
        let text = topic
            .utf8_text(src)
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        out.insert((line, text), topic);
    }
    out
}

/// The client arm: a gate-admitted site the agreement rule does not already
/// admit, carrying at least one operand that is a bare parameter.
fn client_candidates(
    source: &Source,
    scan: &Parsed<'_, '_>,
    facts: &extract::Facts,
    f: &mut Findings,
) {
    let Parsed { plugin, root, src, ref unit, resolver } = *scan;
    let Some(query) = plugin.query("invocations") else { return };
    // The parent's spelling of the FR-FW-04 ledger gate, not a copy of it: this
    // predicate fixes the client-arm denominator in two published measurements,
    // and a hand-written twin that later diverges is the exact defect this
    // module's reuse discipline exists to avoid.
    if !super::gate_admits(plugin, facts) {
        return;
    }
    for (_, arg) in
        super::collect_sites(query, root, src, &plugin.semantics().invocation_methods)
    {
        let mut nodes = Vec::new();
        operands(arg, src, &mut nodes);
        let kinds: Vec<_> = nodes.iter().map(|n| classify(*n, src, unit, FOLD_DEPTH)).collect();
        let judged = judge(&nodes, &kinds, src, unit, resolver, true);
        // The population AC1 names is the **no-key** residue — the 30 production
        // sites CR-115's agreement rule leaves unresolved. A site the rule
        // already admits, or one whose keys agreed and merely composed no route,
        // is not waiting on a hop and is not counted here.
        if !matches!(judged.verdict, Verdict::NoKey(_)) {
            continue;
        }
        let parameters: Vec<usize> = judged
            .outcomes
            .iter()
            .enumerate()
            .filter(|(_, o)| {
                matches!(o, Some(KeyOutcome::Unresolved(Refusal::MethodParameter)))
            })
            .map(|(i, _)| i)
            .collect();
        if parameters.is_empty() {
            continue;
        }
        *f.client_sites.entry(source.tree).or_default() += 1;
        for i in parameters {
            let line = nodes[i].start_position().row as u32 + 1;
            push_candidate(Arm::ClientCall, source, nodes[i], src, line, f);
        }
    }
}

/// Turn one parameter operand into a [`Candidate`], recording the slot it
/// occupies and — when the declaring scope is not a method — the residue that
/// blocks it before any call site is looked at.
fn push_candidate(
    arm: Arm,
    source: &Source,
    operand: Node<'_>,
    src: &[u8],
    line: u32,
    f: &mut Findings,
) {
    let Some(name) = operand_name(operand, src) else {
        f.operands_without_a_slot += 1;
        return;
    };
    let Some(scope) = declaring_scope(operand, &name, src) else {
        f.operands_without_a_slot += 1;
        return;
    };
    let Some((slot, arity)) = parameter_slot(scope, &name, src) else {
        f.operands_without_a_slot += 1;
        return;
    };
    let method = scope
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(src).ok())
        .unwrap_or_default()
        .to_string();
    let blocked = (scope.kind() != "method_declaration").then_some(Residue::NotAMethodParameter);
    f.candidates.push(Candidate {
        arm,
        tree: source.tree,
        member: member_of(&source.rel),
        module: source.module.clone(),
        file: source.rel.clone(),
        line,
        callee: Callee { name: method, arity },
        slot,
        operand: name,
        blocked,
    });
}

// ── Pass 2: the hop ─────────────────────────────────────────────────────────

/// The methods the hop must look up, and the slots it must read: `(name,
/// arity)` → the positional slots any candidate occupies.
fn wanted(f: &Findings) -> BTreeMap<Callee, BTreeSet<usize>> {
    let mut out: BTreeMap<Callee, BTreeSet<usize>> = BTreeMap::new();
    for c in f.candidates.iter().filter(|c| c.blocked.is_none()) {
        out.entry(c.callee.clone()).or_default().insert(c.slot);
    }
    out
}

/// Parse one file and record every call site of a wanted method, with the
/// argument at each wanted slot already resolved.
///
/// `declarations` is the same-name ambiguity census — **measurement
/// scaffolding**, not work a real implementation would do. It is an `Option` so
/// that S-416's cost criterion can separate the two by running the same pass
/// twice, once with it and once without, rather than disclosing the bias in
/// prose. S-392's two passes both pass `Some`, so neither of its recorded cost
/// figures moves.
fn observe_calls(
    source: &Source,
    estate: &Estate<'_>,
    want: &BTreeMap<Callee, BTreeSet<usize>>,
    names: &BTreeSet<String>,
    out: &mut Vec<Observation>,
    declarations: Option<&mut BTreeMap<(String, String), BTreeSet<String>>>,
) {
    let Some(plugin) = estate.registry.for_path(&source.rel) else { return };
    let mut parser = Parser::new();
    if parser.set_language(plugin.language()).is_err() {
        return;
    }
    let Some(parsed) = parser.parse(&source.text, None) else { return };
    let src = source.text.as_bytes();
    let unit = Unit::build(parsed.root_node(), src);
    let resolver = estate.resolver(&source.module);

    if let Some(declarations) = declarations {
        record_declarations(plugin, parsed.root_node(), src, source, names, declarations);
    }

    let Ok(query) = Query::new(plugin.language(), CALL_QUERY) else { return };
    let captures = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, parsed.root_node(), src);
    while let Some(m) = matches.next() {
        let mut name_node = None;
        let mut args_node = None;
        let mut ref_node = None;
        for cap in m.captures {
            match captures[cap.index as usize] {
                "call.name" => name_node = Some(cap.node),
                "call.args" => args_node = Some(cap.node),
                "call.ref" => ref_node = Some(cap.node),
                _ => {}
            }
        }
        if let Some(reference) = ref_node {
            observe_reference(reference, src, source, want, names, out);
            continue;
        }
        let (Some(name_node), Some(args_node)) = (name_node, args_node) else { continue };
        let Ok(name) = name_node.utf8_text(src) else { continue };
        if !names.contains(name) {
            continue;
        }
        let arguments = slots(args_node);
        let callee = Callee { name: name.to_string(), arity: arguments.len() };
        let Some(wanted_slots) = want.get(&callee) else { continue };
        let values: BTreeMap<usize, ArgValue> = wanted_slots
            .iter()
            .filter_map(|s| Some((*s, arg_value(*arguments.get(*s)?, src, &unit, resolver))))
            .collect();
        // S-416: where each forwarded argument came FROM, recorded at the same
        // time the value is read because that is the only point at which the
        // argument's AST node is in hand. Frame one does not use this; it
        // records it so frame two has a target to look up.
        let forwards = values
            .iter()
            .filter(|(_, v)| **v == ArgValue::NeedsAnotherHop)
            .filter_map(|(s, _)| Some((*s, forward_from(*arguments.get(*s)?, src)?)))
            .collect();
        let receiver_is_super = name_node
            .parent()
            .and_then(|call| call.child_by_field_name("object"))
            .is_some_and(|object| object.kind() == "super");
        let super_dispatches_to =
            receiver_is_super.then(|| enclosing_superclass_name(name_node, src)).flatten();
        out.push(Observation {
            callee,
            module: source.module.clone(),
            tree: source.tree,
            file: source.rel.clone(),
            line: name_node.start_position().row as u32 + 1,
            declaration: enclosing_declaration(name_node, src),
            values,
            forwards,
            receiver_is_super,
            super_dispatches_to,
            is_reference: false,
        });
    }
}

/// A `Foo::bar` method reference naming a wanted method.
///
/// It reaches the wrapper and supplies no argument, so every wanted slot is
/// recorded as needing another hop — which refuses the method under
/// [FR-WS-23] AC1 rather than being silently absent from the agreement. The
/// arity is unknown at a reference, so it is recorded against every wanted
/// arity of that name; that is the conservative direction.
///
/// [FR-WS-23]: ../../../docs/specs/requirements/FR-WS-23.md
fn observe_reference(
    reference: Node<'_>,
    src: &[u8],
    source: &Source,
    want: &BTreeMap<Callee, BTreeSet<usize>>,
    names: &BTreeSet<String>,
    out: &mut Vec<Observation>,
) {
    let Ok(text) = reference.utf8_text(src) else { return };
    let Some(name) = text.rsplit("::").next().map(str::trim) else { return };
    if !names.contains(name) {
        return;
    }
    for (callee, wanted_slots) in want.iter().filter(|(c, _)| c.name == name) {
        out.push(Observation {
            callee: callee.clone(),
            module: source.module.clone(),
            tree: source.tree,
            file: source.rel.clone(),
            line: reference.start_position().row as u32 + 1,
            declaration: enclosing_declaration(reference, src),
            values: wanted_slots.iter().map(|s| (*s, ArgValue::NeedsAnotherHop)).collect(),
            // A reference supplies no argument, so there is no forwarded
            // parameter to attribute and no second frame to take: frame two
            // records it as `NotFollowable` rather than following it.
            forwards: BTreeMap::new(),
            // `Foo::bar` has a qualifier, never the `super` keyword in the
            // position `method_invocation` puts a receiver.
            receiver_is_super: false,
            super_dispatches_to: None,
            is_reference: true,
        });
    }
}

/// The same-name ambiguity census: which methods in this module declare one of
/// the wanted names. The `Calls` ledger's target is a bare name, so two
/// declarations sharing one are two things the ledger cannot tell apart.
fn record_declarations(
    plugin: &dyn logos_core::plugin::LanguagePlugin,
    root: Node<'_>,
    src: &[u8],
    source: &Source,
    names: &BTreeSet<String>,
    declarations: &mut BTreeMap<(String, String), BTreeSet<String>>,
) {
    let Ok(query) = Query::new(plugin.language(), DECL_QUERY) else { return };
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, root, src);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let Ok(name) = cap.node.utf8_text(src) else { continue };
            if !names.contains(name) {
                continue;
            }
            declarations
                .entry((source.module.clone(), name.to_string()))
                .or_default()
                .insert(format!("{}:{}", source.rel, cap.node.start_position().row + 1));
        }
    }
}

/// **S-416.** How many methods in each build module share a `(name, arity)` —
/// the signature frame two looks up — counted over the tree the decisive
/// reading admits.
///
/// This is a DECLARATION walk over every source, not a by-product of a call
/// walk, and the distinction is the whole point. An earlier version read the
/// census off the files the `Calls` ledger names as CALLERS of a forwarding
/// method; a method's declaration almost never sits in a file that calls it, so
/// the count came back 0 for exactly the modules it was supposed to describe.
///
/// Keyed on `(module, name, arity)` and restricted to `src/main`, because that
/// is what frame two binds on under the decisive reading. Keying on the bare
/// name — S-392's `ambiguous_by_name` — would count `sendMessage/2` against a
/// `sendMessage/3` lookup, and walking the test tree would count declarations
/// the main-only reading never admits.
fn census_declarations(
    sources: &[&Source],
    registry: &LanguageRegistry,
    names: &BTreeSet<String>,
) -> BTreeMap<(String, String, usize), BTreeSet<String>> {
    let mut out: BTreeMap<(String, String, usize), BTreeSet<String>> = BTreeMap::new();
    for source in sources.iter().filter(|s| s.tree == Tree::Main) {
        let Some(plugin) = registry.for_path(&source.rel) else { continue };
        let mut parser = Parser::new();
        if parser.set_language(plugin.language()).is_err() {
            continue;
        }
        let Some(parsed) = parser.parse(&source.text, None) else { continue };
        let src = source.text.as_bytes();
        let Ok(query) = Query::new(plugin.language(), DECL_SIGNATURE_QUERY) else { continue };
        let captures = query.capture_names();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, parsed.root_node(), src);
        while let Some(m) = matches.next() {
            let (mut name_node, mut params_node) = (None, None);
            for cap in m.captures {
                match captures[cap.index as usize] {
                    "decl.name" => name_node = Some(cap.node),
                    "decl.params" => params_node = Some(cap.node),
                    _ => {}
                }
            }
            let (Some(name_node), Some(params_node)) = (name_node, params_node) else { continue };
            let Ok(name) = name_node.utf8_text(src) else { continue };
            if !names.contains(name) {
                continue;
            }
            out.entry((source.module.clone(), name.to_string(), slots(params_node).len()))
                .or_default()
                .insert(format!("{}:{}", source.rel, name_node.start_position().row + 1));
        }
    }
    out
}

/// Whether one observed call site is admitted to a candidate's agreement: same
/// build module, and — under the main-tree rule — `src/main` only.
///
/// The single spelling of the rule [CR-131] §3.2 C1's second relaxation is
/// about. It is read by [`decide`], which makes the verdict, and by the census
/// attribution, which names the cause; those were two hand-written copies, and
/// a third lived in the report. A rule that decides a blocking gate should not
/// be re-typed for each reader.
///
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
fn admitted_by(observation: &Observation, candidate: &Candidate, main_only: bool) -> bool {
    observation.module == candidate.module && (!main_only || observation.tree == Tree::Main)
}

/// Decide one candidate against the call sites observed for its method.
///
/// The precedence is declared rather than incidental: a missing call site
/// beats a boundary, a boundary beats a second hop, a second hop beats an
/// unresolvable operand, and disagreement is last — because a disagreement
/// between two sites one of which needs another hop is not yet known to be a
/// disagreement at all.
fn decide(candidate: &Candidate, observed: &[&Observation], main_only: bool) -> Hop {
    if let Some(blocked) = candidate.blocked {
        return Hop::Refused(blocked);
    }
    if observed.is_empty() {
        return Hop::Refused(Residue::NoCallSites);
    }
    let in_module: Vec<&&Observation> =
        observed.iter().filter(|o| admitted_by(o, candidate, main_only)).collect();
    if in_module.is_empty() {
        return Hop::Refused(Residue::OutOfModuleOnly);
    }
    let values: Vec<&ArgValue> =
        in_module.iter().filter_map(|o| o.values.get(&candidate.slot)).collect();
    // Defensive, and unreachable by construction — stated so that a reader does
    // NOT conclude `UnresolvableOperand` has two causes. `Callee` carries the
    // arity `parameter_slot` read off the parameter list, every candidate slot
    // is an index into that same list, and `observe_calls` matches only when
    // the argument count equals the arity — so `arguments.get(slot)` is always
    // `Some`. The one real cause is the branch below, which is what the finding
    // attributes all six production instances to.
    if values.len() < in_module.len() {
        return Hop::Refused(Residue::UnresolvableOperand);
    }
    if values.iter().any(|v| **v == ArgValue::NeedsAnotherHop) {
        return Hop::Refused(Residue::TwoOrMoreHops);
    }
    // `FrameTwoRefused` rides with `Unresolvable` deliberately: at two frames a
    // refused second frame is an admitted call site whose argument did not
    // resolve, which is the same fault at a different depth. The one exception
    // is `FrameTwo::ThreeOrMoreFrames`, which `rewrite_at_two_frames` maps to
    // `NeedsAnotherHop` above so the hop-count refusal keeps outranking it —
    // one precedence, declared once, applied at either depth.
    if values.iter().any(|v| matches!(v, ArgValue::Unresolvable(_) | ArgValue::FrameTwoRefused(_))) {
        return Hop::Refused(Residue::UnresolvableOperand);
    }
    let identities: BTreeSet<String> = values.iter().filter_map(|v| v.identity()).collect();
    if identities.len() != 1 {
        return Hop::Refused(Residue::Disagree);
    }
    let files: BTreeSet<&str> = in_module.iter().map(|o| o.file.as_str()).collect();
    Hop::Resolved {
        value: (*values[0]).clone(),
        call_sites: in_module.len(),
        caller_files: files.len(),
    }
}

// ── Pass 3: the second frame (S-416) ────────────────────────────────────────
//
// [CR-131] §3.2 C1 bounds forwarding at TWO frames. The second frame is the
// first frame applied once more, to a target the first frame named: nothing in
// this section re-decides what a publish site is, what a bare parameter is, or
// how call sites agree. `observe_calls` does the looking and `decide` does the
// deciding, both unchanged; this section only says WHICH methods frame two must
// look up and how a refused second frame is spelled as an argument value.
//
// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md

/// The methods the SECOND frame must look up, and the slots it must read.
///
/// Read off the first frame's own observations, so a method reaches this set
/// only because a real in-module call site of a real candidate forwarded a real
/// parameter into it. It is not a re-scan of the estate for "methods that look
/// like wrappers" — that would be a second population, and the floor is stated
/// over one.
fn wanted_at_frame_two(
    f: &Findings,
    observations: &[Observation],
) -> BTreeMap<Callee, BTreeSet<usize>> {
    let mut out: BTreeMap<Callee, BTreeSet<usize>> = BTreeMap::new();
    for c in f.candidates.iter().filter(|c| c.blocked.is_none()) {
        for o in observations.iter().filter(|o| o.callee == c.callee && o.module == c.module) {
            if let Some(forward) = o.forwards.get(&c.slot) {
                out.entry(forward.callee.clone()).or_default().insert(forward.slot);
            }
        }
    }
    out
}

/// A call site that is **not** a caller of the method frame two is looking up,
/// although its name and arity match — [CR-131] C2's "receiver-aware".
///
/// One shape, decidable from the tokens rather than from a type: a
/// `super.m(...)` written inside `class C extends B` calls **`B.m`**. It is a
/// caller of the method frame two is looking up only when that method is
/// declared in `B` — never when it is declared in `C` itself, and never when it
/// is declared in a SIBLING subclass of `B`.
///
/// This is not a nicety, and both halves were found by running the harness
/// against the estate rather than by reading it. [`Callee`] is `(name, arity)`,
/// all the `Calls` ledger's target text can express, and S-392 recorded 7 wanted
/// names declared twice inside one build module, every pair "a base
/// `KafkaProducer.sendMessage/3` together with a subclass override of the same
/// signature". Without this rule:
///
/// * frame two looks up `ArchiveEventKafkaProducer.sendMessage/3`, finds the
///   `super.sendMessage(...)` line **inside that very method**, reads its
///   argument — the method's own parameter — and reports THREE frames. The first
///   run reported 0 of 13 and every one of S-392's seven named sites as 3+
///   frames, entirely from this;
/// * with two sibling producers in one module — `archive-manager` writes
///   `DelayedMessageKafkaProducer` and `ArchiveEventKafkaProducer`, both
///   `extends KafkaProducer` and both overriding `sendMessage/3` — following
///   either one's forward picks up the OTHER's `super.sendMessage(...)` line as
///   a phantom caller. The second run reported 8 of 13 with five sites still
///   refusing, entirely from this.
///
/// Neither conjunct is a proxy for a type, and each case that must NOT be
/// excluded stays in: a genuinely recursive `m()` or `this.m()` inside `m` has
/// no `super` receiver, and the legitimate `D.send` -> `super.send` -> `C.send`
/// chain has `super_dispatches_to == Some("C")`, which is exactly the method
/// being looked up.
///
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
fn dispatches_past(observation: &Observation, forward: &Forward) -> bool {
    observation.receiver_is_super
        && observation.super_dispatches_to.as_deref() != Some(forward.declaring_class.as_str())
}

/// Take one second frame: what do the forwarding method's own callers supply?
///
/// The filters are the first frame's, restated for the second: the same build
/// module, and — under the main-tree rule — `src/main` call sites only, plus the
/// receiver rule [`dispatches_past`] states. The precedence is the first frame's
/// too, in the same order [`decide`] applies it, because a disagreement between
/// two sites one of which forwards again is not yet known to be a disagreement.
fn second_frame(
    forward: &Forward,
    module: &str,
    main_only: bool,
    index: &BTreeMap<Callee, Vec<&Observation>>,
) -> FrameTwo {
    use FrameTwoRefusal as R;
    let Some(all) = index.get(&forward.callee) else { return FrameTwo::Refused(R::NoCallSite) };
    let sites: Vec<&Observation> =
        all.iter().copied().filter(|o| !dispatches_past(o, forward)).collect();
    let in_module: Vec<&Observation> =
        sites.iter().copied().filter(|o| o.module == module).collect();
    if in_module.is_empty() {
        // Call sites exist and none is in the module: a boundary, not an
        // absence. The two are different faults and the census keeps them apart.
        return FrameTwo::Refused(if sites.is_empty() { R::NoCallSite } else { R::OutOfModuleCaller });
    }
    let admitted: Vec<&Observation> =
        in_module.iter().copied().filter(|o| !main_only || o.tree == Tree::Main).collect();
    if admitted.is_empty() {
        // Every caller is in the module and every one is in the test tree. The
        // main-tree rule removed them all, so there is no call site this reading
        // admits — reported as such, and the removed sites are named by
        // `Findings::excluded_by_main_tree`.
        return FrameTwo::Refused(R::NoCallSite);
    }
    let values: Vec<&ArgValue> =
        admitted.iter().filter_map(|o| o.values.get(&forward.slot)).collect();
    if values.len() < admitted.len() {
        return FrameTwo::Refused(R::NotFollowable);
    }
    if values.iter().any(|v| **v == ArgValue::NeedsAnotherHop) {
        return FrameTwo::Refused(R::ThreeOrMoreFrames);
    }
    if let Some(ArgValue::Unresolvable(r)) =
        values.iter().find(|v| matches!(v, ArgValue::Unresolvable(_)))
    {
        return FrameTwo::Refused(R::Unresolvable(*r));
    }
    let identities: BTreeSet<String> = values.iter().filter_map(|v| v.identity()).collect();
    if identities.len() != 1 {
        return FrameTwo::Refused(R::DisagreeingCallers);
    }
    FrameTwo::Resolved((*values[0]).clone())
}

/// One candidate's first-frame call sites, with every forwarded argument
/// replaced by what the second frame proved about it.
///
/// The result is fed straight to [`decide`], which is why the two readings share
/// one agreement rule rather than two spellings of it. The mapping is the whole
/// of the trick:
///
/// * a second frame that resolved becomes its terminal value, so the call site
///   now agrees (or disagrees) like any other;
/// * [`FrameTwo::ThreeOrMoreFrames`] becomes [`ArgValue::NeedsAnotherHop`], so
///   `decide` refuses with `Residue::TwoOrMoreHops` — which at this depth reads
///   "three or more frames", and the census spells it that way;
/// * every other refusal becomes [`ArgValue::FrameTwoRefused`], which `decide`
///   treats exactly as it treats an unresolvable operand.
fn rewrite_at_two_frames(
    candidate: &Candidate,
    observed: &[&Observation],
    main_only: bool,
    index: &BTreeMap<Callee, Vec<&Observation>>,
) -> Vec<Observation> {
    observed
        .iter()
        .map(|o| {
            let mut next = (*o).clone();
            if next.values.get(&candidate.slot) != Some(&ArgValue::NeedsAnotherHop) {
                return next;
            }
            let replacement = match o.forwards.get(&candidate.slot) {
                None => ArgValue::FrameTwoRefused(FrameTwoRefusal::NotFollowable),
                Some(forward) => match second_frame(forward, &o.module, main_only, index) {
                    FrameTwo::Resolved(value) => value,
                    FrameTwo::Refused(FrameTwoRefusal::ThreeOrMoreFrames) => {
                        ArgValue::NeedsAnotherHop
                    }
                    FrameTwo::Refused(other) => ArgValue::FrameTwoRefused(other),
                },
            };
            next.values.insert(candidate.slot, replacement);
            next
        })
        .collect()
}

/// Decide one candidate at two frames, under one call-site reading.
fn decide_at_two_frames(
    candidate: &Candidate,
    observed: &[&Observation],
    main_only: bool,
    index: &BTreeMap<Callee, Vec<&Observation>>,
) -> Hop {
    let rewritten = rewrite_at_two_frames(candidate, observed, main_only, index);
    let borrowed: Vec<&Observation> = rewritten.iter().collect();
    decide(candidate, &borrowed, main_only)
}

/// Name the two-frame refusal, from the verdict [`decide`] returned and the
/// values it read.
///
/// The verdict is never recomputed here — that would be a second precedence to
/// keep in step. This maps `decide`'s own answer onto the two-frame vocabulary,
/// and only in the one case where `decide`'s vocabulary is coarser than the
/// census needs (`UnresolvableOperand`, which at two frames has four causes)
/// does it look at the values, picking the cause by
/// [`FrameTwo::census_rank`].
fn two_frame_reason(hop: &Hop, values: &[&ArgValue]) -> Option<TwoFrameResidue> {
    let residue = hop.residue()?;
    Some(match residue {
        Residue::NotAMethodParameter => TwoFrameResidue::NotAMethodParameter,
        Residue::NoCallSites => TwoFrameResidue::NoCallSiteAtFrameOne,
        Residue::OutOfModuleOnly => TwoFrameResidue::OutOfModuleAtFrameOne,
        Residue::TwoOrMoreHops => TwoFrameResidue::ThreeOrMoreFrames,
        Residue::Disagree => TwoFrameResidue::DisagreeAtFrameOne,
        // Four causes at this depth, and the ranking has to span BOTH frames.
        // Filtering to `FrameTwoRefused` first was wrong: a plain
        // `Unresolvable` is the frame-ONE Mockito mechanism that
        // `TwoFrameResidue::UnresolvableOperand` exists to name, it outranks
        // every frame-two boundary in `census_rank`, and dropping it meant a
        // candidate blocked by an unresolvable `src/main` operand was reported
        // as a pure frame-two artefact — which is precisely the story the
        // "ambiguous pool" caveat rests on, so it must not be able to borrow a
        // row from frame one.
        Residue::UnresolvableOperand => values
            .iter()
            .filter_map(|v| match v {
                ArgValue::FrameTwoRefused(t) => Some(t.clone()),
                ArgValue::Unresolvable(r) => Some(FrameTwoRefusal::Unresolvable(*r)),
                _ => None,
            })
            .min_by_key(FrameTwoRefusal::census_rank)
            .map_or(TwoFrameResidue::UnresolvableOperand, |t| match t {
                FrameTwoRefusal::NoCallSite => TwoFrameResidue::NoCallSiteAtFrameTwo,
                FrameTwoRefusal::OutOfModuleCaller => TwoFrameResidue::OutOfModuleCaller,
                FrameTwoRefusal::DisagreeingCallers => TwoFrameResidue::DisagreeingCallers,
                _ => TwoFrameResidue::UnresolvableOperand,
            }),
    })
}

// ── The measurement ─────────────────────────────────────────────────────────

/// Read every source the plugin registry claims, once. IO is done here and not
/// inside a timed pass, so [`Cost`] measures the hop's computation rather than
/// the estate's disk.
fn read_sources(root: &Path, registry: &LanguageRegistry, config: &ConfigCorpus) -> Vec<Source> {
    // `parents(false)`, `git_global(false)`, `ignore(false)`: the containment
    // settings the parent harness's walk uses, for the reason it gives — a
    // developer's global ignore file must not quietly change a published
    // measurement.
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .build();
    let mut out = Vec::new();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else { continue };
        let rel = rel.to_string_lossy().to_string();
        let Some(plugin) = registry.for_path(&rel) else { continue };
        let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
        out.push(Source {
            module: config.module_of(&rel).to_string(),
            tree: Tree::of(&rel),
            language: plugin.name().to_string(),
            rel,
            text,
        });
    }
    out
}

fn findings(root: &Path) -> &'static Findings {
    static ONCE: std::sync::OnceLock<Findings> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| measure_forwarding(root))
}

fn measure_forwarding(root: &Path) -> Findings {
    let registry = LanguageRegistry::load(root).expect("plugin registry loads");
    let config = ConfigCorpus::discover(root);
    let properties = PropertiesIndex::build(root, &config, &registry);
    let symbols = SymbolContext::default();
    let sources = read_sources(root, &registry, &config);
    let estate =
        Estate {
            registry: &registry,
            lookup: CorpusLookup(&config),
            properties: &properties,
            symbols: &symbols,
        };

    let mut f = Findings::default();
    for s in &sources {
        f.members.insert(member_of(&s.rel));
    }
    let java: Vec<&Source> = sources.iter().filter(|s| s.language == JAVA).collect();
    f.java_files = java.len();

    let mut ledger = CallsLedger::default();
    let started = Instant::now();
    for s in &java {
        scan_for_candidates(s, &estate, &mut ledger, &mut f);
    }
    f.cost.enumerate = started.elapsed();

    let want = wanted(&f);
    let names: BTreeSet<String> = want.keys().map(|c| c.name.clone()).collect();

    let mut declarations = BTreeMap::new();
    let started = Instant::now();
    let mut observations = Vec::new();
    for s in &java {
        observe_calls(s, &estate, &want, &names, &mut observations, Some(&mut declarations));
    }
    f.cost.hop_whole_estate = started.elapsed();
    f.cost.estate_files = java.len();

    // The targeted pass: only the files the `Calls` ledger names as callers.
    // This is what a ledger-driven implementation would actually open, so it is
    // the figure the perf reconciliation should use — and the gap between its
    // observation count and the whole-estate one is a CRA-06 measurement, not a
    // timing artefact.
    let targets = ledger.files_naming(&names);
    let started = Instant::now();
    let mut targeted = Vec::new();
    let mut ignored = BTreeMap::new();
    let mut targeted_files = 0;
    for s in java.iter().filter(|s| targets.contains(s.rel.as_str())) {
        targeted_files += 1;
        observe_calls(s, &estate, &want, &names, &mut targeted, Some(&mut ignored));
    }
    f.cost.hop_targeted = started.elapsed();
    f.cost.targeted_files = targeted_files;

    decide_every_candidate(&mut f, &observations);

    // ── The second frame (S-416) ──
    //
    // Driven off the first frame's observations, so the population is the one
    // the floor is stated over and not a second scan of the estate. The pass is
    // restricted to the files the `Calls` ledger names as callers of a
    // forwarding method — what a real implementation would open — and run
    // TWICE: once with the ambiguity census off, which is the parse-and-resolve
    // figure, and once with it on, so the scaffolding's share is a measurement
    // rather than a caveat.
    let want2 = wanted_at_frame_two(&f, &observations);
    let names2: BTreeSet<String> = want2.keys().map(|c| c.name.clone()).collect();
    let targets2 = ledger.files_naming(&names2);
    let frame_two_sources: Vec<&&Source> =
        java.iter().filter(|s| targets2.contains(s.rel.as_str())).collect();

    let started = Instant::now();
    let mut observations2 = Vec::new();
    for s in &frame_two_sources {
        observe_calls(s, &estate, &want2, &names2, &mut observations2, None);
    }
    f.cost.frame_two = started.elapsed();
    f.cost.frame_two_files = frame_two_sources.len();
    f.cost.frame_two_callees = want2.len();

    let started = Instant::now();
    let mut scaffolded = Vec::new();
    let mut ignored2 = BTreeMap::new();
    for s in &frame_two_sources {
        observe_calls(s, &estate, &want2, &names2, &mut scaffolded, Some(&mut ignored2));
    }
    f.cost.frame_two_with_scaffolding = started.elapsed();

    // The signature census frame two's attribution is read against. A walk over
    // every main-tree source for the DECLARATIONS of the forwarding names — not
    // over the caller files, which is where an earlier version read it and got
    // 0 for the very modules it describes.
    let signatures = census_declarations(&java, &registry, &names2);
    decide_at_two_frames_for_every_candidate(&mut f, &observations, &observations2, &signatures);

    f.ledger = answer_the_ledger_question(&f, &ledger, &observations, &targeted, &declarations);
    f
}

/// Run [`decide_at_two_frames`] for every candidate under both call-site
/// readings, and record the evidence each verdict rests on.
fn decide_at_two_frames_for_every_candidate(
    f: &mut Findings,
    observations: &[Observation],
    frame_two: &[Observation],
    signatures: &BTreeMap<(String, String, usize), BTreeSet<String>>,
) {
    let mut by_callee: BTreeMap<&Callee, Vec<&Observation>> = BTreeMap::new();
    for o in observations {
        by_callee.entry(&o.callee).or_default().push(o);
    }
    let mut index: BTreeMap<Callee, Vec<&Observation>> = BTreeMap::new();
    for o in frame_two {
        index.entry(o.callee.clone()).or_default().push(o);
    }

    let mut two_frame = BTreeMap::new();
    let mut two_frame_main_only = BTreeMap::new();
    let mut reasons = BTreeMap::new();
    let mut forwarding = BTreeMap::new();
    let mut detail = BTreeMap::new();
    let mut excluded = BTreeMap::new();
    let mut declarations = BTreeMap::new();
    let mut chains = BTreeMap::new();

    for (i, c) in f.candidates.iter().enumerate() {
        let observed = by_callee.get(&c.callee).cloned().unwrap_or_default();
        let in_module: Vec<&&Observation> =
            observed.iter().filter(|o| o.module == c.module).collect();

        // The join key for S-392's seven named sites: every in-module call site
        // that forwards a parameter, untruncated.
        forwarding.insert(
            i,
            in_module
                .iter()
                .filter(|o| o.values.get(&c.slot) == Some(&ArgValue::NeedsAnotherHop))
                .map(|o| format!("{}:{} [{}]", o.file, o.line, o.tree.label()))
                .collect::<Vec<_>>(),
        );

        // Every in-module call site the main-tree rule removes, NAMED with what
        // its argument resolved to. CR-131 C1 justifies the main-only reading on
        // "a Mockito stub is not a publish"; this is what makes that auditable.
        excluded.insert(
            i,
            in_module
                .iter()
                .filter(|o| o.tree != Tree::Main)
                .filter_map(|o| {
                    let value = o.values.get(&c.slot)?;
                    Some(format!("{}:{} [{}] {}", o.file, o.line, o.tree.label(), value.label()))
                })
                .collect::<Vec<_>>(),
        );

        // The chains a second frame actually carried, for the generality caveat.
        chains.insert(
            i,
            in_module
                .iter()
                .filter(|o| o.tree == Tree::Main)
                .filter_map(|o| o.forwards.get(&c.slot))
                // Defensive, and unreachable by construction — stated so a
                // reader does not take it for a live filter. A candidate whose
                // second frame refused cannot be `Resolved` at all: the refusal
                // becomes `FrameTwoRefused` or `NeedsAnotherHop` and `decide`
                // turns it into a refusal, and `two_frame_generality` admits
                // only resolved candidates. Kept because the map is also read
                // by `resolutions_through_a_second_frame`, where the intent
                // "chains that actually carried a value" should be explicit.
                .filter(|forward| {
                    matches!(second_frame(forward, &c.module, true, &index), FrameTwo::Resolved(_))
                })
                .map(|forward| {
                    format!("{}/{}({})", c.module, forward.callee.name, forward.callee.arity)
                })
                .collect::<BTreeSet<_>>(),
        );

        // What each second frame proved, under the DECISIVE reading.
        detail.insert(
            i,
            in_module
                .iter()
                .filter(|o| o.tree == Tree::Main)
                .filter(|o| o.values.get(&c.slot) == Some(&ArgValue::NeedsAnotherHop))
                .map(|o| match o.forwards.get(&c.slot) {
                    None => format!(
                        "{}:{} => {}",
                        o.file,
                        o.line,
                        FrameTwoRefusal::NotFollowable.label()
                    ),
                    Some(forward) => format!(
                        "{}:{} forwards slot {} of {}({}) => {}",
                        o.file,
                        o.line,
                        forward.slot,
                        forward.callee.name,
                        forward.callee.arity,
                        second_frame(forward, &o.module, true, &index).label(),
                    ),
                })
                .collect::<Vec<_>>(),
        );

        // How ambiguous the second frame's lookup is, in this candidate's own
        // module: the most declarations any forwarding method's bare name has.
        // 1 means `(name, arity)` and "receiver-aware" are the same rule here.
        declarations.insert(
            i,
            in_module
                .iter()
                .filter_map(|o| o.forwards.get(&c.slot))
                .map(|forward| {
                    signatures
                        .get(&(
                            c.module.clone(),
                            forward.callee.name.clone(),
                            forward.callee.arity,
                        ))
                        .map_or(0, BTreeSet::len)
                })
                .max()
                .unwrap_or_default(),
        );

        let headline = decide_at_two_frames(c, &observed, false, &index);

        // ONE rewrite, and the verdict and the values are read off it together.
        // They used to be two independent calls with identical arguments, so
        // "the values the decisive verdict was read off" held only because the
        // function happens to be deterministic — and the filter picking the
        // admitted values was a third hand-written copy of `decide`'s own
        // admission rule, which is the twin this file exists to avoid.
        // `admitted_by` is now that rule's single spelling.
        let rewritten = rewrite_at_two_frames(c, &observed, true, &index);
        let borrowed: Vec<&Observation> = rewritten.iter().collect();
        let decisive = decide(c, &borrowed, true);
        let values: Vec<&ArgValue> = borrowed
            .iter()
            .filter(|o| admitted_by(o, c, true))
            .filter_map(|o| o.values.get(&c.slot))
            .collect();
        if let Some(reason) = two_frame_reason(&decisive, &values) {
            reasons.insert(i, reason);
        }

        two_frame.insert(i, headline);
        two_frame_main_only.insert(i, decisive);
    }

    f.hops_two_frame = two_frame;
    f.hops_two_frame_main_only = two_frame_main_only;
    f.two_frame_residue = reasons;
    f.forwarding_sites = forwarding;
    f.frame_two_detail = detail;
    f.excluded_by_main_tree = excluded;
    f.frame_two_name_declarations = declarations;
    f.frame_two_chains = chains;
}

/// Run [`decide`] for every candidate under both call-site readings, and record
/// the generality caveat the floor declared in advance.
fn decide_every_candidate(f: &mut Findings, observations: &[Observation]) {
    let mut by_callee: BTreeMap<&Callee, Vec<&Observation>> = BTreeMap::new();
    for o in observations {
        by_callee.entry(&o.callee).or_default().push(o);
    }
    let mut hops = BTreeMap::new();
    let mut main_only = BTreeMap::new();
    let mut methods = BTreeSet::new();
    let mut members = BTreeSet::new();
    let mut evidence = BTreeMap::new();
    let mut blockers = BTreeMap::new();
    for (i, c) in f.candidates.iter().enumerate() {
        let observed = by_callee.get(&c.callee).cloned().unwrap_or_default();
        evidence.insert(
            i,
            observed
                .iter()
                .filter(|o| o.module == c.module)
                .map(|o| format!("{}:{} [{}]", o.file, o.line, o.tree.label()))
                .collect::<Vec<_>>(),
        );
        // Computed before the blocker scan so a competing value can be labelled
        // as such: the same in-module values `decide` reads.
        let identities: BTreeSet<String> = observed
            .iter()
            .filter(|o| o.module == c.module)
            .filter_map(|o| o.values.get(&c.slot)?.identity())
            .collect();
        let disagrees = identities.len() > 1;
        let mut blocking: Vec<(u8, String)> = observed
            .iter()
            .filter(|o| o.module == c.module)
            .filter_map(|o| {
                let value = o.values.get(&c.slot)?;
                // Rank by the precedence `decide` applies, so the first line
                // printed is the site that DECIDED the reason printed beside
                // it. Sampling in walk order showed four test-tree refusals
                // under a verdict of "2+ hops", which reads as a contradiction.
                // FR-WS-23 AC2 requires a disagreement to emit PER-SITE
                // LABELLED CANDIDATES, never an average. A resolved-but-
                // competing value is therefore evidence too — without rank 2 a
                // `Disagree` refusal printed its reason and its count and never
                // said which site held which value, which is the one thing a
                // reader needs to judge whether the rule is implementable.
                let rank = match value {
                    ArgValue::NeedsAnotherHop => 0,
                    ArgValue::Unresolvable(_) => 1,
                    _ if disagrees => 2,
                    _ => return None,
                };
                Some((rank, format!("{}:{} [{}] {}", o.file, o.line, o.tree.label(), value.label())))
            })
            .collect();
        blocking.sort();
        let total = blocking.len();
        let mut evidence_lines: Vec<String> =
            blocking.into_iter().map(|(_, s)| s).take(BLOCKER_SAMPLE).collect();
        if total > BLOCKER_SAMPLE {
            evidence_lines.push(format!("… and {} more", total - BLOCKER_SAMPLE));
        }
        blockers.insert(i, evidence_lines);
        let hop = decide(c, &observed, false);
        if hop.resolved() && c.arm == Arm::BrokerPublish && c.tree == Tree::Main {
            methods.insert(format!("{}/{}", c.module, c.callee.name));
            members.insert(c.member.clone());
        }
        hops.insert(i, hop);
        main_only.insert(i, decide(c, &observed, true));
    }
    f.hops = hops;
    f.hops_main_only = main_only;
    f.caller_sites = evidence;
    f.blockers = blockers;
    f.resolved_methods = methods;
    f.resolved_members = members;
}

/// [AC3] — what the existing `Calls` ledger can answer, measured rather than
/// asserted from reading `RefFact`'s fields.
fn answer_the_ledger_question(
    f: &Findings,
    ledger: &CallsLedger,
    observations: &[Observation],
    targeted: &[Observation],
    declarations: &BTreeMap<(String, String), BTreeSet<String>>,
) -> LedgerAnswer {
    let names: BTreeSet<&str> =
        f.candidates.iter().map(|c| c.callee.name.as_str()).collect();
    let in_module: Vec<&Observation> = observations
        .iter()
        .filter(|o| !o.is_reference)
        .filter(|o| f.candidates.iter().any(|c| c.callee == o.callee && c.module == o.module))
        .collect();

    // The dedup grain is `(source declaration, target)`, so every call site
    // after the first inside one declaration is a site the agreement rule
    // cannot see.
    let mut per_declaration: BTreeMap<(&str, &str, &str), usize> = BTreeMap::new();
    for o in in_module.iter().filter(|o| o.declaration.is_some()) {
        let declaration = o.declaration.as_deref().unwrap_or_default();
        *per_declaration
            .entry((o.file.as_str(), declaration, o.callee.name.as_str()))
            .or_default() += 1;
    }
    let collapsed: usize = per_declaration.values().map(|n| n.saturating_sub(1)).sum();

    let resolved_values: BTreeSet<String> = f
        .hops
        .values()
        .filter_map(|h| match h {
            Hop::Resolved { value, .. } => value.identity(),
            Hop::Refused(_) => None,
        })
        .map(|id| id.split_once(':').map(|(_, v)| v.to_string()).unwrap_or(id))
        .collect();

    LedgerAnswer {
        files_extracted: ledger.files,
        calls_rows: ledger.rows,
        rows_naming_a_wanted_method: names
            .iter()
            .filter_map(|n| ledger.rows_by_name.get(*n))
            .sum(),
        caller_declarations: names
            .iter()
            .filter_map(|n| ledger.by_name.get(*n))
            .map(BTreeSet::len)
            .sum(),
        syntactic_call_sites: in_module.len(),
        collapsed_by_dedup: collapsed,
        rows_carrying_the_operand: resolved_values
            .iter()
            .filter(|v| ledger.all_targets.contains(*v))
            .count(),
        ambiguous_by_name: declarations.values().filter(|d| d.len() > 1).count(),
        call_sites_the_ledger_misses: observations
            .iter()
            .filter(|o| !o.is_reference)
            .count()
            .saturating_sub(targeted.iter().filter(|o| !o.is_reference).count()),
        method_references: observations.iter().filter(|o| o.is_reference).count(),
        sample_targets: ledger.targets.clone(),
    }
}

// ── The report ──────────────────────────────────────────────────────────────

/// [AC1] the populations, production and test, never summed.
fn report_populations(f: &Findings) {
    println!(
        "\n=== S-392: the one-hop parameter-forwarding residue ===\n\
         Estate: {} members, {} Java files, {} files carrying a header-form publish site.\n",
        f.members.len(),
        f.java_files,
        f.publish_files,
    );
    println!("--- AC1: populations, production and test reported separately ---");
    println!("{:<16} {:>8} {:>8}", "population", "main", "test");
    let row = |label: &str, m: &BTreeMap<Tree, usize>| {
        println!(
            "{label:<16} {:>8} {:>8}",
            m.get(&Tree::Main).copied().unwrap_or_default(),
            m.get(&Tree::Test).copied().unwrap_or_default(),
        );
    };
    row("publish sites", &f.publish_sites);
    row("client sites*", &f.client_sites);
    println!("* gate-admitted client-call sites carrying at least one parameter operand.\n");

    println!("{:<16} {:<6} {:>10} {:>10} {:>8}", "arm", "tree", "candidates", "resolved", "%");
    for arm in [Arm::BrokerPublish, Arm::ClientCall] {
        for tree in [Tree::Main, Tree::Test] {
            let n = f.candidates_in(arm, tree);
            let ok = f.resolved_in(arm, tree, &f.hops);
            let pct = (ok * 100).checked_div(n).unwrap_or_default();
            println!("{:<16} {:<6} {n:>10} {ok:>10} {pct:>7}%", arm.label(), tree.label());
        }
    }
}

/// One hop outcome, as the reports spell it. Hoisted because the identical
/// eight-line closure was pasted into both `report_residue` and
/// `report_two_frame_residue`; it is pure formatting with no measurement
/// semantics, so the two reports keep their own headings and lose nothing.
fn describe_hop(hop: Option<&Hop>) -> String {
    match hop {
        Some(Hop::Resolved { value, call_sites, caller_files }) => format!(
            "RESOLVED to {} from {call_sites} site(s) in {caller_files} file(s)",
            value.label(),
        ),
        Some(Hop::Refused(r)) => format!("refused: {}", r.label()),
        None => "not judged".into(),
    }
}

/// [AC2] the residue: what fails, and how much of it is the bound itself.
fn report_residue(f: &Findings) {
    println!("\n--- AC2: the residue, by reason ---");
    for arm in [Arm::BrokerPublish, Arm::ClientCall] {
        for tree in [Tree::Main, Tree::Test] {
            let census = f.residue(arm, tree);
            if census.is_empty() {
                continue;
            }
            println!("{} / {}:", arm.label(), tree.label());
            // Every reason, zeros included, on the arm and tree carrying the
            // floor. AC2 names TWO quantities — "two or more hops **or** reach
            // outside their module" — and suppressing a zero row leaves a reader
            // of the durable finding unable to separate them. Elsewhere the
            // zero rows are noise, so they stay suppressed.
            let decisive = arm == Arm::BrokerPublish && tree == Tree::Main;
            for reason in Residue::ALL {
                let n = census.get(&reason).copied().unwrap_or_default();
                if n > 0 || decisive {
                    println!("    {n:>4}  {}", reason.label());
                }
            }
        }
    }
    println!(
        "\nbeyond the one-module bound (2+ hops or out-of-module), production publish: {} of {}",
        f.beyond_the_bound(Arm::BrokerPublish, Tree::Main),
        f.candidates_in(Arm::BrokerPublish, Tree::Main),
    );
    // AC2's other half, stated as a figure rather than left implicit in a
    // suppressed census row.
    println!(
        "in-module call sites: {} observed; {} AGREE under the headline reading, {} across the \
         main-only resolutions",
        f.ledger.syntactic_call_sites,
        f.agreeing_call_sites(Arm::BrokerPublish, Tree::Main, &f.hops),
        f.agreeing_call_sites(Arm::BrokerPublish, Tree::Main, &f.hops_main_only),
    );
    println!(
        "sensitivity — the same figure counting only `src/main` call sites toward agreement: \
         {} resolved (headline reading: {}); {} candidate(s) are refused SOLELY by test-tree \
         call sites",
        f.production_publish_resolved_main_only(),
        f.production_publish_resolved(),
        f.refused_only_by_test_call_sites(Arm::BrokerPublish, Tree::Main),
    );
    println!(
        "\nper-candidate detail. Every PRODUCTION candidate of both arms — the two \n\
         populations AC1 names — plus any candidate anywhere whose call sites DISAGREE, \n\
         because FR-WS-23 AC2 requires a disagreement to emit per-site labelled candidates \n\
         and the estate's only disagreement is in the test tree:"
    );
    for (i, c) in f.candidates.iter().enumerate() {
        let disagrees = f.hops.get(&i).and_then(Hop::residue) == Some(Residue::Disagree);
        if c.tree != Tree::Main && !disagrees {
            continue;
        }
        let verdict = describe_hop(f.hops.get(&i));
        println!(
            "    [{}/{}] {}:{}  {}({}) slot {} operand `{}`  => {verdict}",
            c.arm.label(),
            c.tree.label(),
            c.file,
            c.line,
            c.callee.name,
            c.callee.arity,
            c.slot,
            c.operand,
        );
        if let Some(sites) = f.caller_sites.get(&i).filter(|s| !s.is_empty()) {
            println!("        {} in-module call site(s) read", sites.len());
        }
        for blocker in f.blockers.get(&i).into_iter().flatten() {
            println!("        blocked by {blocker}");
        }
        // The main-only reading, per candidate — so the sensitivity headline is
        // auditable site by site and the VALUE each would resolve to is
        // measured rather than asserted in prose.
        if f.hops.get(&i) != f.hops_main_only.get(&i) {
            println!("        main-only reading: {}", describe_hop(f.hops_main_only.get(&i)));
        }
    }
}

/// [AC3] whether the `Calls` ledger answers the lookup the hop needs.
fn report_ledger(f: &Findings) {
    let l = &f.ledger;
    println!("\n--- AC3: can the existing `Calls` ledger answer the hop? (CRA-06) ---");
    // One width-specified format, as `report_populations` uses: the nine rows
    // were hand-padded and two of them were a column off, so the table the run
    // printed was not the table the finding reproduces.
    let row = |label: &str, n: usize| println!("    {label:<40} {n:>8}");
    row("files run through `extract::extract`", l.files_extracted);
    row("`Calls` rows recorded (denominator)", l.calls_rows);
    row("rows naming a wanted method", l.rows_naming_a_wanted_method);
    row("distinct (caller declaration, method)", l.caller_declarations);
    row("in-module call sites observed", l.syntactic_call_sites);
    row("call sites the dedup collapses away", l.collapsed_by_dedup);
    row("call sites in files the ledger misses", l.call_sites_the_ledger_misses);
    row("method references to a wanted method", l.method_references);
    row("methods whose bare name is ambiguous", l.ambiguous_by_name);
    row("ledger fields carrying the operand", l.rows_carrying_the_operand);
    println!("    sample targets: {:?}", l.sample_targets);
    println!(
        "\n    VERDICT on CRA-06: the direction is {}; the operand is {}.",
        if l.answers_the_direction() { "ANSWERABLE from the ledger" } else { "NOT answerable" },
        if l.answers_the_operand() { "carried too" } else { "NOT carried — it needs the source" },
    );
}

/// [AC5] the measured cost of the hop.
fn report_cost(f: &Findings) {
    let c = &f.cost;
    let candidates = f.candidates.len();
    println!("\n--- AC5: the measured cost of the hop ---");
    println!("    enumerate the sites (today's work)  {:>8.1?}  over {} files", c.enumerate, c.estate_files);
    println!("    hop, whole estate                   {:>8.1?}  over {} files", c.hop_whole_estate, c.estate_files);
    println!("    hop, ledger-targeted files only     {:>8.1?}  over {} files", c.hop_targeted, c.targeted_files);
    println!(
        "    amortised per candidate (targeted)  {:>8} us  over {candidates} candidates\n\
         \x20   — amortised, not marginal: the targeted pass parses each caller file once\n\
         \x20   whatever the candidate count, so this figure falls as candidates rise.",
        c.per_candidate_us(candidates),
    );
    println!(
        "    IO is excluded from all three: sources are read before any timer starts, so \n\
         \x20   these are parse-and-resolve figures. A real implementation re-reads what the\n\
         \x20   extract pass has already read (CR-121 CRA-07)."
    );
}

/// The generality caveat the floor declared in advance — reported whatever the
/// verdict, so a PASS is read for what it licenses.
fn report_generality(f: &Findings) {
    println!("\n--- the declared generality caveat ---");
    println!(
        "    resolved production publish sites rest on {} distinct wrapper method(s) \
         across {} member(s): {:?}",
        f.resolved_methods.len(),
        f.resolved_members.len(),
        f.resolved_methods,
    );
}

// ── S-416's report ──────────────────────────────────────────────────────────

/// The 2x2 grid [CR-131] §3.2 C1 asks to be printed in full: every in-module
/// call site vs `src/main` call sites only, at one frame and at two.
///
/// Only the bottom-right cell carries the floor. The other three are printed so
/// a reader can see exactly how much of the result each of the two relaxations
/// bought — one frame to two, and every call site to main-tree only — instead of
/// being handed one number and a claim about where it came from.
///
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
fn report_two_frame_grid(f: &Findings) {
    let n = f.candidates_in(Arm::BrokerPublish, Tree::Main);
    println!("\n--- S-416: the two readings, at one frame and at two ---");
    println!("production (src/main) broker publish sites resolved, of {n}:\n");
    println!("{:<14} {:>24} {:>24}", "", "every in-module site", "src/main sites only");
    println!(
        "{:<14} {:>24} {:>24}",
        "one frame",
        f.production_publish_resolved(),
        f.production_publish_resolved_main_only(),
    );
    println!(
        "{:<14} {:>24} {:>24}  <= THE FLOOR IS ON THIS CELL",
        "two frames",
        f.production_publish_resolved_two_frame(),
        f.production_publish_resolved_two_frame_main_only(),
    );
    println!(
        "\nthe second frame's own contribution — resolved at two frames and refused at one:\n\
        \x20   {} under the every-site reading, {} under the main-only reading",
        f.bought_by_the_second_frame(Arm::BrokerPublish, Tree::Main, false),
        f.bought_by_the_second_frame(Arm::BrokerPublish, Tree::Main, true),
    );
}

/// Every two-frame refusal, with its own reason — [CR-131] C1's enumeration.
///
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
fn report_two_frame_residue(f: &Findings) {
    println!("\n--- S-416: the two-frame residue, by reason (decisive reading) ---");
    let census = f.two_frame_census(Arm::BrokerPublish, Tree::Main);
    // Every reason, zeros included, on the arm and tree carrying the floor —
    // the rule `report_residue` applies for the same purpose: a suppressed row
    // hides one of the quantities the criterion names.
    for reason in TwoFrameResidue::ALL {
        println!("    {:>4}  {}", census.get(&reason).copied().unwrap_or_default(), reason.label());
    }

    // The limit on this harness, in BOTH directions, as figures. Frame two
    // binds on `(name, arity)`; CR-131 C2 proposes the rule as receiver-aware.
    // Where a module declares a base plus SEVERAL overrides of one signature,
    // this harness pools their callers — which can refuse a disagreement the
    // estate does not write, AND can manufacture a resolution from a caller of
    // a different class. Both directions are reported; neither is asserted.
    let ambiguous = f.refusals_from_an_ambiguous_pool(Arm::BrokerPublish, Tree::Main);
    println!(
        "\nof the {} production refusals, {} look up a signature their own build module \
         declares 3+ times (a base plus SEVERAL overrides):",
        census.values().sum::<usize>(),
        ambiguous.len(),
    );
    for (i, declarations) in &ambiguous {
        let c = &f.candidates[*i];
        println!(
            "    {}:{}  `{}` is declared {declarations}x in `{}`",
            c.file, c.line, c.callee.name, c.module,
        );
    }

    let through_two = f.resolutions_through_a_second_frame(Arm::BrokerPublish, Tree::Main);
    let sound = f.resolutions_on_an_unambiguous_pool(Arm::BrokerPublish, Tree::Main);
    println!(
        "\nand of the {} production sites RESOLVED THROUGH a second frame, {} drew their \
         value from an UNAMBIGUOUS pool (a base plus exactly one override, so the only \
         call targeting the base is the override's own `super`, which the receiver rule \
         already removes):",
        through_two.len(),
        sound.len(),
    );
    for (i, declarations) in &sound {
        let c = &f.candidates[*i];
        println!(
            "    {}:{}  `{}` is declared {declarations}x in `{}`  => the pool cannot mix",
            c.file, c.line, c.callee.name, c.module,
        );
    }
    println!(
        "    Both rows matter and they are NOT the same claim. The refusals above are ones\n\
        \x20   this MEASUREMENT makes and the proposed RULE would not, so for them the headline\n\
        \x20   is a floor. The resolutions here are ones the pooling could in principle have\n\
        \x20   MANUFACTURED — a value drawn from a caller of a different class's same-signature\n\
        \x20   method — and the figure above is what rules that out, site by site. A resolution\n\
        \x20   through a second frame on an AMBIGUOUS pool would be unsound and is asserted\n\
        \x20   not to occur; without that second guard `floor, never a ceiling` would be an\n\
        \x20   assertion about only half the residue."
    );

    println!("\nper-candidate detail, every PRODUCTION broker-publish candidate:");
    for (i, c) in f.candidates.iter().enumerate() {
        if c.arm != Arm::BrokerPublish || c.tree != Tree::Main {
            continue;
        }
        println!(
            "    {}:{}  {}({}) slot {} operand `{}`",
            c.file, c.line, c.callee.name, c.callee.arity, c.slot, c.operand,
        );
        println!("        one frame,  main-only: {}", describe_hop(f.hops_main_only.get(&i)));
        println!("        two frames, every site: {}", describe_hop(f.hops_two_frame.get(&i)));
        println!("        two frames, main-only:  {}", describe_hop(f.hops_two_frame_main_only.get(&i)));
        if let Some(reason) = f.two_frame_residue.get(&i) {
            println!("        two-frame reason: {}", reason.label());
        }
        for line in f.frame_two_detail.get(&i).into_iter().flatten() {
            println!("        frame 2: {line}");
        }
        // The relaxation, made auditable. Never a count on its own: the sites
        // the main-tree rule removes are the entire justification for reading
        // agreement over src/main, so they are named.
        for line in f.excluded_by_main_tree.get(&i).into_iter().flatten() {
            println!("        EXCLUDED by the main-tree rule: {line}");
        }
    }
}

/// The generality caveat `two_frame_forwarding_floor.txt` declared in advance,
/// reported whatever the verdict — so a PASS is read for what it licenses.
fn report_two_frame_generality(f: &Findings) {
    let (methods, members, chains) = f.two_frame_generality();
    println!("\n--- S-416: the declared generality caveat, over the decisive reading ---");
    println!(
        "    the resolved production publish sites rest on {} distinct wrapper method(s) \
         across {} member(s).",
        methods.len(),
        members.len(),
    );
    println!(
        "    {} distinct forwarding chain(s) carried a SECOND frame: {:?}",
        chains.len(),
        chains,
    );
    println!(
        "    The second figure is the one this story added, and it is the narrow one: read it \n\
        \x20   beside the headline, not instead of it. A two-frame yield resting on a handful of\n\
        \x20   chains licenses the bound on those chains and no more."
    );
}

/// S-392's seven named two-frame sites, reconciled BY NAME against this run.
///
/// Evidence for the headline, never a second headline: the gate is decided by
/// the count of 13. A named site this run cannot see at all is harness drift and
/// makes the run VOID — asserted separately, in [`assert_two_frame_non_vacuous`].
fn report_named_site_reconciliation(f: &Findings) -> (usize, usize) {
    println!("\n--- S-416: S-392's seven named two-frame sites, reconciled by name ---");
    let (mut found, mut resolved) = (0, 0);
    for (module, site) in S392_NAMED_TWO_FRAME_SITES {
        let Some(i) = f.candidate_forwarding_at(module, site) else {
            println!("    {module:<32} {site:<40} NOT SEEN BY THIS RUN — harness drift");
            continue;
        };
        found += 1;
        let hop = f.hops_two_frame_main_only.get(&i);
        let verdict = match hop {
            Some(Hop::Resolved { value, .. }) => {
                resolved += 1;
                format!("RESOLVED at two frames to {}", value.label())
            }
            Some(Hop::Refused(_)) => match f.two_frame_residue.get(&i) {
                Some(reason) => format!("refused: {}", reason.label()),
                None => "refused".into(),
            },
            None => "not judged".into(),
        };
        let candidate = &f.candidates[i];
        println!("    {module:<32} {site:<40} {verdict}");
        println!("        candidate: {}:{}", candidate.file, candidate.line);
    }
    println!(
        "\n    {found} of {} named sites seen by this run; {resolved} resolve at two frames \
         under the decisive reading.",
        S392_NAMED_TWO_FRAME_SITES.len(),
    );
    (found, resolved)
}

/// [CR-131] C1's cost criterion: the parse-and-resolve figure over its
/// denominator, with the measurement scaffolding separated by MEASUREMENT.
///
/// S-392 disclosed its scaffolding bias in prose ("they run the same-name
/// ambiguity census inside the timer even though it is measurement
/// scaffolding"). Here the same pass is run twice, with the census off and on,
/// so the share is a number.
///
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
fn report_frame_two_cost(f: &Findings) {
    let c = &f.cost;
    println!("\n--- S-416: the second frame's measured cost ---");
    println!(
        "    frame 2, parse-and-resolve only     {:>8.1?}  over {} files, {} forwarding method(s)",
        c.frame_two, c.frame_two_files, c.frame_two_callees,
    );
    println!(
        "    frame 2, with the ambiguity census  {:>8.1?}  over the same {} files",
        c.frame_two_with_scaffolding, c.frame_two_files,
    );
    println!(
        "    scaffolding's share                 {:>8.1?}  — measured, not disclosed in prose",
        c.frame_two_with_scaffolding.saturating_sub(c.frame_two),
    );
    println!(
        "    per forwarding method (parse-and-resolve)  {:>8} us  over {} method(s)",
        if c.frame_two_callees == 0 {
            0
        } else {
            c.frame_two.as_micros() / c.frame_two_callees as u128
        },
        c.frame_two_callees,
    );
    println!(
        "    Same four caveats as AC5 above and for the same reasons: IO excluded, debug \n\
         \x20   build, single-threaded, amortised rather than marginal. The queries are still\n\
         \x20   compiled afresh per file, so this remains an UPPER bound — the bias is against\n\
         \x20   the change request, which is the safe direction."
    );
}

// ── The gate ────────────────────────────────────────────────────────────────

/// The blocking measurement gate for [CR-121] CRA-05.
///
/// Skips without `LOGOS_REF_WORKSPACE`, so `cargo test --workspace` stays green
/// on a machine without the estate. The verdict is an **assertion**, not a
/// printed table: without one, a regression that flipped the finding would pass
/// silently, and the finding is what decides whether [S-393] and [S-394] are
/// planned at all.
///
/// [S-393]: ../../../docs/planning/journal.md#s-393-a-parameter-passed-operand-resolves-one-hop-within-its-module
/// [S-394]: ../../../docs/planning/journal.md#s-394-config-resolved-topic-identity-so-a-publish-meets-a-subscribe
#[test]
fn measure_the_one_hop_forwarding_residue_over_the_reference_workspace() {
    let Some(root) = super::corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-392 forwarding gate (see this module's docs and forwarding_finding.txt for \
             the recorded finding)."
        );
        return;
    };
    let f = findings(&root);
    report_populations(f);
    report_residue(f);
    report_ledger(f);
    report_cost(f);
    report_generality(f);
    println!("\n--- recorded finding ---\n{RECORDED_FINDING}");

    assert_non_vacuous(f, &root);

    let resolved = f.production_publish_resolved();
    println!(
        "\nVERDICT: {resolved} of {} production publish sites resolve at one hop, against a \
         floor of {ONE_HOP_FLOOR} declared before the run  =>  {}",
        f.candidates_in(Arm::BrokerPublish, Tree::Main),
        if resolved >= ONE_HOP_FLOOR { "HOLDS" } else { "FALSIFIED" },
    );
    assert_the_recorded_finding(f, resolved);
}

/// The recorded verdict, as assertions rather than as a printed table: without
/// them a regression that flipped the finding would still pass, and the finding
/// is what decides whether S-393 and S-394 are planned.
fn assert_the_recorded_finding(f: &Findings, resolved: usize) {
    assert!(
        resolved < ONE_HOP_FLOOR,
        "S-392's recorded finding is that {RECORDED_RESOLVED} of 13 production publish sites          resolve at one hop, below the floor of {ONE_HOP_FLOOR} declared before the run; this          run found {resolved}. If that is real, CR-121's forwarding gate has re-opened:          re-decide CR-121 §8, un-withdraw FR-WS-23 and plan S-393 and S-394 — do not relax          this assertion.",
    );
    assert_eq!(
        resolved, RECORDED_RESOLVED,
        "the headline figure moved from the recorded {RECORDED_RESOLVED} to {resolved}          without the finding being re-recorded",
    );
    // Both halves of the split are pinned. A change that moved sites from one
    // mechanism to the other while leaving the headline at zero would otherwise
    // pass silently, and it is exactly the change that would matter: the second
    // mechanism is a defect in FR-WS-23 AC1's wording, the first is not
    // reachable by any wording at all.
    assert_eq!(
        f.production_publish_resolved_main_only(),
        RECORDED_RESOLVED_MAIN_ONLY,
        "the main-only sensitivity moved from {RECORDED_RESOLVED_MAIN_ONLY} to {}; the          falsification turns on it staying below the floor too, so that neither reading of          FR-WS-23 AC1 rescues the gate",
        f.production_publish_resolved_main_only(),
    );
    assert_eq!(
        f.residue(Arm::BrokerPublish, Tree::Main).get(&Residue::TwoOrMoreHops).copied(),
        Some(RECORDED_TWO_OR_MORE_HOPS),
        "the two-or-more-hop residue moved from {RECORDED_TWO_OR_MORE_HOPS}; this is the          mechanism the verdict rests on and CR-121 §7's slope risk row is written over",
    );
    assert_eq!(
        f.refused_only_by_test_call_sites(Arm::BrokerPublish, Tree::Main),
        RECORDED_REFUSED_BY_TEST_ONLY,
        "the test-stub-blocked count moved from {RECORDED_REFUSED_BY_TEST_ONLY}",
    );
    const {
        // The headline constant, which nothing used to constrain: it could be
        // edited to any value and CI stayed green, because the gate body runs
        // only under LOGOS_REF_WORKSPACE.
        assert!(
            RECORDED_RESOLVED < ONE_HOP_FLOOR,
            "the recorded headline must itself be below the floor, or the finding is not a \
             falsification at all",
        );
        assert!(
            RECORDED_RESOLVED_MAIN_ONLY < ONE_HOP_FLOOR,
            "the favourable-reading counterfactual must itself be below the floor, or the              falsification rests on the reading of FR-WS-23 AC1 rather than on the              measurement",
        );
        assert!(
            RECORDED_TWO_OR_MORE_HOPS + RECORDED_REFUSED_BY_TEST_ONLY
                == PRODUCTION_PUBLISH_SITES,
            "the two mechanisms must partition the population, or one of them is a bucket              rather than a diagnosis",
        );
    }

    // Census floors, not equalities: the estate can only grow, and a re-clone
    // must not redden the run. The verdict figures above are pinned exactly;
    // these guard the order of magnitude the finding was recorded at.
    assert!(
        f.java_files >= 2000 && f.members.len() >= 40,
        "the recorded finding measured 2447 Java files across 82 members; this run saw {}          across {}. A collapsed corpus is a broken harness, not a new finding.",
        f.java_files,
        f.members.len(),
    );
    assert!(
        f.ledger.calls_rows >= 40_000 && f.ledger.syntactic_call_sites > 0,
        "the recorded finding measured 45625 `Calls` rows and 141 in-module call sites;          this run saw {} and {}. AC3's answer rests on both.",
        f.ledger.calls_rows,
        f.ledger.syntactic_call_sites,
    );
    // The denominator the VERDICT line prints and the floor is stated over. It
    // was the only figure in that sentence nothing pinned, so a future estate
    // could have printed "0 of 12" beside a finding that says 13 and stayed
    // green. `assert_non_vacuous` floors the SITE count; this pins the
    // CANDIDATE count, and the two differ by exactly the silent drops
    // `operands_without_a_slot` now counts.
    assert_eq!(
        f.candidates_in(Arm::BrokerPublish, Tree::Main),
        PRODUCTION_PUBLISH_SITES,
        "the production publish population moved from {PRODUCTION_PUBLISH_SITES} candidates; \
         the floor and every figure beside it are stated over that denominator",
    );
    // AC3's coverage half. The finding reasons from this being zero ("the file
    // set they point at covers every syntactic call site"); it was printed and
    // never asserted.
    assert_eq!(
        f.ledger.call_sites_the_ledger_misses, 0,
        "the recorded finding rests on the `Calls` ledger naming every file that holds a \
         call site; this run found {} it does not reach, which changes CRA-06's answer",
        f.ledger.call_sites_the_ledger_misses,
    );
    assert!(
        !f.ledger.answers_the_operand() && f.ledger.answers_the_direction(),
        "AC3's recorded answer is that the ledger answers the DIRECTION and never the          OPERAND. This run answers direction={}, operand={} — CRA-06 has changed and the          cost of S-393 with it.",
        f.ledger.answers_the_direction(),
        f.ledger.answers_the_operand(),
    );
}

/// **S-416 — [CR-131] §3.2 C1's blocking measurement gate.**
///
/// The floor is declared in `two_frame_forwarding_floor.txt`, committed on its
/// own before any of this story's measurement code existed, and is stated over
/// exactly one cell of the grid this test prints: **two frames, agreement taken
/// over `src/main` call sites only, >= 7 of the 13 production publish sites**.
/// The other three cells are printed beside it and carry no floor.
///
/// Skips without `LOGOS_REF_WORKSPACE`, like its S-392 sibling; the fixtures
/// below pin every branch it exercises, corpus or no corpus.
///
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
#[test]
fn two_frame_wrapper_residue_over_main_tree_call_sites() {
    let Some(root) = super::corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-416 two-frame gate (see two_frame_forwarding_floor.txt for the declared \
             floor and two_frame_forwarding_finding.txt for the recorded finding)."
        );
        return;
    };
    let f = findings(&root);
    report_two_frame_grid(f);
    report_two_frame_residue(f);
    report_two_frame_generality(f);
    let (found, named_resolved) = report_named_site_reconciliation(f);
    report_frame_two_cost(f);

    // S-392's guards first: this measurement is an INCREMENT over that one, and
    // an increment over a base that moved is not the quantity CRA-03 asserts.
    assert_non_vacuous(f, &root);
    assert_two_frame_non_vacuous(f, found);

    let resolved = f.production_publish_resolved_two_frame_main_only();
    let holds = resolved >= TWO_FRAME_FLOOR;
    println!(
        "\nVERDICT: {resolved} of {} production publish sites resolve at TWO FRAMES with \
         agreement over `src/main` call sites only, against a floor of {TWO_FRAME_FLOOR} \
         declared before the run  =>  {}",
        f.candidates_in(Arm::BrokerPublish, Tree::Main),
        if holds { "HOLDS" } else { "FALSIFIED" },
    );
    println!(
        "    ({named_resolved} of S-392's {} named two-frame sites resolve — evidence for the \
         headline, not a second headline.)",
        S392_NAMED_TWO_FRAME_SITES.len(),
    );
    if !holds {
        println!(
            "    CR-131 CRA-03 is FALSIFIED. No requirement is created — FR-WS-26 is not \
             filed and FR-WS-23 stays WITHDRAWN IN PLACE — and S-417 stays UNPLANNED."
        );
    }
    println!("\n--- recorded finding ---\n{TWO_FRAME_RECORDED_FINDING}");
    assert_the_recorded_two_frame_finding(f, resolved, named_resolved);
}

/// The recorded verdict, as assertions rather than as a printed table: without
/// them a regression that flipped the finding would still pass, and the finding
/// is what decides whether [S-417] is planned.
///
/// [S-417]: ../../../docs/planning/journal.md#s-417-a-parameter-passed-topic-operand-resolves-through-its-wrappers-callers-within-two-frames
fn assert_the_recorded_two_frame_finding(f: &Findings, resolved: usize, named_resolved: usize) {
    assert!(
        resolved >= TWO_FRAME_FLOOR,
        "S-416's recorded finding is that {RECORDED_TWO_FRAME_MAIN_ONLY} of 13 production          publish sites resolve at two frames over main-tree call sites, at or above the floor          of {TWO_FRAME_FLOOR} declared before the run; this run found {resolved}. If that is          real, CR-131 CRA-03 has fallen: re-decide CR-131 §8, and S-417 and FR-WS-26 go back          to unplanned — do not relax this assertion.",
    );
    assert_eq!(
        resolved, RECORDED_TWO_FRAME_MAIN_ONLY,
        "the headline figure moved from the recorded {RECORDED_TWO_FRAME_MAIN_ONLY} to          {resolved} without the finding being re-recorded",
    );
    assert_eq!(
        f.production_publish_resolved_two_frame(),
        RECORDED_TWO_FRAME_ALL_SITES,
        "the every-site two-frame reading moved from {RECORDED_TWO_FRAME_ALL_SITES}. The          finding rests on the two relaxations being DEPENDENT — the second frame buys nothing          without the main-tree rule — and that is the half a reader is most likely to take on          trust",
    );
    assert_eq!(
        f.bought_by_the_second_frame(Arm::BrokerPublish, Tree::Main, true),
        RECORDED_BOUGHT_BY_FRAME_TWO,
        "the second frame's own contribution moved from {RECORDED_BOUGHT_BY_FRAME_TWO}. The          gate clears its floor by exactly this much: at 0 the reading is 6 and CRA-03 fails",
    );
    assert_eq!(
        f.two_frame_census(Arm::BrokerPublish, Tree::Main).values().sum::<usize>(),
        RECORDED_TWO_FRAME_REFUSALS,
        "the two-frame refusal count moved from {RECORDED_TWO_FRAME_REFUSALS}",
    );
    // The finding's central caveat, pinned as an EQUALITY rather than as prose:
    // every refusal is attributable to this harness's `(name, arity)` frame-two
    // binding, which is what makes the headline a floor on C2's rule rather than
    // a ceiling. A run that separates these two numbers has a residue the caveat
    // does not cover.
    let from_ambiguity = f.refusals_from_an_ambiguous_pool(Arm::BrokerPublish, Tree::Main).len();
    assert_eq!(
        from_ambiguity, RECORDED_REFUSALS_FROM_NAME_AMBIGUITY,
        "the refusals attributable to the (name, arity) frame-two binding moved from          {RECORDED_REFUSALS_FROM_NAME_AMBIGUITY} to {from_ambiguity}. The recorded finding          reports 8 of 13 as a FLOOR on what CR-131 C2's receiver-aware rule would resolve,          and that reading rests on this equality",
    );
    // The OTHER direction, and the one the review had to ask for. Pooling can
    // manufacture a resolution as well as refuse one, so a headline drawn
    // partly from second frames is only sound if every one of those second
    // frames read an unambiguous pool. Without this the "floor, never a
    // ceiling" sentence would be a claim about half the residue.
    let through_two = f.resolutions_through_a_second_frame(Arm::BrokerPublish, Tree::Main);
    let sound = f.resolutions_on_an_unambiguous_pool(Arm::BrokerPublish, Tree::Main);
    assert_eq!(
        sound.len(),
        through_two.len(),
        "{} of the {} production sites resolved THROUGH a second frame drew their value from \
         a pool that spans several overrides. Such a resolution can be manufactured from a \
         caller of a different class's same-signature method, so the headline is not sound \
         as recorded — re-verify those sites by hand before trusting the verdict.",
        through_two.len() - sound.len(),
        through_two.len(),
    );
    assert_eq!(
        through_two.len(),
        RECORDED_BOUGHT_BY_FRAME_TWO,
        "the number of sites resolved through a second frame moved from          {RECORDED_BOUGHT_BY_FRAME_TWO}; it is the quantity the soundness guard above covers",
    );
    assert_eq!(
        named_resolved, RECORDED_NAMED_SITES_RESOLVED,
        "the number of S-392's seven named sites that resolve moved from          {RECORDED_NAMED_SITES_RESOLVED}",
    );
    const {
        assert!(
            RECORDED_TWO_FRAME_MAIN_ONLY >= TWO_FRAME_FLOOR,
            "the recorded headline must itself be at or above the floor, or the finding is not \
             a pass at all",
        );
        assert!(
            RECORDED_TWO_FRAME_MAIN_ONLY <= PRODUCTION_PUBLISH_SITES,
            "the recorded headline cannot exceed the population it is stated over",
        );
        assert!(
            RECORDED_TWO_FRAME_MAIN_ONLY - RECORDED_BOUGHT_BY_FRAME_TWO
                == RECORDED_RESOLVED_MAIN_ONLY,
            "the two-frame headline must be S-392's one-frame main-only figure plus what the \
             second frame bought, or one of the three is not measuring what it says",
        );
        assert!(
            RECORDED_TWO_FRAME_MAIN_ONLY + RECORDED_TWO_FRAME_REFUSALS
                == PRODUCTION_PUBLISH_SITES,
            "resolved and refused must partition the population, or a candidate went unjudged",
        );
        assert!(
            RECORDED_REFUSALS_FROM_NAME_AMBIGUITY == RECORDED_TWO_FRAME_REFUSALS,
            "the recorded finding states EVERY refusal as attributable to the (name, arity) \
             binding; if that stops being true the 'floor, never a ceiling' reading goes with it",
        );
    }
}

/// S-416's own non-vacuity preconditions, V5 and V6, exactly as
/// `two_frame_forwarding_floor.txt` declared them. Each failure is **VOID**, not
/// a verdict.
///
/// Split into its two halves so each is reachable from a fixture. As one
/// function it was reachable only under `LOGOS_REF_WORKSPACE`, so CI ran none of
/// it and the whole body could be deleted with the suite green — the exact gap
/// this file records as having been found and closed for S-392's V1..V4, and
/// then reintroduced here.
fn assert_two_frame_non_vacuous(f: &Findings, named_sites_found: usize) {
    assert_the_one_frame_base_is_unmoved(f, named_sites_found);
    assert_the_second_frame_did_something(f);
}

/// **V5** — the one-frame readings reproduce S-392 EXACTLY. Deliberately an
/// equality: the second frame is an increment over a fixed base, and an
/// increment over a base that moved is not the quantity CRA-03 asserts.
fn assert_the_one_frame_base_is_unmoved(f: &Findings, named_sites_found: usize) {
    assert_eq!(
        f.production_publish_resolved(),
        RECORDED_RESOLVED,
        "V5: S-392's headline one-frame figure was {RECORDED_RESOLVED} and this run reads \
         {}. The base moved, so the two-frame figure is not the increment CRA-03 asserts. \
         VOID, not falsified.",
        f.production_publish_resolved(),
    );
    assert_eq!(
        f.production_publish_resolved_main_only(),
        RECORDED_RESOLVED_MAIN_ONLY,
        "V5: S-392's one-frame main-only figure was {RECORDED_RESOLVED_MAIN_ONLY} and this \
         run reads {}. VOID, not falsified.",
        f.production_publish_resolved_main_only(),
    );
    assert_eq!(
        f.residue(Arm::BrokerPublish, Tree::Main).get(&Residue::TwoOrMoreHops).copied(),
        Some(RECORDED_TWO_OR_MORE_HOPS),
        "V5: S-392 measured {RECORDED_TWO_OR_MORE_HOPS} sites two or more hops away — the \
         population this story's second frame exists to reach. VOID, not falsified.",
    );
    assert_eq!(
        named_sites_found,
        S392_NAMED_TWO_FRAME_SITES.len(),
        "V5: this run sees {named_sites_found} of S-392's {} NAMED two-frame call sites. A \
         site the record names and the harness cannot find is drift between the two runs, \
         and the reconciliation is the evidence the headline rests on. VOID, not falsified.",
        S392_NAMED_TWO_FRAME_SITES.len(),
    );
}

/// **V6** — the second frame does something. A two-frame reading that equals its
/// one-frame reading has measured a no-op, and a floor cleared by a no-op is
/// cleared by the first frame plus an arithmetic accident.
fn assert_the_second_frame_did_something(f: &Findings) {
    assert!(
        f.bought_by_the_second_frame(Arm::BrokerPublish, Tree::Main, true) > 0,
        "V6: no production candidate resolves at two frames that did not resolve at one, so \
         the second frame changed nothing and this run has not measured one. VOID, not \
         falsified.",
    );
}

/// The non-vacuity preconditions the floor declared in advance: V1..V4.
///
/// A zero from a walk that walked nothing reads exactly like a zero from a walk
/// that walked everything, and only these separate them. Each failure here is
/// **VOID**, not a falsification — the gate must be re-run, not recorded.
fn assert_non_vacuous(f: &Findings, root: &Path) {
    assert!(
        f.members.len() >= 2,
        "V1: the workspace at {} yielded {} member(s) — point LOGOS_REF_WORKSPACE at the \
         reference estate rather than at a single repository. VOID, not falsified.",
        root.display(),
        f.members.len(),
    );
    assert!(
        f.java_files > 0 && f.publish_files > 0,
        "V2: {} Java files walked and {} carrying a publish site. A walk that reached no \
         publish site measures nothing about forwarding. VOID, not falsified.",
        f.java_files,
        f.publish_files,
    );
    let production = f.publish_sites.get(&Tree::Main).copied().unwrap_or_default();
    assert!(
        production >= PRODUCTION_PUBLISH_SITES,
        "V3: found {production} production publish sites, fewer than the {PRODUCTION_PUBLISH_SITES} \
         FR-WS-10's withdrawal note and `brokers.scm` both enumerate. The harness is not \
         looking at the corpus the claim is about. VOID, not falsified.",
    );
    assert_eq!(
        f.operands_without_a_slot, 0,
        "{} operand(s) classified as a bare parameter could not be given a positional slot \
         (varargs, or a single-identifier lambda parameter). Each is a candidate dropped \
         from the denominator the floor is stated over. VOID, not falsified.",
        f.operands_without_a_slot,
    );
    assert_eq!(
        f.sites_without_a_node, 0,
        "V3: {} publish site(s) recognised by `collect_header_publishes` could not be \
         matched to an AST node by this module's own query, so the two lists have drifted \
         and the candidate population is under-read. VOID, not falsified.",
        f.sites_without_a_node,
    );
    assert!(
        f.ledger.calls_rows > 0,
        "V4: the production extract pass recorded zero `Calls` rows across the whole \
         estate, so the ledger side of AC3 measured nothing. VOID, not falsified.",
    );
}

// ── Fixtures ────────────────────────────────────────────────────────────────
//
// These run on every `cargo test`, corpus or no corpus. They matter for the
// same reason the parent module's do: the gate above skips without the estate,
// so without them this file would pin nothing in CI — and the number it
// produces is what decides whether S-393 and S-394 are planned.
//
// Each one builds a throwaway estate on disk and runs `measure_forwarding`
// over it — the SAME entry point the corpus run uses, so a fixture cannot pass
// over logic the measurement does not exercise.
#[cfg(test)]
mod fixtures {
    use super::*;

    /// A `pom.xml` is what makes a directory a build module
    /// (`corpus::MODULE_DESCRIPTORS`); its content is irrelevant.
    const POM: &str = "<project/>\n";

    /// The publish wrapper the whole estate is written around: a topic that is
    /// a bare parameter, in `setHeader`'s second (and last) argument.
    fn producer(method: &str, params: &str) -> String {
        format!(
            "package p;\n\
             import org.springframework.kafka.support.KafkaHeaders;\n\
             public class Producer {{\n\
             \x20   public void {method}({params}) {{\n\
             \x20       Message m = MessageBuilder.withPayload(payload)\n\
             \x20           .setHeader(KafkaHeaders.TOPIC, topic)\n\
             \x20           .build();\n\
             \x20       kafkaTemplate.send(m);\n\
             \x20   }}\n\
             }}\n"
        )
    }

    fn caller(class: &str, body: &str) -> String {
        format!("package p;\npublic class {class} {{\n{body}\n}}\n")
    }

    /// Build a throwaway estate and measure it. The `TempDir` is returned so it
    /// outlives the findings that name its paths.
    fn estate(files: &[(&str, String)]) -> (tempfile::TempDir, Findings) {
        let dir = tempfile::tempdir().expect("tempdir");
        for (path, body) in files {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().expect("a parent")).expect("mkdir");
            std::fs::write(full, body).expect("write");
        }
        let f = measure_forwarding(dir.path());
        (dir, f)
    }

    /// The production publish candidates and their hop outcomes, in file order.
    /// The production broker-publish candidates' outcomes under one reading, in
    /// file order. The predicate lives here once: `production_hops` and
    /// `production_two_frame` spelled it twice, so a change to what counts as a
    /// production candidate would have left the one-frame and two-frame fixture
    /// suites measuring different populations. `Findings::resolved_in` already
    /// parameterizes on the map this way.
    fn production_outcomes(f: &Findings, hops: &BTreeMap<usize, Hop>) -> Vec<Hop> {
        f.candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.arm == Arm::BrokerPublish && c.tree == Tree::Main)
            .map(|(i, _)| hops.get(&i).cloned().expect("every candidate is judged"))
            .collect()
    }

    fn production_hops(f: &Findings) -> Vec<Hop> {
        production_outcomes(f, &f.hops)
    }

    #[test]
    fn the_floor_is_the_one_declared_before_the_run() {
        // Reads the DECLARATION, not the constant. Asserting `ONE_HOP_FLOOR == 7`
        // would compare a value with the literal written a few hundred lines
        // above it — a comparison no mutation can falsify, and which says
        // nothing about what was declared before the run. S-384 shipped exactly
        // that shape first and had to replace it.
        let declared: usize = DECLARED_FLOOR
            .lines()
            .find_map(|l| l.trim().strip_prefix(">= ")?.split_whitespace().next()?.parse().ok())
            .expect("the declaration states its floor as a `>= NN …` line");
        assert_eq!(
            declared, ONE_HOP_FLOOR,
            "ONE_HOP_FLOOR is {ONE_HOP_FLOOR} but the floor declared before the run was \
             {declared}. The declaration is the record: change the constant only by \
             re-deciding CR-121 §8, never to make a run clear it.",
        );
        assert!(
            DECLARED_FLOOR.contains("2026-09-12T12:33:41Z"),
            "the declaration must carry the UTC timestamp that makes it a floor rather than \
             a post-hoc rationalisation",
        );
        assert!(
            DECLARED_FLOOR.contains(&PRODUCTION_PUBLISH_SITES.to_string()),
            "the declaration must name the enumerated production population the floor is \
             stated over",
        );
    }

    #[test]
    fn one_agreeing_call_site_resolves_the_topic() {
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { producer.sendMessage(body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(f.publish_sites.get(&Tree::Main), Some(&1), "one production publish site");
        assert_eq!(f.sites_without_a_node, 0, "the site must join to an AST node");
        assert_eq!(f.operands_without_a_slot, 0, "the operand must get a positional slot");
        // The generality caveat `forwarding_floor.txt` promised IN ADVANCE. A
        // promised report that no test can see disappear is the weakest kind.
        assert_eq!(f.resolved_methods.len(), 1, "one wrapper method behind the resolution");
        assert_eq!(f.resolved_members, BTreeSet::from(["svc".to_string()]));
        assert_eq!(
            production_hops(&f),
            vec![Hop::Resolved {
                value: ArgValue::Literal("orders".into()),
                call_sites: 1,
                caller_files: 1,
            }],
        );
    }

    #[test]
    fn two_agreeing_call_sites_resolve_and_two_disagreeing_do_not() {
        let agree = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Service.java",
                caller(
                    "Service",
                    "  void a() { producer.sendMessage(body, \"orders\"); }\n\
                     \x20 void b() { producer.sendMessage(body, \"orders\"); }",
                ),
            ),
        ]);
        assert_eq!(
            production_hops(&agree.1),
            vec![Hop::Resolved {
                value: ArgValue::Literal("orders".into()),
                call_sites: 2,
                caller_files: 1,
            }],
            "TWO agreeing sites — `call_sites` is the content of this half, so a bare \
             `.resolved()` would pass identically if only one site had been observed",
        );

        let disagree = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Service.java",
                caller(
                    "Service",
                    "  void a() { producer.sendMessage(body, \"orders\"); }\n\
                     \x20 void b() { producer.sendMessage(body, \"shipments\"); }",
                ),
            ),
        ]);
        assert_eq!(
            production_hops(&disagree.1),
            vec![Hop::Refused(Residue::Disagree)],
            "FR-WS-23 AC2: disagreement emits per-site candidates and no edge, never an average",
        );
        assert_eq!(
            disagree.1.two_frame_census(Arm::BrokerPublish, Tree::Main),
            BTreeMap::from([(TwoFrameResidue::DisagreeAtFrameOne, 1)]),
            "the wrapper's OWN call sites disagree — nothing to do with a second frame",
        );
    }

    #[test]
    fn a_call_site_passing_a_same_unit_constant_folds_to_its_literal() {
        // The shape the first version of `arg_value` got wrong: it asked
        // `static_literal`, which sees a `(string_literal)` node and nothing
        // else, so a constant the unit proves outright was recorded as
        // unresolvable. On the estate this is how every IT call site is
        // written, and it under-read the yield the gate is judged on.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Service.java",
                caller(
                    "Service",
                    "  private static final String TOPIC = \"orders\";\n\
                     \x20 void go() { producer.sendMessage(body, TOPIC); }",
                ),
            ),
        ]);
        assert_eq!(
            production_hops(&f),
            vec![Hop::Resolved {
                value: ArgValue::Literal("orders".into()),
                call_sites: 1,
                caller_files: 1,
            }],
        );
    }

    #[test]
    fn a_call_site_passing_its_own_parameter_needs_a_second_hop() {
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go(String t) { producer.sendMessage(body, t); }"),
            ),
        ]);
        assert_eq!(
            production_hops(&f),
            vec![Hop::Refused(Residue::TwoOrMoreHops)],
            "the bound is one hop with no recursion: a caller that forwards its own parameter \
             is the residue FR-WS-23 AC3 refuses and counts",
        );
        assert_eq!(f.beyond_the_bound(Arm::BrokerPublish, Tree::Main), 1);
    }

    #[test]
    fn a_caller_in_another_build_module_does_not_reach_across_the_boundary() {
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("other/pom.xml", POM.into()),
            (
                "other/src/main/java/Service.java",
                caller("Service", "  void go() { producer.sendMessage(body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(
            production_hops(&f),
            vec![Hop::Refused(Residue::OutOfModuleOnly)],
            "FR-WS-23 fixes the hop inside one build module; an out-of-module call site is \
             refused with a reason, not followed",
        );
        assert_eq!(f.beyond_the_bound(Arm::BrokerPublish, Tree::Main), 1);
        assert_eq!(
            f.two_frame_census(Arm::BrokerPublish, Tree::Main),
            BTreeMap::from([(TwoFrameResidue::OutOfModuleAtFrameOne, 1)]),
            "the boundary is frame ONE's, not frame two's",
        );
    }

    #[test]
    fn a_wrapper_nothing_calls_is_refused_rather_than_reported_as_agreeing() {
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
        ]);
        assert_eq!(production_hops(&f), vec![Hop::Refused(Residue::NoCallSites)]);
        assert_eq!(
            f.two_frame_census(Arm::BrokerPublish, Tree::Main),
            BTreeMap::from([(TwoFrameResidue::NoCallSiteAtFrameOne, 1)]),
            "frame ONE found no call site — distinct from the frame-two variant",
        );
    }

    #[test]
    fn a_comment_in_the_argument_list_does_not_shift_the_slot() {
        // The near miss: tree-sitter counts a `comment` as a named sibling, so
        // an unfiltered `named_children` would read slot 1 as the comment and
        // bind the WRONG operand — a silently wrong topic, which is worse than
        // no topic. `brokers.scm` documents the same fact costing its anchored
        // patterns a commented argument list.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Service.java",
                caller(
                    "Service",
                    "  void go() { producer.sendMessage(body, /* the topic */ \"orders\"); }",
                ),
            ),
        ]);
        assert_eq!(
            production_hops(&f),
            vec![Hop::Resolved {
                value: ArgValue::Literal("orders".into()),
                call_sites: 1,
                caller_files: 1,
            }],
        );

        // The same near miss in the PARAMETER list, which `parameter_slot`
        // reads through the same helper, and in the line-comment spelling —
        // tree-sitter-java has two comment kinds and an exact-match filter
        // catches neither.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            (
                "svc/src/main/java/Producer.java",
                producer("sendMessage", "Object payload, // the topic\n        String topic"),
            ),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { producer.sendMessage(body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(
            production_hops(&f),
            vec![Hop::Resolved {
                value: ArgValue::Literal("orders".into()),
                call_sites: 1,
                caller_files: 1,
            }],
            "a commented parameter list must not shift the slot the operand occupies",
        );
    }

    #[test]
    fn a_test_tree_publish_site_is_counted_separately_from_production() {
        // Sprint 65's finding, as a fixture: a test-tree yield must never reach
        // the production row. CR-117 looked validated at 38 of 54 precisely
        // because the two were read together.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("svc/src/test/java/ProducerIT.java", producer("sendIt", "Object payload, String topic")),
            (
                "svc/src/test/java/ServiceIT.java",
                caller("ServiceIT", "  void go() { producer.sendIt(body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(f.publish_sites.get(&Tree::Main), Some(&1));
        assert_eq!(f.publish_sites.get(&Tree::Test), Some(&1));
        assert_eq!(f.production_publish_resolved(), 0, "the test tree's yield is not production's");
        assert_eq!(f.resolved_in(Arm::BrokerPublish, Tree::Test, &f.hops), 1);
        // The census must PARTITION by arm and tree, not pool. Summing the two
        // trees is exactly how CR-117 read as validated at 38 of 54 while being
        // false on the population its criterion was written over, so the split
        // is pinned here and not only reported.
        assert_eq!(
            f.residue(Arm::BrokerPublish, Tree::Main),
            BTreeMap::from([(Residue::NoCallSites, 1)]),
            "the production wrapper is uncalled; the test tree's caller is a different method",
        );
        assert_eq!(
            f.residue(Arm::BrokerPublish, Tree::Test),
            BTreeMap::new(),
            "the test tree's own candidate resolved, so it contributes no residue",
        );
    }

    #[test]
    fn the_second_hop_outranks_a_disagreement_it_would_otherwise_explain() {
        // Precedence, asserted rather than left incidental: sites that disagree,
        // one of which forwards a parameter, are not yet KNOWN to disagree — the
        // unresolved one might resolve to either value, so the honest reason is
        // the hop, not the disagreement.
        //
        // THREE call sites, and all three are load-bearing. An earlier version
        // used two — one literal and one forwarded parameter — which names this
        // precedence and does not exercise it: with only one resolved identity
        // there is no disagreement for the hop to outrank, and reordering the
        // two checks left the fixture green. The mutation sweep caught it.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Service.java",
                caller(
                    "Service",
                    "  void a() { producer.sendMessage(body, \"orders\"); }\n\
                     \x20 void b() { producer.sendMessage(body, \"shipments\"); }\n\
                     \x20 void c(String t) { producer.sendMessage(body, t); }",
                ),
            ),
        ]);
        assert_eq!(
            production_hops(&f),
            vec![Hop::Refused(Residue::TwoOrMoreHops)],
            "two resolved identities DO disagree here, and the hop must still outrank it",
        );
    }

    #[test]
    fn only_the_main_tree_reading_can_differ_from_the_headline() {
        // The sensitivity the report prints: a production wrapper whose only
        // disagreeing call site is in the test tree resolves under the
        // main-only reading and refuses under the headline one. Both are
        // reported; neither is silently chosen.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { producer.sendMessage(body, \"orders\"); }"),
            ),
            (
                "svc/src/test/java/ServiceIT.java",
                caller("ServiceIT", "  void go() { producer.sendMessage(body, \"orders-it\"); }"),
            ),
        ]);
        assert_eq!(f.production_publish_resolved(), 0, "headline: every in-module site must agree");
        assert_eq!(f.production_publish_resolved_main_only(), 1, "main-only: the test site is out");
        // The function the gate reads RECORDED_REFUSED_BY_TEST_ONLY off. The two
        // components above were pinned; the thing that combines them was not.
        assert_eq!(f.refused_only_by_test_call_sites(Arm::BrokerPublish, Tree::Main), 1);
        assert_eq!(f.agreeing_call_sites(Arm::BrokerPublish, Tree::Main, &f.hops), 0);
        assert_eq!(f.agreeing_call_sites(Arm::BrokerPublish, Tree::Main, &f.hops_main_only), 1);
    }

    #[test]
    fn a_no_key_client_call_operand_is_a_candidate_and_resolves_one_hop() {
        // The second arm of AC1. The import is what admits the file through the
        // FR-FW-04 ledger gate; without it the site is not in the population at
        // all, which is the same gate the 111-site production figure is taken
        // over.
        let client = "package p;\n\
             import org.springframework.web.reactive.function.client.WebClient;\n\
             public class Client {\n\
             \x20   public String fetch(String path) {\n\
             \x20       return webClient.get().uri(path).retrieve();\n\
             \x20   }\n\
             }\n";
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Client.java", client.into()),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { client.fetch(\"/api/v1/things\"); }"),
            ),
        ]);
        assert_eq!(
            f.client_sites.get(&Tree::Main),
            Some(&1),
            "the client arm must reach the gate-admitted no-key site",
        );
        assert_eq!(f.resolved_in(Arm::ClientCall, Tree::Main, &f.hops), 1);
    }

    #[test]
    fn a_publish_site_whose_topic_is_not_a_parameter_is_not_a_forwarding_candidate() {
        // The population boundary. `Topics.ORDERS` is a recognised publish site
        // and an honest `topic-not-literal` refusal, but it is not what the hop
        // is for: nothing is forwarded, so no call site can supply it. Widening
        // candidate detection from "MethodParameter" to "anything unresolved"
        // would silently enlarge the 13-site denominator the floor is stated
        // over, which is the one number this gate turns on.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            (
                "svc/src/main/java/Producer.java",
                "package p;\n\
                 import org.springframework.kafka.support.KafkaHeaders;\n\
                 public class Producer {\n\
                 \x20   public void sendMessage(Object payload) {\n\
                 \x20       MessageBuilder.withPayload(payload)\n\
                 \x20           .setHeader(KafkaHeaders.TOPIC, Topics.ORDERS)\n\
                 \x20           .build();\n\
                 \x20   }\n\
                 }\n"
                    .to_string(),
            ),
        ]);
        assert_eq!(f.publish_sites.get(&Tree::Main), Some(&1), "it IS a publish site");
        assert_eq!(
            f.candidates_in(Arm::BrokerPublish, Tree::Main),
            0,
            "…and it is NOT a forwarding candidate",
        );
    }

    #[test]
    fn a_constructor_parameter_is_refused_before_any_call_site_is_looked_at() {
        // FR-WS-23 says "method". A constructor's argument slot is a different
        // call shape and this measurement does not claim it, so the candidate
        // is blocked at enumeration with its own reason rather than counted as
        // a method with no call site. The mutation sweep found this variant
        // had no fixture at all.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            (
                "svc/src/main/java/Producer.java",
                "package p;\n\
                 import org.springframework.kafka.support.KafkaHeaders;\n\
                 public class Producer {\n\
                 \x20   public Producer(String topic) {\n\
                 \x20       MessageBuilder.withPayload(p).setHeader(KafkaHeaders.TOPIC, topic).build();\n\
                 \x20   }\n\
                 }\n"
                    .to_string(),
            ),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { new Producer(\"orders\"); }"),
            ),
        ]);
        assert_eq!(f.publish_sites.get(&Tree::Main), Some(&1));
        assert_eq!(production_hops(&f), vec![Hop::Refused(Residue::NotAMethodParameter)]);
        // …and the two-frame census carries the frame-ONE reason unchanged. A
        // second frame cannot help a candidate whose operand was never a method
        // parameter, and the census must say which frame refused it.
        assert_eq!(
            f.two_frame_census(Arm::BrokerPublish, Tree::Main),
            BTreeMap::from([(TwoFrameResidue::NotAMethodParameter, 1)]),
        );
    }

    #[test]
    fn a_client_site_the_agreement_rule_already_admits_is_not_a_candidate() {
        // The client arm's population is the NO-KEY residue, not every site
        // with a parameter operand. Here the leading operand resolves to a
        // committed key and the trailing parameter is the `{}` a route template
        // already expresses, so CR-115's rule admits the site today — it is
        // waiting on nothing and must not inflate this measurement's
        // denominator.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/resources/application.yml", "api:\n  base-path: /v1\n".into()),
            (
                "svc/src/main/java/ApiProps.java",
                "package p;\n\
                 @ConfigurationProperties(prefix = \"api\")\n\
                 public class ApiProps { private String basePath; }\n"
                    .to_string(),
            ),
            (
                "svc/src/main/java/Client.java",
                "package p;\n\
                 import org.springframework.web.reactive.function.client.WebClient;\n\
                 public class Client {\n\
                 \x20   private final ApiProps apiProps;\n\
                 \x20   public String fetch(String path) {\n\
                 \x20       return webClient.get().uri(apiProps.getBasePath() + path).retrieve();\n\
                 \x20   }\n\
                 }\n"
                    .to_string(),
            ),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { client.fetch(\"/things\"); }"),
            ),
        ]);
        assert_eq!(
            f.client_sites.get(&Tree::Main),
            None,
            "an already-admitted site is not part of the no-key residue this arm measures",
        );
        assert_eq!(f.candidates_in(Arm::ClientCall, Tree::Main), 0);
    }

    #[test]
    fn a_call_site_passing_a_config_accessor_resolves_to_its_key() {
        // The `ArgValue::Key` branch — the one the six main-only resolutions on
        // the estate are made of ("resolve to a @ConfigurationProperties key"),
        // and which had no fixture: replacing it with `NeedsAnotherHop` left the
        // whole suite green.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/resources/application.yml", "spring:\n  kafka:\n    topics:\n      archive-commands: archiveCommands\n".into()),
            (
                "svc/src/main/java/KafkaTopics.java",
                "package p;\n\
                 @ConfigurationProperties(prefix = \"spring.kafka.topics\")\n\
                 public class KafkaTopics { private String archiveCommands; }\n"
                    .to_string(),
            ),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Service.java",
                caller(
                    "Service",
                    "  private final KafkaTopics kafkaTopics;\n\
                     \x20 void go() { producer.sendMessage(body, kafkaTopics.getArchiveCommands()); }",
                ),
            ),
        ]);
        assert_eq!(
            production_hops(&f),
            vec![Hop::Resolved {
                value: ArgValue::Key("spring.kafka.topics.archiveCommands".into()),
                call_sites: 1,
                caller_files: 1,
            }],
        );
    }

    #[test]
    fn an_in_module_call_site_whose_argument_resolves_to_nothing_refuses() {
        // `Residue::UnresolvableOperand` carries SIX of the thirteen production
        // sites in the recorded finding — the Mockito-stub mechanism — and had
        // no fixture at all. `any()` here is the same shape the estate writes:
        // a call whose receiver the unit does not bind.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { producer.sendMessage(body, any()); }"),
            ),
        ]);
        assert_eq!(production_hops(&f), vec![Hop::Refused(Residue::UnresolvableOperand)]);
        assert_eq!(
            f.beyond_the_bound(Arm::BrokerPublish, Tree::Main),
            0,
            "an unresolvable operand is NOT beyond the one-module bound — it is inside it \
             and simply does not resolve; conflating the two would inflate AC2's headline",
        );
    }

    #[test]
    fn a_method_reference_is_a_call_site_that_proves_nothing() {
        // The only direction in which this measurement could OVER-read: a
        // `producer::sendMessage` reaches the wrapper and supplies no argument,
        // so under FR-WS-23 AC1 the module's call sites do not all agree.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Service.java",
                caller(
                    "Service",
                    "  void a() { producer.sendMessage(body, \"orders\"); }\n\
                     \x20 void b() { list.forEach(producer::sendMessage); }",
                ),
            ),
        ]);
        assert_eq!(
            production_hops(&f),
            vec![Hop::Refused(Residue::TwoOrMoreHops)],
            "without the reference arm this reported RESOLVED from the one literal site",
        );
    }

    #[test]
    fn two_declarations_of_one_name_in_a_module_are_counted_as_ambiguous() {
        // AC3's ambiguity census. The `Calls` ledger's target is a bare name, so
        // two declarations sharing one are two things it cannot tell apart; the
        // recorded finding quotes this figure as 7.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Unrelated.java",
                caller("Unrelated", "  void sendMessage(Object payload, String topic) { }"),
            ),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { producer.sendMessage(body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(
            f.ledger.ambiguous_by_name, 1,
            "two `sendMessage` declarations in one build module",
        );
    }

    #[test]
    fn the_per_candidate_cost_divides_by_its_stated_denominator() {
        // AC5's arithmetic, as a pure unit test: the durations are
        // non-deterministic through `measure_forwarding`, so this is the one
        // place testing a helper in isolation is the right shape.
        let cost = Cost { hop_targeted: Duration::from_millis(1), ..Cost::default() };
        assert_eq!(cost.per_candidate_us(4), 250);
        assert_eq!(cost.per_candidate_us(0), 0, "no candidates is not a division by zero");
    }

    // ── The non-vacuity guards, which used to run only under the corpus ──────
    //
    // These are what separate VOID from FALSIFIED, and deleting all four left
    // the suite green. Each builds a deliberately vacuous estate and asserts the
    // guard panics with its own label.

    #[test]
    #[should_panic(expected = "V1")]
    fn a_single_member_estate_is_void_not_falsified() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pom.xml"), POM).expect("write");
        let f = measure_forwarding(dir.path());
        assert_non_vacuous(&f, dir.path());
    }

    #[test]
    #[should_panic(expected = "V2")]
    fn an_estate_with_no_publish_site_is_void_not_falsified() {
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Plain.java", caller("Plain", "  void go() { }")),
            ("other/pom.xml", POM.into()),
            // A second member the registry actually claims, so V1 is satisfied
            // and V2 is the guard under test. Without it V1 fires first and the
            // fixture proves a different guard than its name says.
            ("other/src/main/java/Other.java", caller("Other", "  void go() { }")),
        ]);
        let dir = tempfile::tempdir().expect("tempdir");
        assert_non_vacuous(&f, dir.path());
    }

    #[test]
    #[should_panic(expected = "V3")]
    fn fewer_than_the_enumerated_population_is_void_not_falsified() {
        // One publish site is not thirteen. A harness looking at a corpus that
        // is not the estate must not be able to report "0 of 1 => FALSIFIED".
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("other/pom.xml", POM.into()),
            ("other/src/main/java/Other.java", caller("Other", "  void go() { }")),
        ]);
        let dir = tempfile::tempdir().expect("tempdir");
        assert_non_vacuous(&f, dir.path());
    }

    #[test]
    fn an_explicit_receiver_parameter_does_not_shift_the_slot() {
        // `formal_parameters` admits a leading `Foo this`, which occupies a
        // parameter slot but is supplied by no argument at any call site.
        // Counting it shifted `topic` from slot 1 to slot 2 and the wrapper then
        // matched no call site at all — a silent NoCallSites on a wrapper that
        // is called.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            (
                "svc/src/main/java/Producer.java",
                producer("sendMessage", "Producer this, Object payload, String topic"),
            ),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { producer.sendMessage(body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(
            production_hops(&f),
            vec![Hop::Resolved {
                value: ArgValue::Literal("orders".into()),
                call_sites: 1,
                caller_files: 1,
            }],
        );
    }

    #[test]
    fn a_lambda_parameter_topic_is_excluded_before_the_slot_arithmetic() {
        // Where the lambda shape is actually filtered, asserted rather than
        // assumed. A reasonable reading is that it reaches `push_candidate` and
        // is dropped there for want of a positional slot — a lambda's
        // "parameter list" is one bare identifier with no children. It does not:
        // the parent's `resolve_key` declines to call it a bare parameter one
        // step earlier, so it is excluded at the candidate gate and
        // `operands_without_a_slot` stays zero.
        //
        // That makes the counter DEFENSIVE, and this fixture is what says so.
        // It is reachable only if the parent's classifier and this module's slot
        // arithmetic ever disagree about what a parameter is — which is exactly
        // the drift worth failing the run over, and why the gate asserts it zero
        // rather than ignoring it.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            (
                "svc/src/main/java/Producer.java",
                "package p;\n\
                 import org.springframework.kafka.support.KafkaHeaders;\n\
                 public class Producer {\n\
                 \x20   public void publishAll(java.util.List<String> topics) {\n\
                 \x20       topics.forEach(topic ->\n\
                 \x20           MessageBuilder.withPayload(p).setHeader(KafkaHeaders.TOPIC, topic).build());\n\
                 \x20   }\n\
                 }\n"
                    .to_string(),
            ),
        ]);
        assert_eq!(f.publish_sites.get(&Tree::Main), Some(&1), "it IS a recognised publish site");
        assert_eq!(
            f.candidates_in(Arm::BrokerPublish, Tree::Main),
            0,
            "…and it is not a forwarding candidate",
        );
        assert_eq!(
            f.operands_without_a_slot, 0,
            "excluded by the classifier, not dropped by the slot arithmetic — so the two \
             agree, which is the only thing this counter exists to check",
        );
    }

    // ── S-416: the second frame ──────────────────────────────────────────────
    //
    // The corpus gate skips without the estate, so without these the two-frame
    // reading would pin nothing in CI — and its number is what decides whether
    // S-417 is planned at all. Each builds a throwaway estate and runs the SAME
    // `measure_forwarding` entry point the corpus run uses.

    /// A thin wrapper that forwards its own `topic` parameter to the real
    /// producer: the shape S-392 named seven times on the estate, and the only
    /// shape the second frame exists for.
    fn forwarder(class: &str, method: &str) -> String {
        format!(
            "package p;\n\
             public class {class} {{\n\
             \x20   public void {method}(Object payload, String topic) {{\n\
             \x20       producer.sendMessage(payload, topic);\n\
             \x20   }}\n\
             }}\n"
        )
    }

    /// The production candidates' TWO-FRAME outcomes under the decisive
    /// reading, in file order — the sibling of [`production_hops`].
    fn production_two_frame(f: &Findings) -> Vec<Hop> {
        production_outcomes(f, &f.hops_two_frame_main_only)
    }

    #[test]
    fn the_two_frame_floor_is_the_one_declared_before_the_run() {
        // Reads the DECLARATION, not the constant — the shape S-384 had to
        // replace, and which `the_floor_is_the_one_declared_before_the_run`
        // already applies to S-392's floor.
        let declared: usize = TWO_FRAME_DECLARED_FLOOR
            .lines()
            .find_map(|l| l.trim().strip_prefix(">= ")?.split_whitespace().next()?.parse().ok())
            .expect("the declaration states its floor as a `>= NN …` line");
        assert_eq!(
            declared, TWO_FRAME_FLOOR,
            "TWO_FRAME_FLOOR is {TWO_FRAME_FLOOR} but the floor declared before the run was \
             {declared}. The declaration is the record: change the constant only by \
             re-deciding CR-131 §3.2, never to make a run clear it.",
        );
        assert!(
            TWO_FRAME_DECLARED_FLOOR.contains("2026-09-15T07:06:00Z"),
            "the declaration must carry the UTC timestamp that makes it a floor rather than \
             a post-hoc rationalisation",
        );
        assert!(
            TWO_FRAME_DECLARED_FLOOR.contains(&PRODUCTION_PUBLISH_SITES.to_string()),
            "the declaration must name the enumerated production population the floor is \
             stated over",
        );
        // The floor file names all seven sites the reconciliation AC is stated
        // over, so the constant and the declaration cannot drift apart.
        for (_, site) in S392_NAMED_TWO_FRAME_SITES {
            assert!(
                TWO_FRAME_DECLARED_FLOOR.contains(site),
                "the declaration must name `{site}`, one of S-392's seven two-frame sites \
                 the reconciliation is stated over",
            );
        }
    }

    #[test]
    fn the_recorded_finding_states_the_figures_the_constants_pin() {
        // The finding file is prose and the constants are code, and prose that
        // was true when it was written is the single most common review finding
        // in this project. This makes the tie mechanical: the verdict, the
        // headline, the floor and the date must all appear in the text the gate
        // prints as its evidence.
        for needle in [
            "CR-131 CRA-03 HOLDS",
            "2026-09-15T07:06:00Z",
            &format!("{RECORDED_TWO_FRAME_MAIN_ONLY} of the 13 production publish sites"),
            &format!("floor of {TWO_FRAME_FLOOR}"),
        ] {
            assert!(
                TWO_FRAME_RECORDED_FINDING.contains(needle),
                "the recorded finding must state `{needle}` — it is the evidence the gate \
                 prints, and a figure it does not carry is a figure nobody can check",
            );
        }
        // The grid, whose four cells are the criterion CR-131 C1 states. Every
        // one appears in the finding's own table.
        for row in [
            format!("    one frame                           {RECORDED_RESOLVED}"),
            format!("    two frames                          {RECORDED_TWO_FRAME_ALL_SITES}"),
        ] {
            assert!(
                TWO_FRAME_RECORDED_FINDING.contains(&row),
                "the recorded finding's grid must carry the row `{row}`",
            );
        }
    }

    #[test]
    fn two_frames_resolve_what_one_frame_refuses() {
        // The whole point of the story, as a fixture: the wrapper's only call
        // site forwards a parameter (S-392's `Residue::TwoOrMoreHops`), and the
        // forwarder's own caller supplies the literal.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("svc/src/main/java/Thin.java", forwarder("Thin", "send")),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { thin.send(body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(
            production_hops(&f),
            vec![Hop::Refused(Residue::TwoOrMoreHops)],
            "one frame must still refuse — S-392's reading is the base this is an increment \
             over, and a change that moved it would make the increment meaningless",
        );
        assert_eq!(
            production_two_frame(&f),
            vec![Hop::Resolved {
                value: ArgValue::Literal("orders".into()),
                call_sites: 1,
                caller_files: 1,
            }],
        );
        assert_eq!(f.bought_by_the_second_frame(Arm::BrokerPublish, Tree::Main, true), 1);
        assert_eq!(f.two_frame_census(Arm::BrokerPublish, Tree::Main), BTreeMap::new());
    }

    #[test]
    fn three_frames_refuse_and_are_named_as_three() {
        // The bound. A third forwarding layer is where CR-131 C1 stops, and the
        // refusal must say so rather than reporting the generic hop residue —
        // `Residue::TwoOrMoreHops` reads "two or more" and at this depth that
        // sentence is wrong.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("svc/src/main/java/Thin.java", forwarder("Thin", "send")),
            (
                "svc/src/main/java/Thinner.java",
                "package p;\n\
                 public class Thinner {\n\
                 \x20   public void relay(Object payload, String topic) { thin.send(payload, topic); }\n\
                 }\n"
                    .to_string(),
            ),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { thinner.relay(body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(production_two_frame(&f), vec![Hop::Refused(Residue::TwoOrMoreHops)]);
        assert_eq!(
            f.two_frame_census(Arm::BrokerPublish, Tree::Main),
            BTreeMap::from([(TwoFrameResidue::ThreeOrMoreFrames, 1)]),
            "the census must say THREE frames, not `two or more`",
        );
        assert_eq!(f.bought_by_the_second_frame(Arm::BrokerPublish, Tree::Main, true), 0);
    }

    #[test]
    fn an_out_of_module_caller_at_the_second_frame_refuses() {
        // The module bound, one frame deeper. The forwarder is in the module and
        // its own caller is not, so the second frame must refuse at the boundary
        // rather than reach across it.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("svc/src/main/java/Thin.java", forwarder("Thin", "send")),
            ("other/pom.xml", POM.into()),
            (
                "other/src/main/java/Service.java",
                caller("Service", "  void go() { thin.send(body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(production_two_frame(&f), vec![Hop::Refused(Residue::UnresolvableOperand)]);
        assert_eq!(
            f.two_frame_census(Arm::BrokerPublish, Tree::Main),
            BTreeMap::from([(TwoFrameResidue::OutOfModuleCaller, 1)]),
            "an out-of-module CALLER at frame two is its own reason, never pooled with an \
             operand that resolves to nothing",
        );
    }

    #[test]
    fn disagreeing_callers_at_the_second_frame_refuse() {
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("svc/src/main/java/Thin.java", forwarder("Thin", "send")),
            (
                "svc/src/main/java/Service.java",
                caller(
                    "Service",
                    "  void a() { thin.send(body, \"orders\"); }\n\
                     \x20 void b() { thin.send(body, \"shipments\"); }",
                ),
            ),
        ]);
        assert_eq!(production_two_frame(&f), vec![Hop::Refused(Residue::UnresolvableOperand)]);
        assert_eq!(
            f.two_frame_census(Arm::BrokerPublish, Tree::Main),
            BTreeMap::from([(TwoFrameResidue::DisagreeingCallers, 1)]),
            "CR-131 C1 refuses on disagreement at either frame; it never averages",
        );
    }

    #[test]
    fn a_forwarding_method_nothing_calls_refuses_at_the_second_frame() {
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("svc/src/main/java/Thin.java", forwarder("Thin", "send")),
        ]);
        assert_eq!(production_two_frame(&f), vec![Hop::Refused(Residue::UnresolvableOperand)]);
        assert_eq!(
            f.two_frame_census(Arm::BrokerPublish, Tree::Main),
            BTreeMap::from([(TwoFrameResidue::NoCallSiteAtFrameTwo, 1)]),
        );
    }

    #[test]
    fn a_mockito_stub_blocks_the_every_site_reading_and_is_named_by_the_main_only_one() {
        // The estate's dominant shape, and the one CR-131 C1's second relaxation
        // is entirely about: the build module contains the test tree, and the
        // test tree stubs the wrapper. Under the every-site reading the stub
        // refuses the whole method; under the main-tree rule it is removed — and
        // it must be NAMED when it is, or the relaxation cannot be audited.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("svc/src/main/java/Thin.java", forwarder("Thin", "send")),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { thin.send(body, \"orders\"); }"),
            ),
            (
                "svc/src/test/java/ServiceTest.java",
                caller(
                    "ServiceTest",
                    "  void stub() { doNothing().when(producer).sendMessage(any(), any()); }",
                ),
            ),
        ]);
        assert_eq!(
            f.production_publish_resolved_two_frame(),
            0,
            "every-site reading: the Mockito stub is a call site whose operand resolves to \
             nothing, so it refuses the method",
        );
        assert_eq!(
            f.production_publish_resolved_two_frame_main_only(),
            1,
            "main-only reading: a stub is not a publish",
        );
        let named: Vec<&String> = f.excluded_by_main_tree.values().flatten().collect();
        assert_eq!(
            named.len(),
            1,
            "the site the main-tree rule removed must be NAMED, not silently dropped — it is \
             the entire justification for the relaxation. Named: {named:?}",
        );
        assert!(
            named[0].contains("ServiceTest.java:3") && named[0].contains("[test]"),
            "the named exclusion must carry the file, the line and the tree: {named:?}",
        );
        assert!(
            named[0].contains("unresolvable"),
            "…and what its argument resolved to, so a reader can judge whether it is a stub: \
             {named:?}",
        );
    }

    #[test]
    fn the_main_tree_rule_applies_at_the_second_frame_too() {
        // The sibling of the fixture above, one frame deeper, and the gap the
        // mutation sweep found: there the stub called the WRAPPER, so only
        // frame one's tree filter was exercised and frame two's could be
        // deleted with the suite staying green. Here the stub calls the
        // FORWARDING method, so the relaxation has to be applied again at the
        // second frame or the whole candidate refuses.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("svc/src/main/java/Thin.java", forwarder("Thin", "send")),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { thin.send(body, \"orders\"); }"),
            ),
            (
                "svc/src/test/java/ThinTest.java",
                caller("ThinTest", "  void stub() { doNothing().when(thin).send(any(), any()); }"),
            ),
        ]);
        assert_eq!(
            f.production_publish_resolved_two_frame(),
            0,
            "every-site reading: the stub is a call site of the FORWARDING method whose \
             operand resolves to nothing, so the second frame refuses",
        );
        assert_eq!(
            f.production_publish_resolved_two_frame_main_only(),
            1,
            "main-only reading: the second frame must apply the main-tree rule as well as \
             the first",
        );
    }

    #[test]
    fn the_second_frame_never_resolves_fewer_than_the_first() {
        // Monotonicity, on the shape where it could plausibly break: a wrapper
        // that already resolves at one frame must not be DE-resolved by the
        // rewrite, which touches only the values that said `NeedsAnotherHop`.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { producer.sendMessage(body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(f.production_publish_resolved_main_only(), 1);
        assert_eq!(f.production_publish_resolved_two_frame_main_only(), 1);
        assert_eq!(f.production_publish_resolved(), 1);
        assert_eq!(f.production_publish_resolved_two_frame(), 1);
        assert_eq!(
            f.bought_by_the_second_frame(Arm::BrokerPublish, Tree::Main, true),
            0,
            "nothing was bought: it already resolved at one frame",
        );
    }

    #[test]
    fn three_frames_outrank_a_second_frame_disagreement_they_would_explain() {
        // The precedence, one frame deeper than
        // `the_second_hop_outranks_a_disagreement_it_would_otherwise_explain`
        // asserts it. Three callers of the forwarder: two disagreeing literals
        // and one that forwards again. All three are load-bearing — with only
        // one resolved identity there is no disagreement for the frame count to
        // outrank, and the sibling fixture records that exact mistake.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("svc/src/main/java/Thin.java", forwarder("Thin", "send")),
            (
                "svc/src/main/java/Service.java",
                caller(
                    "Service",
                    "  void a() { thin.send(body, \"orders\"); }\n\
                     \x20 void b() { thin.send(body, \"shipments\"); }\n\
                     \x20 void c(String t) { thin.send(body, t); }",
                ),
            ),
        ]);
        assert_eq!(
            f.two_frame_census(Arm::BrokerPublish, Tree::Main),
            BTreeMap::from([(TwoFrameResidue::ThreeOrMoreFrames, 1)]),
            "two callers DO disagree here, and the frame count must still outrank it",
        );
    }

    #[test]
    fn a_constructor_parameter_at_the_second_frame_is_not_followed() {
        // `forward_from` refuses a non-method declaring scope, the same boundary
        // `push_candidate` applies to a publish site's own operand. A
        // constructor's argument slot is a different call shape and this
        // measurement does not claim it.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Thin.java",
                "package p;\n\
                 public class Thin {\n\
                 \x20   public Thin(Object payload, String topic) { producer.sendMessage(payload, topic); }\n\
                 }\n"
                    .to_string(),
            ),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { new Thin(body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(production_two_frame(&f), vec![Hop::Refused(Residue::UnresolvableOperand)]);
        assert_eq!(
            f.two_frame_census(Arm::BrokerPublish, Tree::Main),
            BTreeMap::from([(TwoFrameResidue::UnresolvableOperand, 1)]),
            "a constructor parameter is not followable, and the census says so rather than \
             claiming the second frame found no call site",
        );
    }

    #[test]
    fn the_named_site_join_reads_the_basename_and_line() {
        // The reconciliation's join key, on an estate small enough to read by
        // hand. The recorded finding writes `DelayedMessageKafkaProducer.java:23`
        // and nothing else, so the join must match a BASENAME — and must not
        // match a different line in the same file, which is the near miss a
        // substring test would admit.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("svc/src/main/java/Thin.java", forwarder("Thin", "send")),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { thin.send(body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(
            f.candidate_forwarding_at("svc", "Thin.java:4"),
            Some(0),
            "the forwarding call site is `svc/src/main/java/Thin.java:4`, joined on its \
             basename: {:?}",
            f.forwarding_sites,
        );
        assert_eq!(
            f.candidate_forwarding_at("svc", "Thin.java:5"),
            None,
            "a different LINE in the same file must not match — the line is half the key",
        );
        assert_eq!(
            f.candidate_forwarding_at("svc", "hin.java:4"),
            None,
            "a suffix of the basename must not match either: `hin.java` is one character \
             from `Thin.java` and is a different file",
        );
    }

    #[test]
    fn the_named_site_join_separates_two_modules_writing_the_same_basename() {
        // The estate really does this: two of S-392's seven named sites are the
        // byte-identical string `DelayedMessageKafkaProducer.java:23`, one in
        // `deprecated-mailbox-core/manager` and one in `mailbox-manager`. With
        // the basename and line as the whole key, BOTH rows resolved to the
        // first match — the reconciliation printed one member's candidate under
        // the other's heading, and V5 counted seven sites found while only six
        // distinct candidates backed them.
        //
        // Two modules, the same file name, the same line, different topics.
        let (_d, f) = estate(&[
            ("alpha/pom.xml", POM.into()),
            ("alpha/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("alpha/src/main/java/Thin.java", forwarder("Thin", "send")),
            (
                "alpha/src/main/java/Service.java",
                caller("Service", "  void go() { thin.send(body, \"alpha-orders\"); }"),
            ),
            ("beta/pom.xml", POM.into()),
            ("beta/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("beta/src/main/java/Thin.java", forwarder("Thin", "send")),
            (
                "beta/src/main/java/Service.java",
                caller("Service", "  void go() { thin.send(body, \"beta-orders\"); }"),
            ),
        ]);
        let alpha = f
            .candidate_forwarding_at("alpha", "Thin.java:4")
            .expect("alpha's forwarding site is seen");
        let beta = f
            .candidate_forwarding_at("beta", "Thin.java:4")
            .expect("beta's forwarding site is seen");
        assert_ne!(
            alpha, beta,
            "the same basename and line in two modules must reconcile to DIFFERENT candidates; \
             joining on the basename alone returns the first match for both, which is how a \
             vanished site hides behind its twin and V5 passes anyway",
        );
        assert_eq!(f.candidates[alpha].module, "alpha");
        assert_eq!(f.candidates[beta].module, "beta");
        // …and a module that writes no such site must not borrow another's.
        assert_eq!(f.candidate_forwarding_at("gamma", "Thin.java:4"), None);
    }

    /// The estate's actual shape, reduced: an abstract base whose `sendMessage`
    /// carries the publish site, and thin subclass overrides that forward their
    /// own parameter through `super`. `extends Base<K, V>` is written generic
    /// because the estate writes it generic and a `generic_type` is the wrapper
    /// `simple_type_name` has to see through.
    fn base_producer() -> String {
        "package p;\n\
         import org.springframework.kafka.support.KafkaHeaders;\n\
         public abstract class Base<K, V> {\n\
         \x20   public void sendMessage(K kafkaKey, V payload, String topic) {\n\
         \x20       Message m = MessageBuilder.withPayload(payload)\n\
         \x20           .setHeader(KafkaHeaders.TOPIC, topic)\n\
         \x20           .build();\n\
         \x20       kafkaTemplate.send(m);\n\
         \x20   }\n\
         }\n"
            .to_string()
    }

    fn override_producer(class: &str) -> String {
        format!(
            "package p;\n\
             public class {class} extends Base<Key, Payload> {{\n\
             \x20   public void sendMessage(Key kafkaKey, Payload payload, String topic) {{\n\
             \x20       super.sendMessage(kafkaKey, payload, topic);\n\
             \x20   }}\n\
             }}\n"
        )
    }

    #[test]
    fn a_super_call_is_not_a_caller_of_the_override_that_writes_it() {
        // The defect the first corpus run had, as a fixture. `Callee` is
        // `(name, arity)`, so frame two's lookup of `Alpha.sendMessage/3` finds
        // the `super.sendMessage(...)` line INSIDE `Alpha.sendMessage` — whose
        // argument is that method's own parameter. Reading it reports THREE
        // frames for a site that resolves in two.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Base.java", base_producer()),
            ("svc/src/main/java/Alpha.java", override_producer("Alpha")),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { alpha.sendMessage(k, body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(
            production_hops(&f),
            vec![Hop::Refused(Residue::TwoOrMoreHops)],
            "one frame refuses: `Alpha.java`'s `super` forward is one of the two in-module \
             call sites and it passes a parameter",
        );
        assert_eq!(
            production_two_frame(&f),
            vec![Hop::Resolved {
                value: ArgValue::Literal("orders".into()),
                call_sites: 2,
                caller_files: 2,
            }],
            "two frames resolve — a `super.m()` inside `m` is a call of the SUPERCLASS's \
             `m`, never of itself, so it is not a caller at frame two. TWO sites, because \
             frame ONE binds on (name, arity) and `Service`'s call of the OVERRIDE matches \
             the base's signature too; both reach the same literal, so they agree",
        );
    }

    #[test]
    fn a_super_call_in_a_sibling_override_is_not_a_caller_either() {
        // The second half, and the one a self-call rule alone does NOT catch:
        // `archive-manager` writes two producers extending one base, and the
        // `super.sendMessage(...)` inside Beta is not a caller of Alpha's
        // override. Reading it reports three frames for both.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Base.java", base_producer()),
            ("svc/src/main/java/Alpha.java", override_producer("Alpha")),
            ("svc/src/main/java/Beta.java", override_producer("Beta")),
            (
                "svc/src/main/java/Service.java",
                caller(
                    "Service",
                    "  void a() { alpha.sendMessage(k, body, \"orders\"); }\n\
                     \x20 void b() { beta.sendMessage(k, body, \"orders\"); }",
                ),
            ),
        ]);
        assert_eq!(
            production_two_frame(&f),
            vec![Hop::Resolved {
                value: ArgValue::Literal("orders".into()),
                call_sites: 4,
                caller_files: 3,
            }],
            "neither sibling's `super` line is a caller of the other's override, so both \
             forwards resolve. FOUR sites in THREE files: the two `super` forwards plus the \
             two `Service` calls that frame one's (name, arity) binding also admits",
        );
    }

    #[test]
    fn a_super_call_reaching_its_own_superclass_is_a_caller() {
        // The direction the rule must NOT over-exclude, and the one that fails
        // if `simple_type_name` reads the wrong child of a `generic_type`: then
        // `super_dispatches_to` is `None` for every call, every `super` site is
        // excluded, and this three-class chain loses its real caller.
        //
        // Deep extends Middle extends Base. `Deep.sendMessage` forwards through
        // `super`, so it IS a caller of `Middle.sendMessage` and frame two must
        // read it.
        //
        // `Middle` is declared GENERIC and `Deep extends Middle<Key, Payload>`
        // deliberately: that `extends` clause is a `generic_type`, so the only
        // way to learn that Deep's `super` reaches Middle is to read the FIRST
        // named child through it. Reading the last yields the `type_arguments`
        // node, `simple_type_name` returns `None`, Deep's real `super` call is
        // discarded, and the chain silently reports "no call site" instead of
        // its true length. The estate writes exactly this shape —
        // `extends KafkaProducer<ArchiveEventKafkaKey, SpecificRecord>`.
        let middle = "package p;\n\
             public class Middle<K, V> extends Base<K, V> {\n\
             \x20   public void sendMessage(K kafkaKey, V payload, String topic) {\n\
             \x20       super.sendMessage(kafkaKey, payload, topic);\n\
             \x20   }\n\
             }\n";
        let deep = "package p;\n\
             public class Deep extends Middle<Key, Payload> {\n\
             \x20   public void sendMessage(Key kafkaKey, Payload payload, String topic) {\n\
             \x20       super.sendMessage(kafkaKey, payload, topic);\n\
             \x20   }\n\
             }\n";
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Base.java", base_producer()),
            ("svc/src/main/java/Middle.java", middle.into()),
            ("svc/src/main/java/Deep.java", deep.into()),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { deep.sendMessage(k, body, \"orders\"); }"),
            ),
        ]);
        // Three frames: Base <- Middle <- Deep <- Service. Two frames reach
        // Deep's parameter and stop, which is the bound doing its job — but it
        // must stop at THREE FRAMES, not at "no call site", because the latter
        // is what an over-excluding receiver rule produces.
        assert_eq!(
            f.two_frame_census(Arm::BrokerPublish, Tree::Main),
            BTreeMap::from([(TwoFrameResidue::ThreeOrMoreFrames, 1)]),
            "the chain must be seen and refused for its LENGTH. `no call site at frame 2` \
             here would mean the receiver rule discarded Deep's real `super` call",
        );
    }

    #[test]
    fn the_generality_caveat_counts_the_chains_a_second_frame_carried() {
        // Promised in the floor file BEFORE the run — "the harness reports the
        // number of distinct wrapper methods, distinct forwarding chains and
        // distinct members behind the resolved count, whatever the verdict".
        // A promised report no test can see disappear is the weakest kind, which
        // is the note `one_agreeing_call_site_resolves_the_topic` already makes
        // about S-392's version of this caveat.
        //
        // Two candidates: one resolves THROUGH a forwarding chain, one resolves
        // at the first frame. The chain count must be 1, not 2 — it counts what
        // the SECOND frame carried, not what resolved.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("svc/src/main/java/Thin.java", forwarder("Thin", "send")),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { thin.send(body, \"orders\"); }"),
            ),
            ("other/pom.xml", POM.into()),
            ("other/src/main/java/Direct.java", producer("publish", "Object payload, String topic")),
            (
                "other/src/main/java/Caller.java",
                caller("Caller", "  void go() { direct.publish(body, \"shipments\"); }"),
            ),
        ]);
        assert_eq!(f.production_publish_resolved_two_frame_main_only(), 2);
        let (methods, members, chains) = f.two_frame_generality();
        assert_eq!(methods.len(), 2, "two distinct wrapper methods: {methods:?}");
        assert_eq!(members, BTreeSet::from(["other".to_string(), "svc".to_string()]));
        assert_eq!(
            chains,
            BTreeSet::from(["svc/send(2)".to_string()]),
            "ONE chain carried a second frame; `other` resolved at the first and contributes \
             no chain",
        );
    }

    // ── S-416's non-vacuity guards, which used to run only under the corpus ──
    //
    // The same gap this file records for S-392's V1..V4 ("deleting all four
    // left the suite green"), reintroduced by S-416 and caught by the review's
    // mutation sweep: the whole body of `assert_two_frame_non_vacuous` could be
    // replaced by `let _ = (f, named_sites_found);` with 45 of 45 still passing.

    #[test]
    #[should_panic(expected = "V5")]
    fn a_run_whose_one_frame_base_moved_is_void_not_falsified() {
        // V5 is an equality against S-392's recorded figures, so ANY estate
        // that is not the reference workspace trips it — which is the point:
        // the two-frame figure is only an increment if the base is the base.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { producer.sendMessage(body, \"orders\"); }"),
            ),
        ]);
        assert_the_one_frame_base_is_unmoved(&f, S392_NAMED_TWO_FRAME_SITES.len());
    }

    #[test]
    #[should_panic(expected = "V6")]
    fn a_second_frame_that_buys_nothing_is_void_not_falsified() {
        // A wrapper that already resolves at one frame: the two-frame reading
        // equals the one-frame reading, so the run has measured a no-op. A
        // floor cleared by a no-op is cleared by the first frame.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { producer.sendMessage(body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(f.production_publish_resolved_two_frame_main_only(), 1, "it resolves…");
        assert_eq!(
            f.bought_by_the_second_frame(Arm::BrokerPublish, Tree::Main, true),
            0,
            "…but the second frame bought none of it",
        );
        assert_the_second_frame_did_something(&f);
    }

    #[test]
    fn a_super_call_in_an_anonymous_subclass_is_not_attributed_to_the_outer_class() {
        // `enclosing_type_name` and `enclosing_superclass_name` walk up through
        // `node.parent()`. An anonymous class body is
        // `object_creation_expression -> class_body`, which is NOT one of the
        // declaration kinds either walk stopped on, so both used to climb past
        // it to the enclosing NAMED class — and a `super.send(…)` inside
        // `new Thin() { … }` was attributed to whatever the OUTER class
        // extends. `dispatches_past` then discarded a real caller, and
        // discarding callers RAISES the resolved count, so the old doc comment
        // calling that "the conservative direction" had it backwards.
        //
        // Two real callers supplying different topics: the wrapper must refuse.
        let holder = "package p;\n\
             public class Holder extends Other {\n\
             \x20   Thin make() {\n\
             \x20       return new Thin() {\n\
             \x20           public void send(Object payload, String topic) {\n\
             \x20               super.send(payload, \"other-topic\");\n\
             \x20           }\n\
             \x20       };\n\
             \x20   }\n\
             }\n";
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("svc/src/main/java/Thin.java", forwarder("Thin", "send")),
            ("svc/src/main/java/Other.java", caller("Other", "  void unused() { }")),
            ("svc/src/main/java/Holder.java", holder.into()),
            (
                "svc/src/main/java/Caller.java",
                caller("Caller", "  void go() { thin.send(body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(
            f.production_publish_resolved_two_frame_main_only(),
            0,
            "`Thin.send` has two real callers disagreeing on the topic — the anonymous \
             subclass's `super.send` reaches `Thin`, so it must NOT be discarded. Resolving \
             here means the receiver rule threw a caller away and inflated the headline.",
        );
        assert_eq!(
            f.two_frame_census(Arm::BrokerPublish, Tree::Main),
            BTreeMap::from([(TwoFrameResidue::DisagreeingCallers, 1)]),
        );
    }

    #[test]
    fn a_frame_one_unresolvable_operand_outranks_a_frame_two_boundary() {
        // `two_frame_reason` filtered the blocking values down to
        // `FrameTwoRefused` before ranking them, so a plain frame-ONE
        // `Unresolvable` — the Mockito mechanism `UnresolvableOperand` exists
        // to name — could never win, even though `census_rank` deliberately
        // ranks it ABOVE every frame-two boundary. The census then reported a
        // frame-one fault as a pure frame-two artefact, which is exactly the
        // attribution the "ambiguous pool" caveat rests on.
        //
        // Two blockers on one candidate: an admitted `src/main` call site whose
        // operand resolves to nothing (rank 1), and a forwarder with no caller
        // at all (rank 3).
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("svc/src/main/java/Thin.java", forwarder("Thin", "send")),
            (
                "svc/src/main/java/Unres.java",
                caller("Unres", "  void go() { producer.sendMessage(body, registry.lookup()); }"),
            ),
        ]);
        assert_eq!(
            f.two_frame_census(Arm::BrokerPublish, Tree::Main),
            BTreeMap::from([(TwoFrameResidue::UnresolvableOperand, 1)]),
            "the frame-ONE unresolvable operand outranks the frame-two `no call site`; \
             reporting the boundary would hide a cause that is not a pooling artefact",
        );
    }

    #[test]
    fn the_signature_census_counts_declarers_by_name_and_arity_in_the_main_tree() {
        // The figure the whole "ambiguous pool" attribution is read off, and it
        // was wrong in both directions before the review. It was a by-product of
        // a walk over the files the ledger names as CALLERS of a forwarding
        // method — and a method's declaration is almost never in a file that
        // calls it — so it read 0 for the very modules it describes. It also
        // counted the BARE NAME across every arity and both trees, while frame
        // two binds on `(name, arity)` in `src/main`.
        //
        // Here `send` is declared three times in `svc`: `Thin.send/2` (the
        // forwarder frame two looks up), `Sibling.send/2`, and `Other.send/1` —
        // plus `Ignored.send/2` in the TEST tree. The lookup key is
        // `(svc, send, 2)`, so the count must be 2, not 4.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("svc/src/main/java/Thin.java", forwarder("Thin", "send")),
            (
                "svc/src/main/java/Sibling.java",
                caller("Sibling", "  public void send(Object payload, String topic) { }"),
            ),
            ("svc/src/main/java/Other.java", caller("Other", "  public void send(String topic) { }")),
            (
                "svc/src/test/java/Ignored.java",
                caller("Ignored", "  public void send(Object payload, String topic) { }"),
            ),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { thin.send(body, \"orders\"); }"),
            ),
        ]);
        let candidate = f
            .candidates
            .iter()
            .position(|c| c.arm == Arm::BrokerPublish && c.tree == Tree::Main)
            .expect("one production candidate");
        assert_eq!(
            f.frame_two_name_declarations.get(&candidate),
            Some(&2),
            "two main-tree declarations of `send/2` — the differing arity and the test-tree \
             twin must not be counted: {:?}",
            f.frame_two_name_declarations,
        );
        assert!(
            !f.pool_spans_several_overrides(candidate),
            "two declarations is a base plus ONE override, which cannot mix",
        );
    }

    #[test]
    fn a_pool_spanning_several_overrides_is_named_on_both_sides() {
        // The discriminator, and the guard the review had to ask for: pooling
        // can MANUFACTURE a resolution as well as refuse one. Three same-
        // signature declarations in the module, so the pool spans two overrides
        // and a value drawn from it is not attributable to one receiver.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Base.java", base_producer()),
            ("svc/src/main/java/Alpha.java", override_producer("Alpha")),
            ("svc/src/main/java/Beta.java", override_producer("Beta")),
            (
                "svc/src/main/java/Service.java",
                caller(
                    "Service",
                    "  void a() { alpha.sendMessage(k, body, \"orders\"); }\n\
                     \x20 void b() { beta.sendMessage(k, body, \"orders\"); }",
                ),
            ),
        ]);
        let candidate = f
            .candidates
            .iter()
            .position(|c| c.arm == Arm::BrokerPublish && c.tree == Tree::Main)
            .expect("one production candidate");
        assert_eq!(f.frame_two_name_declarations.get(&candidate), Some(&3));
        assert!(
            f.pool_spans_several_overrides(candidate),
            "a base plus TWO overrides: the pool mixes their callers",
        );
        // It resolves — the two overrides happen to agree — and that is exactly
        // the case the soundness guard must catch, because the agreement is not
        // established per receiver.
        assert_eq!(f.production_publish_resolved_two_frame_main_only(), 1);
        assert_eq!(
            f.resolutions_through_a_second_frame(Arm::BrokerPublish, Tree::Main).len(),
            1,
        );
        assert!(
            f.resolutions_on_an_unambiguous_pool(Arm::BrokerPublish, Tree::Main).is_empty(),
            "…and it must NOT count as sound: the estate run asserts these two figures are \
             equal, so a resolution like this one fails the gate rather than passing quietly",
        );
    }

    #[test]
    fn the_frame_two_cost_is_reported_over_its_own_denominators() {
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            ("svc/src/main/java/Thin.java", forwarder("Thin", "send")),
            (
                "svc/src/main/java/Service.java",
                caller("Service", "  void go() { thin.send(body, \"orders\"); }"),
            ),
        ]);
        assert_eq!(
            f.cost.frame_two_callees, 1,
            "one forwarding method — `send(2)` — is the second frame's lookup denominator",
        );
        assert!(
            f.cost.frame_two_files > 0,
            "the second frame must have opened the file the ledger names as a caller of the \
             forwarding method; a zero denominator reports a cost over nothing",
        );
    }

    #[test]
    fn the_ledger_answers_the_direction_and_never_the_operand() {
        // CRA-06, on an estate small enough to read by hand. Two call sites in
        // ONE caller declaration is the shape that proves the dedup claim: the
        // ledger keys on (source, target, form, kind, relation) and ignores
        // `line`, so the second site is not in it.
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
            (
                "svc/src/main/java/Service.java",
                caller(
                    "Service",
                    "  void go() {\n\
                     \x20   producer.sendMessage(a, \"orders\");\n\
                     \x20   producer.sendMessage(b, \"orders\");\n\
                     \x20 }",
                ),
            ),
        ]);
        assert!(f.ledger.calls_rows > 0, "the extract pass must have recorded Calls rows");
        assert!(
            f.ledger.answers_the_direction(),
            "the ledger must name the declaration that calls the wrapper",
        );
        assert!(
            !f.ledger.answers_the_operand(),
            "no `Calls` field carries the argument text — the hop needs the source, and this \
             is measured rather than read off RefFact's field list",
        );
        assert_eq!(f.ledger.syntactic_call_sites, 2, "two in-module call sites, both observed");
        assert_eq!(f.ledger.ambiguous_by_name, 0, "one declaration of the name in this module");
        assert_eq!(f.ledger.call_sites_the_ledger_misses, 0);
        assert_eq!(
            f.ledger.collapsed_by_dedup, 1,
            "two call sites in one declaration are one ledger row, so the agreement rule \
             cannot see the second from the ledger alone",
        );
    }
}
