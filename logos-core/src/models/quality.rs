//! Quality read-models — results from the governance and quality tools.
//!
//! Covers the quality/pipeline Engine methods (ADR-01):
//! `scan`, `gate`, `check_rules`, `evolution`, `dsm`, `doc_gaps`,
//! `health`, `session_start`, `session_end`, `rescan`

use std::collections::BTreeMap;

use serde::Serialize;

use crate::history::{DegradedReason, FileTemporal};
use crate::models::outcome::OutcomeCounts;
use crate::models::pipeline::RelationCoverage;

/// The **non-persisting** quality readout for the report tier ([FR-IN-07],
/// [CR-095]) — what an agent-host session-start hook shows.
///
/// Deliberately distinct from [`GateResult`]: `gate` exists to *record* the
/// signal ([FR-GV-09] — "every gate writes a snapshot, saved or compared"),
/// while this exists only to *show* it. It therefore writes nothing, so a hook
/// firing at every session boundary never takes the graph-DB write lock and
/// never appends to the [FR-GV-06] `evolution` series.
///
/// The two halves have deliberately different freshness, and say so rather than
/// blurring it:
/// - `signal` / `baseline_signal` are computed **fresh** on every readout (the
///   deterministic `compute` half of the snapshot path, minus the write).
/// - `violations` are read from the **persisted** table — the last
///   [`check_rules`](crate::Engine::check_rules) run's findings. Re-evaluating
///   them cannot be done read-only: [FR-GV-02]'s evaluator re-materialises the
///   whole derived policy graph on every run (BR-12), which is a write. So the
///   readout reports what was last recorded rather than paying a write to look
///   current, and the rendering says "last recorded" rather than implying live.
///   Since [CR-096] the staleness is **quantified** rather than merely
///   labelled: [`check`](crate::Engine::check_rules) records a run marker
///   ([FR-GV-21]) the readout dates its findings from. Its presence is
///   *necessary* to state a *recorded* clean check ([BR-41]) and, since
///   [CR-140], no longer sufficient: the marker must also record a **non-empty
///   evaluated set**, since a `0` over nothing evaluated is not a pass
///   ([FR-GV-03]). See [`CheckRun`].
///
/// [BR-41]: ../../../docs/specs/software-spec.md#4-cross-cutting-non-functional-requirements
/// [FR-GV-21]: ../../../docs/specs/requirements/FR-GV-21.md
/// [FR-GV-03]: ../../../docs/specs/requirements/FR-GV-03.md
/// [CR-096]: ../../../docs/requests/CR-096-recorded-check-marker.md
/// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
///
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-06]: ../../../docs/specs/requirements/FR-GV-06.md
/// [FR-GV-09]: ../../../docs/specs/requirements/FR-GV-09.md
/// [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
/// [CR-095]: ../../../docs/requests/CR-095-session-start-quality-readout.md
#[derive(Debug, Default, Serialize)]
pub struct QualityReadout {
    /// The freshly computed 0–10000 signal; `None` = "n/a" ([ADR-12]) —
    /// reported as such, never as a zero. **Which** absence it is, is
    /// [`signal_absence`](Self::signal_absence); this field never carries the
    /// cause, so the two can never state it two ways.
    ///
    /// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
    pub signal: Option<u32>,
    /// Why [`signal`](Self::signal) is absent ([CR-138], [FR-EH-04]); `None`
    /// exactly when a signal is present.
    ///
    /// Populated by [`SignalAbsence::classify`] from the very snapshot whose
    /// `aggregate_signal` is missing, so the cause reported is the one the
    /// metric computation's own gating condition established. A `None` here
    /// beside an absent `signal` is a readout assembled without the
    /// discriminant (a bare [`Default`], in practice): the rendering then names
    /// **no** cause rather than falling back to the most familiar one, which is
    /// the defect [CR-138] exists to remove.
    ///
    /// [FR-EH-04]: ../../../docs/specs/requirements/FR-EH-04.md
    /// [CR-138]: ../../../docs/requests/CR-138-a-readout-names-the-cause-its-gating-condition-establishes.md
    pub signal_absence: Option<SignalAbsence>,
    /// The blessed baseline signal ([FR-GV-05]); `None` when none is saved.
    ///
    /// [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
    pub baseline_signal: Option<u32>,
    /// `signal − baseline_signal`, present only when both are.
    pub delta: Option<i64>,
    /// The [FR-RC-03] freshness line, marked assumed-fresh — the readout never
    /// reconciles (that would be a write).
    ///
    /// [FR-RC-03]: ../../../docs/specs/requirements/FR-RC-03.md
    pub freshness: String,
    /// Violation messages from the last recorded `check_rules` run, capped by
    /// the caller.
    ///
    /// `None` means **the table is empty**, which on its own is ambiguous:
    /// `check_rules` clears and rewrites the table on every run, so a clean
    /// check and no check at all leave it identical. The table is therefore
    /// never the ground for a verdict — [`check`](Self::check) is. A rendering
    /// may say "clean" only from a marker recording a run that found nothing
    /// ([BR-41]); with no marker it must say "no rule check has run", never
    /// "0 violations", which would assert a pass that may never have happened.
    ///
    /// [BR-41]: ../../../docs/specs/software-spec.md#4-cross-cutting-non-functional-requirements
    pub violations: Option<Vec<String>>,
    /// `Some` when the freshly computed snapshot found Modularity **not
    /// applicable** ([CR-156]) — the same value as that snapshot's
    /// [`MetricSnapshot::modularity_not_applicable`], carried here so the report
    /// tier, which shows no per-dimension breakdown, still says why a small
    /// graph's signal spans one dimension fewer. `None` = Modularity applied.
    ///
    /// [CR-156]: ../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
    pub modularity_not_applicable: Option<ModularityNotApplicable>,
    /// How many violations the last recorded run found in total, before any
    /// display cap — so a truncated list can say what it dropped.
    ///
    /// Strictly derived from the persisted `violations` **rows**, exactly as
    /// before [CR-096]. The marker's own recorded total lives on
    /// [`CheckRun::recorded_count`] and is deliberately not mirrored here: one
    /// transaction writes both halves, so a second copy of the figure could
    /// only ever disagree with this one, and a disagreement is reported as a
    /// warning rather than silently resolved.
    ///
    /// [CR-096]: ../../../docs/requests/CR-096-recorded-check-marker.md
    pub violation_count: Option<usize>,
    /// What is known about the [`check`](crate::Engine::check_rules) run those
    /// violations came from ([CR-096]); `None` = **no run is known of at all**,
    /// which is what licenses the rendering to say "no rule check has run".
    ///
    /// [CR-096]: ../../../docs/requests/CR-096-recorded-check-marker.md
    pub check: Option<CheckRun>,
    /// Degradations (an unreadable store, an absent graph) — never an error:
    /// the report tier reports, it never blocks ([FR-GV-05]).
    pub warnings: Vec<String>,
}

/// **The absence taxonomy** — stated once, here, for every surface that reports
/// a figure it does not have ([S-434], [CR-138], [CR-135] §3.3, [NFR-CC-04]).
///
/// Three surfaces report an absence — the governance/CLI readout
/// (`governance::readout`, `governance::gate`), the CLI itself (`cli/src`), and
/// the SPA (`web/ui/src`) — and before Sprint 73 they did it three ways. This
/// module is the statement they share. It adds no state and classifies nothing
/// itself: the classifiers are [`SignalAbsence`], [`EvaluatedSetAbsence`],
/// [`CrossFileAbsence`], [`DenominatorAbsence`], and the SPA's own
/// `signalAbsence` / `snapshotStaleness` — the six vocabularies of R0's table
/// below. What lives here is the
/// contract all of them already keep, written down so the *next* surface adopts
/// it rather than guessing at a precedent.
///
/// Conformance is audited from source by
/// `logos-core/tests/absence_taxonomy_audit.rs`, which enumerates every site
/// rather than trusting this prose.
///
/// # R0 — One classifier per question
///
/// An absence classifier answers exactly **one** question about exactly **one**
/// figure. A second question gets a second classifier, never more arms on the
/// first — otherwise every match carries arms that cannot arise for it.
///
/// This is why there are six vocabularies and not one, and each is a different
/// question rather than a different dialect:
///
/// | Classifier | The question it answers |
/// |---|---|
/// | [`SignalAbsence`] | why the 0–10000 metric signal is missing — a fact about the **graph** |
/// | [`EvaluatedSetAbsence`] | why the rule check has no denominator — a fact about the **contract** |
/// | [`CrossFileAbsence`] | why one language's relation class has no cross-file edge to count — a fact about the **resolver's output** ([S-441]) |
/// | [`DenominatorAbsence`] | why a relational answer carries no language row for its denominator — a fact about the **answer's anchors** ([S-442]); declared beside the answer types, since two of its spellings are lexicon words this file may not hold outside [`absence::SENTINELS`] |
/// | `healthModel.ts` `signalAbsence` | why the Health page has no signal to show — a fact about a **persisted snapshot**, so it has a middle arm (`unscanned`) the computing readout cannot reach |
/// | `healthModel.ts` `snapshotStaleness` | why a signal that **exists** is not asserted current — not an absence at all, and deliberately separate ([CR-135] §3.3) |
///
/// `signalAbsence` is the **reference model** the Rust vocabulary follows, not
/// a defect: its three-way classification was settled by [CR-135] §3.3 and
/// re-confirmed unchanged under [S-422]'s story review.
///
/// # R1 — Name only the cause the gating condition establishes
///
/// A readout attributing an absence to a cause its own condition has not
/// established is [FR-EH-04] AC2's failure, and it is what [CR-138] reproduced:
/// one binary reporting `indexed: true, node_count: 9` and `signal n/a (empty
/// graph)` in the same breath. When nothing establishes a cause, **name none** —
/// a bare `n/a` — rather than the most familiar one.
///
/// # R2 — The arm carries what establishes it
///
/// The figures that rule the other arms out ride with the cause, carried from
/// the source that established them rather than recomputed at the rendering, so
/// a reader can check the claim without a second command.
///
/// # R3 — No command the record cannot attribute
///
/// An arm names a remediation command only where the classifying condition
/// identifies one. `unindexed` names `logos index` because that condition is
/// exactly "nothing is indexed"; [`SignalAbsence::EmptyGraph`] names none
/// because the one-line readout has no room and the freshness line already
/// carries it; [`EvaluatedSetAbsence`] names none because the marker it reads
/// is written by `replace_violations`, which both `check` and `scan` call
/// ([FR-IN-07]).
///
/// # R4 — Never the favourable reading
///
/// An absent figure is never rendered as a zero, a pass, a date or an age the
/// surface cannot establish. A negative or implausible age is **named**, not
/// rendered; a `0` over no evaluated set is *"no pass is stated"*, never
/// *"clean"* ([FR-GV-03], [BR-41]).
///
/// # R5 — One condition, one spelling, derived once per surface
///
/// Two channels of one surface do not each build the sentence: a pair that can
/// drift, does. The summary and the full readout share
/// `render_signal_absence`, `render_evaluated_set` and `headline_count` for
/// exactly this reason. Where two channels must genuinely differ — the baseline
/// fork, where only the full readout has room to name `gate --save` — the
/// divergence is **recorded at the site** rather than left to look accidental.
///
/// R5 binds within a surface. Across the language boundary the same sentence is
/// necessarily two literals ([`readout::render_age`]'s two degradations and
/// `dashboardModel.ts`'s `UNKNOWN_AGE_AHEAD_OF_NOW` are one condition in two
/// languages); the audit pins them byte-identical instead.
///
/// [`readout::render_age`]: crate::governance::readout
/// [S-422]: ../../../docs/planning/journal.md#s-422-the-health-readout-is-internally-consistent-and-never-stale
/// [S-434]: ../../../docs/planning/journal.md#s-434-one-absence-taxonomy-audited-across-the-three-reporting-surfaces
/// [S-441]: ../../../docs/planning/journal.md#s-441-resolution-coverage-is-reported-per-language-with-its-denominator
/// [S-442]: ../../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
/// [`DenominatorAbsence`]: crate::models::navigation::DenominatorAbsence
/// [BR-41]: ../../../docs/specs/software-spec.md#4-cross-cutting-non-functional-requirements
/// [FR-EH-04]: ../../../docs/specs/requirements/FR-EH-04.md
/// [FR-GV-03]: ../../../docs/specs/requirements/FR-GV-03.md
/// [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [CR-135]: ../../../docs/requests/CR-135-the-health-readout-is-internally-consistent-and-never-stale.md
/// [CR-138]: ../../../docs/requests/CR-138-a-readout-names-the-cause-its-gating-condition-establishes.md
pub mod absence {
    /// The token every absence-reporting source file cites to point here.
    ///
    /// One spelling, held in one place, so "the three surfaces reference the
    /// taxonomy" is a fact the audit checks rather than a convention reviewers
    /// remember. A TypeScript file cannot link a Rust item, so the citation is
    /// this literal path in a comment; a Rust file uses it in an intra-doc
    /// link, which contains the same token.
    pub const TAXONOMY_REFERENCE: &str = "models::quality::absence";

    /// The product's **closed** absence vocabulary — the words that mean
    /// "this figure is not here" on any surface.
    ///
    /// A new surface uses one of these spellings rather than inventing a
    /// fifth; that is the contract, and it is also what makes the audit's
    /// enumeration possible at all. `logos-core/tests/absence_taxonomy_audit.rs`
    /// walks the three surfaces for exactly these, matched case-insensitively
    /// on a whole-token boundary, and compares what it finds against a dated
    /// census.
    ///
    /// # What this list can and cannot catch
    ///
    /// It catches every site that *uses* the vocabulary, which is every
    /// conformant site and every site conformant except in its reasoning. It
    /// cannot catch a site that reports an absence in words nobody has used
    /// before — no lexical census can — so the enumeration is stated as being
    /// over this lexicon rather than over "all absences", and the lexicon is
    /// here, in the open, rather than buried in the test.
    ///
    /// Matching is deliberately **not** restricted to string literals: a
    /// sentinel in a type alias, a `case` label or a JSX text node is a site
    /// too, and an identifier that merely happens to spell one is a false
    /// positive the census adjudicates by hand. A census may be wrong in the
    /// loud direction only.
    ///
    /// # The three resolution spellings ([S-441], [FR-RS-09])
    ///
    /// `no-references-recorded`, `no-resolved-edges` and `same-file-only` are
    /// the serialised tags of [`CrossFileAbsence`](super::CrossFileAbsence) —
    /// the typed resolution denominator's named states. They were added here,
    /// rather than spelled at their first surface, so the vocabulary the
    /// relational answers consume next ([S-442]) is closed before its second
    /// consumer exists. Being serde-derived, they are not source literals, so
    /// the lexical census finds no site for them;
    /// `the_resolution_denominator_speaks_only_the_lexicon` in the audit pins
    /// them to this list structurally instead.
    ///
    /// # The relational answer's spelling ([S-442], [FR-NV-14])
    ///
    /// `no-language-recorded` is the one wording the relational answers'
    /// denominator added: [`DenominatorAbsence`]'s arm for anchors that
    /// resolved into no language-tagged file. Its two other arms reuse
    /// `unindexed` and `n/a` as written, so the relational answers speak three
    /// words of this list and coin one.
    ///
    /// [`DenominatorAbsence`]: crate::models::navigation::DenominatorAbsence
    /// [S-441]: ../../../docs/planning/journal.md#s-441-resolution-coverage-is-reported-per-language-with-its-denominator
    /// [S-442]: ../../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
    /// [FR-RS-09]: ../../../docs/specs/requirements/FR-RS-09.md
    /// [FR-NV-14]: ../../../docs/specs/requirements/FR-NV-14.md
    pub const SENTINELS: &[&str] = &[
        "at an unknown age",
        "de-indexed",
        "empty graph",
        "evaluated set unknown",
        "indeterminate",
        "moved-past",
        "n/a",
        "no baseline",
        "no pass is stated",
        "no rules contract",
        "no-language-recorded",
        "no-production-scope",
        "no-references-recorded",
        "no-resolved-edges",
        "none recorded",
        "not comparable",
        "nothing was evaluated",
        "same-file-only",
        "unindexed",
        "unscanned",
    ];
}

/// Why a [`QualityReadout`] has no signal to report ([CR-138], [FR-EH-04]).
///
/// An absent signal has **two** causes on this surface and only one of them is
/// an empty graph. Reporting both as "empty graph" is what [FR-EH-04] AC2
/// forbids — a readout attributing an absence to a cause its own gating
/// condition has not established — and it is what shipped: a crate whose only
/// source is `tests/only_tests.rs` reports `indexed: true, node_count: 9` from
/// `logos status` and `signal n/a (empty graph)` from `quality-report`, in the
/// same breath ([CR-138] §2).
///
/// # Two arms, not three
///
/// The Health page classifies the same absence **three** ways
/// (`unindexed` / `unscanned` / `no-production-scope`, `web/ui/src/views/health/
/// healthModel.ts`) and is the reference model this vocabulary follows. Its
/// middle arm cannot arise here: that page reads a *persisted* snapshot, so
/// "indexed but never scanned" is a state it can be in, while this readout
/// **computes** its signal on every call ([FR-IN-07] — the non-persisting
/// report tier) and therefore always has one snapshot's worth of answer.
///
/// # Why the first arm is `EmptyGraph` and not `unindexed`
///
/// Deliberately not the Health page's spelling, because it is not the Health
/// page's fact, and the two predicates are duals rather than copies:
///
/// - Health's `unindexed` is `files > 0 || nodes > 0` being false — "is there
///   **anything** at all?", an index-state question over the whole store.
/// - `EmptyGraph` here is `files > 0 && nodes > 0` being false — "is there a
///   graph **built from ingested source**?".
///
/// The conjunction is load-bearing and was found by reproduction, not by
/// reasoning. `nodes > 0` alone is **not** evidence that any code was indexed:
/// the annotation pass materialises one derived `Layer`/`Boundary` vertex per
/// `rules.toml` declaration unconditionally, whether or not a file matches. A
/// project that declares two layers and a boundary and has never been indexed
/// therefore reports `file_count: 0, node_count: 3` — three vertices Logos
/// manufactured itself. Classifying that as [`NoProductionScope`] would state
/// "3 node(s) indexed" about a store where nothing was indexed (the value
/// [FR-EH-04] AC3 forbids), and would route the reader to the arm that
/// deliberately names **no** command — when `logos index` is exactly what they
/// need. Requiring an ingested file rules it out with a figure already in hand.
///
/// [`NoProductionScope`]: Self::NoProductionScope
///
/// [FR-EH-04]: ../../../docs/specs/requirements/FR-EH-04.md
/// [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
/// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
/// [CR-138]: ../../../docs/requests/CR-138-a-readout-names-the-cause-its-gating-condition-establishes.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "cause", rename_all = "kebab-case")]
pub enum SignalAbsence {
    /// The store holds **no node**: there is no graph to score, so the metric
    /// graph's emptiness says nothing beyond the store's own.
    ///
    /// `logos index` is the step that changes this, and the rendering names it
    /// nowhere — the one-line readout has no room for a remediation and the
    /// full one already carries the freshness line. Naming a command here would
    /// bring it under [FR-EH-04]'s first AC; naming none does not.
    EmptyGraph,
    /// Source was ingested and a graph was built from it, but its **production
    /// scope** ([FR-QM-08]) is empty — every vertex was dropped before scoring,
    /// as `is_test`, as a derived policy vertex, or as a promoted broker marker.
    ///
    /// Both figures are what *establishes* the arm rather than decoration, and
    /// they are carried rather than recomputed at the rendering:
    /// `indexed_nodes > 0` is exactly what rules out [`EmptyGraph`](Self::EmptyGraph),
    /// and `test_functions` is the production filter's own count of what it
    /// excluded ([FR-QM-07]) — the usual reason the scope came out empty, and
    /// reported as the count it is even when it is `0` (a graph of nothing but
    /// derived vertices), never inflated into a claim that every node was a
    /// test.
    ///
    /// **No command changes this state**, so no rendering of it names one — the
    /// same conclusion the Health page reached for its `no-production-scope`
    /// arm.
    ///
    /// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
    NoProductionScope {
        /// Nodes the store holds, from [`StoreCounts::nodes`] — the same figure
        /// `logos status` reports as `node_count`, read from the same query, so
        /// the two surfaces cannot contradict each other about **that figure**.
        /// Their contradicting is [CR-138]'s reproduction.
        ///
        /// They still answer different *questions* about "indexed" (see the
        /// type doc), so this is agreement on the number, not on the predicate.
        /// Reaching this arm at all means a file was ingested, so every node
        /// counted here belongs to a graph built from source.
        ///
        /// [`StoreCounts::nodes`]: crate::graph_store::StoreCounts::nodes
        indexed_nodes: u64,
        /// Function/method nodes the production filter excluded as `is_test`
        /// ([FR-QM-08]) — [`MetricSnapshot::test_function_count`], carried
        /// verbatim.
        test_functions: u64,
    },
}

impl SignalAbsence {
    /// Classify the absence behind a snapshot that produced no signal, or
    /// `None` when it produced one.
    ///
    /// # The predicate is the metric's own
    ///
    /// The gate is [`MetricSnapshot::empty`] — the *same* field
    /// [`crate::metrics::compute`] tests to decide whether to emit an
    /// `aggregate_signal` at all, set from the production metric graph's vertex
    /// count. It is deliberately not a second opinion computed from
    /// [`function_count`](MetricSnapshot::function_count),
    /// [`node_count`](MetricSnapshot::node_count) or a re-read of the store:
    /// each of those *happens* to agree on today's fixtures and is a different
    /// question (a graph of production types with no production functions has
    /// `function_count == 0` and is not empty), and a discriminant that merely
    /// happens to agree is the defect class [CR-138] exists to close. The
    /// agreement is pinned by test rather than left to inspection —
    /// `logos-core/tests/quality_readout_scope.rs`.
    ///
    /// `indexed_nodes` and `indexed_files` are the **store's** counts, the two
    /// facts the snapshot cannot supply: it describes the graph *after* the
    /// production filter, so its own `node_count` reads zero on both arms and
    /// can never separate them. Their caller reads both from one
    /// [`GraphStore::counts`](crate::graph_store::GraphStore::counts).
    ///
    /// Both are required for [`NoProductionScope`](Self::NoProductionScope),
    /// and `indexed_files` is not redundant: derived `Layer`/`Boundary`
    /// vertices exist without any file, so `indexed_nodes > 0` alone does not
    /// establish that code was ever indexed. See the type doc.
    ///
    /// [CR-138]: ../../../docs/requests/CR-138-a-readout-names-the-cause-its-gating-condition-establishes.md
    #[must_use]
    pub fn classify(
        metrics: &MetricSnapshot,
        indexed_nodes: u64,
        indexed_files: u64,
    ) -> Option<Self> {
        if !metrics.empty {
            return None;
        }
        if indexed_nodes == 0 || indexed_files == 0 {
            return Some(Self::EmptyGraph);
        }
        Some(Self::NoProductionScope {
            indexed_nodes,
            test_functions: metrics.test_function_count,
        })
    }
}

/// What the readout knows about the [`check`](crate::Engine::check_rules) run
/// its violations came from ([CR-096], [FR-GV-21]).
///
/// Two things can produce one of these, and they are **not** equally
/// authoritative — which is why [`recorded_count`](Self::recorded_count)
/// carries the distinction rather than a separate flag that could drift from
/// it:
///
/// - **The [FR-GV-21] marker** (`recorded_count: Some(_)`). A run demonstrably
///   happened, at a known time and a known `HEAD`. Only this *can* license
///   stating a *clean* check ([BR-41]) — a clean run records `Some(0)`, which
///   is precisely the fact an empty `violations` table cannot express. It is
///   not sufficient on its own: since [CR-140] a clean result also requires a
///   recorded non-empty evaluated set, so `recorded_count: Some(0)` beside an
///   absent [`checked_rules`](Self::checked_rules) is a **vacuous** run, not a
///   pass. The full condition lives in one place, `recorded_clean_over` in
///   `governance::readout`; this field is one of its three inputs.
/// - **The violation rows' own `created_at`** (`recorded_count: None`), on a
///   store written before the marker migration. The rows date themselves, so
///   such a store gets dated findings immediately — but with no marker there
///   is nothing to say a run happened at all when the table is empty, and no
///   `HEAD` was ever recorded, so no clean check and no tree comparison is
///   asserted from it.
///
/// [BR-41]: ../../../docs/specs/software-spec.md#4-cross-cutting-non-functional-requirements
/// [FR-GV-21]: ../../../docs/specs/requirements/FR-GV-21.md
/// [CR-096]: ../../../docs/requests/CR-096-recorded-check-marker.md
/// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct CheckRun {
    /// Unix-seconds the run happened at.
    pub ran_at: i64,
    /// How long ago that was, in seconds, **at the moment this readout was
    /// taken**.
    ///
    /// Derived here rather than in the rendering so the renderer stays a pure
    /// function of this read-model — otherwise every rendering test would read
    /// the wall clock. Negative under clock skew (a store carried across
    /// machines, a clock stepped back); the rendering names that case rather
    /// than clamping it into a plausible-looking age.
    pub age_seconds: i64,
    /// What `HEAD` was **when the run happened**, and nothing more.
    ///
    /// It supports "has the tree moved since this ran". It does *not* mean the
    /// findings were introduced by that commit — the same field on
    /// `metric_snapshots` was once read the second way and produced a
    /// confidently wrong attribution ([CR-095] review, finding 12, retracted).
    /// `None` when `HEAD` did not resolve at run time, or when the record came
    /// from the rows rather than a marker; the rendering then omits the tree
    /// comparison rather than substituting a placeholder.
    ///
    /// [CR-095]: ../../../docs/requests/CR-095-session-start-quality-readout.md
    pub commit_sha: Option<String>,
    /// Current `HEAD`, for the comparison against [`commit_sha`](Self::commit_sha);
    /// `None` outside a repo or without git.
    pub head_sha: Option<String>,
    /// `HEAD` has moved since the run — the findings were measured against a
    /// **different tree**, which is the distinction [`age_seconds`](Self::age_seconds)
    /// alone cannot draw and the whole reason `HEAD` is recorded.
    ///
    /// `false` whenever the comparison cannot be drawn (either sha absent):
    /// an unresolvable `HEAD` is never *treated* as a moved tree, mirroring the
    /// [FR-CV-06] coverage-artifact staleness rule this follows
    /// ([NFR-RA-05] — never guessed).
    ///
    /// [FR-CV-06]: ../../../docs/specs/requirements/FR-CV-06.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    pub tree_moved: bool,
    /// The violation total the [FR-GV-21] marker recorded, or `None` when this
    /// record was recovered from the rows of a pre-migration store.
    ///
    /// `Some(0)` is the **recorded clean run** — the one state an empty
    /// `violations` table can never express, and the only ground on which the
    /// readout may state a pass ([BR-41]).
    ///
    /// [BR-41]: ../../../docs/specs/software-spec.md#4-cross-cutting-non-functional-requirements
    /// [FR-GV-21]: ../../../docs/specs/requirements/FR-GV-21.md
    pub recorded_count: Option<i64>,
    /// How many rules the run evaluated — [`recorded_count`](Self::recorded_count)'s
    /// **denominator** ([CR-140] §3.2, [NFR-CC-04]), and `Some` only when that
    /// denominator is non-empty.
    ///
    /// A recorded clean run may be stated as clean **only** from this: [FR-GV-03]
    /// defines clean as *"a contract was evaluated and held"*, so a count of `0`
    /// over an evaluated set of nothing is a vacuous run, not a pass. Why the set
    /// is empty or unknown is [`evaluated_absence`](Self::evaluated_absence);
    /// this field never carries the cause, so the two can never state it two ways
    /// — the pairing [`signal`](QualityReadout::signal) /
    /// [`signal_absence`](QualityReadout::signal_absence) already uses, and the
    /// reason both are produced by one call to
    /// [`EvaluatedSetAbsence::classify`].
    ///
    /// [FR-GV-03]: ../../../docs/specs/requirements/FR-GV-03.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
    pub checked_rules: Option<u32>,
    /// Why there is no [`checked_rules`](Self::checked_rules) denominator to
    /// state ([CR-140] §3.2); `None` exactly when there is one.
    ///
    /// A `None` here beside a `None` `checked_rules` is a record assembled
    /// without the discriminant (a bare [`Default`], in practice): the rendering
    /// then names the set **unknown** without attributing a cause, rather than
    /// falling back to the most reassuring one. That is
    /// [`QualityReadout::signal_absence`]'s rule, applied to the second figure on
    /// the same readout.
    ///
    /// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
    pub evaluated_absence: Option<EvaluatedSetAbsence>,
}

/// Why a [`CheckRun`] has no evaluated-set denominator to report ([CR-140]
/// §3.2, [FR-IN-07], [FR-GV-03]).
///
/// The sibling of [`SignalAbsence`], deliberately: the same readout carries two
/// figures that can be absent, and [S-434] audits both against **one**
/// taxonomy, so this follows that type's shape rather than inventing a second
/// vocabulary — a `classify` constructor, the rule that an arm carries whatever
/// establishes it, the same serde tagging (`tag = "cause"`, kebab-case), and
/// the same refusal to name a remediation command.
///
/// Two differences from [`SignalAbsence`], stated rather than glossed as
/// sameness, because [S-434] has to reconcile them:
/// - **`classify` returns the figure as well as the absence.** `SignalAbsence`
///   returns the absence alone and reads its figure from a different source, so
///   it cannot do this; see [`classify`](Self::classify). This shape is the
///   stronger one — mutual exclusivity is structural here and test-enforced
///   there — so the reconciliation worth making is to lift it into
///   `SignalAbsence`, not to weaken this.
/// - **This derives `Copy`** and `SignalAbsence` does not, though its fields
///   would allow it. Incidental rather than meaningful; either type may gain or
///   drop it without consequence.
///
/// # Why this is a second enum rather than two more [`SignalAbsence`] arms
///
/// They answer different questions about different figures: [`SignalAbsence`]
/// says why the 0–10000 metric signal is missing, which is a fact about the
/// **graph**; this says why the rule check has no denominator, which is a fact
/// about the **contract**. Folding them together would make every match on
/// either figure carry arms that cannot arise for it — and would put
/// `EmptyGraph` in the position of explaining an absent rule count, which no
/// gating condition here establishes ([FR-EH-04] AC2's failure, one figure
/// over).
///
/// # No arm names a command
///
/// [FR-IN-07]'s appended criterion is that the rendered line **names no
/// command**, and the reason is recorded here rather than only at the
/// rendering: the marker is written by `replace_violations`, which both
/// [`check_rules`](crate::Engine::check_rules) and [`scan`](crate::Engine::scan)
/// call, so any command name in the sentence is an attribution the record
/// cannot support. [`NoContract`](Self::NoContract) is the one arm with an
/// obvious remediation, and it does not name it either — for the reason
/// [`SignalAbsence::EmptyGraph`] does not name `logos index`.
///
/// [S-434]: ../../../docs/planning/journal.md#s-434-one-absence-taxonomy-audited-across-the-three-reporting-surfaces
/// [FR-EH-04]: ../../../docs/specs/requirements/FR-EH-04.md
/// [FR-GV-03]: ../../../docs/specs/requirements/FR-GV-03.md
/// [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
/// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "cause", rename_all = "kebab-case")]
pub enum EvaluatedSetAbsence {
    /// No rules contract was authored at all — what `logos check` exits `4` on
    /// ([FR-GV-22]) while printing *"nothing was evaluated"*.
    ///
    /// Its own state, not a flavour of [`NoRulesAuthored`](Self::NoRulesAuthored):
    /// an unconfigured project and a configured one that enforces nothing are
    /// different situations, and `checked_rules = 0` is recorded by both.
    ///
    /// [FR-GV-22]: ../../../docs/specs/requirements/FR-GV-22.md
    NoContract,
    /// A rules contract is present and authors **no rules** — the state
    /// [`logos init`](crate::init)'s default template produces, so the ordinary
    /// state of a freshly initialised project rather than an edge case.
    NoRulesAuthored,
    /// No evaluated set was recorded at all — *unknown*, never zero.
    ///
    /// Two stores reach this, and neither may be rendered favourably:
    /// - a marker written **before** migration 21, which has the three
    ///   evaluated-set columns as `NULL` ([CR-140] CRA-05). An existing install
    ///   is the common case, which is why this arm exists rather than a default;
    /// - a record recovered from the violation **rows** of a store written
    ///   before the marker existed at all ([CR-096]), which never recorded an
    ///   evaluated set either.
    ///
    /// [CR-096]: ../../../docs/requests/CR-096-recorded-check-marker.md
    /// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
    Unrecorded,
}

impl EvaluatedSetAbsence {
    /// Split what a marker recorded about its evaluated set into **the figure
    /// and the absence**, exactly one of which is `Some`.
    ///
    /// Returning the pair — rather than the absence alone, as
    /// [`SignalAbsence::classify`] does — is what makes "a denominator and a
    /// reason it is missing can never both be present" structural instead of a
    /// comment at the call site. [`SignalAbsence`] takes its figure from a
    /// different source (the metric snapshot) and so cannot do this; both of
    /// these come from the same marker row.
    ///
    /// Both arguments are the marker's columns as stored: `None` is `NULL`,
    /// which means the row predates migration 21. The columns are written by one
    /// statement, so in practice they are `NULL` together; either being `NULL`
    /// is treated as [`Unrecorded`](Self::Unrecorded) rather than half-believed.
    ///
    /// A recorded `checked_rules > 0` settles the question by itself: a run that
    /// evaluated rules has a denominator whatever else the row says, so the
    /// figure is reported and no absence is claimed.
    ///
    /// A **negative** `checked_rules` is neither: migration 21 put a `CHECK` on
    /// `rules_present` and none on this column, so a corrupted or hand-edited
    /// row can carry one. It reads as [`Unrecorded`](Self::Unrecorded) rather
    /// than falling through to the zero-rules arms, which would state *"a
    /// contract authoring no rules"* — a specific, plausible-looking claim about
    /// a row that records nothing usable. That is the same posture the
    /// over-large clamp below takes, and the one `render_age` takes for an
    /// impossible timestamp: name the value unusable, never render it as fact.
    #[must_use]
    pub fn classify(
        checked_rules: Option<i64>,
        rules_present: Option<bool>,
    ) -> (Option<u32>, Option<Self>) {
        match (checked_rules, rules_present) {
            (Some(checked), _) if checked > 0 => {
                // Clamped rather than truncated: the producer's own figure is a
                // `u32` (`governance::evaluate`), so a wider value is a corrupted
                // or hand-edited row, and saturating keeps it large rather than
                // wrapping it into a small, plausible-looking count.
                (Some(u32::try_from(checked).unwrap_or(u32::MAX)), None)
            }
            // A count no run could have produced records no evaluated set.
            (Some(corrupt), _) if corrupt < 0 => (None, Some(Self::Unrecorded)),
            (None, _) | (_, None) => (None, Some(Self::Unrecorded)),
            (Some(_), Some(true)) => (None, Some(Self::NoRulesAuthored)),
            (Some(_), Some(false)) => (None, Some(Self::NoContract)),
        }
    }
}

/// Why one language's relation class has **no cross-file edge to count**
/// ([S-441], [FR-RS-09], [CR-142] D3) — the named states of the typed
/// resolution denominator.
///
/// The third Rust sibling of [`SignalAbsence`] and [`EvaluatedSetAbsence`], and
/// shaped like the second: a `classify` constructor that returns **the figure
/// and the absence, exactly one of which is `Some`**, the same serde tagging
/// (`tag = "cause"`, kebab-case), and arms that carry whatever establishes them
/// (R2). Its tags are spelled in [`absence::SENTINELS`], so the relational
/// answers that attach this denominator next ([S-442]) speak the closed lexicon
/// rather than a fourth dialect.
///
/// # Why a named state rather than a `0`
///
/// A cross-file count of `0` beside a same-file count of 245 reads as a
/// measurement of a codebase that happens not to call across files. It is not:
/// it is the resolver producing no cross-file binding for that language at all,
/// which is [CR-142]'s defect, and it survived 74 sprints averaged into one
/// global ratio ([FR-RS-04]). R4 forbids the favourable reading, so the zero is
/// never serialised as a figure — the field is `None` and this names why.
///
/// # A question about edges, not about references
///
/// Every arm is decided by the **resolved edge set** — the set `callers` and
/// `impact` traverse — and never by the ledger's `resolved` flag alone. The two
/// are different populations: an edge is unique per `(source, target, kind)`,
/// and one whose target lies in no indexed file has no locality to report. So
/// [`NoResolvedEdges`](Self::NoResolvedEdges) carries the ledger's `bound`
/// figure beside `references` rather than claiming it is `0`: R1 — the arm
/// names the fact its condition establishes, that no edge left this language,
/// and reports the ledger's own count as the count it is.
///
/// # No arm names a command
///
/// No command changes any of these states — the resolver's reach is a property
/// of the language plugin, not of anything the reader can run (R3).
///
/// [S-441]: ../../../docs/planning/journal.md#s-441-resolution-coverage-is-reported-per-language-with-its-denominator
/// [S-442]: ../../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
/// [FR-RS-04]: ../../../docs/specs/requirements/FR-RS-04.md
/// [FR-RS-09]: ../../../docs/specs/requirements/FR-RS-09.md
/// [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "cause", rename_all = "kebab-case")]
pub enum CrossFileAbsence {
    /// The language has resolved edges of this class, and **every one** stays
    /// inside the file it starts in — the resolver binds same-file references
    /// and none across a file boundary. [CR-142] §3.1's fingerprint: measured at
    /// 1.4.15, every non-Rust language in both corpora was in this state for
    /// `Calls`.
    ///
    /// `same_file_edges > 0` is what establishes the arm, and is carried so a
    /// reader can see the language is not simply unresolved.
    ///
    /// [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
    SameFileOnly {
        /// Resolved edges of this class whose two endpoints share a file.
        same_file_edges: u64,
    },
    /// The language recorded references of this class and **no resolved edge
    /// with a locality** leaves any node of it, same-file or cross-file. An
    /// edge whose target lies in no indexed file has no locality and is counted
    /// in neither column, so it does not rule this arm out.
    NoResolvedEdges {
        /// Ledger rows of this class the language's files recorded — `> 0` is
        /// what separates this arm from [`NoReferencesRecorded`](Self::NoReferencesRecorded).
        references: u64,
        /// Of those rows, the ones the ledger flags bound — reported as the
        /// ledger's own count, which is usually `0` here but is not what the
        /// arm asserts (see the type doc).
        bound: u64,
    },
    /// The language's files recorded **no reference** of this class, so there
    /// is nothing for the resolver to bind — a data grammar's `Calls`, in
    /// practice. Its own state because "nothing to resolve" and "resolved
    /// nothing" are different facts, and a reader acts on the second.
    NoReferencesRecorded,
}

impl CrossFileAbsence {
    /// Split one relation class's counts into **the cross-file figure and the
    /// absence**, exactly one of which is `Some` — [`EvaluatedSetAbsence::classify`]'s
    /// shape, so "a count and a reason it is missing can never both be present"
    /// is structural rather than a comment at the call site.
    ///
    /// A non-zero `cross_file_edges` settles the question by itself. Otherwise
    /// the arms are tried in the order of what establishes them: resolved edges
    /// that all stay in their file, then references with no resolved edge, then
    /// no reference at all.
    #[must_use]
    pub fn classify(
        references: u64,
        bound: u64,
        same_file_edges: u64,
        cross_file_edges: u64,
    ) -> (Option<u64>, Option<Self>) {
        if cross_file_edges > 0 {
            (Some(cross_file_edges), None)
        } else if same_file_edges > 0 {
            (None, Some(Self::SameFileOnly { same_file_edges }))
        } else if references > 0 {
            (None, Some(Self::NoResolvedEdges { references, bound }))
        } else {
            (None, Some(Self::NoReferencesRecorded))
        }
    }
}

/// Full architecture-quality scan result (FR-QM-01..06, S-020).
///
/// The 0–10000 signal is the geometric-mean aggregate (ADR-12); `None` is the
/// empty-graph "n/a" sentinel, mirroring [`MetricSnapshot::aggregate_signal`].
/// Every scan reconciles first and stamps `freshness` (ADR-11, FR-RC-01/03).
#[derive(Debug, Default, Serialize)]
pub struct ScanResult {
    /// The 0–10000 quality signal (ADR-12); `None` = "n/a" (empty graph).
    pub signal: Option<u32>,
    /// The FR-RC-03 freshness line: `reconciled N files · HEAD <sha> · M
    /// unresolved refs`, prefixed `INCOMPLETE` on a partial reconcile
    /// (NFR-RA-11) or marked assumed-fresh under `--no-reconcile` (FR-RC-04).
    pub freshness: String,
    /// `rules.toml` violations found by this run (FR-GV-02).
    pub violations: Vec<Violation>,
    pub metrics: MetricSnapshot,
    /// Per-dimension worst-offender detail for the five CR-005 structural
    /// dimensions (FR-QM-09..13, CR-005): the top-N offending functions/containers
    /// per dimension, deterministically ordered and capped. Empty lists when a
    /// dimension has no offenders (or dropped out); the review-phase visibility
    /// the `scan` surface gains. Persisted with the snapshot ([FR-QM-15]), so the
    /// read-only twin carries the same lists — or
    /// [`recorded: false`](WorstOffenders::recorded) for a snapshot that
    /// recorded none.
    ///
    /// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
    pub worst_offenders: WorstOffenders,
    /// The **non-gated temporal tier** (CR-006, [FR-GH-07], [BR-26]): per-file
    /// churn / co-change / defect-heuristic columns, explicitly labeled as
    /// advisory so the two-tier boundary is visible, never implied
    /// ([NFR-CC-04]). Computed independently of the gated columns above, which
    /// stay byte-identical whether `history.db` is present or absent.
    ///
    /// [FR-GH-07]: ../../../docs/specs/requirements/FR-GH-07.md
    /// [BR-26]: ../../../docs/specs/software-spec.md#322-git-history-analytics
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub temporal: TemporalTier,
    /// Degradations (reconcile skips, unreadable files) — never an error.
    pub warnings: Vec<String>,
    /// Advisory notes — never a `warnings` entry, so a CI parser scanning
    /// `warnings` is unaffected (CR-119, HF-1). Carries the reconcile's
    /// minified-JS exclusion notice; elided from the serialized report when
    /// empty, so a run with nothing to note renders byte-identical to before.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// The non-gated temporal tier rendered in scan detail ([FR-GH-07]).
///
/// Carries the values from the git-history analytics tier (CR-006), kept
/// strictly separate from the gated quality columns: a `gated: false` marker
/// and a tier label make the boundary explicit ([NFR-CC-04], [BR-26]), and the
/// defect column is labeled a **heuristic** ([FR-GH-05]). When the tier is
/// degraded (non-git / `git` absent / shallow) or unavailable, `files` is empty
/// and `notice` explains why — `n/a`, never fabricated ([FR-GH-08],
/// [NFR-RA-05]).
///
/// [FR-GH-05]: ../../../docs/specs/requirements/FR-GH-05.md
/// [FR-GH-08]: ../../../docs/specs/requirements/FR-GH-08.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Default, Serialize)]
pub struct TemporalTier {
    /// The tier label — advisory, never gated ([BR-26], [NFR-CC-04]).
    pub tier: &'static str,
    /// Always `false`: the temporal tier never moves the gate ([BR-26]).
    pub gated: bool,
    /// The mandatory heuristic label on each file's `defect_commits`
    /// ([FR-GH-05]).
    pub defect_label: &'static str,
    /// The HEAD the temporal values were computed at; `None` when degraded.
    pub head_sha: Option<String>,
    /// The effective `[history]` config hash ([FR-GH-09]); `None` when degraded.
    pub config_hash: Option<String>,
    /// `Some` when the tier degraded instead of computing ([FR-GH-08]).
    pub degraded: Option<DegradedReason>,
    /// A one-line notice (degraded reason and/or first-mine), or `None`.
    pub notice: Option<String>,
    /// Per-file temporal columns, canonical path order; a file absent here has
    /// no in-window history → `n/a` ([FR-GH-03]).
    pub files: Vec<FileTemporal>,
}

/// Per-dimension worst-offender lists for the five CR-005 structural dimensions
/// (CR-005 §3.2 review-phase visibility): the top-N offenders per dimension,
/// each list deterministically ordered (by offending severity, then node id) and
/// capped ([NFR-RA-06]). A dimension with no offenders — or one that dropped out
/// of the aggregate (Cohesion/Focus with no construct) — carries an empty list.
///
/// The lists explain *which* code drives a low dimension score, so a reviewer can
/// act on the signal; they never enter the aggregate or the gate (report detail
/// only, exactly as `doc_gaps` is advisory).
///
/// Every metric snapshot persists the lists it computed ([FR-QM-15]), so the
/// read-only Health bundle projects them from the snapshot it reads.
/// [`recorded`](Self::recorded) is what separates the two empties a reader
/// would otherwise conflate: a list that is empty because nothing crossed a
/// threshold, and a list that is empty because the snapshot was written before
/// offenders were persisted ([NFR-CC-04]). The serialized shape is
/// `{"recorded": bool, "nesting": [..], "conciseness": [..], "cohesion": [..],
/// "focus": [..], "uniqueness": [..]}`; with `recorded: false` every list is
/// `[]` and means nothing.
///
/// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WorstOffenders {
    /// `true` when these lists are the ones the snapshot's computation produced
    /// — freshly computed by `scan`, or read back from a snapshot that persisted
    /// them. `false` (the default) is **"offenders not recorded"**: a snapshot
    /// written before [FR-QM-15], or no snapshot at all. Never inferred from
    /// the lists being empty.
    ///
    /// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
    pub recorded: bool,
    /// Deeply-nested production functions (`max_nesting_depth ≥ T_nest`),
    /// deepest first (FR-QM-09).
    pub nesting: Vec<Offender>,
    /// Production brain methods (CC ∧ LOC ∧ nesting thresholds all met), highest
    /// complexity first (FR-QM-10).
    pub conciseness: Vec<Offender>,
    /// Low-cohesion production classes (LCOM4 ≥ 2 over their bodied methods),
    /// most fragmented first (FR-QM-11).
    pub cohesion: Vec<Offender>,
    /// God class-like containers (bodied methods ≥ `T_m` ∨ span ≥ `T_span`),
    /// most methods first (FR-QM-12).
    pub focus: Vec<Offender>,
    /// Production functions in a near-clone group (`clone_group IS NOT NULL`),
    /// grouped by clone group, largest duplicated mass (members × mean line
    /// count) first, then group id, then member id (FR-QM-13, CR-163).
    pub uniqueness: Vec<Offender>,
}

impl WorstOffenders {
    /// The five dimension names, in canonical order — the persisted
    /// `metric_snapshot_offenders.dimension` values and the serialized field
    /// names, spelled once.
    pub const DIMENSIONS: [&'static str; 5] =
        ["nesting", "conciseness", "cohesion", "focus", "uniqueness"];

    /// Each list paired with its [`DIMENSIONS`](Self::DIMENSIONS) name, in
    /// canonical order.
    pub fn lists(&self) -> [(&'static str, &[Offender]); 5] {
        let [nesting, conciseness, cohesion, focus, uniqueness] = Self::DIMENSIONS;
        [
            (nesting, &self.nesting),
            (conciseness, &self.conciseness),
            (cohesion, &self.cohesion),
            (focus, &self.focus),
            (uniqueness, &self.uniqueness),
        ]
    }

    /// The list a persisted dimension name belongs to, or `None` for a name
    /// outside [`DIMENSIONS`](Self::DIMENSIONS).
    pub fn list_mut(&mut self, dimension: &str) -> Option<&mut Vec<Offender>> {
        match dimension {
            "nesting" => Some(&mut self.nesting),
            "conciseness" => Some(&mut self.conciseness),
            "cohesion" => Some(&mut self.cohesion),
            "focus" => Some(&mut self.focus),
            "uniqueness" => Some(&mut self.uniqueness),
            _ => None,
        }
    }
}

/// One worst-offender entry: the offending symbol and a deterministic,
/// human-readable severity descriptor (CR-005 review-phase visibility).
///
/// `detail` encodes the offending magnitude (e.g. `"nesting depth 6"`,
/// `"LCOM4 4"`, `"23 methods · span 540"`) derived from integer facts, so it is
/// byte-identical across runs and the four CI targets ([NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Offender {
    /// The offending symbol's name.
    pub name: String,
    /// The defining file, when the node is bound to one.
    pub file: String,
    /// 1-based declaration start line, when recorded.
    pub line: Option<i64>,
    /// A deterministic descriptor of the offending magnitude.
    pub detail: String,
}

/// A single architecture-rule violation (FR-GV-02).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Violation {
    /// The rule key: a constraint name (`max_cc`), `layer-ordering`,
    /// `boundary:<from>-><to>`, `forbidden_import:<from>-><to>`,
    /// `require_tested:<paths>`, or `require_documented:<paths>`.
    pub rule: String,
    /// `"constraint"`, `"layer"`, or `"boundary"` — the `violations` table
    /// `rule_type` discriminator (SRS §5.1). The CR-002/CR-003 families reuse
    /// these without a migration: a `forbidden_import` is a `boundary`, a
    /// `require_tested` or `require_documented` gap is a `constraint`; the `rule`
    /// key disambiguates.
    pub rule_type: String,
    /// `"error"` or `"warning"`; `check` exits 1 on any error (FR-GV-03).
    /// All `rules.toml` violations are errors in v1 — checked-in policy is
    /// binding (ratified 2026-06-06).
    pub severity: String,
    /// The offending file (empty for a project-wide violation, e.g.
    /// `max_cycles`).
    pub file: String,
    /// The offending node's storage id, when the violation points at one
    /// (per-function constraints) — the `violations.node_id` column.
    pub node_id: Option<i64>,
    pub message: String,
}

/// Snapshot of the five quality metrics — raw + normalized per metric, the
/// counts the run scored, and the aggregate signal (S-018, FR-QM-01..07,
/// ADR-12).
///
/// Field order is the canonical dimension order the aggregate reduces in
/// (ADR-08, FR-QM-14): the five original metrics (modularity, acyclicity, depth,
/// equality, redundancy) then the five CR-005 structural dimensions (nesting,
/// conciseness, cohesion, focus, uniqueness).
///
/// The applicable original five keep the ADR-12 **zero short-circuit** (a hard
/// `0` collapses the signal — anti-gaming). The new five are **floored at 0.01**
/// (FR-QM-14): they drag the signal but never alone collapse it. Cohesion and
/// Focus are [`Option`]: `None` is the **applicability drop-out** (ADR-21) — the
/// construct does not exist in the repo (no classes / no class-like
/// containers), the snapshot persists NULL + a `false` applicability flag, and
/// the dimension drops out of the geometric-mean denominator (a class-less repo
/// gets a deterministic 9-dimension mean, FR-QM-11/12/14, UAT-QM-10). Modularity
/// takes the same drop-out on a graph with fewer than five edges (CR-156) but
/// keeps its computed pair — see
/// [`modularity_not_applicable`](Self::modularity_not_applicable) — and so
/// leaves the short-circuit as well as the mean.
#[derive(Debug, Default, Serialize)]
pub struct MetricSnapshot {
    /// Newman Q on the directory partition (FR-QM-01). Always the computed
    /// pair, including when Modularity is not applicable — see
    /// [`modularity_not_applicable`](Self::modularity_not_applicable).
    pub modularity: MetricValue,
    /// `Some` when Modularity is **not applicable** ([CR-156]): the graph it is
    /// computed on has fewer than [`MODULARITY_MIN_EDGES`] edges, too few for
    /// community structure. Modularity then leaves both the geometric mean and
    /// the [ADR-12] zero short-circuit — the [ADR-21] rule-2 drop-out Cohesion
    /// and Focus take — while [`modularity`](Self::modularity) keeps its
    /// computed values. `None` = applicable, which is also how a snapshot
    /// persisted before the flag existed reads.
    ///
    /// [CR-156]: ../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
    /// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
    /// [ADR-21]: ../../../docs/specs/architecture/decisions/ADR-21.md
    /// [`MODULARITY_MIN_EDGES`]: crate::metrics::MODULARITY_MIN_EDGES
    pub modularity_not_applicable: Option<ModularityNotApplicable>,
    /// Cycle count via `tarjan_scc` (FR-QM-02).
    pub acyclicity: MetricValue,
    /// Longest path over the condensation (FR-QM-03).
    pub depth: MetricValue,
    /// `1 − Gini` of per-function cyclomatic complexity (FR-QM-04).
    pub equality: MetricValue,
    /// `1 − dead/duplicate function ratio` (FR-QM-05).
    pub redundancy: MetricValue,
    /// `1 − deep-nesting ratio`, floored at 0.01 (FR-QM-09, CR-005): production
    /// functions with `max_nesting_depth ≥ T_nest` over production functions.
    pub nesting: MetricValue,
    /// `1 − brain-method ratio`, floored at 0.01 (FR-QM-10, CR-005): a brain
    /// method meets all of CC ≥ `T_cc` ∧ LOC ≥ `T_loc` ∧ nesting ≥ `T_bn`.
    pub conciseness: MetricValue,
    /// Mean of `1/LCOM4` over production classes, floored at 0.01 (FR-QM-11,
    /// CR-005); `None` = **n/a drop-out** (no applicable classes, ADR-21).
    pub cohesion: Option<MetricValue>,
    /// `1 − god-container ratio` over class-like containers, floored at 0.01
    /// (FR-QM-12, CR-005); `None` = **n/a drop-out** (no class-like containers).
    pub focus: Option<MetricValue>,
    /// `1 − near-clone ratio`, floored at 0.01 (FR-QM-13, CR-005): production
    /// functions in any near-clone group (`clone_group IS NOT NULL`) over
    /// production functions.
    pub uniqueness: MetricValue,
    /// The hash of the effective detection-threshold set this run scored under
    /// (FR-QM-14, BR-25, ADR-21) — persisted in every snapshot so a tuning change
    /// is visible and gate-gated (the announced auto-re-baseline, FR-GV-10).
    /// A property of the run configuration, not the graph, so it is recorded on
    /// every snapshot including the empty-graph sentinel.
    pub thresholds_hash: String,
    /// Vertices in the metric graph the run scored.
    pub node_count: u64,
    /// Edges in the metric graph the run scored.
    pub edge_count: u64,
    /// Production function/method nodes considered by Equality/Redundancy —
    /// `is_test=true` functions are excluded from the scope (FR-QM-08, BR-18).
    pub function_count: u64,
    /// Test function/method nodes excluded from the production scope
    /// (FR-QM-08): the "N test functions excluded from metrics" count, surfaced
    /// for transparency and persisted on the snapshot (FR-QM-07, NFR-CC-04).
    pub test_function_count: u64,
    /// The empty-scope honesty flag ([ADR-12]): this snapshot's
    /// [`node_count`](Self::node_count) — the **production** metric graph's, not
    /// the store's — is zero.
    ///
    /// Named carefully, because the distinction is the whole of [CR-138]: a
    /// populated store whose every vertex is test scope sets this too, and is
    /// not an empty graph. [`SignalAbsence::classify`] reads *this* field and
    /// nothing else to decide that a signal is absent, then separates the two
    /// causes with the store's own node count.
    ///
    /// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
    /// [CR-138]: ../../../docs/requests/CR-138-a-readout-names-the-cause-its-gating-condition-establishes.md
    pub empty: bool,
    /// The rounded 0–10000 signal ([ADR-08]); `None` serialises as `null` — the
    /// "n/a" sentinel emitted exactly when [`empty`](Self::empty) is set, never
    /// a misleading ~8033 ([ADR-12], [NFR-CC-04]). `Some(0)` is the real zero
    /// short-circuit.
    ///
    /// The two are welded here on purpose: a reader of this field learns *that*
    /// there is no signal, and [`SignalAbsence`] is the only place that says
    /// **why**, so the cause has one spelling rather than one per surface.
    ///
    /// [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub aggregate_signal: Option<u32>,
}

/// Why Modularity is not applicable on a snapshot ([CR-156]), carrying the
/// figures that establish it: the edge count `m` of the graph Modularity was
/// computed on and the fixed threshold it fell short of.
///
/// Built only by [`for_edges`](Self::for_edges), so the reason has one
/// spelling on every surface that renders it (`scan --json`,
/// `quality-report --json`, the dashboard).
///
/// [CR-156]: ../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModularityNotApplicable {
    /// `m` — the edges of the production metric graph Modularity was computed on.
    pub edges: u64,
    /// The fixed threshold, [`MODULARITY_MIN_EDGES`](crate::metrics::MODULARITY_MIN_EDGES).
    pub min_edges: u64,
    /// The rendered reason: `"<m> of <min_edges> dependency edges — too few for
    /// community structure"`.
    pub reason: String,
}

impl ModularityNotApplicable {
    /// The drop-out for a graph of `edges` edges, or `None` when it has at
    /// least [`MODULARITY_MIN_EDGES`](crate::metrics::MODULARITY_MIN_EDGES) and
    /// Modularity applies. The single place the threshold is compared.
    pub fn for_edges(edges: u64) -> Option<Self> {
        let min_edges = crate::metrics::MODULARITY_MIN_EDGES;
        (edges < min_edges).then(|| Self {
            edges,
            min_edges,
            reason: format!(
                "{edges} of {min_edges} dependency edges — too few for community structure"
            ),
        })
    }
}

/// One metric's raw + normalized pair (FR-QM-07).
///
/// `raw` is the pre-normalization quantity (Q, cycle count, depth, Gini,
/// redundant ratio) so evolution can show *which* dimension moved; `normalized`
/// is the [0,1] value entering the geometric mean.
#[derive(Debug, Default, Serialize)]
pub struct MetricValue {
    pub raw: f64,
    pub normalized: f64,
}

/// Gate check result for CI integration (FR-GV-04/05, BR-10).
///
/// The gate fails iff `current < baseline − epsilon` (aggregate-regression
/// only, DL-04); per-metric regressions are detail, never an independent
/// failure. No baseline → informational pass.
#[derive(Debug, Default, Serialize)]
pub struct GateResult {
    pub passed: bool,
    /// `true` when this run upserted the baseline (`gate --save` /
    /// `session_start`, FR-GV-04).
    pub saved: bool,
    /// The 0–10000 signal this verdict gated on; `None` = "n/a", reported as such
    /// and never as a zero (ADR-12). **Which absence it means depends on who built
    /// the result**: [`gate`](crate::Engine::gate) computes it fresh, so `None` is
    /// an empty production graph. [`latest_gate`](crate::Engine::latest_gate) reads
    /// the last *persisted* snapshot, where `None` has **two** causes it does not
    /// distinguish — no snapshot has been recorded at all (no `scan` has run, the far
    /// commoner case on a freshly indexed project), or a snapshot exists whose
    /// `aggregate_signal` is itself `None` because nothing was in production scope to
    /// score ([FR-QM-08]). A readout gating on this must not name a cause this field
    /// does not establish ([FR-EH-04], [CR-130]); whether a snapshot exists is what
    /// separates the two, and the evolution series is where a surface can read it.
    ///
    /// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
    /// [FR-EH-04]: ../../../docs/specs/requirements/FR-EH-04.md
    /// [CR-130]: ../../../docs/requests/CR-130-a-readout-names-a-remediation-that-cannot-apply.md
    pub signal: Option<u32>,
    /// The baseline signal compared against, when one existed.
    pub baseline_signal: Option<u32>,
    /// Test function/method nodes excluded from the production-scope metrics
    /// (FR-QM-08): the "N test functions excluded from metrics" count this gate
    /// scored, surfaced alongside the signal (FR-QM-07, NFR-CC-04).
    pub test_function_count: u64,
    /// The optional explicit floor (`gate --threshold`); failing it also
    /// fails the gate.
    pub threshold: Option<u32>,
    /// The float-noise tolerance on the integer signal (BR-10: ≈1.0).
    pub epsilon: f64,
    /// Which metrics moved down vs the baseline — reported as detail (BR-10).
    pub regressions: Vec<MetricRegression>,
    /// Structural-integrity faults folded in from the fast `doctor` check
    /// (CR-052, [FR-GV-18], [NFR-RA-13]): any non-empty entry hard-fails the
    /// gate (`passed = false`) independent of the metric signal — a corrupted
    /// graph blocks the session even when the signal holds the baseline
    /// ([FR-GV-05]). Empty on a structurally sound graph.
    ///
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    /// [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
    /// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
    pub structural_faults: Vec<String>,
    /// The FR-RC-03 freshness line.
    pub freshness: String,
    pub message: String,
    /// Degradations — never an error.
    pub warnings: Vec<String>,
    /// Advisory notes — never a `warnings` entry, so a CI parser scanning
    /// `warnings` is unaffected (CR-119, HF-1). Carries the reconcile's
    /// minified-JS exclusion notice; elided from the serialized report when
    /// empty, so a run with nothing to note renders byte-identical to before.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// The Health bundle's snapshot-derived pair ([FR-UI-04]): the read-only gate
/// verdict and the read-only scan result, **projected from a single read** of
/// the last persisted snapshot ([CR-135] §3.2, [ADR-28], [ADR-43]).
///
/// The shape exists so that one property holds by construction rather than by
/// timing: both fields describe the *same* `metric_snapshots` row. Built from
/// two independent reads — as the Health handler did until [CR-135] — a `scan`
/// committing between them leaves the verdict describing the older row and the
/// metric grid the newer one, and nothing in the payload lets a reader tell the
/// two generations apart. The window is **removed** here rather than narrowed:
/// there is one read, and the two fields are projections of its value.
///
/// This is the treatment `navigate::status` already gives the [FR-IX-12] LOC
/// roll-up's two keys, for the same reason and with the same reasoning recorded
/// at the read.
///
/// Not a wire shape of its own: the web surface destructures the pair into the
/// existing Health DTO's `gate` and `scan` fields, so the [FR-UI-21] payload is
/// byte-unchanged.
///
/// [FR-UI-04]: ../../../docs/specs/requirements/FR-UI-04.md
/// [FR-UI-21]: ../../../docs/specs/requirements/FR-UI-21.md
/// [FR-IX-12]: ../../../docs/specs/requirements/FR-IX-12.md
/// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
/// [ADR-43]: ../../../docs/specs/architecture/decisions/ADR-43.md
/// [CR-135]: ../../../docs/requests/CR-135-the-health-readout-is-internally-consistent-and-never-stale.md
#[derive(Debug, Default, Serialize)]
pub struct LatestHealth {
    /// The read-only gate verdict over the snapshot this pair was read from.
    pub gate: GateResult,
    /// The read-only scan result over that **same** snapshot.
    pub scan: ScanResult,
}

/// One metric's regression vs the baseline (FR-GV-05 detail reporting).
#[derive(Debug, Default, Serialize)]
pub struct MetricRegression {
    /// Canonical metric name (modularity, acyclicity, depth, equality,
    /// redundancy).
    pub metric: String,
    /// The baseline's normalized [0,1] value.
    pub baseline: f64,
    /// This run's normalized [0,1] value.
    pub current: f64,
    /// `current − baseline` (negative = regressed).
    pub delta: f64,
}

/// Architecture-rules compliance report (FR-GV-02).
#[derive(Debug, Default, Serialize)]
pub struct RulesReport {
    /// `Some(false)` when any violation has `severity == "error"` — the
    /// `check` exit-1 discriminator (FR-GV-03); `Some(true)` when nothing did.
    /// `None` when no `rules.toml` contract was loaded and nothing else
    /// (an always-on structural/admission fold-in, [FR-GV-18]/[FR-GV-20])
    /// fired either — a verdict over an empty evaluated set is not a verdict
    /// ([FR-GV-22], [NFR-CC-04]). The CLI's three-state `check` exit-code
    /// discriminator: `Some(true)` → 0, `Some(false)` → 1, `None` → 4 (`4`
    /// collapses to `0` under its `--allow-no-rules` opt-out).
    pub passed: Option<bool>,
    /// Active rules evaluated: set constraints + layer ordering (when layers
    /// are declared) + one per boundary + one per forbidden-import + one per
    /// require-tested contract + one per require-documented contract.
    pub checked_rules: u32,
    /// `true` when a `rules.toml` contract was loaded; `false` when none exists
    /// (the default `<root>/.logos/rules.toml` is absent and an empty contract
    /// was evaluated). The honest "no contract authored yet" signal a surface
    /// needs to tell an empty result apart from an absent one — `checked_rules`
    /// cannot, since the CR-005 structural budgets always evaluate (NFR-CC-04).
    pub rules_present: bool,
    pub violations: Vec<Violation>,
    /// Unix-seconds timestamp of the run marker this run recorded (S-313,
    /// [FR-GV-21]) — additive on `check --json`, nothing renamed or removed.
    ///
    /// It is the same value as the `created_at` stamped on every `violations`
    /// row the run wrote and as the marker's own `ran_at`: one run, one time.
    /// `None` on a report that persisted nothing (`Default`), so a consumer
    /// never reads a fabricated run time.
    ///
    /// Note that a persisted run is not the same set as this report's
    /// `violations`: the always-on structural ([FR-GV-18]) and admission
    /// ([FR-GV-20]) fold-ins are appended after persistence and deliberately
    /// never enter the table.
    ///
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    /// [FR-GV-20]: ../../../docs/specs/requirements/FR-GV-20.md
    /// [FR-GV-21]: ../../../docs/specs/requirements/FR-GV-21.md
    pub ran_at: Option<i64>,
    /// The FR-RC-03 freshness line.
    pub freshness: String,
    /// Degradations — never an error.
    pub warnings: Vec<String>,
    /// Advisory notes — never a `warnings` entry, so a CI parser scanning
    /// `warnings` is unaffected (CR-119, HF-1). Carries the reconcile's
    /// minified-JS exclusion notice; elided from the serialized report when
    /// empty, so a run with nothing to note renders byte-identical to before.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl RulesReport {
    /// The `check` exit-code projection (FR-GV-22 / FR-CL-03): 0/1/4,
    /// collapsed to 0 by `allow_absent` for callers that have deliberately
    /// authored no contract yet.
    pub fn exit_code(&self, allow_absent: bool) -> i32 {
        match self.passed {
            Some(true) => 0,
            Some(false) => 1,
            None if allow_absent => 0,
            None => 4,
        }
    }
}

/// Signal evolution over stored snapshots (FR-GV-06).
///
/// Reports history — it is not an aggregate evaluation and does not
/// reconcile (BR-03 lists the reconciling runs; `evolution` is not one).
#[derive(Debug, Default, Serialize)]
pub struct EvolutionReport {
    /// The window actually applied (default 30).
    pub limit: u32,
    /// The most recent snapshots, oldest first, with per-metric deltas.
    pub snapshots: Vec<EvolutionPoint>,
    /// Degradations (e.g. no snapshots recorded yet) — never an error.
    pub warnings: Vec<String>,
}

/// One snapshot in the evolution series, with deltas vs its predecessor.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EvolutionPoint {
    /// The `metric_snapshots` row id (append-only series order).
    pub snapshot_id: i64,
    /// Unix-seconds timestamp the snapshot was recorded at.
    pub created_at: i64,
    /// Optional VCS commit pin (FR-GV-09).
    pub commit_sha: Option<String>,
    /// The 0–10000 signal; `None` = "n/a" (empty graph, ADR-12).
    pub signal: Option<u32>,
    /// Signed signal movement vs the previous snapshot in the series; `None`
    /// for the first point or when either signal is "n/a".
    pub signal_delta: Option<i64>,
    /// Per-metric normalized values and movement (FR-GV-06 trend detail).
    pub metric_deltas: Vec<MetricDelta>,
}

/// One metric's value and movement at one evolution point.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MetricDelta {
    /// Canonical metric name.
    pub metric: String,
    /// This snapshot's normalized [0,1] value.
    pub normalized: f64,
    /// `normalized − previous.normalized`; `None` for the first point, and
    /// for Modularity whenever it is not applicable at either point — a
    /// dimension outside the signal has no movement of the signal to report.
    pub delta: Option<f64>,
    /// `Some` on the Modularity entry of a snapshot where it was **not
    /// applicable** ([CR-156]): the reason and the m-of-5 count, beside the
    /// computed `normalized` value it still stores. `None` for every other
    /// metric and for an applicable Modularity.
    ///
    /// [CR-156]: ../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
    pub not_applicable: Option<ModularityNotApplicable>,
}

/// Dependency structure matrix (FR-GV-07).
///
/// `matrix[i][j]` counts dependency edges from `rows[i]` to `rows[j]`; rows
/// are ordered by layer order then name, so forward (downward) dependencies
/// sit below the diagonal and back-edges above.
#[derive(Debug, Default, Serialize)]
pub struct DsmReport {
    /// `"module"` (default) or `"file"`.
    pub granularity: String,
    /// Row/column labels in matrix order.
    pub rows: Vec<DsmRow>,
    /// The square matrix: `matrix[i][j]` = count of dep edges `i → j`.
    pub matrix: Vec<Vec<u32>>,
    /// The FR-RC-03 freshness line.
    pub freshness: String,
    /// Degradations — never an error.
    pub warnings: Vec<String>,
    /// Advisory notes — never a `warnings` entry, so a CI parser scanning
    /// `warnings` is unaffected (CR-119, HF-1). Carries the reconcile's
    /// minified-JS exclusion notice; elided from the serialized report when
    /// empty, so a run with nothing to note renders byte-identical to before.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// One row (= column) of the DSM.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DsmRow {
    /// The aggregate's label: a module key or a file path.
    pub name: String,
    /// The `[[layers]]` band ordering the row, when one matched (file-backed
    /// aggregates only; pure module keys are unassigned).
    pub layer: Option<String>,
}

/// Documentation-gap analysis result (FR-GV-14).
///
/// The scope is the *public API*: only `exported` Function/Method symbols are
/// considered. A symbol is "documented" if any [`DocSection`](crate::model::NodeKind::DocSection)
/// resolves a [`DocReference`](crate::model::EdgeKind::DocReference) to it
/// (FR-DG-04) — reference presence in the doc graph, never documentation
/// quality (the `caveat`).
#[derive(Debug, Default, Serialize)]
pub struct DocGapsReport {
    /// Exported functions/methods referenced by no `DocSection`, sorted by file
    /// then name, truncated to `limit`.
    pub undocumented: Vec<DocGap>,
    /// Exported function/method nodes considered (the public-API scope).
    pub total_functions: u64,
    /// Of those, the count referenced by at least one `DocSection`.
    pub documented_functions: u64,
    /// Rounded 0–10000 documentation signal (ADR-08 integer posture, AR-03);
    /// `None` when there are no exported functions to document ("n/a", NFR-CC-04).
    pub documentation_ratio: Option<u32>,
    /// The cap applied to `undocumented` (default 50).
    pub limit: u32,
    /// `true` when more gaps existed than `limit` allowed to list.
    pub truncated: bool,
    /// The mandatory honesty caveat — always emitted.
    pub caveat: String,
    /// The FR-RC-03 freshness line.
    pub freshness: String,
    /// Degradations — never an error.
    pub warnings: Vec<String>,
    /// Advisory notes — never a `warnings` entry, so a CI parser scanning
    /// `warnings` is unaffected (CR-119, HF-1). Carries the reconcile's
    /// minified-JS exclusion notice; elided from the serialized report when
    /// empty, so a run with nothing to note renders byte-identical to before.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// One undocumented exported function/method (FR-GV-14).
#[derive(Debug, Default, Serialize)]
pub struct DocGap {
    pub name: String,
    pub file: String,
    /// 1-based declaration start line, when recorded.
    pub line: Option<i64>,
}

/// System health check — ARCHITECTURE health: DB integrity, schema version,
/// graph coherence (S-020). For INDEX freshness see the navigation `status`.
#[derive(Debug, Default, Serialize)]
pub struct HealthInfo {
    /// `true` when the store opens, the schema is current, and the FTS index
    /// is coherent.
    pub ok: bool,
    pub db_path: String,
    pub db_size_bytes: u64,
    /// The store's `PRAGMA user_version` (FR-DB-04).
    pub schema_version: i64,
    /// FTS5 external-content coherence (a desync is a Correctness fault,
    /// ADR-14 — surfaced here, loud).
    pub fts_ok: bool,
    /// Graph structural integrity (CR-052, [FR-GV-18], [NFR-RA-13]): `true` when
    /// the node store holds one node per `symbol_id` and no orphan rows. Drift
    /// is a Correctness fault — surfaced here and hard-failing the gate.
    ///
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    /// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
    pub structural_ok: bool,
    /// One line per detected structural fault (empty when `structural_ok`) —
    /// the `doctor` verdict folded into `health` (CR-052, [FR-GV-18]).
    ///
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    pub structural_faults: Vec<String>,
    /// Indexed source files.
    pub files: u64,
    /// Graph vertices.
    pub nodes: u64,
    /// Graph relationships.
    pub edges: u64,
    /// Reference-ledger rows not currently bound to an edge.
    pub unresolved_refs: u64,
    /// The FR-RC-03 freshness line.
    pub freshness: String,
    pub message: String,
}

/// The `doctor` structural-integrity verdict (CR-052, [FR-GV-18], [NFR-RA-13],
/// [ADR-46]): the fast always-on guard that asserts one node per `symbol_id`
/// and zero orphan rows in O(a handful of indexed queries), extended by S-215
/// ([FR-GV-20], [ADR-48]) with the always-on admission tripwire.
///
/// The read-model twin of [`StructuralReport`](crate::graph_store::StructuralReport):
/// the same census counts plus the derived `ok` verdict, human-readable
/// `faults`, and a summary `message`. `doctor` exits 1 on `ok == false`, and the
/// same verdict hard-fails `session_end` ([FR-GV-05]) and `check_rules`
/// ([FR-GV-02]) independent of the metric signal.
///
/// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
/// [FR-GV-20]: ../../../docs/specs/requirements/FR-GV-20.md
/// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
/// [ADR-48]: ../../../docs/specs/architecture/decisions/ADR-48.md
#[derive(Debug, Default, Serialize)]
pub struct DoctorReport {
    /// `true` when the graph holds one node per `symbol_id`, no orphan rows,
    /// and no indexed file violates the current admission rules.
    pub ok: bool,
    /// `COUNT(*) FROM nodes`.
    pub node_count: u64,
    /// `COUNT(DISTINCT symbol_id) FROM nodes` — equals `node_count` when sound.
    pub distinct_symbol_ids: u64,
    /// Nodes leaked past the one-per-`symbol_id` invariant (Channel A, ADR-46).
    pub duplicate_symbol_nodes: u64,
    /// Nodes whose non-NULL `file_id` references a missing file.
    pub dangling_file_refs: u64,
    /// Edges whose `source`/`target` references a missing node.
    pub dangling_edge_endpoints: u64,
    /// Shingles whose `node_id` references a missing node.
    pub orphan_shingles: u64,
    /// Indexed files the *current* admission rules would reject — gitignored,
    /// under a nested `.git` boundary, in `ignored_dirs`, or glob-excluded
    /// (S-215, [FR-GV-20]). The exact count; never truncated.
    ///
    /// [FR-GV-20]: ../../../docs/specs/requirements/FR-GV-20.md
    pub unadmitted_files: u64,
    /// A capped, lexically-ordered sample of [`unadmitted_files`](Self::unadmitted_files)
    /// paths ([NFR-RA-06]) — bounded so the read-model stays a fixed size even
    /// on a badly drifted graph; the count above stays exact.
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub unadmitted_sample: Vec<String>,
    /// One line per detected fault (empty when `ok`).
    pub faults: Vec<String>,
    /// Documentation directory-symlinks that exist under the documentation-
    /// include set but ended up unindexed ([FR-IX-11]) — a git-ignored symlink
    /// with no sanctioned bypass, or one whose target escapes containment. Purely
    /// diagnostic: this does **not** affect [`ok`](Self::ok) or the exit status,
    /// it only names the dropped path(s) and reason so a silent doc-drop is
    /// visible ([CR-071]).
    ///
    /// [FR-IX-11]: ../../../docs/specs/requirements/FR-IX-11.md
    #[serde(default)]
    pub doc_symlink_warnings: Vec<String>,
    /// The zero-admission diagnostic ([FR-IX-13]): the rendered explanation for a
    /// root that admitted no files because every immediate child was pruned as a
    /// nested git boundary — a parent folder of sibling repositories — naming the
    /// prune count, a bounded sample of the pruned names, and `logos init
    /// --workspace` as the remedy. The identical line `index` emits as a warning
    /// and `status` carries in its `warnings`, derived from the one shared helper
    /// so the three surfaces cannot disagree.
    ///
    /// `None` — an honest absent, never an empty string ([NFR-CC-04]) — whenever
    /// the root admitted files or pruned no immediate-child boundary, which is
    /// every ordinary repository including one vendoring a submodule.
    ///
    /// Purely diagnostic, exactly like [`doc_symlink_warnings`](Self::doc_symlink_warnings):
    /// it does **not** affect [`ok`](Self::ok) or the exit status, which still
    /// moves on structural drift alone.
    ///
    /// [FR-IX-13]: ../../../docs/specs/requirements/FR-IX-13.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[serde(default)]
    pub zero_admission_warning: Option<String>,
    /// Git-hook reachability findings for the working tree `doctor` ran in
    /// (CR-106, [FR-IN-03]): an installed `core.hooksPath` whose hooks are
    /// missing from *this* tree and therefore fire nothing, or hooks seeded as
    /// copies rather than symlinks (and whether those copies have gone stale).
    ///
    /// Empty when hooks are absent by choice or belong to another hook
    /// manager — opt-in stays opt-in and an unhooked project is not degraded
    /// by a finding it cannot act on.
    ///
    /// **Detection is not tradeable here.** No logos git hook ever fired in a
    /// linked worktree, and the defect survived because "installed" was never
    /// checked against "fires": `init` reported success, `git config` returned
    /// a value, and the scripts were on disk. This field is what asks the
    /// question those three signals cannot answer.
    ///
    /// Purely diagnostic, like the two fields above: it does **not** affect
    /// [`ok`](Self::ok) or the exit status, which still moves on structural
    /// drift alone.
    ///
    /// [FR-IN-03]: ../../../docs/specs/requirements/FR-IN-03.md
    #[serde(default)]
    pub hook_warnings: Vec<String>,
    pub message: String,
}

/// One store's whole-graph census for the deep [`VerifyReport`] diff (CR-052,
/// [FR-GV-19], [NFR-RA-06]): the row counts read from a read-only connection. A
/// report carries two — the live graph and the fresh shadow reindex that defines
/// the equivalence target ([NFR-RA-06]).
///
/// [FR-GV-19]: ../../../docs/specs/requirements/FR-GV-19.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct VerifyCensus {
    /// `COUNT(*) FROM files` — indexed source files.
    pub files: u64,
    /// `COUNT(*) FROM nodes` — graph vertices.
    pub nodes: u64,
    /// `COUNT(*) FROM edges` — graph relationships.
    pub edges: u64,
}

/// The on-demand **deep-`verify`** verdict (CR-052, [FR-GV-19], [NFR-RA-06],
/// [ADR-46]): the live graph diffed against a throwaway shadow store reindexed
/// via the always-purge [`index`](../../../docs/specs/requirements/FR-IX-01.md)
/// path. The fresh reindex defines ground truth ([NFR-RA-06]); the diff catches
/// the **Channel-B orphans** — files the live store retains but a fresh index
/// would drop — that the fast structural [`DoctorReport`] cannot see, and embeds
/// that fast check ([FR-GV-18]) as `structural`.
///
/// The count deltas are `live − reindex`: a positive `node_delta` (with
/// `leaked_symbols`) is the drift signature — stale rows the live store leaked;
/// `orphaned_symbols` are reindex-only symbols the live graph is missing. The
/// symbol samples are lexically ordered and capped for a bounded read-model
/// ([NFR-RA-06]); the `*_total` counts are exact.
///
/// `verify` exits 1 on `ok == false` on the CLI ([FR-GV-19]); the MCP tool and
/// the web read-model ([FR-UI-25]) serialize this report verbatim.
///
/// [FR-GV-19]: ../../../docs/specs/requirements/FR-GV-19.md
/// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
/// [FR-UI-25]: ../../../docs/specs/requirements/FR-UI-25.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
#[derive(Debug, Default, Serialize)]
pub struct VerifyReport {
    /// `true` when the live graph matches a fresh reindex: zero count deltas, an
    /// empty symbol-set diff, and `structural.ok` ([FR-GV-19]).
    pub ok: bool,
    /// The live graph census, read from a read-only connection — the live store
    /// is never mutated by `verify` ([FR-GV-19]).
    pub live: VerifyCensus,
    /// The fresh shadow-reindex census — the equivalence target ([NFR-RA-06]).
    pub reindex: VerifyCensus,
    /// `live.nodes − reindex.nodes`: a positive value is a live surplus (the
    /// leak signature), negative a live deficit. Zero on a sound graph.
    pub node_delta: i64,
    /// `live.edges − reindex.edges`.
    pub edge_delta: i64,
    /// `live.files − reindex.files`.
    pub file_delta: i64,
    /// Symbols present in the live graph but absent from a fresh reindex — the
    /// Channel-B leak ([ADR-46]): stale nodes the live store retains.
    pub leaked_total: u64,
    /// A deterministic, lexically-ordered, capped sample of `leaked_total`
    /// ([NFR-RA-06]).
    pub leaked_symbols: Vec<String>,
    /// Symbols present in a fresh reindex but absent from the live graph — the
    /// live graph under-counts (a missed insertion or an over-eager purge).
    pub orphaned_total: u64,
    /// A deterministic, lexically-ordered, capped sample of `orphaned_total`
    /// ([NFR-RA-06]).
    pub orphaned_symbols: Vec<String>,
    /// The embedded fast structural check on the live graph ([FR-GV-18]) — the
    /// same verdict `doctor` reports, folded into the deep check.
    pub structural: DoctorReport,
    pub message: String,
}

/// Session lifecycle info — `session_start` (FR-GV-04).
///
/// `session_start` is the MCP spelling of `gate --save`: it computes a fresh
/// snapshot and upserts the baseline; `session_end` returns a [`GateResult`].
#[derive(Debug, Default, Serialize)]
pub struct SessionInfo {
    /// The baseline snapshot's row id, as the session handle.
    pub session_id: String,
    /// Unix-seconds timestamp the session (baseline) was recorded at.
    pub started_at: i64,
    /// The signal recorded as the baseline; `None` = "n/a" (empty graph).
    pub signal: Option<u32>,
    /// The FR-RC-03 freshness line.
    pub freshness: String,
    pub message: String,
}

/// Observability stats — usage/perf telemetry from `telemetry.db`
/// (FR-OB-04, NFR-OO-03).
#[derive(Debug, Default, Serialize)]
pub struct StatsInfo {
    /// The reporting window in days (FR-OB-04: default 7).
    pub window_days: u32,
    pub calls_total: u64,
    /// Per-`(surface, tool)` usage breakdown, sorted by surface then tool.
    pub calls_by_tool: Vec<ToolUsage>,
    pub latency_p50_ms: u64,
    pub latency_p95_ms: u64,
    pub latency_p99_ms: u64,
    /// Estimated ad-hoc file reads avoided by navigation calls — an estimate,
    /// honestly labeled (NFR-CC-04; constants ratified per SRS OQ-01).
    pub reads_saved_estimate: u64,
    /// `reads_saved_estimate` × net tokens per avoided read — the headline
    /// dogfood metric (NFR-OO-03).
    pub tokens_saved_estimate: u64,
    /// Per-relation-class cross-artifact binding counts read live from the graph
    /// ledger (CR-011, [FR-OB-04], [FR-CG-11]), keyed by the relation's payload
    /// token (`proto-import`, `route`, `type-name`, …). Empty for a repository
    /// with no artifact wiring, or when the graph store cannot be read (the
    /// telemetry surface degrades to a warning, never an error). A `BTreeMap` so
    /// the report order is deterministic ([NFR-RA-06]).
    ///
    /// [FR-OB-04]: ../../../docs/specs/requirements/FR-OB-04.md
    /// [FR-CG-11]: ../../../docs/specs/requirements/FR-CG-11.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub artifact_bindings: BTreeMap<String, RelationCoverage>,
    /// Per-UTC-day activity across the window, **oldest day first**
    /// ([FR-OB-04]): raw events grouped by `date(at)` merged with any
    /// `daily_rollup` days the window reaches back into (the same dual source
    /// as `calls_by_tool`, so aged-out days still contribute). Empty when the
    /// window recorded nothing.
    ///
    /// [FR-OB-04]: ../../../docs/specs/requirements/FR-OB-04.md
    pub activity_by_day: Vec<DailyActivity>,
    /// Dev-vs-`main` usage split ([FR-OB-08]): at most two buckets — `"dev"`, the
    /// cumulative sum of every non-`main` origin (each a worktree branch), and
    /// `"main"`, the primary checkout plus legacy NULL rows (pre-v2), folded via
    /// `COALESCE(origin,'main')`; sorted so `"dev"` precedes `"main"`. Raw events
    /// only — `daily_rollup` carries no `origin` column, so this breakdown
    /// deliberately omits rolled-up days rather than fabricate an attribution for
    /// them ([NFR-CC-04]); consequently its call sum can be less than
    /// `calls_total` over a window old enough to reach aged-out rollups.
    ///
    /// [FR-OB-08]: ../../../docs/specs/requirements/FR-OB-08.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub calls_by_origin: Vec<OriginUsage>,
    /// **Tool × origin cross-tab** ([FR-OB-11]): per-tool call counts split by
    /// the same dev-vs-`main` bucket as [`StatsInfo::calls_by_origin`], so
    /// *"which navigation came from dev panes?"* is answerable — neither
    /// `calls_by_tool` (surface × tool, no origin) nor `calls_by_origin`
    /// (origin, no tool) can answer it alone. Sorted by tool, then origin
    /// (`"dev"` before `"main"`) ([NFR-RA-06]).
    ///
    /// **Raw events only**, for the same reason as `calls_by_origin`:
    /// `daily_rollup` carries no `origin` column. See
    /// [`StatsInfo::attribution_coverage`], which states that limit in the
    /// payload rather than leaving it to documentation ([NFR-CC-04]).
    ///
    /// [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub calls_by_tool_origin: Vec<ToolOriginUsage>,
    /// The same cross-tab rolled up to [`ToolUsage::class`] × origin — the
    /// dogfood table ([NFR-OO-03]) as `stats` output rather than hand-written
    /// prose ([FR-OB-11]). Derived from `calls_by_tool_origin`, so it carries
    /// exactly the same coverage limits. Sorted by class, then origin.
    ///
    /// [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md
    /// [NFR-OO-03]: ../../../docs/specs/requirements/NFR-OO-03.md
    pub calls_by_class: Vec<ClassUsage>,
    /// What the two attribution projections above actually cover ([FR-OB-11]).
    ///
    /// An unlabelled figure is what [NFR-CC-04] forbids, and these two carry
    /// three limits a reader cannot infer from the numbers: they are
    /// raw-events-only, they therefore cover at most the raw retention window
    /// however long a window was asked for, and legacy `NULL` origins fold into
    /// `"main"`. Stated in the payload, not in documentation.
    ///
    /// [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub attribution_coverage: AttributionCoverage,
    /// Degradations (e.g. no telemetry recorded yet), never an error.
    pub warnings: Vec<String>,
}

/// Usage of one tool on one surface within the stats window (FR-OB-04).
#[derive(Debug, Default, Serialize)]
pub struct ToolUsage {
    /// The recording surface, e.g. `"cli"`, `"mcp"`, `"watcher"`, `"web"`,
    /// `"shell"`, `"wikigen"`, or `"chat"`. Self-referential reads — the
    /// Statistics tab's own `stats` request and the shell's `status` readout —
    /// are excluded per event, so opening the tab never inflates its own
    /// numbers ([FR-OB-09]); a graph query issued through the dashboard is
    /// counted like any other, so `"web"` rows do appear here.
    ///
    /// [FR-OB-09]: ../../../docs/specs/requirements/FR-OB-09.md
    pub surface: String,
    /// Engine method or pipeline pass name.
    pub tool: String,
    /// The tool's class ([FR-OB-11]): `"navigation"`, `"quality-gate"`,
    /// `"session-gate"`, `"engine-internal"`, `"read-model"`, or
    /// `"unregistered"` for a name written by an older build that today's
    /// registry no longer knows — such rows are counted rather than rewritten,
    /// so an honest "unknown" beats a guessed class ([NFR-CC-04]).
    ///
    /// A label on the existing raw-plus-rollup counts, so unlike
    /// [`StatsInfo::calls_by_tool_origin`] it is **not** raw-events-only: every
    /// tool this read-model reports carries a class.
    ///
    /// [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub class: String,
    pub calls: u64,
    pub ok_calls: u64,
    /// What these calls answered ([FR-OB-14]): `answered_calls` and
    /// `classified_calls`, flattened into the cell beside `outcome_absence`.
    /// Both counts ship and no rate does — see [`OutcomeCounts`].
    ///
    /// [FR-OB-14]: ../../../docs/specs/requirements/FR-OB-14.md
    #[serde(flatten)]
    pub outcomes: OutcomeCounts,
}

/// One cell of the tool × origin cross-tab ([FR-OB-11]): a tool's calls within
/// one dev-vs-`main` bucket. Raw events only — see
/// [`StatsInfo::attribution_coverage`].
///
/// [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md
#[derive(Debug, Default, Serialize)]
pub struct ToolOriginUsage {
    /// Engine method or pipeline pass name.
    pub tool: String,
    /// The tool's class, as [`ToolUsage::class`].
    pub class: String,
    /// The dev-vs-`main` bucket — `"dev"` (all worktree branches) or `"main"`.
    pub origin: String,
    pub calls: u64,
    pub ok_calls: u64,
    /// What these calls answered ([FR-OB-14]): `answered_calls` and
    /// `classified_calls`, flattened into the cell beside `outcome_absence`.
    /// Both counts ship and no rate does — see [`OutcomeCounts`].
    ///
    /// [FR-OB-14]: ../../../docs/specs/requirements/FR-OB-14.md
    #[serde(flatten)]
    pub outcomes: OutcomeCounts,
}

/// One cell of the class × origin breakdown ([FR-OB-11]) — the cross-tab rolled
/// up to [`ToolUsage::class`]. Raw events only, as the cross-tab it derives from.
///
/// [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md
#[derive(Debug, Default, Serialize)]
pub struct ClassUsage {
    /// The tool class, as [`ToolUsage::class`].
    pub class: String,
    /// The dev-vs-`main` bucket — `"dev"` or `"main"`.
    pub origin: String,
    pub calls: u64,
    pub ok_calls: u64,
    /// What these calls answered ([FR-OB-14]): `answered_calls` and
    /// `classified_calls`, flattened into the cell beside `outcome_absence`.
    /// Both counts ship and no rate does — see [`OutcomeCounts`].
    ///
    /// [FR-OB-14]: ../../../docs/specs/requirements/FR-OB-14.md
    #[serde(flatten)]
    pub outcomes: OutcomeCounts,
}

/// What [`StatsInfo::calls_by_tool_origin`] and [`StatsInfo::calls_by_class`]
/// actually cover ([FR-OB-11], [NFR-CC-04]).
///
/// Every field is a statement the payload makes about itself, so a consumer
/// rendering either projection can label it without consulting documentation —
/// the honest-omission posture [FR-OB-08] already set for `calls_by_origin`.
///
/// [FR-OB-08]: ../../../docs/specs/requirements/FR-OB-08.md
/// [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Serialize)]
pub struct AttributionCoverage {
    /// Always `true`: `daily_rollup` is keyed `(day, surface, tool)` and carries
    /// no `origin` column, so a rolled-up day contributes to `calls_total` and
    /// `activity_by_day` but is **absent** from both attribution projections
    /// rather than mis-attributed.
    pub raw_events_only: bool,
    /// The window the caller asked for, echoing [`StatsInfo::window_days`].
    pub requested_window_days: u32,
    /// The window the two projections are **guaranteed** to cover:
    /// `requested_window_days` capped at the raw-event retention horizon
    /// (~90 days, [NFR-OO-04]), past which raw rows become eligible to be folded
    /// into `daily_rollup` and deleted.
    ///
    /// A floor, not a measurement. Pruning is flush-triggered rather than
    /// time-driven, so a store that has not flushed recently still holds older
    /// raw events and the projections then cover more than this says.
    /// Under-stating is the safe direction ([NFR-CC-04]).
    ///
    /// [NFR-OO-04]: ../../../docs/specs/requirements/NFR-OO-04.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub covered_window_days: u32,
    /// `true` when `covered_window_days < requested_window_days` — the request
    /// reached past the retention horizon, so the attribution projections are
    /// guaranteed less of the window than the rest of the read-model. Like
    /// `covered_window_days` this follows from the request, not from the data:
    /// a store younger than the horizon may still have covered it in full.
    pub truncated_by_retention: bool,
    /// Always `true`: rows written before the [FR-OB-08] migration have
    /// `origin IS NULL` and fold into `"main"` via `COALESCE(origin,'main')`,
    /// inflating the historical `"main"` bucket.
    ///
    /// [FR-OB-08]: ../../../docs/specs/requirements/FR-OB-08.md
    pub legacy_null_origin_folds_into_main: bool,
    /// The same limits as display-ready prose, for a surface that renders the
    /// projections without re-deriving the wording ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub notes: Vec<String>,
}

/// The two structural limits hold even on a zeroed read-model — a degraded
/// `stats` (unreadable store) must not answer `raw_events_only: false`, which
/// would be a claim, not an absence ([NFR-CC-04]). The window figures are `0`
/// there because no window was covered.
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
impl Default for AttributionCoverage {
    fn default() -> Self {
        Self {
            raw_events_only: true,
            requested_window_days: 0,
            covered_window_days: 0,
            truncated_by_retention: false,
            legacy_null_origin_folds_into_main: true,
            notes: Vec::new(),
        }
    }
}

/// One UTC day's activity in the stats window ([FR-OB-04]): total calls and the
/// successful subset, keyed by the `'YYYY-MM-DD'` calendar day.
///
/// [FR-OB-04]: ../../../docs/specs/requirements/FR-OB-04.md
#[derive(Debug, Default, Serialize)]
pub struct DailyActivity {
    /// The UTC calendar day, `'YYYY-MM-DD'`.
    pub day: String,
    pub calls: u64,
    pub ok_calls: u64,
    /// What these calls answered ([FR-OB-14]): `answered_calls` and
    /// `classified_calls`, flattened into the cell beside `outcome_absence`.
    /// Both counts ship and no rate does — see [`OutcomeCounts`].
    ///
    /// [FR-OB-14]: ../../../docs/specs/requirements/FR-OB-14.md
    #[serde(flatten)]
    pub outcomes: OutcomeCounts,
}

/// Calls attributed to one dev-vs-`main` bucket ([FR-OB-08]): `"main"` for the
/// primary checkout (and legacy NULL rows), or `"dev"` for all worktree branches
/// combined.
///
/// [FR-OB-08]: ../../../docs/specs/requirements/FR-OB-08.md
#[derive(Debug, Default, Serialize)]
pub struct OriginUsage {
    /// The dev-vs-`main` bucket — `"dev"` (all worktree branches) or `"main"`.
    pub origin: String,
    pub calls: u64,
    pub ok_calls: u64,
    /// What these calls answered ([FR-OB-14]): `answered_calls` and
    /// `classified_calls`, flattened into the cell beside `outcome_absence`.
    /// Both counts ship and no rate does — see [`OutcomeCounts`].
    ///
    /// [FR-OB-14]: ../../../docs/specs/requirements/FR-OB-14.md
    #[serde(flatten)]
    pub outcomes: OutcomeCounts,
}

/// Languages registered in the plugin substrate (FR-PL-06).
///
/// Lists the grammars that loaded successfully plus any that were skipped at
/// load due to an ABI mismatch, so `logos languages` makes a degraded grammar
/// set visible rather than silent (FR-PL-03, UAT-PL-02).
#[derive(Debug, Default, Serialize)]
pub struct LanguagesInfo {
    pub languages: Vec<LanguageDescriptor>,
    /// Grammars skipped at load (ABI mismatch). Empty in the healthy case.
    pub skipped: Vec<SkippedLanguage>,
    /// Set when the registry failed to load at all — a malformed descriptor or
    /// a query that fails to compile (typically a declared capability with no
    /// loadable query file, or a broken on-disk override), naming the offending
    /// file (FR-PL-02, FR-PL-03, S-340). Distinct from an ABI mismatch
    /// ([`skipped`](Self::skipped)), which disables only the affected grammar:
    /// this is a hard failure of the *entire* registry, so `languages` and
    /// `skipped` are both empty alongside it — an honest failed preflight, never
    /// an empty-but-healthy-looking listing ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_error: Option<String>,
}

/// Descriptor for one registered language/grammar (FR-PL-06).
#[derive(Debug, Default, Serialize)]
pub struct LanguageDescriptor {
    pub name: String,
    /// File extensions this grammar claims (without the leading dot).
    pub extensions: Vec<String>,
    /// Basename claims for an extensionless artifact format (CR-010, FR-CG-01),
    /// e.g. `["Dockerfile"]`. Empty for code/doc plugins. Surfaced so
    /// `logos languages` lists an artifact plugin's filename claims alongside its
    /// extensions ([FR-PL-06] as modified).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filenames: Vec<String>,
    /// Whether this is a config/artifact-class plugin (CR-010, FR-CG-01) — `true`
    /// marks the third plugin class beside code and documentation. Surfaced so
    /// `logos languages` distinguishes artifact plugins.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub artifact: bool,
    /// The member-path separator joining symbol segments (`::`, `.`, `/`) — not
    /// the import-specifier grammar, which a plugin declares apart (S-439).
    pub module_separator: String,
    /// Extraction capabilities this grammar supports (e.g. `["symbols"]`).
    pub capabilities: Vec<String>,
    /// The tree-sitter ABI version the grammar was loaded at.
    pub abi_version: u32,
    /// Capabilities whose active query is sourced from an on-disk override
    /// rather than the embedded default (FR-PL-04). Empty when none.
    pub overridden_capabilities: Vec<String>,
    /// The cross-file reach the plugin declares ([FR-PL-09], [CR-180]): what its
    /// references bind **across files**, as opposed to what it captures. `None`
    /// (and absent from the payload) for the documentation and artifact classes,
    /// which bind no code reference.
    ///
    /// [FR-PL-09]: ../../../docs/specs/requirements/FR-PL-09.md
    /// [CR-180]: ../../../docs/requests/CR-180-scala-is-declared-as-limited-support-and-every-language-declares-its-reach.md
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<LanguageReach>,
}

/// A code language's declared cross-file reach, as `logos languages` states it
/// ([FR-PL-09]).
///
/// [FR-PL-09]: ../../../docs/specs/requirements/FR-PL-09.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LanguageReach {
    /// `resolved`, `partial`, `same-file` or `symbols`.
    pub level: String,
    /// The relations bound across a file boundary, drawn from `calls`,
    /// `imports`, `type_relations`, `member_access` and `routes`; empty for
    /// `same-file` and `symbols`.
    pub cross_file: Vec<String>,
}

/// A grammar skipped at load, with the reason (FR-PL-03).
#[derive(Debug, Default, Serialize)]
pub struct SkippedLanguage {
    pub name: String,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`RulesReport::exit_code`]'s FR-GV-22 projection, in isolation from the
    /// engine/CLI plumbing that otherwise only exercises it transitively: a
    /// clean, loaded contract exits 0; a violation exits 1 whether or not a
    /// contract was loaded (a real finding always wins over "absent"); no
    /// contract and nothing else fired exits 4, collapsed to 0 by
    /// `allow_absent` for callers that have deliberately authored none.
    #[test]
    fn rules_report_exit_code_projects_the_three_states() {
        let report = |passed| RulesReport { passed, ..Default::default() };
        assert_eq!(report(Some(true)).exit_code(false), 0);
        assert_eq!(report(Some(false)).exit_code(false), 1);
        assert_eq!(report(Some(false)).exit_code(true), 1);
        assert_eq!(report(None).exit_code(false), 4);
        assert_eq!(report(None).exit_code(true), 0);
    }
}
