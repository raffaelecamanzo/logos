//! **S-392 — the one-hop parameter-forwarding residue** ([CR-121] CRA-05 and
//! CRA-06, [FR-WS-23], [FR-WS-10], [NFR-CC-04]).
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
//! # The floor, declared before the run
//!
//! See [`DECLARED_FLOOR`] and [`ONE_HOP_FLOOR`]. The declaration was written to
//! `docs/planning/sprints/.pending/S-392-T1-floor.txt` at
//! 2026-09-12T12:33:41Z, before any of this module existed, and is reproduced
//! here byte-for-byte because that directory is gitignored.
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
}

impl ArgValue {
    /// The identity two call sites are compared on. `None` for a value that
    /// does not resolve at all, which never agrees with anything.
    fn identity(&self) -> Option<String> {
        match self {
            Self::Literal(text) => Some(format!("lit:{text}")),
            Self::Key(key) => Some(format!("key:{key}")),
            Self::NeedsAnotherHop | Self::Unresolvable(_) => None,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Literal(text) => format!("literal {text:?}"),
            Self::Key(key) => format!("config key {key}"),
            Self::NeedsAnotherHop => "a parameter of the caller (two or more hops)".into(),
            Self::Unresolvable(r) => format!("unresolvable: {}", r.label()),
        }
    }
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
fn observe_calls(
    source: &Source,
    estate: &Estate<'_>,
    want: &BTreeMap<Callee, BTreeSet<usize>>,
    names: &BTreeSet<String>,
    out: &mut Vec<Observation>,
    declarations: &mut BTreeMap<(String, String), BTreeSet<String>>,
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

    record_declarations(plugin, parsed.root_node(), src, source, names, declarations);

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
        let values = wanted_slots
            .iter()
            .filter_map(|s| Some((*s, arg_value(*arguments.get(*s)?, src, &unit, resolver))))
            .collect();
        out.push(Observation {
            callee,
            module: source.module.clone(),
            tree: source.tree,
            file: source.rel.clone(),
            line: name_node.start_position().row as u32 + 1,
            declaration: enclosing_declaration(name_node, src),
            values,
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
    let in_module: Vec<&&Observation> = observed
        .iter()
        .filter(|o| o.module == candidate.module)
        .filter(|o| !main_only || o.tree == Tree::Main)
        .collect();
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
    if values.iter().any(|v| matches!(v, ArgValue::Unresolvable(_))) {
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
        observe_calls(s, &estate, &want, &names, &mut observations, &mut declarations);
    }
    f.cost.hop_whole_estate = started.elapsed();
    f.cost.estate_files = java.len();

    // The targeted pass: only the files the `Calls` ledger names as callers.
    // This is what a ledger-driven implementation would actually open, so it is
    // the figure the perf reconciliation should use — and the gap between its
    // observation count and the whole-estate one is a CRA-06 measurement, not a
    // timing artefact.
    let targets: BTreeSet<&str> = names
        .iter()
        .filter_map(|n| ledger.by_name.get(n))
        .flat_map(|set| set.iter().map(|(file, _)| file.as_str()))
        .collect();
    let started = Instant::now();
    let mut targeted = Vec::new();
    let mut ignored = BTreeMap::new();
    let mut targeted_files = 0;
    for s in java.iter().filter(|s| targets.contains(s.rel.as_str())) {
        targeted_files += 1;
        observe_calls(s, &estate, &want, &names, &mut targeted, &mut ignored);
    }
    f.cost.hop_targeted = started.elapsed();
    f.cost.targeted_files = targeted_files;

    decide_every_candidate(&mut f, &observations);
    f.ledger = answer_the_ledger_question(&f, &ledger, &observations, &targeted, &declarations);
    f
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
        let describe = |hop: Option<&Hop>| match hop {
            Some(Hop::Resolved { value, call_sites, caller_files }) => format!(
                "RESOLVED to {} from {call_sites} site(s) in {caller_files} file(s)",
                value.label(),
            ),
            Some(Hop::Refused(r)) => format!("refused: {}", r.label()),
            None => "not judged".into(),
        };
        let verdict = describe(f.hops.get(&i));
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
            println!("        main-only reading: {}", describe(f.hops_main_only.get(&i)));
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
    fn production_hops(f: &Findings) -> Vec<Hop> {
        f.candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.arm == Arm::BrokerPublish && c.tree == Tree::Main)
            .map(|(i, _)| f.hops.get(&i).cloned().expect("every candidate is judged"))
            .collect()
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
    }

    #[test]
    fn a_wrapper_nothing_calls_is_refused_rather_than_reported_as_agreeing() {
        let (_d, f) = estate(&[
            ("svc/pom.xml", POM.into()),
            ("svc/src/main/java/Producer.java", producer("sendMessage", "Object payload, String topic")),
        ]);
        assert_eq!(production_hops(&f), vec![Hop::Refused(Residue::NoCallSites)]);
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
